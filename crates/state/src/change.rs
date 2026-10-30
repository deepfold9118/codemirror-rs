//! Document changes. Port of `change.ts`.

use std::fmt;

use crate::text::{normalize_line_breaks, Text};

/// Distinguishes different ways in which positions can be mapped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapMode {
    /// Map a position to a valid new position, even when its context was
    /// deleted.
    Simple,
    /// Return `None` if deletion happens across the position.
    TrackDel,
    /// Return `None` if the character _before_ the position is deleted.
    TrackBefore,
    /// Return `None` if the character _after_ the position is deleted.
    TrackAfter,
}

/// The result of [`ChangeDesc::touches_range`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeTouch {
    /// No change touches the range.
    No,
    /// A change touches the range.
    Yes,
    /// A single change entirely covers the range.
    Cover,
}

// Sections are encoded as pairs of integers. The first is the length in
// the current document, and the second is -1 for unaffected sections, and
// the length of the replacement content otherwise. So an insertion would
// be (0, n>0), a deletion (n>0, 0), and a replacement two positive numbers.
type Sections = Vec<isize>;

/// A change description is a variant of [`ChangeSet`] that doesn't store
/// the inserted text. As such, it can't be applied, but is cheaper to store
/// and manipulate.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct ChangeDesc {
    pub(crate) sections: Sections,
}

impl ChangeDesc {
    pub(crate) fn create(sections: Sections) -> ChangeDesc {
        ChangeDesc { sections }
    }

    /// Build a description from its sections, as `(length, inserted)`
    /// pairs where `inserted` is `None` for unchanged sections.
    pub fn from_sections(sections: &[(usize, Option<usize>)]) -> ChangeDesc {
        let mut out = Vec::with_capacity(sections.len() * 2);
        for &(len, ins) in sections {
            out.push(len as isize);
            out.push(ins.map_or(-1, |i| i as isize));
        }
        ChangeDesc { sections: out }
    }

    /// The length of the document before the change.
    pub fn length(&self) -> usize {
        self.sections.iter().step_by(2).sum::<isize>() as usize
    }

    /// The length of the document after the change.
    pub fn new_length(&self) -> usize {
        let mut result = 0;
        for pair in self.sections.chunks(2) {
            result += if pair[1] < 0 { pair[0] } else { pair[1] };
        }
        result as usize
    }

