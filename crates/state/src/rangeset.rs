//! Range sets. Port of `rangeset.ts`.
//!
//! A range set stores ranges, each with a value, in sorted chunks of up to
//! 250 ranges. Ranges that overlap the previous range in a way that breaks
//! the sorting spill into a next layer, so every layer can be searched by
//! binary search. Sets are immutable and cheap to clone; mapping a set
//! through changes reuses the chunks the changes don't touch.
//!
//! Positions are stored as `i64` internally so the sentinel values
//! CodeMirror uses (`±1e9`) and side arithmetic stay simple; the public API
//! uses `usize`.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::change::{ChangeDesc, MapMode, RangeTouch};

/// The maximum amount of ranges to store in a single chunk.
const CHUNK_SIZE: usize = 250;
/// A value larger than any position.
const FAR: i64 = 1 << 50;

/// Each range is associated with a value implementing this trait.
pub trait RangeValue: Send + Sync + 'static {
    /// Compare this value with another value. Used when comparing range
    /// sets. Values that are the same allocation always compare equal; by
    /// default, others never do. Unless you are only creating a fixed number
    /// of unique values, it is a good idea to implement this.
    fn value_eq(&self, _other: &Self) -> bool {
        false
    }

    /// The bias value at the start of the range. Determines how the range
    /// is positioned relative to other ranges starting at this position.
    fn start_side(&self) -> i64 {
        0
    }

    /// The bias value at the end of the range.
    fn end_side(&self) -> i64 {
        0
    }

    /// The mode with which the location of the range should be mapped when
    /// its `from` and `to` are the same, to decide whether a change deletes
    /// the range.
    fn map_mode(&self) -> MapMode {
        MapMode::TrackDel
    }

    /// Whether this value marks a point range. Regular ranges affect the
    /// part of the document they cover, and are meaningless when empty.
    /// Point ranges have a meaning on their own. When non-empty, a point
    /// range is treated as atomic and shadows any ranges contained in it.
    fn point(&self) -> bool {
        false
    }
}

fn cmp_val<T: RangeValue>(a: &Arc<T>, b: &Arc<T>) -> bool {
    Arc::ptr_eq(a, b) || a.value_eq(b)
}

/// CodeMirror's `a - b || c - d` comparisons.
fn cmp2(a: i64, b: i64, c: i64, d: i64) -> i64 {
    if a != b {
        a - b
    } else {
        c - d
    }
}

fn side_assoc(side: i64) -> i32 {
    side.signum() as i32
}

/// A range associates a value with a range of positions.
pub struct Range<T> {
    /// The range's start position.
    pub from: usize,
    /// Its end position.
    pub to: usize,
    /// The value associated with this range.
    pub value: Arc<T>,
}

impl<T> Clone for Range<T> {
    fn clone(&self) -> Self {
        Range { from: self.from, to: self.to, value: self.value.clone() }
    }
}

impl<T: RangeValue> Range<T> {
    /// Create a range.
    pub fn new(from: usize, to: usize, value: impl Into<Arc<T>>) -> Range<T> {
        Range { from, to, value: value.into() }
    }
}

fn cmp_range<T: RangeValue>(a: &Range<T>, b: &Range<T>) -> Ordering {
    a.from.cmp(&b.from).then(a.value.start_side().cmp(&b.value.start_side()))
}

/// Receives the differences found by [`RangeSet::compare`].
pub trait RangeComparator<T> {
    /// A range (in positions in the new document) has the given sets of
    /// values associated with it, which are different in the old (A) and
    /// new (B) sets.
    fn compare_range(&mut self, from: usize, to: usize, active_a: &[Arc<T>], active_b: &[Arc<T>]);
    /// A changed (or inserted, or deleted) point range.
    fn compare_point(&mut self, from: usize, to: usize, point_a: Option<&Arc<T>>, point_b: Option<&Arc<T>>);
    /// Return true to receive [`bound_change`](RangeComparator::bound_change)
    /// calls.
    fn wants_bound_changes(&self) -> bool {
        false
    }
    /// A changed boundary between ranges. For example, if the same span is
    /// covered by two partial ranges before and one bigger range after,
    /// this is called at the point where the ranges used to be split.
    fn bound_change(&mut self, _pos: usize) {}
}

/// Receives the spans created by a set of ranges in [`RangeSet::spans`].
/// The entire iterated range is covered with either `span` or `point`
/// calls.
pub trait SpanIterator<T> {
    /// Called for any ranges not covered by point decorations. `active`
    /// holds the values that the range is marked with (and may be empty).
    /// `open_start` indicates how many of those ranges are open (continued)
    /// at the start of the span.
    fn span(&mut self, from: usize, to: usize, active: &[Arc<T>], open_start: usize);
    /// Called when going over a point decoration. The active range
    /// decorations that cover the point and have a higher precedence are
    /// provided in `active`. The open count in `open_start` counts the
    /// number of those ranges that started before the point. If the point
    /// started before the iterated range, `open_start` will be
    /// `active.len() + 1` to signal this.
    fn point(&mut self, from: usize, to: usize, value: &Arc<T>, active: &[Arc<T>], open_start: usize, index: usize);
}

struct Chunk<T> {
    from: Vec<i64>,
    to: Vec<i64>,
    value: Vec<Arc<T>>,
    // Chunks are marked with the largest point that occurs in them (or -1
    // for no points), so that scans that are only interested in points
    // (such as the heightmap-related logic) can skip range-only chunks.
    max_point: i64,
}

impl<T: RangeValue> Chunk<T> {
    fn length(&self) -> i64 {
        *self.to.last().unwrap()
    }

