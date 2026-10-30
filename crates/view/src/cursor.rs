//! Cursor motion and coordinate queries. Port of `cursor.ts`.
//!
//! Motion is logical (by grapheme cluster): bidirectional text is not
//! handled yet, so `moveVisually` moves in string order. Functions that
//! need text layout take the GPUI window.

use std::rc::Rc;

use codemirror_state::{find_cluster_break, CharCategory, EditorSelection, EditorState, SelectionRange};
use gpui::{point, px, Bounds, Pixels, Point, Window};

use crate::decoration::BlockType;
use crate::editorview::EditorView;
use crate::heightmap::BlockInfo;

/// Select the group (word, space or other characters) around `pos`.
pub fn group_at(state: &EditorState, pos: usize, bias: i32) -> SelectionRange {
    let categorize = state.char_categorizer(pos);
    let line = state.doc().line_at(pos);
    let line_pos = pos - line.from;
    if line.length() == 0 {
        return EditorSelection::cursor(pos);
    }
    let bias = if line_pos == 0 {
        1
    } else if line_pos == line.length() {
        -1
    } else {
        bias
    };
    let (mut from, mut to) = (line_pos, line_pos);
    if bias < 0 {
        from = find_cluster_break(&line.text, line_pos, false, true);
    } else {
        to = find_cluster_break(&line.text, line_pos, true, true);
    }
    let cat = categorize(&line.text[from..to]);
    while from > 0 {
        let prev = find_cluster_break(&line.text, from, false, true);
        if categorize(&line.text[prev..from]) != cat {
            break;
        }
        from = prev;
    }
    while to < line.length() {
        let next = find_cluster_break(&line.text, to, true, true);
        if categorize(&line.text[to..next]) != cat {
            break;
        }
        to = next;
    }
    EditorSelection::undirectional_range(from + line.from, to + line.from)
}

/// A function that, given the first character moved over, returns a
/// predicate deciding whether to continue over each next character.
pub type ByChar<'a> = &'a dyn Fn(&str) -> Box<dyn FnMut(&str) -> bool>;

/// A position with the side it associates with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PosAssoc {
    pub pos: usize,
    pub assoc: i32,
}

impl EditorView {
    // The block (text line or widget) that holds `pos`.
    fn block_at(&self, pos: usize, side: i32) -> BlockInfo {
        let line = self.line_block_at(pos);
        let Some(children) = line.children() else { return line };
        let mut best: Option<&BlockInfo> = None;
        for l in children {
            if l.from > pos {
                break;
            }
            if l.to() < pos {
                continue;
            }
            if l.from < pos && l.to() > pos {
                return l.clone();
            }
            let better = match best {
                None => true,
                Some(b) => {
                    l.block_type() == BlockType::Text
                        && (b.block_type() != l.block_type() || if side < 0 { l.from < pos } else { l.to() > pos })
                }
            };
            if better {
                best = Some(l);
            }
        }
        best.cloned().unwrap_or(line)
    }

    /// Document coordinates of a position: (x from the start of the text,
    /// top and bottom from the top of the document).
    pub fn doc_coords_at_pos(&self, window: &mut Window, pos: usize, side: i32) -> (f32, f32, f32) {
        let block = self.block_at(pos, side);
        if block.block_type() != BlockType::Text {
            return (0.0, block.top as f32, block.bottom() as f32);
        }
        let line = self.state().doc().line_at(block.from);
        let layout = self.layout(window, block.from, None);
        let (x, y) = layout.coords(pos.clamp(line.from, line.to) - line.from, side);
        let top = block.top as f32 + y;
        (x, top, top + self.default_line_height())
    }

    /// The window rectangle (zero width, line height) of a position, based
    /// on the last frame's scroll position.
    pub fn coords_at_pos(&self, window: &mut Window, pos: usize, side: i32) -> Bounds<Pixels> {
        let (x, top, bottom) = self.doc_coords_at_pos(window, pos, side);
        let o = self.origin();
        Bounds::from_corners(point(o.x + px(x), o.y + px(top)), point(o.x + px(x), o.y + px(bottom)))
    }

    /// The document position at the given document coordinates. With
    /// `scan_y`, block widgets are skipped in that direction.
    pub fn pos_at_doc_coords(&self, window: &mut Window, x: f32, y: f64, scan_y: Option<i32>) -> PosAssoc {
        let mut y = y;
        let block = loop {
            if y < 0.0 {
                return PosAssoc { pos: 0, assoc: 1 };
            }
            if y > self.content_height() {
                return PosAssoc { pos: self.state().doc().len(), assoc: -1 };
            }
            let block = self.element_at_height(y);
            let Some(scan) = scan_y else { break block };
            if block.block_type() == BlockType::Text {
                break block;
            }
            let half_line = self.default_line_height() as f64 / 2.0;
            y = if scan > 0 { block.bottom() + half_line } else { block.top - half_line };
        };
        if block.block_type() != BlockType::Text {
            return if y < (block.top + block.bottom()) / 2.0 {
                PosAssoc { pos: block.from, assoc: 1 }
            } else {
                PosAssoc { pos: block.to(), assoc: -1 }
            };
        }
        let layout = self.layout(window, block.from, None);
        let (off, assoc) = layout.pos_at(x, (y - block.top) as f32);
        PosAssoc { pos: block.from + off, assoc }
    }