    /// False when there are actual changes in this set.
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty() || (self.sections.len() == 2 && self.sections[1] < 0)
    }

    /// Iterate over the unchanged parts left by these changes. `posA`
    /// provides the position of the range in the old document, `posB` the
    /// new position in the changed document.
    pub fn iter_gaps(&self, mut f: impl FnMut(usize, usize, usize)) {
        let (mut pos_a, mut pos_b) = (0, 0);
        for pair in self.sections.chunks(2) {
            let (len, ins) = (pair[0] as usize, pair[1]);
            if ins < 0 {
                f(pos_a, pos_b, len);
                pos_b += len;
            } else {
                pos_b += ins as usize;
            }
            pos_a += len;
        }
    }

    /// Iterate over the ranges changed by these changes. `fromA`/`toA`
    /// provides the extent of the change in the starting document,
    /// `fromB`/`toB` the extent of the replacement in the changed document.
    ///
    /// When `individual` is true, adjacent changes (which are kept separate
    /// for [position mapping](ChangeDesc::map_pos)) are reported separately.
    pub fn iter_changed_ranges(&self, mut f: impl FnMut(usize, usize, usize, usize), individual: bool) {
        iter_changes(&self.sections, None, |fa, ta, fb, tb, _| f(fa, ta, fb, tb), individual)
    }

    /// Get a description of the inverted form of these changes.
    pub fn inverted_desc(&self) -> ChangeDesc {
        let mut sections = Vec::with_capacity(self.sections.len());
        for pair in self.sections.chunks(2) {
            let (len, ins) = (pair[0], pair[1]);
            if ins < 0 {
                sections.extend([len, ins]);
            } else {
                sections.extend([ins, len]);
            }
        }
        ChangeDesc { sections }
    }

    /// Compute the combined effect of applying another set of changes after
    /// this one. The length of the document after this set should match
    /// the length before `other`.
    ///
    /// # Panics
    ///
    /// Panics when the lengths don't match.
    pub fn compose_desc(&self, other: &ChangeDesc) -> ChangeDesc {
        if self.is_empty() {
            other.clone()
        } else if other.is_empty() {
            self.clone()
        } else {
            ChangeDesc::create(compose_sets(&self.sections, None, &other.sections, None).0)
        }
    }

    /// Map this description, which should start with the same document as
    /// `other`, over another set of changes, so that it can be applied
    /// after it. When `before` is true, map as if the changes in `self`
    /// happened before the ones in `other`.
    pub fn map_desc(&self, other: &ChangeDesc, before: bool) -> ChangeDesc {
        if other.is_empty() {
            self.clone()
        } else {
            ChangeDesc::create(map_set(&self.sections, None, &other.sections, before).0)
        }
    }

    /// Map a given position through these changes, to produce a position
    /// pointing into the new document.
    ///
    /// `assoc` indicates which side the position should be associated with.
    /// When it is negative, the mapping will try to keep the position close
    /// to the character before it (if any), and will move it before
    /// insertions at that point or replacements across that point. When it
    /// is zero or positive, the position is associated with the character
    /// after it, and will be moved forward for insertions at or
    /// replacements across the position. CodeMirror's default is -1.
    ///
    /// # Panics
    ///
    /// Panics when `pos` is outside the document.
    pub fn map_pos(&self, pos: usize, assoc: i32) -> usize {
        self.map_pos_mode(pos, assoc, MapMode::Simple).expect("simple mapping always produces a position")
    }

    /// Like [`map_pos`](ChangeDesc::map_pos), but returns `None` when
    /// `mode` asks to report a deletion that happened around the position.
    pub fn map_pos_mode(&self, pos: usize, assoc: i32, mode: MapMode) -> Option<usize> {
        let pos = pos as isize;
        let (mut pos_a, mut pos_b) = (0isize, 0isize);
        for pair in self.sections.chunks(2) {
            let (len, ins) = (pair[0], pair[1]);
            let end_a = pos_a + len;
            if ins < 0 {
                if end_a > pos {
                    return Some((pos_b + (pos - pos_a)) as usize);
                }
                pos_b += len;
            } else {
                if mode != MapMode::Simple
                    && end_a >= pos
                    && ((mode == MapMode::TrackDel && pos_a < pos && end_a > pos)
                        || (mode == MapMode::TrackBefore && pos_a < pos)
                        || (mode == MapMode::TrackAfter && end_a > pos))
                {
                    return None;
                }
                if end_a > pos || (end_a == pos && assoc < 0 && len == 0) {
                    return Some(if pos == pos_a || assoc < 0 { pos_b } else { pos_b + ins } as usize);
                }
                pos_b += ins;
            }
            pos_a = end_a;
        }
        assert!(pos <= pos_a, "Position {pos} is out of range for changeset of length {pos_a}");
        Some(pos_b as usize)
    }

    /// Check whether these changes touch a given range.
    pub fn touches_range(&self, from: usize, to: usize) -> RangeTouch {
        let mut pos = 0;
        for pair in self.sections.chunks(2) {
            if pos > to {
                break;
            }
            let (len, ins) = (pair[0] as usize, pair[1]);
            let end = pos + len;
            if ins >= 0 && pos <= to && end >= from {
                return if pos < from && end > to { RangeTouch::Cover } else { RangeTouch::Yes };
            }
            pos = end;
        }
        RangeTouch::No
    }
}

impl fmt::Display for ChangeDesc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, pair) in self.sections.chunks(2).enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{}", pair[0])?;
            if pair[1] >= 0 {
                write!(f, ":{}", pair[1])?;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for ChangeDesc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChangeDesc({self})")
    }
}

/// Text to insert in a [`ChangeSpec`].
#[derive(Clone, Debug)]
pub enum Insert {
    /// A string, split into lines with the state's line separator.
    Str(String),
    /// A ready-made document.
    Text(Text),
}

impl From<&str> for Insert {
    fn from(s: &str) -> Insert {
        Insert::Str(s.to_string())
    }
}