    /// Find the index of the given position and side. Uses the ranges'
    /// `from` pos when `end == false`, `to` when `end == true`.
    fn find_index(&self, pos: i64, side: i64, end: bool, start_at: usize) -> usize {
        let arr = if end { &self.to } else { &self.from };
        let (mut lo, mut hi) = (start_at, arr.len());
        loop {
            if lo == hi {
                return lo;
            }
            let mid = (lo + hi) >> 1;
            let val_side = if end { self.value[mid].end_side() } else { self.value[mid].start_side() };
            let diff = cmp2(arr[mid], pos, val_side, side);
            if mid == lo {
                return if diff >= 0 { lo } else { hi };
            }
            if diff >= 0 {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
    }

    fn between(&self, offset: i64, from: i64, to: i64, f: &mut dyn FnMut(usize, usize, &Arc<T>) -> bool) -> bool {
        let start = self.find_index(from, -FAR, true, 0);
        let end = self.find_index(to, FAR, false, start);
        for i in start..end {
            if !f((self.from[i] + offset) as usize, (self.to[i] + offset) as usize, &self.value[i]) {
                return false;
            }
        }
        true
    }

    fn map(
        &self,
        offset: i64,
        changes: &ChangeDesc,
        mut base_pos: i64,
        mut base_side: i64,
        spill: &mut dyn FnMut(i64, i64, Arc<T>),
    ) -> (Option<Chunk<T>>, i64) {
        let (mut value, mut from, mut to): (Vec<Arc<T>>, Vec<i64>, Vec<i64>) = (Vec::new(), Vec::new(), Vec::new());
        let (mut new_pos, mut max_point) = (-1i64, -1i64);
        'iter: for i in 0..self.value.len() {
            let val = &self.value[i];
            let (cur_from, cur_to) = (self.from[i] + offset, self.to[i] + offset);
            let (new_from, mut new_to);
            if cur_from == cur_to {
                let Some(mapped) =
                    changes.map_pos_mode(cur_from as usize, side_assoc(val.start_side()), val.map_mode())
                else {
                    continue;
                };
                new_from = mapped as i64;
                new_to = new_from;
                if val.start_side() != val.end_side() {
                    new_to = changes.map_pos(cur_from as usize, side_assoc(val.end_side())) as i64;
                    if new_to < new_from {
                        continue;
                    }
                }
            } else {
                new_from = changes.map_pos(cur_from as usize, side_assoc(val.start_side())) as i64;
                new_to = changes.map_pos(cur_to as usize, side_assoc(val.end_side())) as i64;
                if new_from > new_to || (new_from == new_to && val.start_side() > 0 && val.end_side() <= 0) {
                    continue;
                }
            }
            if cmp2(new_to, new_from, val.end_side(), val.start_side()) < 0 {
                continue;
            }
            if new_pos < 0 {
                new_pos = new_from;
            }
            if val.point() {
                max_point = max_point.max(new_to - new_from);
            }

            if cmp2(new_from, base_pos, val.start_side(), base_side) >= 0 {
                value.push(val.clone());
                from.push(new_from - new_pos);
                to.push(new_to - new_pos);
                base_pos = new_to;
                base_side = val.end_side();
            } else {
                if new_from == new_to {
                    // Try to reorder points to fit in here
                    let mut i = value.len();
                    while i > 0 {
                        if cmp2(new_from, to[i - 1] + new_pos, val.start_side(), value[i - 1].end_side()) >= 0 {
                            value.insert(i, val.clone());
                            from.insert(i, new_from - new_pos);
                            to.insert(i, new_to - new_pos);
                            continue 'iter;
                        }
                        if cmp2(new_from, from[i - 1] + new_pos, val.end_side(), value[i - 1].start_side()) > 0 {
                            break;
                        }
                        i -= 1;
                    }
                }
                // Otherwise, spill into a new layer
                spill(new_from, new_to, val.clone());
            }
        }
        let mapped = if value.is_empty() { None } else { Some(Chunk { from, to, value, max_point }) };
        (mapped, new_pos)
    }
}

struct RangeSetInner<T> {
    chunk_pos: Vec<i64>,
    chunk: Vec<Arc<Chunk<T>>>,
    next_layer: Option<RangeSet<T>>,
    max_point: i64,
}

/// A range set stores a collection of [ranges](Range) in a way that makes
/// them efficient to [map](RangeSet::map) and [update](RangeSet::update).
/// This is an immutable data structure; cloning it is cheap.
pub struct RangeSet<T> {
    inner: Arc<RangeSetInner<T>>,
}

impl<T> Clone for RangeSet<T> {
    fn clone(&self) -> Self {
        RangeSet { inner: self.inner.clone() }
    }
}

/// Arguments to [`RangeSet::update`].
pub struct RangeSetUpdate<'a, T> {
    /// Ranges to add. These should be sorted by `from` position and
    /// `start_side` unless `sort` is true.
    pub add: Vec<Range<T>>,
    /// Whether the library should sort the ranges in `add`.
    pub sort: bool,
    /// Filter the ranges already in the set. Only those for which this
    /// function returns `true` are kept.
    pub filter: Option<&'a dyn Fn(usize, usize, &T) -> bool>,
    /// Limits the range on which the filter is applied. Filtering only a
    /// small range, as opposed to the entire set, can make updates cheaper.
    pub filter_from: usize,
    /// The end position to apply the filter to (the end of the set when
    /// `None`).
    pub filter_to: Option<usize>,
}

impl<T> Default for RangeSetUpdate<'_, T> {
    fn default() -> Self {
        RangeSetUpdate { add: Vec::new(), sort: false, filter: None, filter_from: 0, filter_to: None }
    }
}

impl<T: RangeValue> Default for RangeSet<T> {
    fn default() -> Self {
        RangeSet::empty()
    }
}

impl<T: RangeValue> RangeSet<T> {
    fn create(chunk_pos: Vec<i64>, chunk: Vec<Arc<Chunk<T>>>, next_layer: RangeSet<T>, max_point: i64) -> Self {
        let next_layer = if next_layer.is_empty() { None } else { Some(next_layer) };
        RangeSet { inner: Arc::new(RangeSetInner { chunk_pos, chunk, next_layer, max_point }) }
    }

    /// The empty set of ranges.
    pub fn empty() -> Self {
        RangeSet {
            inner: Arc::new(RangeSetInner {
                chunk_pos: Vec::new(),
                chunk: Vec::new(),
                next_layer: None,
                max_point: -1,
            }),
        }
    }

    /// True when the set holds no ranges.
    pub fn is_empty(&self) -> bool {
        self.inner.chunk.is_empty()
    }