    /// The document position at a window point, using the last frame's
    /// scroll position.
    pub fn pos_at_coords(&self, window: &mut Window, p: Point<Pixels>) -> PosAssoc {
        let o = self.origin();
        self.pos_at_doc_coords(window, f32::from(p.x - o.x), f32::from(p.y - o.y) as f64, None)
    }

    /// Move a cursor position by grapheme cluster. `by` can be used to
    /// continue moving over characters as long as a predicate holds.
    pub fn move_by_char(&self, start: &SelectionRange, forward: bool, by: Option<ByChar>) -> SelectionRange {
        let doc = self.state().doc();
        let mut line = doc.line_at(start.head());
        let mut cur = *start;
        let mut check: Option<Box<dyn FnMut(&str) -> bool>> = None;
        loop {
            let off = cur.head() - line.from;
            let next_off = find_cluster_break(&line.text, off, forward, true);
            let (next, ch) = if next_off == off {
                if line.number == if forward { doc.lines() } else { 1 } {
                    return cur;
                }
                line = doc.line(if forward { line.number + 1 } else { line.number - 1 });
                let pos = if forward { line.from } else { line.to };
                (EditorSelection::cursor_with(pos, if forward { -1 } else { 1 }, None, None), "\n".to_string())
            } else {
                let (a, b) = if forward { (off, next_off) } else { (next_off, off) };
                let next = EditorSelection::cursor_with(line.from + next_off, if forward { -1 } else { 1 }, None, None);
                (next, line.text[a..b].to_string())
            };
            match &mut check {
                None => {
                    let Some(by) = by else { return next };
                    check = Some(by(&ch));
                }
                Some(f) => {
                    if !f(&ch) {
                        return cur;
                    }
                }
            }
            cur = next;
        }
    }

    /// Move a cursor position across the next group of either letters
    /// (word characters), whitespace, or other characters.
    pub fn move_by_group(&self, start: &SelectionRange, forward: bool) -> SelectionRange {
        let categorize = Rc::new(self.state().char_categorizer(start.head()));
        let by = move |initial: &str| -> Box<dyn FnMut(&str) -> bool> {
            let categorize = categorize.clone();
            let mut cat = categorize(initial);
            Box::new(move |next: &str| {
                let next_cat = categorize(next);
                if cat == CharCategory::Space {
                    cat = next_cat;
                }
                cat == next_cat
            })
        };
        self.move_by_char(start, forward, Some(&by))
    }

    /// Move to the next line boundary in the given direction. If
    /// `include_wrap` is true, line wrapping is on, and there is a further
    /// wrap point on the current line, the wrap point is returned.
    pub fn move_to_line_boundary(
        &self,
        window: &mut Window,
        start: &SelectionRange,
        forward: bool,
        include_wrap: bool,
    ) -> SelectionRange {
        let side = if start.assoc() != 0 { start.assoc() } else { -1 };
        let block = self.block_at(start.head(), side);
        if include_wrap && block.block_type() == BlockType::Text && self.line_wrapping() {
            let line = self.state().doc().line_at(block.from);
            let layout = self.layout(window, block.from, None);
            let assoc = if start.assoc() < 0 && start.head() > block.from { -1 } else { 1 };
            let (a, b) = layout.row_range(start.head().clamp(line.from, line.to) - line.from, assoc);
            return if forward {
                EditorSelection::cursor_with(line.from + b, if line.from + b == line.to { 1 } else { -1 }, None, None)
            } else {
                EditorSelection::cursor_with(line.from + a, 1, None, None)
            };
        }
        EditorSelection::cursor_with(
            if forward { block.to() } else { block.from },
            if forward { 1 } else { -1 },
            None,
            None,
        )
    }

    /// Move a cursor position vertically. When `distance` isn't given, it
    /// defaults to moving to the next line (including wrapped lines).
    /// Otherwise, `distance` should provide a positive distance in pixels.
    pub fn move_vertically(
        &self,
        window: &mut Window,
        start: &SelectionRange,
        forward: bool,
        distance: Option<f32>,
    ) -> SelectionRange {
        let start_pos = start.head();
        if start_pos == if forward { self.state().doc().len() } else { 0 } {
            return EditorSelection::cursor_with(start_pos, start.assoc(), None, None);
        }
        let side = if start.assoc() != 0 {
            start.assoc()
        } else if if start.is_empty() { forward } else { start.head() == start.from } {
            1
        } else {
            -1
        };
        let (x, top, bottom) = self.doc_coords_at_pos(window, start_pos, side);
        let goal = start.goal_column.map_or(x, |g| g as f32);
        let start_y = if forward { bottom } else { top } as f64;
        let half_text = (self.default_line_height() / 2.0).floor() as f64;
        let dist = distance.map_or(half_text, |d| d as f64);
        let dir = if forward { 1.0 } else { -1.0 };
        let mut scan = 0.0;
        loop {
            let y = start_y + (dist + scan) * dir;
            let pos = self.pos_at_doc_coords(window, goal, y, Some(dir as i32));
            if if forward { y > self.content_height() } else { y < 0.0 } {
                return EditorSelection::cursor_with(pos.pos, pos.assoc, None, None);
            }
            let (_, t, b) = self.doc_coords_at_pos(window, pos.pos, pos.assoc);
            let mid = ((t + b) / 2.0) as f64;
            if if forward { mid > start_y } else { mid < start_y } {
                return EditorSelection::cursor_with(pos.pos, pos.assoc, None, Some(goal as f64));
            }
            scan += half_text.max(1.0);
        }
    }
}