impl From<String> for Insert {
    fn from(s: String) -> Insert {
        Insert::Str(s)
    }
}

impl From<Text> for Insert {
    fn from(t: Text) -> Insert {
        Insert::Text(t)
    }
}

impl From<&Text> for Insert {
    fn from(t: &Text) -> Insert {
        Insert::Text(t.clone())
    }
}

/// Describes document changes succinctly: a single change (a deletion,
/// insertion, or replacement, depending on which fields are present), a
/// [`ChangeSet`], or a list of change specs.
#[derive(Clone, Debug)]
pub enum ChangeSpec {
    /// Replace `from..to` (or insert at `from` when `to` is `None`) with
    /// `insert`.
    Change { from: usize, to: Option<usize>, insert: Option<Insert> },
    /// An existing change set.
    Set(ChangeSet),
    /// Several specs, all in the coordinates of the starting document.
    List(Vec<ChangeSpec>),
}

impl ChangeSpec {
    /// Insert text at `at`.
    pub fn insert(at: usize, text: impl Into<Insert>) -> ChangeSpec {
        ChangeSpec::Change { from: at, to: None, insert: Some(text.into()) }
    }

    /// Replace `from..to` with text.
    pub fn replace(from: usize, to: usize, text: impl Into<Insert>) -> ChangeSpec {
        ChangeSpec::Change { from, to: Some(to), insert: Some(text.into()) }
    }

    /// Delete `from..to`.
    pub fn delete(from: usize, to: usize) -> ChangeSpec {
        ChangeSpec::Change { from, to: Some(to), insert: None }
    }

    /// No changes.
    pub fn none() -> ChangeSpec {
        ChangeSpec::List(Vec::new())
    }
}

impl From<ChangeSet> for ChangeSpec {
    fn from(set: ChangeSet) -> ChangeSpec {
        ChangeSpec::Set(set)
    }
}

impl From<Vec<ChangeSpec>> for ChangeSpec {
    fn from(list: Vec<ChangeSpec>) -> ChangeSpec {
        ChangeSpec::List(list)
    }
}

/// A change set represents a group of modifications to a document. It
/// stores the document length, and can only be applied to documents with
/// exactly that length.
///
/// A change set dereferences to its [`ChangeDesc`], so the description's
/// methods (`map_pos`, `length`, ...) can be called on it directly.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct ChangeSet {
    desc: ChangeDesc,
    pub(crate) inserted: Vec<Text>,
}

impl std::ops::Deref for ChangeSet {
    type Target = ChangeDesc;
    fn deref(&self) -> &ChangeDesc {
        &self.desc
    }
}

impl ChangeSet {
    pub(crate) fn create_set(sections: Sections, inserted: Vec<Text>) -> ChangeSet {
        ChangeSet { desc: ChangeDesc { sections }, inserted }
    }

    /// Apply the changes to a document, returning the modified document.
    ///
    /// # Panics
    ///
    /// Panics when the document's length doesn't match the change set.
    pub fn apply(&self, doc: &Text) -> Text {
        assert!(self.length() == doc.len(), "Applying change set to a document with the wrong length");
        let mut doc = doc.clone();
        iter_changes(
            &self.sections,
            Some(&self.inserted),
            |from_a, to_a, from_b, _to_b, text| {
                doc = doc.replace(from_b, from_b + (to_a - from_a), &text);
            },
            false,
        );
        doc
    }

    /// Given the document as it existed _before_ the changes, return a
    /// change set that represents the inverse of this set, which could be
    /// used to go from the document created by the changes back to the
    /// document as it existed before the changes.
    pub fn invert(&self, doc: &Text) -> ChangeSet {
        let mut sections = self.sections.clone();
        let mut inserted = Vec::new();
        let mut pos = 0usize;
        let mut i = 0;
        while i < sections.len() {
            let (len, ins) = (sections[i], sections[i + 1]);
            if ins >= 0 {
                sections[i] = ins;
                sections[i + 1] = len;
                let index = i >> 1;
                while inserted.len() < index {
                    inserted.push(Text::empty());
                }
                inserted.push(if len > 0 { doc.slice(pos, pos + len as usize) } else { Text::empty() });
            }
            pos += len as usize;
            i += 2;
        }
        ChangeSet::create_set(sections, inserted)
    }