    /// True when both values are the same set.
    pub fn ptr_eq(&self, other: &RangeSet<T>) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    fn next_layer(&self) -> RangeSet<T> {
        self.inner.next_layer.clone().unwrap_or_else(RangeSet::empty)
    }

    /// The number of chunks in the top layer (exposed for tests).
    #[doc(hidden)]
    pub fn chunk_count(&self) -> usize {
        self.inner.chunk.len()
    }

    /// True when some ranges spilled into a second layer (exposed for
    /// tests).
    #[doc(hidden)]
    pub fn has_next_layer(&self) -> bool {
        self.inner.next_layer.is_some()
    }

    /// True when the first chunks of the two sets are the same allocation
    /// (exposed for tests).
    #[doc(hidden)]
    pub fn shares_first_chunk(&self, other: &RangeSet<T>) -> bool {
        match (self.inner.chunk.first(), other.inner.chunk.first()) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// The largest point size in the set (-1 when it has no points).
    pub fn max_point(&self) -> i64 {
        self.inner.max_point
    }

    /// The end of the last range in the set.
    pub fn length(&self) -> usize {
        let n = self.inner.chunk.len();
        if n == 0 {
            0
        } else {
            (self.chunk_end(n - 1) as usize).max(self.next_layer().length())
        }
    }

    /// The number of ranges in the set.
    pub fn size(&self) -> usize {
        if self.is_empty() {
            return 0;
        }
        self.next_layer().size() + self.inner.chunk.iter().map(|c| c.value.len()).sum::<usize>()
    }

    fn chunk_end(&self, index: usize) -> i64 {
        self.inner.chunk_pos[index] + self.inner.chunk[index].length()
    }

    /// Update the range set, optionally adding new ranges or filtering out
    /// existing ones.
    pub fn update(&self, spec: RangeSetUpdate<'_, T>) -> RangeSet<T> {
        let RangeSetUpdate { mut add, sort, filter, filter_from, filter_to } = spec;
        let filter_from = filter_from as i64;
        let filter_to = filter_to.unwrap_or_else(|| self.length()) as i64;
        if add.is_empty() && filter.is_none() {
            return self.clone();
        }
        if sort {
            add.sort_by(cmp_range);
        }
        if self.is_empty() {
            return if add.is_empty() { self.clone() } else { RangeSet::of(add, false) };
        }

        let mut cur = LayerCursor::new(self.clone(), None, -1, 0);
        cur.goto(0, -FAR);
        let mut i = 0;
        let mut spill: Vec<Range<T>> = Vec::new();
        let mut builder = RangeSetBuilder::new();
        while cur.value.is_some() || i < add.len() {
            if i < add.len() && cmp2(cur.from, add[i].from as i64, cur.start_side(), add[i].value.start_side()) >= 0 {
                let range = add[i].clone();
                i += 1;
                if !builder.add_inner(range.from as i64, range.to as i64, range.value.clone(), false) {
                    spill.push(range);
                }
            } else if cur.range_index == 1
                && cur.chunk_index < self.inner.chunk.len()
                && (i == add.len() || self.chunk_end(cur.chunk_index) < add[i].from as i64)
                && (filter.is_none()
                    || filter_from > self.chunk_end(cur.chunk_index)
                    || filter_to < self.inner.chunk_pos[cur.chunk_index])
                && builder.add_chunk(self.inner.chunk_pos[cur.chunk_index], self.inner.chunk[cur.chunk_index].clone())
            {
                cur.next_chunk();
            } else {
                let value = cur.value.clone().unwrap();
                let keep = match filter {
                    None => true,
                    Some(f) => {
                        filter_from > cur.to || filter_to < cur.from || f(cur.from as usize, cur.to as usize, &value)
                    }
                };
                if keep && !builder.add_inner(cur.from, cur.to, value.clone(), false) {
                    spill.push(Range { from: cur.from as usize, to: cur.to as usize, value });
                }
                cur.next();
            }
        }

        let next = self.next_layer();
        let next = if next.is_empty() && spill.is_empty() {
            RangeSet::empty()
        } else {
            next.update(RangeSetUpdate {
                add: spill,
                sort: false,
                filter,
                filter_from: filter_from as usize,
                filter_to: Some(filter_to as usize),
            })
        };
        builder.finish_inner(next)
    }

    /// Map this range set through a set of changes, return the new set.
    pub fn map(&self, changes: &ChangeDesc) -> RangeSet<T> {
        if changes.is_empty() || self.is_empty() {
            return self.clone();
        }
        let mut chunks: Vec<Arc<Chunk<T>>> = Vec::new();
        let mut chunk_pos: Vec<i64> = Vec::new();
        let mut max_point = -1;
        let mut spilled: Option<RangeSetBuilder<T>> = None;
        for i in 0..self.inner.chunk.len() {
            let (start, chunk) = (self.inner.chunk_pos[i], &self.inner.chunk[i]);
            match changes.touches_range(start as usize, (start + chunk.length()) as usize) {
                RangeTouch::No => {
                    max_point = max_point.max(chunk.max_point);
                    chunks.push(chunk.clone());
                    chunk_pos.push(changes.map_pos(start as usize, -1) as i64);
                }
                RangeTouch::Yes => {
                    let (prev_pos, prev_side) = match (chunks.last(), chunk_pos.last()) {
                        (Some(c), Some(p)) => (p + c.length(), c.value.last().unwrap().end_side()),
                        _ => (-1, -1),
                    };
                    let mut spill = |from: i64, to: i64, value: Arc<T>| {
                        spilled.get_or_insert_with(RangeSetBuilder::new).add_range(from, to, value, false);
                    };
                    let (mapped, pos) = chunk.map(start, changes, prev_pos, prev_side, &mut spill);
                    if let Some(mapped) = mapped {
                        max_point = max_point.max(mapped.max_point);
                        chunks.push(Arc::new(mapped));
                        chunk_pos.push(pos);
                    }
                }
                RangeTouch::Cover => {}
            }
        }
        let mut next = self.next_layer().map(changes);
        if let Some(spilled) = spilled {
            next = spilled.finish_inner(next);
        }
        if chunks.is_empty() {
            next
        } else {
            RangeSet::create(chunk_pos, chunks, next, max_point)
        }
    }

    /// Iterate over the ranges that touch the region `from` to `to`,
    /// calling `f` for each. There is no guarantee that the ranges will be
    /// reported in any specific order. When the callback returns `false`,
    /// iteration stops.
    pub fn between(&self, from: usize, to: usize, mut f: impl FnMut(usize, usize, &Arc<T>) -> bool) {
        self.between_inner(from as i64, to as i64, &mut f);
    }

    fn between_inner(&self, from: i64, to: i64, f: &mut dyn FnMut(usize, usize, &Arc<T>) -> bool) -> bool {
        if self.is_empty() {
            return true;
        }
        for i in 0..self.inner.chunk.len() {
            let (start, chunk) = (self.inner.chunk_pos[i], &self.inner.chunk[i]);
            if to >= start && from <= start + chunk.length() && !chunk.between(start, from - start, to - start, f) {
                return false;
            }
        }
        self.next_layer().between_inner(from, to, f)
    }

    /// Iterate over the ranges in this set, in order, including all ranges
    /// that end at or after `from`.
    pub fn iter(&self, from: usize) -> RangeCursor<T> {
        RangeSet::iter_sets(std::slice::from_ref(self), from)
    }

    /// Iterate over the ranges in a collection of sets, in order, starting
    /// from `from`.
    pub fn iter_sets(sets: &[RangeSet<T>], from: usize) -> RangeCursor<T> {
        let mut cursor = Cursor::from_sets(sets, None, -1);
        cursor.goto(from as i64, -FAR);
        RangeCursor(cursor)
    }

    /// Iterate over two groups of sets, calling methods on `comparator` to
    /// notify it of possible differences. `text_diff` indicates how the
    /// underlying data changed between these ranges, and is needed to
    /// synchronize the iteration. `min_point_size` can be used to ignore
    /// all non-point ranges, and points below the given size; when -1, all
    /// ranges are compared.
    pub fn compare(
        old_sets: &[RangeSet<T>],
        new_sets: &[RangeSet<T>],
        text_diff: &ChangeDesc,
        comparator: &mut dyn RangeComparator<T>,
        min_point_size: i64,
    ) {
        let keep = |set: &&RangeSet<T>| set.max_point() > 0 || (!set.is_empty() && set.max_point() >= min_point_size);
        let a: Vec<RangeSet<T>> = old_sets.iter().filter(keep).cloned().collect();
        let b: Vec<RangeSet<T>> = new_sets.iter().filter(keep).cloned().collect();
        let shared = Arc::new(find_shared_chunks(&a, &b, Some(text_diff)));

        let mut side_a = SpanCursor::new(&a, Some(shared.clone()), min_point_size);
        let mut side_b = SpanCursor::new(&b, Some(shared), min_point_size);

        text_diff.iter_gaps(|from_a, from_b, length| {
            compare(&mut side_a, from_a as i64, &mut side_b, from_b as i64, length as i64, comparator)
        });
        if text_diff.is_empty() && text_diff.length() == 0 {
            compare(&mut side_a, 0, &mut side_b, 0, 0, comparator);
        }
    }

    /// Compare the contents of two groups of range sets, returning true if
    /// they are equivalent in the given range (to the end when `to` is
    /// `None`).
    pub fn eq(old_sets: &[RangeSet<T>], new_sets: &[RangeSet<T>], from: usize, to: Option<usize>) -> bool {
        let to = to.map_or(FAR - 1, |t| t as i64);
        let a: Vec<RangeSet<T>> =
            old_sets.iter().filter(|s| !s.is_empty() && !new_sets.iter().any(|n| n.ptr_eq(s))).cloned().collect();
        let b: Vec<RangeSet<T>> =
            new_sets.iter().filter(|s| !s.is_empty() && !old_sets.iter().any(|o| o.ptr_eq(s))).cloned().collect();
        if a.len() != b.len() {
            return false;
        }
        if a.is_empty() {
            return true;
        }
        let shared = Arc::new(find_shared_chunks(&a, &b, None));
        let mut side_a = SpanCursor::new(&a, Some(shared.clone()), 0);
        let mut side_b = SpanCursor::new(&b, Some(shared), 0);
        side_a.goto(from as i64, -FAR);
        side_b.goto(from as i64, -FAR);
        loop {
            let points_differ = match (&side_a.point, &side_b.point) {
                (Some(pa), Some(pb)) => !cmp_val(pa, pb),
                (Some(_), None) => true,
                _ => false,
            };
            if side_a.to != side_b.to || !same_values(&side_a.active, &side_b.active) || points_differ {
                return false;
            }
            if side_a.to > to {
                return true;
            }
            side_a.next();
            side_b.next();
        }
    }

    /// Iterate over a group of range sets at the same time, notifying the
    /// iterator about the ranges covering every given piece of content.
    /// Returns the open count (see [`SpanIterator::span`]) at the end of
    /// the iteration. When `min_point_size` is greater than -1, only points
    /// of at least this size are taken into account.
    pub fn spans(
        sets: &[RangeSet<T>],
        from: usize,
        to: usize,
        iterator: &mut dyn SpanIterator<T>,
        min_point_size: i64,
    ) -> usize {
        let (from, to) = (from as i64, to as i64);
        let mut cursor = SpanCursor::new(sets, None, min_point_size);
        cursor.goto(from, -FAR);
        let mut pos = from;
        let mut open_ranges = cursor.open_start.max(0) as usize;
        loop {
            let cur_to = cursor.to.min(to);
            if let Some(point) = cursor.point.clone() {
                let active = cursor.active_for_point(cursor.to);
                let open_count = if cursor.point_from < from {
                    active.len() + 1
                } else if point.start_side() < 0 {
                    active.len()
                } else {
                    active.len().min(open_ranges)
                };
                iterator.point(pos as usize, cur_to as usize, &point, &active, open_count, cursor.point_rank);
                open_ranges = cursor.open_end(cur_to).min(active.len());
            } else if cur_to > pos {
                iterator.span(pos as usize, cur_to as usize, &cursor.active, open_ranges);
                open_ranges = cursor.open_end(cur_to);
            }
            if cursor.to > to {
                return open_ranges + usize::from(cursor.point.is_some() && cursor.to > to);
            }
            pos = cursor.to;
            cursor.next();
        }
    }

    /// Create a range set for the given ranges. By default, this expects
    /// the ranges to be _sorted_ (by start position and, if two start at
    /// the same position, `value.start_side()`). Pass `sort: true` to sort
    /// them.
    ///
    /// # Panics
    ///
    /// Panics when the ranges are not sorted and `sort` is false.
    pub fn of(ranges: Vec<Range<T>>, sort: bool) -> RangeSet<T> {
        let mut ranges = ranges;
        if sort && ranges.windows(2).any(|w| cmp_range(&w[0], &w[1]) == Ordering::Greater) {
            ranges.sort_by(cmp_range);
        }
        let mut build = RangeSetBuilder::new();
        for range in ranges {
            build.add(range.from, range.to, range.value);
        }
        build.finish()
    }

    /// Join a list of range sets into a single set.
    pub fn join(sets: &[RangeSet<T>]) -> RangeSet<T> {
        let Some(last) = sets.last() else { return RangeSet::empty() };
        let mut result = last.clone();
        for set in sets[..sets.len() - 1].iter().rev() {
            let mut layer = set.clone();
            while !layer.is_empty() {
                let max_point = layer.inner.max_point.max(result.inner.max_point);
                result = RangeSet::create(layer.inner.chunk_pos.clone(), layer.inner.chunk.clone(), result, max_point);
                layer = layer.next_layer();
            }
        }
        result
    }
}

/// A range set builder helps build up a [range set](RangeSet) directly,
/// without first allocating a list of [`Range`]s.
pub struct RangeSetBuilder<T> {
    chunks: Vec<Arc<Chunk<T>>>,
    chunk_pos: Vec<i64>,
    chunk_start: i64,
    last: Option<Arc<T>>,
    last_from: i64,
    last_to: i64,
    from: Vec<i64>,
    to: Vec<i64>,
    value: Vec<Arc<T>>,
    max_point: i64,
    set_max_point: i64,
    next_layer: Option<Box<RangeSetBuilder<T>>>,
}

impl<T: RangeValue> Default for RangeSetBuilder<T> {
    fn default() -> Self {
        RangeSetBuilder::new()
    }
}

impl<T: RangeValue> RangeSetBuilder<T> {
    /// Create an empty builder.
    pub fn new() -> Self {
        RangeSetBuilder {
            chunks: Vec::new(),
            chunk_pos: Vec::new(),
            chunk_start: -1,
            last: None,
            last_from: -FAR,
            last_to: -FAR,
            from: Vec::new(),
            to: Vec::new(),
            value: Vec::new(),
            max_point: -1,
            set_max_point: -1,
            next_layer: None,
        }
    }

