//! Selections. Port of `selection.ts`.

use crate::change::ChangeDesc;

// A range's flags field is used like this:
// - 3 bits for bidi level (7 means unset) (only meaningful for cursors)
// - 2 bits to indicate the side the cursor is associated with
// - 1 bit to indicate whether the range is inverted (head before anchor)
//   (only meaningful for non-empty ranges)
// - 1 bit for the 'undecided' flag, which is used for MacOS style range
//   selections that will pick a head and anchor side based on which
//   direction the next shift-cursor-motion goes.
const BIDI_LEVEL_MASK: u8 = 7;
const ASSOC_BEFORE: u8 = 8;
const ASSOC_AFTER: u8 = 16;
const INVERTED: u8 = 32;
const UNDIRECTIONAL: u8 = 64;

/// A single selection range. When
/// [`ALLOW_MULTIPLE_SELECTIONS`](crate::ALLOW_MULTIPLE_SELECTIONS) is
/// enabled, a [selection](EditorSelection) may hold multiple ranges. By
/// default, selections hold exactly one range.
#[derive(Clone, Copy, Debug)]
pub struct SelectionRange {
    /// The lower boundary of the range.
    pub from: usize,
    /// The upper boundary of the range.
    pub to: usize,
    flags: u8,
    /// The goal column (stored vertical offset) associated with a cursor.
    /// This is used to preserve the vertical position when moving across
    /// lines of different length.
    pub goal_column: Option<f64>,
}

impl SelectionRange {
    pub(crate) fn create(from: usize, to: usize, flags: u8, goal_column: Option<f64>) -> SelectionRange {
        SelectionRange { from, to, flags, goal_column }
    }

    /// The anchor of the range—the side that doesn't move when you extend
    /// it.
    pub fn anchor(&self) -> usize {
        if self.flags & INVERTED != 0 {
            self.to
        } else {
            self.from
        }
    }

    /// The head of the range, which is moved when the range is
    /// [extended](SelectionRange::extend).
    pub fn head(&self) -> usize {
        if self.flags & INVERTED != 0 {
            self.from
        } else {
            self.to
        }
    }

    /// True when `anchor` and `head` are at the same position.
    pub fn is_empty(&self) -> bool {
        self.from == self.to
    }

    /// If this is a cursor that is explicitly associated with the character
    /// on one of its sides, this returns the side. -1 means the character
    /// before its position, 1 the character after, and 0 means no
    /// association.
    pub fn assoc(&self) -> i32 {
        if self.flags & ASSOC_BEFORE != 0 {
            -1
        } else if self.flags & ASSOC_AFTER != 0 {
            1
        } else {
            0
        }
    }

    /// When set, some selection-extending commands treat the range's head
    /// and anchor as exchangeable. Used to implement MacOS-style
    /// undirectional selections.
    pub fn undirectional(&self) -> bool {
        self.flags & UNDIRECTIONAL != 0
    }

    /// The bidirectional text level associated with this cursor, if any.
    pub fn bidi_level(&self) -> Option<u8> {
        let level = self.flags & BIDI_LEVEL_MASK;
        if level == 7 {
            None
        } else {
            Some(level)
        }
    }

    /// Map this range through a change, producing a valid range in the
    /// updated document. CodeMirror's default `assoc` is -1.
    pub fn map(&self, change: &ChangeDesc, assoc: i32) -> SelectionRange {
        let (from, to) = if self.is_empty() {
            let p = change.map_pos(self.from, assoc);
            (p, p)
        } else {
            (change.map_pos(self.from, 1), change.map_pos(self.to, -1))
        };
        SelectionRange { from, to, ..*self }
    }

    /// Extend this range to cover at least `from` to `to`.
    pub fn extend(&self, from: usize, to: usize, assoc: i32) -> SelectionRange {
        let anchor = self.anchor();
        if from <= anchor && to >= anchor {
            return EditorSelection::range_with(from, to, None, None, assoc);
        }
        let head = if from.abs_diff(anchor) > to.abs_diff(anchor) { from } else { to };
        EditorSelection::range_with(anchor, head, None, None, assoc)
    }

    /// Compare this range to another range. When `include_assoc` is true,
    /// cursors must also have the same [`assoc`](SelectionRange::assoc).
    pub fn eq(&self, other: &SelectionRange, include_assoc: bool) -> bool {
        self.anchor() == other.anchor()
            && self.head() == other.head()
            && self.goal_column == other.goal_column
            && (!include_assoc || !self.is_empty() || self.assoc() == other.assoc())
    }
}

/// An editor selection holds one or more selection ranges.
#[derive(Clone, Debug)]
pub struct EditorSelection {
    /// The ranges in the selection, sorted by position. Ranges cannot
    /// overlap (but they may touch, if they aren't empty).
    pub ranges: Vec<SelectionRange>,
    /// The index of the _main_ range in the selection (which is usually
    /// the range that was added last).
    pub main_index: usize,
}

impl EditorSelection {
    /// Map a selection through a change. Used to adjust the selection
    /// position for changes. CodeMirror's default `assoc` is -1.
    pub fn map(&self, change: &ChangeDesc, assoc: i32) -> EditorSelection {
        if change.is_empty() {
            return self.clone();
        }
        EditorSelection::create(self.ranges.iter().map(|r| r.map(change, assoc)).collect(), self.main_index)
    }

    /// Compare this selection to another selection. By default, ranges are
    /// compared only by position. When `include_assoc` is true, cursor
    /// ranges must also have the same assoc value.
    pub fn eq(&self, other: &EditorSelection, include_assoc: bool) -> bool {
        self.ranges.len() == other.ranges.len()
            && self.main_index == other.main_index
            && self.ranges.iter().zip(&other.ranges).all(|(a, b)| a.eq(b, include_assoc))
    }