    /// Combine two subsequent change sets into a single set. `other` must
    /// start in the document produced by `self`. If `self` goes `docA` →
    /// `docB` and `other` represents `docB` → `docC`, the returned value
    /// will represent the change `docA` → `docC`.
    pub fn compose(&self, other: &ChangeSet) -> ChangeSet {
        if self.is_empty() {
            other.clone()
        } else if other.is_empty() {
            self.clone()
        } else {
            let (sections, inserted) =
                compose_sets(&self.sections, Some(&self.inserted), &other.sections, Some(&other.inserted));
            ChangeSet::create_set(sections, inserted.unwrap())
        }
    }

    /// Given another change set starting in the same document, maps this
    /// change set over the other, producing a new change set that can be
    /// applied to the document produced by applying `other`. When `before`
    /// is `true`, order changes as if `self` comes before `other`,
    /// otherwise (the default in CodeMirror) treat `other` as coming first.
    ///
    /// Given two changes `A` and `B`, `A.compose(B.map(A))` and
    /// `B.compose(A.map(B, true))` will produce the same document. This
    /// provides a basic form of operational transformation, and can be used
    /// for collaborative editing.
    pub fn map(&self, other: &ChangeDesc, before: bool) -> ChangeSet {
        if other.is_empty() {
            self.clone()
        } else {
            let (sections, inserted) = map_set(&self.sections, Some(&self.inserted), &other.sections, before);
            ChangeSet::create_set(sections, inserted.unwrap())
        }
    }

    /// Iterate over the changed ranges in the document, calling `f` for
    /// each, with the range in the original document (`fromA`-`toA`) and
    /// the range that replaces it in the new document (`fromB`-`toB`).
    ///
    /// When `individual` is true, adjacent changes are reported separately.
    pub fn iter_changes(&self, f: impl FnMut(usize, usize, usize, usize, Text), individual: bool) {
        iter_changes(&self.sections, Some(&self.inserted), f, individual)
    }

    /// Get a [change description](ChangeDesc) for this change set.
    pub fn desc(&self) -> &ChangeDesc {
        &self.desc
    }

    /// Split this set into the changes outside `ranges` (pairs of
    /// positions) and a description of the changes that were filtered out.
    pub(crate) fn filter(&self, ranges: &[usize]) -> (ChangeSet, ChangeDesc) {
        let mut result_sections = Vec::new();
        let mut result_inserted = Vec::new();
        let mut filtered_sections = Vec::new();
        let mut iter = SectionIter::new(&self.sections, Some(&self.inserted));
        let mut i = 0;
        let mut pos = 0isize;
        'done: loop {
            let next = if i == ranges.len() {
                isize::MAX
            } else {
                i += 1;
                ranges[i - 1] as isize
            };
            while pos < next || (pos == next && iter.len == 0) {
                if iter.done() {
                    break 'done;
                }
                let len = iter.len.min(next - pos);
                add_section(&mut filtered_sections, len, -1, false);
                let ins = if iter.ins == -1 {
                    -1
                } else if iter.off == 0 {
                    iter.ins
                } else {
                    0
                };
                add_section(&mut result_sections, len, ins, false);
                if ins > 0 {
                    add_insert(&mut result_inserted, &result_sections, iter.text());
                }
                iter.forward(len);
                pos += len;
            }
            let end = ranges[i] as isize;
            i += 1;
            while pos < end {
                if iter.done() {
                    break 'done;
                }
                let len = iter.len.min(end - pos);
                add_section(&mut result_sections, len, -1, false);
                let ins = if iter.ins == -1 {
                    -1
                } else if iter.off == 0 {
                    iter.ins
                } else {
                    0
                };
                add_section(&mut filtered_sections, len, ins, false);
                iter.forward(len);
                pos += len;
            }
        }
        (ChangeSet::create_set(result_sections, result_inserted), ChangeDesc::create(filtered_sections))
    }

    /// Create a change set for the given changes, for a document of the
    /// given length, using `line_sep` as line separator (or any of `"\n"`,
    /// `"\r\n"`, `"\r"` when `None`).
    ///
    /// # Panics
    ///
    /// Panics when a change's range is invalid, or when a nested change set
    /// has the wrong length.
    pub fn of(changes: impl Into<ChangeSpec>, length: usize, line_sep: Option<&str>) -> ChangeSet {
        let mut builder = Builder { sections: Vec::new(), inserted: Vec::new(), pos: 0, total: None, length, line_sep };
        builder.process(&changes.into());
        let force = builder.total.is_none();
        builder.flush(force);
        builder.total.unwrap()
    }

    /// Create an empty change set of the given length.
    pub fn empty(length: usize) -> ChangeSet {
        ChangeSet::create_set(if length > 0 { vec![length as isize, -1] } else { Vec::new() }, Vec::new())
    }
}