    fn finish_chunk(&mut self) {
        let chunk = Chunk {
            from: std::mem::take(&mut self.from),
            to: std::mem::take(&mut self.to),
            value: std::mem::take(&mut self.value),
            max_point: self.max_point,
        };
        self.chunks.push(Arc::new(chunk));
        self.chunk_pos.push(self.chunk_start);
        self.chunk_start = -1;
        self.set_max_point = self.set_max_point.max(self.max_point);
        self.max_point = -1;
    }

    /// Add a range. Ranges should be added in sorted (by `from` and
    /// `value.start_side()`) order.
    ///
    /// # Panics
    ///
    /// Panics when ranges are added out of order.
    pub fn add(&mut self, from: usize, to: usize, value: Arc<T>) {
        self.add_range(from as i64, to as i64, value, true);
    }

    fn add_range(&mut self, from: i64, to: i64, value: Arc<T>, strict: bool) {
        if !self.add_inner(from, to, value.clone(), strict) {
            self.next_layer.get_or_insert_with(|| Box::new(RangeSetBuilder::new())).add_range(from, to, value, strict);
        }
    }

    fn add_inner(&mut self, from: i64, to: i64, value: Arc<T>, strict: bool) -> bool {
        let last_end = self.last.as_ref().map_or(0, |l| l.end_side());
        let last_start = self.last.as_ref().map_or(0, |l| l.start_side());
        let diff = cmp2(from, self.last_to, value.start_side(), last_end);
        if strict && diff <= 0 && cmp2(from, self.last_from, value.start_side(), last_start) < 0 {
            panic!("Ranges must be added sorted by `from` position and `startSide`");
        }
        if diff < 0 {
            return false;
        }
        if self.from.len() == CHUNK_SIZE {
            self.finish_chunk();
        }
        if self.chunk_start < 0 {
            self.chunk_start = from;
        }
        self.from.push(from - self.chunk_start);
        self.to.push(to - self.chunk_start);
        self.last_from = from;
        self.last_to = to;
        if value.point() {
            self.max_point = self.max_point.max(to - from);
        }
        self.last = Some(value.clone());
        self.value.push(value);
        true
    }