    /// Get the primary selection range.
    pub fn main(&self) -> SelectionRange {
        self.ranges[self.main_index]
    }

    /// Make sure the selection only has one range. Returns a selection
    /// holding only the main range from this selection.
    pub fn as_single(&self) -> EditorSelection {
        if self.ranges.len() == 1 {
            self.clone()
        } else {
            EditorSelection { ranges: vec![self.main()], main_index: 0 }
        }
    }

    /// Extend this selection with an extra range.
    pub fn add_range(&self, range: SelectionRange, main: bool) -> EditorSelection {
        let mut ranges = Vec::with_capacity(self.ranges.len() + 1);
        ranges.push(range);
        ranges.extend_from_slice(&self.ranges);
        EditorSelection::create(ranges, if main { 0 } else { self.main_index + 1 })
    }

    /// Replace a given range (the main one when `which` is `None`) with
    /// another range, and then normalize the selection to merge and sort
    /// ranges if necessary.
    pub fn replace_range(&self, range: SelectionRange, which: Option<usize>) -> EditorSelection {
        let mut ranges = self.ranges.clone();
        ranges[which.unwrap_or(self.main_index)] = range;
        EditorSelection::create(ranges, self.main_index)
    }

    /// Create a selection holding a single range.
    pub fn single(anchor: usize, head: usize) -> EditorSelection {
        EditorSelection { ranges: vec![EditorSelection::range(anchor, head)], main_index: 0 }
    }

    /// Sort and merge the given set of ranges, creating a valid selection.
    ///
    /// # Panics
    ///
    /// Panics when `ranges` is empty.
    pub fn create(ranges: Vec<SelectionRange>, main_index: usize) -> EditorSelection {
        assert!(!ranges.is_empty(), "A selection needs at least one range");
        let mut pos = 0;
        for range in &ranges {
            if if range.is_empty() { range.from <= pos } else { range.from < pos } {
                return EditorSelection::normalized(ranges, main_index);
            }
            pos = range.to;
        }
        EditorSelection { ranges, main_index }
    }

    /// Create a cursor selection range at the given position.
    pub fn cursor(pos: usize) -> SelectionRange {
        EditorSelection::cursor_with(pos, 0, None, None)
    }

    /// Create a cursor selection range with an association side, bidi level
    /// and goal column.
    pub fn cursor_with(pos: usize, assoc: i32, bidi_level: Option<u8>, goal_column: Option<f64>) -> SelectionRange {
        let assoc_flag = match assoc.signum() {
            0 => 0,
            -1 => ASSOC_BEFORE,
            _ => ASSOC_AFTER,
        };
        let level = bidi_level.map_or(7, |l| l.min(6));
        SelectionRange::create(pos, pos, assoc_flag | level, goal_column)
    }

    /// Create a selection range.
    pub fn range(anchor: usize, head: usize) -> SelectionRange {
        EditorSelection::range_with(anchor, head, None, None, 0)
    }

    /// Create a selection range with a goal column, bidi level and
    /// association side.
    pub fn range_with(
        anchor: usize,
        head: usize,
        goal_column: Option<f64>,
        bidi_level: Option<u8>,
        assoc: i32,
    ) -> SelectionRange {
        let mut flags = bidi_level.map_or(7, |l| l.min(6));
        let mut assoc = assoc;
        if assoc == 0 && anchor != head {
            assoc = if head < anchor { 1 } else { -1 };
        }
        if assoc != 0 {
            flags |= if assoc < 0 { ASSOC_BEFORE } else { ASSOC_AFTER };
        }
        if head < anchor {
            SelectionRange::create(head, anchor, flags | INVERTED, goal_column)
        } else {
            SelectionRange::create(anchor, head, flags, goal_column)
        }
    }

    /// Create an [undirectional](SelectionRange::undirectional) selection
    /// range.
    pub fn undirectional_range(from: usize, to: usize) -> SelectionRange {
        SelectionRange::create(from, to, UNDIRECTIONAL, None)
    }

    fn normalized(mut ranges: Vec<SelectionRange>, main_index: usize) -> EditorSelection {
        // Track the main range through sorting by tagging the ranges with
        // their original index.
        let mut tagged: Vec<(usize, SelectionRange)> = ranges.drain(..).enumerate().collect();
        tagged.sort_by_key(|(_, r)| r.from);
        let mut main_index = tagged.iter().position(|(i, _)| *i == main_index).unwrap_or(0);
        let mut ranges: Vec<SelectionRange> = tagged.into_iter().map(|(_, r)| r).collect();
        let mut i = 1;
        while i < ranges.len() {
            let (range, prev) = (ranges[i], ranges[i - 1]);
            if if range.is_empty() { range.from <= prev.to } else { range.from < prev.to } {
                let (from, to) = (prev.from, range.to.max(prev.to));
                if i <= main_index {
                    main_index -= 1;
                }
                let merged = if range.anchor() > range.head() {
                    EditorSelection::range(to, from)
                } else {
                    EditorSelection::range(from, to)
                };
                i -= 1;
                ranges.splice(i..i + 2, [merged]);
            }
            i += 1;
        }
        EditorSelection { ranges, main_index }
    }
}

pub(crate) fn check_selection(selection: &EditorSelection, doc_length: usize) {
    for range in &selection.ranges {
        assert!(range.to <= doc_length, "Selection points outside of document");
    }
}