impl fmt::Display for ChangeSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.desc.fmt(f)
    }
}

impl fmt::Debug for ChangeSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChangeSet({})", self.desc)
    }
}

struct Builder<'a> {
    sections: Sections,
    inserted: Vec<Text>,
    pos: usize,
    total: Option<ChangeSet>,
    length: usize,
    line_sep: Option<&'a str>,
}

impl Builder<'_> {
    fn flush(&mut self, force: bool) {
        if !force && self.sections.is_empty() {
            return;
        }
        if self.pos < self.length {
            add_section(&mut self.sections, (self.length - self.pos) as isize, -1, false);
        }
        let set = ChangeSet::create_set(std::mem::take(&mut self.sections), std::mem::take(&mut self.inserted));
        self.total = Some(match self.total.take() {
            Some(total) => total.compose(&set.map(&total, false)),
            None => set,
        });
        self.pos = 0;
    }

    fn process(&mut self, spec: &ChangeSpec) {
        match spec {
            ChangeSpec::List(list) => {
                for sub in list {
                    self.process(sub);
                }
            }
            ChangeSpec::Set(set) => {
                assert!(
                    set.length() == self.length,
                    "Mismatched change set length (got {}, expected {})",
                    set.length(),
                    self.length
                );
                self.flush(false);
                self.total = Some(match self.total.take() {
                    Some(total) => total.compose(&set.map(&total, false)),
                    None => set.clone(),
                });
            }
            ChangeSpec::Change { from, to, insert } => {
                let (from, to) = (*from, to.unwrap_or(*from));
                assert!(
                    from <= to && to <= self.length,
                    "Invalid change range {from} to {to} (in doc of length {})",
                    self.length
                );
                let ins_text = match insert {
                    None => Text::empty(),
                    Some(Insert::Text(t)) => t.clone(),
                    Some(Insert::Str(s)) => {
                        Text::from_str_with_separator(&normalize_line_breaks(s, self.line_sep), Some("\n"))
                    }
                };
                let ins_len = ins_text.len();
                if from == to && ins_len == 0 {
                    return;
                }
                if from < self.pos {
                    self.flush(false);
                }
                if from > self.pos {
                    add_section(&mut self.sections, (from - self.pos) as isize, -1, false);
                }
                add_section(&mut self.sections, (to - from) as isize, ins_len as isize, false);
                add_insert(&mut self.inserted, &self.sections, ins_text);
                self.pos = to;
            }
        }
    }
}

fn add_section(sections: &mut Sections, len: isize, ins: isize, force_join: bool) {
    if len == 0 && ins <= 0 {
        return;
    }
    let n = sections.len();
    if n >= 2 && ins <= 0 && ins == sections[n - 1] {
        sections[n - 2] += len;
    } else if n >= 2 && len == 0 && sections[n - 2] == 0 {
        sections[n - 1] += ins;
    } else if force_join {
        sections[n - 2] += len;
        sections[n - 1] += ins;
    } else {
        sections.push(len);
        sections.push(ins);
    }
}

fn add_insert(values: &mut Vec<Text>, sections: &Sections, value: Text) {
    if value.is_empty() {
        return;
    }
    let index = (sections.len() - 2) >> 1;
    if index < values.len() {
        let last = values.last_mut().unwrap();
        *last = last.append(&value);
    } else {
        while values.len() < index {
            values.push(Text::empty());
        }
        values.push(value);
    }
}