    fn add_chunk(&mut self, from: i64, chunk: Arc<Chunk<T>>) -> bool {
        let last_end = self.last.as_ref().map_or(0, |l| l.end_side());
        if cmp2(from, self.last_to, chunk.value[0].start_side(), last_end) < 0 {
            return false;
        }
        if !self.from.is_empty() {
            self.finish_chunk();
        }
        self.set_max_point = self.set_max_point.max(chunk.max_point);
        let last = chunk.value.len() - 1;
        self.last = Some(chunk.value[last].clone());
        self.last_from = chunk.from[last] + from;
        self.last_to = chunk.to[last] + from;
        self.chunks.push(chunk);
        self.chunk_pos.push(from);
        true
    }

    /// Finish the range set. Returns the new set.
    pub fn finish(self) -> RangeSet<T> {
        self.finish_inner(RangeSet::empty())
    }

    fn finish_inner(mut self, next: RangeSet<T>) -> RangeSet<T> {
        if !self.from.is_empty() {
            self.finish_chunk();
        }
        if self.chunks.is_empty() {
            return next;
        }
        let next = match self.next_layer.take() {
            Some(layer) => layer.finish_inner(next),
            None => next,
        };
        RangeSet::create(self.chunk_pos, self.chunks, next, self.set_max_point)
    }
}

type ChunkSet = HashSet<usize>;

fn chunk_key<T>(chunk: &Arc<Chunk<T>>) -> usize {
    Arc::as_ptr(chunk) as usize
}

fn find_shared_chunks<T: RangeValue>(a: &[RangeSet<T>], b: &[RangeSet<T>], text_diff: Option<&ChangeDesc>) -> ChunkSet {
    let mut in_a: HashMap<usize, i64> = HashMap::new();
    for set in a {
        for (i, chunk) in set.inner.chunk.iter().enumerate() {
            if chunk.max_point <= 0 {
                in_a.insert(chunk_key(chunk), set.inner.chunk_pos[i]);
            }
        }
    }
    let mut shared = HashSet::new();
    for set in b {
        for (i, chunk) in set.inner.chunk.iter().enumerate() {
            let Some(&known) = in_a.get(&chunk_key(chunk)) else { continue };
            let mapped = text_diff.map_or(known, |d| d.map_pos(known as usize, -1) as i64);
            let untouched = text_diff
                .is_none_or(|d| d.touches_range(known as usize, (known + chunk.length()) as usize) == RangeTouch::No);
            if mapped == set.inner.chunk_pos[i] && untouched {
                shared.insert(chunk_key(chunk));
            }
        }
    }
    shared
}

struct LayerCursor<T> {
    layer: RangeSet<T>,
    skip: Option<Arc<ChunkSet>>,
    min_point: i64,
    rank: usize,
    from: i64,
    to: i64,
    value: Option<Arc<T>>,
    chunk_index: usize,
    range_index: usize,
}

impl<T: RangeValue> LayerCursor<T> {
    fn new(layer: RangeSet<T>, skip: Option<Arc<ChunkSet>>, min_point: i64, rank: usize) -> Self {
        LayerCursor { layer, skip, min_point, rank, from: 0, to: 0, value: None, chunk_index: 0, range_index: 0 }
    }

    fn start_side(&self) -> i64 {
        self.value.as_ref().map_or(0, |v| v.start_side())
    }

    fn end_side(&self) -> i64 {
        self.value.as_ref().map_or(0, |v| v.end_side())
    }

    fn skipped(&self, index: usize) -> bool {
        self.skip.as_ref().is_some_and(|s| s.contains(&chunk_key(&self.layer.inner.chunk[index])))
    }

    fn goto(&mut self, pos: i64, side: i64) {
        self.chunk_index = 0;
        self.range_index = 0;
        self.goto_inner(pos, side, false);
    }

    fn goto_inner(&mut self, pos: i64, side: i64, mut forward: bool) {
        let chunks = self.layer.inner.chunk.len();
        while self.chunk_index < chunks {
            let next = &self.layer.inner.chunk[self.chunk_index];
            if !(self.skipped(self.chunk_index)
                || self.layer.chunk_end(self.chunk_index) < pos
                || next.max_point < self.min_point)
            {
                break;
            }
            self.chunk_index += 1;
            forward = false;
        }
        if self.chunk_index < chunks {
            let range_index = self.layer.inner.chunk[self.chunk_index].find_index(
                pos - self.layer.inner.chunk_pos[self.chunk_index],
                side,
                true,
                0,
            );
            if !forward || self.range_index < range_index {
                self.set_range_index(range_index);
            }
        }
        self.next();
    }

    fn forward(&mut self, pos: i64, side: i64) {
        if cmp2(self.to, pos, self.end_side(), side) < 0 {
            self.goto_inner(pos, side, true);
        }
    }

    fn next(&mut self) {
        loop {
            if self.chunk_index == self.layer.inner.chunk.len() {
                self.from = FAR;
                self.to = FAR;
                self.value = None;
                break;
            }
            let chunk_pos = self.layer.inner.chunk_pos[self.chunk_index];
            let chunk = self.layer.inner.chunk[self.chunk_index].clone();
            self.from = chunk_pos + chunk.from[self.range_index];
            self.to = chunk_pos + chunk.to[self.range_index];
            let value = chunk.value[self.range_index].clone();
            self.set_range_index(self.range_index + 1);
            let accept = self.min_point < 0 || (value.point() && self.to - self.from >= self.min_point);
            self.value = Some(value);
            if accept {
                break;
            }
        }
    }

    fn set_range_index(&mut self, index: usize) {
        if index == self.layer.inner.chunk[self.chunk_index].value.len() {
            self.chunk_index += 1;
            if self.skip.is_some() {
                while self.chunk_index < self.layer.inner.chunk.len() && self.skipped(self.chunk_index) {
                    self.chunk_index += 1;
                }
            }
            self.range_index = 0;
        } else {
            self.range_index = index;
        }
    }

    fn next_chunk(&mut self) {
        self.chunk_index += 1;
        self.range_index = 0;
        self.next();
    }

    fn compare(&self, other: &LayerCursor<T>) -> Ordering {
        self.from
            .cmp(&other.from)
            .then(self.start_side().cmp(&other.start_side()))
            .then(self.rank.cmp(&other.rank))
            .then(self.to.cmp(&other.to))
            .then(self.end_side().cmp(&other.end_side()))
    }
}

struct HeapCursor<T> {
    heap: Vec<LayerCursor<T>>,
    from: i64,
    to: i64,
    value: Option<Arc<T>>,
    rank: isize,
}

impl<T: RangeValue> HeapCursor<T> {
    fn goto(&mut self, pos: i64, side: i64) {
        for cur in &mut self.heap {
            cur.goto(pos, side);
        }
        for i in (0..=self.heap.len() >> 1).rev() {
            heap_bubble(&mut self.heap, i);
        }
        self.next();
    }