fn iter_changes(
    sections: &[isize],
    inserted: Option<&[Text]>,
    mut f: impl FnMut(usize, usize, usize, usize, Text),
    individual: bool,
) {
    let (mut pos_a, mut pos_b) = (0usize, 0usize);
    let mut i = 0;
    while i < sections.len() {
        let (mut len, mut ins) = (sections[i], sections[i + 1]);
        i += 2;
        if ins < 0 {
            pos_a += len as usize;
            pos_b += len as usize;
        } else {
            let (mut end_a, mut end_b) = (pos_a, pos_b);
            let mut text = Text::empty();
            loop {
                end_a += len as usize;
                end_b += ins as usize;
                if ins > 0 {
                    if let Some(inserted) = inserted {
                        if let Some(t) = inserted.get((i - 2) >> 1) {
                            text = text.append(t);
                        }
                    }
                }
                if individual || i == sections.len() || sections[i + 1] < 0 {
                    break;
                }
                len = sections[i];
                ins = sections[i + 1];
                i += 2;
            }
            f(pos_a, end_a, pos_b, end_b, text);
            pos_a = end_a;
            pos_b = end_b;
        }
    }
}

/// Produce a copy of set A that applies to the document after set B has
/// been applied (assuming both start at the same document).
fn map_set(
    a_sections: &[isize],
    a_inserted: Option<&[Text]>,
    b_sections: &[isize],
    before: bool,
) -> (Sections, Option<Vec<Text>>) {
    let mut sections = Vec::new();
    let mut insert: Option<Vec<Text>> = a_inserted.map(|_| Vec::new());
    let mut a = SectionIter::new(a_sections, a_inserted);
    let mut b = SectionIter::new(b_sections, None);
    // `inserted` tracks, for changes in A that have to be processed
    // piece-by-piece, whether their content has been inserted already, and
    // refers to the section index.
    let mut inserted: isize = -1;
    loop {
        if (a.done() && b.len > 0) || (b.done() && a.len > 0) {
            panic!("Mismatched change set lengths");
        } else if a.ins == -1 && b.ins == -1 {
            // Move across ranges skipped by both sets.
            let len = a.len.min(b.len);
            add_section(&mut sections, len, -1, false);
            a.forward(len);
            b.forward(len);
        } else if b.ins >= 0
            && (a.ins < 0 || inserted == a.i as isize || (a.off == 0 && (b.len < a.len || (b.len == a.len && !before))))
        {
            // If there's a change in B that comes before the next change in
            // A (ordered by start pos, then len, then before flag), skip
            // that (and process any changes in A it covers).
            let mut len = b.len;
            add_section(&mut sections, b.ins, -1, false);
            while len > 0 {
                let piece = a.len.min(len);
                if a.ins >= 0 && inserted < a.i as isize && a.len <= piece {
                    add_section(&mut sections, 0, a.ins, false);
                    if let Some(insert) = insert.as_mut() {
                        add_insert(insert, &sections, a.text());
                    }
                    inserted = a.i as isize;
                }
                a.forward(piece);
                len -= piece;
            }
            b.next();
        } else if a.ins >= 0 {
            // Process the part of a change in A up to the start of the next
            // non-deletion change in B (if overlapping).
            let mut len = 0;
            let mut left = a.len;
            while left > 0 {
                if b.ins == -1 {
                    let piece = left.min(b.len);
                    len += piece;
                    left -= piece;
                    b.forward(piece);
                } else if b.ins == 0 && b.len < left {
                    left -= b.len;
                    b.next();
                } else {
                    break;
                }
            }
            let fresh = inserted < a.i as isize;
            add_section(&mut sections, len, if fresh { a.ins } else { 0 }, false);
            if fresh {
                if let Some(insert) = insert.as_mut() {
                    add_insert(insert, &sections, a.text());
                }
            }
            inserted = a.i as isize;
            a.forward(a.len - left);
        } else if a.done() && b.done() {
            return (sections, insert);
        } else {
            panic!("Mismatched change set lengths");
        }
    }
}