    fn forward(&mut self, pos: i64, side: i64) {
        for cur in &mut self.heap {
            cur.forward(pos, side);
        }
        for i in (0..=self.heap.len() >> 1).rev() {
            heap_bubble(&mut self.heap, i);
        }
        let end_side = self.value.as_ref().map_or(0, |v| v.end_side());
        if cmp2(self.to, pos, end_side, side) < 0 {
            self.next();
        }
    }

    fn next(&mut self) {
        if self.heap.is_empty() {
            self.from = FAR;
            self.to = FAR;
            self.value = None;
            self.rank = -1;
        } else {
            let top = &mut self.heap[0];
            self.from = top.from;
            self.to = top.to;
            self.value = top.value.clone();
            self.rank = top.rank as isize;
            if top.value.is_some() {
                top.next();
            }
            heap_bubble(&mut self.heap, 0);
        }
    }
}

fn heap_bubble<T: RangeValue>(heap: &mut [LayerCursor<T>], mut index: usize) {
    loop {
        let mut child_index = (index << 1) + 1;
        if child_index >= heap.len() {
            break;
        }
        if child_index + 1 < heap.len() && heap[child_index].compare(&heap[child_index + 1]) != Ordering::Less {
            child_index += 1;
        }
        if heap[index].compare(&heap[child_index]) == Ordering::Less {
            break;
        }
        heap.swap(index, child_index);
        index = child_index;
    }
}

enum Cursor<T> {
    Layer(LayerCursor<T>),
    Heap(HeapCursor<T>),
}

impl<T: RangeValue> Cursor<T> {
    fn from_sets(sets: &[RangeSet<T>], skip: Option<Arc<ChunkSet>>, min_point: i64) -> Cursor<T> {
        let mut heap = Vec::new();
        for (i, set) in sets.iter().enumerate() {
            let mut cur = set.clone();
            while !cur.is_empty() {
                if cur.inner.max_point >= min_point {
                    heap.push(LayerCursor::new(cur.clone(), skip.clone(), min_point, i));
                }
                cur = cur.next_layer();
            }
        }
        if heap.len() == 1 {
            Cursor::Layer(heap.pop().unwrap())
        } else {
            Cursor::Heap(HeapCursor { heap, from: 0, to: 0, value: None, rank: -1 })
        }
    }

    fn from(&self) -> i64 {
        match self {
            Cursor::Layer(c) => c.from,
            Cursor::Heap(c) => c.from,
        }
    }

    fn to(&self) -> i64 {
        match self {
            Cursor::Layer(c) => c.to,
            Cursor::Heap(c) => c.to,
        }
    }

    fn value(&self) -> Option<&Arc<T>> {
        match self {
            Cursor::Layer(c) => c.value.as_ref(),
            Cursor::Heap(c) => c.value.as_ref(),
        }
    }

    fn rank(&self) -> usize {
        match self {
            Cursor::Layer(c) => c.rank,
            Cursor::Heap(c) => c.rank.max(0) as usize,
        }
    }

    fn start_side(&self) -> i64 {
        self.value().map_or(0, |v| v.start_side())
    }

    fn goto(&mut self, pos: i64, side: i64) {
        match self {
            Cursor::Layer(c) => c.goto(pos, side),
            Cursor::Heap(c) => c.goto(pos, side),
        }
    }

    fn forward(&mut self, pos: i64, side: i64) {
        match self {
            Cursor::Layer(c) => c.forward(pos, side),
            Cursor::Heap(c) => c.forward(pos, side),
        }
    }

    fn next(&mut self) {
        match self {
            Cursor::Layer(c) => c.next(),
            Cursor::Heap(c) => c.next(),
        }
    }
}

/// A range cursor moves to the next range every time you call
/// [`next`](RangeCursor::next) on it. It starts out pointing at the first
/// range, so call `next` only after reading the first range (if any).
pub struct RangeCursor<T>(Cursor<T>);

impl<T: RangeValue> RangeCursor<T> {
    /// The current range's value, or `None` when the cursor has reached
    /// its end.
    pub fn value(&self) -> Option<&Arc<T>> {
        self.0.value()
    }

    /// The current range's start position.
    pub fn from(&self) -> usize {
        self.0.from() as usize
    }

    /// The current range's end position.
    pub fn to(&self) -> usize {
        self.0.to() as usize
    }

    /// The position of the set that this range comes from in the list of
    /// sets being iterated over.
    pub fn rank(&self) -> usize {
        self.0.rank()
    }

    /// Move the cursor forward.
    pub fn next(&mut self) {
        self.0.next()
    }

    /// Jump the cursor to the given position.
    pub fn goto(&mut self, pos: usize) {
        self.0.goto(pos as i64, -FAR)
    }
}

struct SpanCursor<T> {
    cursor: Cursor<T>,
    active: Vec<Arc<T>>,
    active_to: Vec<i64>,
    active_rank: Vec<usize>,
    min_active: isize,
    // A currently active point range, if any
    point: Option<Arc<T>>,
    point_from: i64,
    point_rank: usize,
    to: i64,
    end_side: i64,
    // The amount of open active ranges at the start of the iterator. Not
    // including points.
    open_start: isize,
}

impl<T: RangeValue> SpanCursor<T> {
    fn new(sets: &[RangeSet<T>], skip: Option<Arc<ChunkSet>>, min_point: i64) -> Self {
        SpanCursor {
            cursor: Cursor::from_sets(sets, skip, min_point),
            active: Vec::new(),
            active_to: Vec::new(),
            active_rank: Vec::new(),
            min_active: -1,
            point: None,
            point_from: 0,
            point_rank: 0,
            to: -FAR,
            end_side: 0,
            open_start: -1,
        }
    }

    fn goto(&mut self, pos: i64, side: i64) {
        self.cursor.goto(pos, side);
        self.active.clear();
        self.active_to.clear();
        self.active_rank.clear();
        self.min_active = -1;
        self.to = pos;
        self.end_side = side;
        self.open_start = -1;
        self.next();
    }

    fn forward(&mut self, pos: i64, side: i64) {
        while self.min_active > -1 {
            let a = self.min_active as usize;
            if cmp2(self.active_to[a], pos, self.active[a].end_side(), side) >= 0 {
                break;
            }
            self.remove_active(a);
        }
        self.cursor.forward(pos, side);
    }

    fn remove_active(&mut self, index: usize) {
        self.active.remove(index);
        self.active_to.remove(index);
        self.active_rank.remove(index);
        self.min_active = find_min_index(&self.active, &self.active_to);
    }

    fn add_active(&mut self, track_open: Option<&mut Vec<i64>>) {
        let (value, to, rank, from) =
            (self.cursor.value().unwrap().clone(), self.cursor.to(), self.cursor.rank(), self.cursor.from());
        let mut i = 0;
        // Organize active marks by rank first, then by size
        while i < self.active_rank.len() && cmp2(rank as i64, self.active_rank[i] as i64, to, self.active_to[i]) > 0 {
            i += 1;
        }
        self.active.insert(i, value);
        self.active_to.insert(i, to);
        self.active_rank.insert(i, rank);
        if let Some(track_open) = track_open {
            track_open.insert(i, from);
        }
        self.min_active = find_min_index(&self.active, &self.active_to);
    }

    // After calling this, if `self.point` is set, the next range is a
    // point. Otherwise, it's a regular range, covered by `self.active`.
    fn next(&mut self) {
        let from = self.to;
        let was_point = self.point.take().is_some();
        let mut track_open: Option<Vec<i64>> = if self.open_start < 0 { Some(Vec::new()) } else { None };
        loop {
            let a = self.min_active;
            if a > -1
                && cmp2(
                    self.active_to[a as usize],
                    self.cursor.from(),
                    self.active[a as usize].end_side(),
                    self.cursor.start_side(),
                ) < 0
            {
                let a = a as usize;
                if self.active_to[a] > from {
                    self.to = self.active_to[a];
                    self.end_side = self.active[a].end_side();
                    break;
                }
                self.remove_active(a);
                if let Some(t) = track_open.as_mut() {
                    t.remove(a);
                }
            } else if self.cursor.value().is_none() {
                self.to = FAR;
                self.end_side = FAR;
                break;
            } else if self.cursor.from() > from {
                self.to = self.cursor.from();
                self.end_side = self.cursor.start_side();
                break;
            } else {
                let next_val = self.cursor.value().unwrap().clone();
                if !next_val.point() {
                    // Opening a range
                    self.add_active(track_open.as_mut());
                    self.cursor.next();
                } else if was_point && self.cursor.to() == self.to && self.cursor.from() < self.cursor.to() {
                    // Ignore any non-empty points that end precisely at the
                    // end of the prev point
                    self.cursor.next();
                } else {
                    // New point
                    self.point_from = self.cursor.from();
                    self.point_rank = self.cursor.rank();
                    self.to = self.cursor.to();
                    self.end_side = next_val.end_side();
                    self.point = Some(next_val);
                    self.cursor.next();
                    self.forward(self.to, self.end_side);
                    break;
                }
            }
        }
        if let Some(track_open) = track_open {
            self.open_start = 0;
            for &start in track_open.iter().rev() {
                if start >= from {
                    break;
                }
                self.open_start += 1;
            }
        }
    }

    fn active_for_point(&self, to: i64) -> Vec<Arc<T>> {
        if self.active.is_empty() {
            return Vec::new();
        }
        let point_end = self.point.as_ref().map_or(0, |p| p.end_side());
        let mut active = Vec::new();
        for i in (0..self.active.len()).rev() {
            if self.active_rank[i] < self.point_rank {
                break;
            }
            if self.active_to[i] > to || (self.active_to[i] == to && self.active[i].end_side() >= point_end) {
                active.push(self.active[i].clone());
            }
        }
        active.reverse();
        active
    }

    fn open_end(&self, to: i64) -> usize {
        self.active_to.iter().rev().take_while(|&&t| t > to).count()
    }
}

fn compare<T: RangeValue>(
    a: &mut SpanCursor<T>,
    start_a: i64,
    b: &mut SpanCursor<T>,
    start_b: i64,
    length: i64,
    comparator: &mut dyn RangeComparator<T>,
) {
    a.goto(start_a, -FAR);
    b.goto(start_b, -FAR);
    let end_b = start_b + length;
    let mut pos = start_b;
    let d_pos = start_b - start_a;
    let bounds = comparator.wants_bound_changes();
    let mut bound_change = false;
    loop {
        let d_end = (a.to + d_pos) - b.to;
        let diff = if d_end != 0 { d_end } else { a.end_side - b.end_side };
        let end = if diff < 0 { a.to + d_pos } else { b.to };
        let clip_end = end.min(end_b);
        if a.point.is_some() || b.point.is_some() {
            let same = match (&a.point, &b.point) {
                (Some(pa), Some(pb)) => {
                    cmp_val(pa, pb) && same_values(&a.active_for_point(a.to), &b.active_for_point(b.to))
                }
                _ => false,
            };
            if !same {
                comparator.compare_point(pos as usize, clip_end as usize, a.point.as_ref(), b.point.as_ref());
            }
            bound_change = false;
        } else {
            if bound_change {
                comparator.bound_change(pos as usize);
                bound_change = false;
            }
            if clip_end > pos && !same_values(&a.active, &b.active) {
                comparator.compare_range(pos as usize, clip_end as usize, &a.active, &b.active);
            }
            if bounds && clip_end < end_b && (d_end != 0 || a.open_end(end) != b.open_end(end)) {
                bound_change = true;
            }
        }
        if end > end_b {
            break;
        }
        pos = end;
        if diff <= 0 {
            a.next();
        }
        if diff >= 0 {
            b.next();
        }
    }
}

fn same_values<T: RangeValue>(a: &[Arc<T>], b: &[Arc<T>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| cmp_val(x, y))
}

fn find_min_index<T: RangeValue>(value: &[Arc<T>], array: &[i64]) -> isize {
    let mut found: isize = -1;
    let mut found_pos = FAR;
    for i in 0..array.len() {
        let less = if array[i] != found_pos {
            array[i] < found_pos
        } else {
            found >= 0 && value[i].end_side() < value[found as usize].end_side()
        };
        if less {
            found = i as isize;
            found_pos = array[i];
        }
    }
    found
}