fn compose_sets(
    a_sections: &[isize],
    a_inserted: Option<&[Text]>,
    b_sections: &[isize],
    b_inserted: Option<&[Text]>,
) -> (Sections, Option<Vec<Text>>) {
    let mut sections = Vec::new();
    let mut insert: Option<Vec<Text>> = a_inserted.map(|_| Vec::new());
    let mut a = SectionIter::new(a_sections, a_inserted);
    let mut b = SectionIter::new(b_sections, b_inserted);
    let mut open = false;
    loop {
        if a.done() && b.done() {
            return (sections, insert);
        } else if a.ins == 0 {
            // Deletion in A
            add_section(&mut sections, a.len, 0, open);
            a.next();
        } else if b.len == 0 && !b.done() {
            // Insertion in B
            add_section(&mut sections, 0, b.ins, open);
            if let Some(insert) = insert.as_mut() {
                add_insert(insert, &sections, b.text());
            }
            b.next();
        } else if a.done() || b.done() {
            panic!("Mismatched change set lengths");
        } else {
            let len = a.len2().min(b.len);
            let section_len = sections.len();
            if a.ins == -1 {
                let ins_b = if b.ins == -1 {
                    -1
                } else if b.off > 0 {
                    0
                } else {
                    b.ins
                };
                add_section(&mut sections, len, ins_b, open);
                if ins_b != 0 {
                    if let Some(insert) = insert.as_mut() {
                        add_insert(insert, &sections, b.text());
                    }
                }
            } else if b.ins == -1 {
                add_section(&mut sections, if a.off > 0 { 0 } else { a.len }, len, open);
                if let Some(insert) = insert.as_mut() {
                    add_insert(insert, &sections, a.text_bit(Some(len)));
                }
            } else {
                add_section(&mut sections, if a.off > 0 { 0 } else { a.len }, if b.off > 0 { 0 } else { b.ins }, open);
                if b.off == 0 {
                    if let Some(insert) = insert.as_mut() {
                        add_insert(insert, &sections, b.text());
                    }
                }
            }
            open = (a.ins > len || (b.ins >= 0 && b.len > len)) && (open || sections.len() > section_len);
            a.forward2(len);
            b.forward(len);
        }
    }
}

struct SectionIter<'a> {
    sections: &'a [isize],
    inserted: Option<&'a [Text]>,
    i: usize,
    len: isize,
    off: isize,
    ins: isize,
}

impl<'a> SectionIter<'a> {
    fn new(sections: &'a [isize], inserted: Option<&'a [Text]>) -> Self {
        let mut iter = SectionIter { sections, inserted, i: 0, len: 0, off: 0, ins: 0 };
        iter.next();
        iter
    }

    fn next(&mut self) {
        if self.i < self.sections.len() {
            self.len = self.sections[self.i];
            self.ins = self.sections[self.i + 1];
            self.i += 2;
        } else {
            self.len = 0;
            self.ins = -2;
        }
        self.off = 0;
    }

    fn done(&self) -> bool {
        self.ins == -2
    }

    fn len2(&self) -> isize {
        if self.ins < 0 {
            self.len
        } else {
            self.ins
        }
    }

    fn text(&self) -> Text {
        let index = (self.i - 2) >> 1;
        self.inserted.and_then(|ins| ins.get(index)).cloned().unwrap_or_default()
    }

    fn text_bit(&self, len: Option<isize>) -> Text {
        let index = (self.i - 2) >> 1;
        match self.inserted.and_then(|ins| ins.get(index)) {
            None => Text::empty(),
            Some(text) => {
                let from = self.off as usize;
                let to = len.map_or(text.len(), |l| from + l as usize);
                text.slice(from, to)
            }
        }
    }

    fn forward(&mut self, len: isize) {
        if len == self.len {
            self.next();
        } else {
            self.len -= len;
            self.off += len;
        }
    }

    fn forward2(&mut self, len: isize) {
        if self.ins == -1 {
            self.forward(len);
        } else if len == self.ins {
            self.next();
        } else {
            self.ins -= len;
            self.off += len;
        }
    }
}
