//! The commands that need the view's layout (motion by character, group,
//! line, page and line boundary; adding cursors; deleting lines) and the
//! key bindings. Port of the rest of `commands.ts` and `history.ts`.
//!
//! Text is treated as left-to-right everywhere (bidirectional text is not
//! ported), so "left" means backward. `cursorSubword*` doesn't use
//! `Intl.Segmenter` for CJK word boundaries.

use std::cell::RefCell;
use std::rc::Rc;

use codemirror_state::{CharCategory, EditorSelection, EditorState, SelectionRange, Transaction, TransactionSpec};
use codemirror_view::{EditorView, KeyBinding, ScrollStrategy, ScrollTarget, SCROLL_INTO_VIEW};
use gpui::{Context, Window};

use crate::commands::{delete_by, extend_sel, move_sel, range_end, selected_line_blocks, update_sel, Dispatch};
use crate::*;

// Run a state-level helper that reads the view, then dispatch what it
// produced.
fn with_view(
    view: &mut EditorView,
    window: &mut Window,
    cx: &mut Context<EditorView>,
    f: impl FnOnce(&EditorView, &mut Window, &EditorState, Dispatch) -> bool,
) -> bool {
    let state = view.state().clone();
    let mut trs: Vec<Transaction> = Vec::new();
    let result = f(view, window, &state, &mut |tr| trs.push(tr));
    for tr in trs {
        view.dispatch(tr, cx);
    }
    result
}

fn cursor_by_char(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>, forward: bool) -> bool {
    with_view(view, window, cx, |v, _, s, d| {
        move_sel(s, d, |r| if r.is_empty() { v.move_by_char(r, forward, None) } else { range_end(r, forward) })
    })
}

/// Move the selection one character to the left.
pub fn cursor_char_left(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    cursor_by_char(view, window, cx, false)
}

/// Move the selection one character to the right.
pub fn cursor_char_right(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    cursor_by_char(view, window, cx, true)
}

/// Move the selection one character forward.
pub fn cursor_char_forward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    cursor_by_char(view, window, cx, true)
}

/// Move the selection one character backward.
pub fn cursor_char_backward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    cursor_by_char(view, window, cx, false)
}

fn cursor_by_group(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>, forward: bool) -> bool {
    with_view(view, window, cx, |v, _, s, d| {
        move_sel(s, d, |r| if r.is_empty() { v.move_by_group(r, forward) } else { range_end(r, forward) })
    })
}

/// Move the selection to the left across one group of word or non-word
/// (but also non-space) characters.
pub fn cursor_group_left(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    cursor_by_group(view, window, cx, false)
}

/// Move the selection one group to the right.
pub fn cursor_group_right(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    cursor_by_group(view, window, cx, true)
}

/// Move the selection one group forward.
pub fn cursor_group_forward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    cursor_by_group(view, window, cx, true)
}

/// Move the selection one group backward.
pub fn cursor_group_backward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    cursor_by_group(view, window, cx, false)
}

fn to_group_start(state: &EditorState, pos: usize, start: &str) -> Box<dyn FnMut(&str) -> bool> {
    let categorize = state.char_categorizer(pos);
    let cat = categorize(start);
    let mut initial = cat != CharCategory::Space;
    Box::new(move |next: &str| {
        let next_cat = categorize(next);
        if next_cat != CharCategory::Space {
            return initial && next_cat == cat;
        }
        initial = false;
        true
    })
}

fn group_forward_win(v: &EditorView, r: &SelectionRange) -> SelectionRange {
    let state = v.state().clone();
    let head = r.head();
    v.move_by_char(r, true, Some(&move |start: &str| to_group_start(&state, head, start)))
}

/// Move the cursor one group forward in the default Windows style, where
/// it moves to the start of the next group.
pub fn cursor_group_forward_win(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    with_view(view, window, cx, |v, _, s, d| {
        move_sel(s, d, |r| if r.is_empty() { group_forward_win(v, r) } else { range_end(r, true) })
    })
}

// Character classes for subword motion: punctuation in the word category
// gets its own class.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SubCat {
    Word,
    Space,
    Other,
    Punct,
}

struct SubwordScan {
    cat: SubCat,
    pos: usize,
    steps: usize,
    done: bool,
    saw_upper: bool,
    saw_lower: bool,
}

fn subword_step(scan: &Rc<RefCell<SubwordScan>>, state: &EditorState, forward: bool, next: &str) -> bool {
    let categorize = state.char_categorizer(scan.borrow().pos);
    let mut s = scan.borrow_mut();
    if s.done {
        return false;
    }
    if forward {
        s.pos += next.len();
    } else {
        s.pos -= next.len();
    }
    let mut next_cat = match categorize(next) {
        CharCategory::Word => SubCat::Word,
        CharCategory::Space => SubCat::Space,
        CharCategory::Other => SubCat::Other,
    };
    if next_cat == SubCat::Word && next.as_bytes()[0] < 128 && next.chars().all(|c| !c.is_ascii_alphanumeric()) {
        // Treat word punctuation specially.
        next_cat = SubCat::Punct;
    }
    if s.cat == SubCat::Space {
        s.cat = next_cat;
    }
    if s.cat != next_cat {
        return false;
    }
    if s.cat == SubCat::Word {
        if next.to_lowercase() == next {
            if !forward && s.saw_upper {
                return false;
            }
            s.saw_lower = true;
        } else if s.saw_lower {
            if forward {
                return false;
            }
            s.done = true;
        } else {
            if s.saw_upper && forward {
                let rope = state.doc().rope();
                let ahead: String = rope.byte_slice(s.pos..).chars().take(1).collect();
                if categorize(&ahead) == CharCategory::Word && ahead.to_lowercase() == ahead {
                    return false;
                }
            }
            s.saw_upper = true;
        }
    }
    s.steps += 1;
    true
}

fn move_by_subword(view: &EditorView, range: &SelectionRange, forward: bool) -> SelectionRange {
    let scan = Rc::new(RefCell::new(SubwordScan {
        cat: SubCat::Space,
        pos: range.from,
        steps: 0,
        done: false,
        saw_upper: false,
        saw_lower: false,
    }));
    let state = view.state().clone();
    let by = {
        let scan = scan.clone();
        move |start: &str| -> Box<dyn FnMut(&str) -> bool> {
            subword_step(&scan, &state, forward, start);
            let (scan, state) = (scan.clone(), state.clone());
            Box::new(move |next: &str| subword_step(&scan, &state, forward, next))
        }
    };
    view.move_by_char(range, forward, Some(&by))
}

fn cursor_by_subword(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>, forward: bool) -> bool {
    with_view(view, window, cx, |v, _, s, d| {
        move_sel(s, d, |r| if r.is_empty() { move_by_subword(v, r, forward) } else { range_end(r, forward) })
    })
}

/// Move the selection one group or camel-case subword forward.
pub fn cursor_subword_forward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    cursor_by_subword(view, window, cx, true)
}

/// Move the selection one group or camel-case subword backward.
pub fn cursor_subword_backward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    cursor_by_subword(view, window, cx, false)
}

fn cursor_by_line(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>, forward: bool) -> bool {
    with_view(view, window, cx, |v, window, s, d| {
        move_sel(s, d, |r| {
            if !r.is_empty() {
                return range_end(r, forward);
            }
            let moved = v.move_vertically(window, r, forward, None);
            if moved.head() != r.head() {
                moved
            } else {
                v.move_to_line_boundary(window, r, forward, true)
            }
        })
    })
}

/// Move the selection one line up.
pub fn cursor_line_up(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    cursor_by_line(view, window, cx, false)
}

/// Move the selection one line down.
pub fn cursor_line_down(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    cursor_by_line(view, window, cx, true)
}

fn page_height(view: &EditorView) -> f32 {
    view.default_line_height().max(view.visible_height() - 5.0)
}

fn cursor_by_page(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>, forward: bool) -> bool {
    let height = page_height(view);
    let state = view.state().clone();
    let selection = update_sel(state.selection(), |r| {
        if r.is_empty() {
            view.move_vertically(window, r, forward, Some(height))
        } else {
            range_end(r, forward)
        }
    });
    if selection.eq(state.selection(), false) {
        return false;
    }
    let mut spec = TransactionSpec::new().selection(selection.clone()).scroll_into_view().user_event("select");
    // Keep the cursor at the same height in the view.
    let visible = view.visible_rect();
    let start = view.coords_at_pos(window, state.selection().main().head(), 1);
    if start.top() > visible.top() && start.bottom() < visible.bottom() {
        spec = spec.effect(SCROLL_INTO_VIEW.of(ScrollTarget {
            range: EditorSelection::cursor(selection.main().head()),
            y: ScrollStrategy::Start,
            x: ScrollStrategy::Nearest,
            y_margin: f32::from(start.top() - visible.top()),
            x_margin: 5.0,
        }));
    }
    view.dispatch_spec(spec, cx);
    true
}

/// Move the selection one page up.
pub fn cursor_page_up(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    cursor_by_page(view, window, cx, false)
}

/// Move the selection one page down.
pub fn cursor_page_down(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    cursor_by_page(view, window, cx, true)
}

fn move_by_line_boundary(
    view: &EditorView,
    window: &mut Window,
    start: &SelectionRange,
    forward: bool,
) -> SelectionRange {
    let line = view.line_block_at(start.head());
    let mut moved = view.move_to_line_boundary(window, start, forward, true);
    if moved.head() == start.head() && moved.head() != if forward { line.to() } else { line.from } {
        moved = view.move_to_line_boundary(window, start, forward, false);
    }
    if !forward && moved.head() == line.from && line.length > 0 {
        let text = view.state().slice_doc(line.from, line.to().min(line.from + 100));
        let space = leading_space(&text);
        if space > 0 && start.head() != line.from + space {
            moved = EditorSelection::cursor(line.from + space);
        }
    }
    moved
}

/// Move the selection to the next line wrap point, or to the end of the
/// line if there isn't one left on this line.
pub fn cursor_line_boundary_forward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    with_view(view, window, cx, |v, window, s, d| move_sel(s, d, |r| move_by_line_boundary(v, window, r, true)))
}

/// Move the selection to previous line wrap point, or failing that to the
/// start of the line. If the line is indented, and the cursor isn't already
/// at the end of the indentation, this will move to the end of the
/// indentation instead of the start of the line.
pub fn cursor_line_boundary_backward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    with_view(view, window, cx, |v, window, s, d| move_sel(s, d, |r| move_by_line_boundary(v, window, r, false)))
}

/// Move the selection one line wrap point to the left.
pub fn cursor_line_boundary_left(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    cursor_line_boundary_backward(view, window, cx)
}

/// Move the selection one line wrap point to the right.
pub fn cursor_line_boundary_right(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    cursor_line_boundary_forward(view, window, cx)
}

/// Move the selection to the start of the line.
pub fn cursor_line_start(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    with_view(view, window, cx, |v, window, s, d| move_sel(s, d, |r| v.move_to_line_boundary(window, r, false, false)))
}

/// Move the selection to the end of the line.
pub fn cursor_line_end(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    with_view(view, window, cx, |v, window, s, d| move_sel(s, d, |r| v.move_to_line_boundary(window, r, true, false)))
}

fn select_by(
    view: &mut EditorView,
    window: &mut Window,
    cx: &mut Context<EditorView>,
    forward: bool,
    how: impl Fn(&EditorView, &mut Window, &SelectionRange) -> SelectionRange,
) -> bool {
    with_view(view, window, cx, |v, window, s, d| extend_sel(s, d, forward, |r| how(v, window, r)))
}

/// Move the selection head one character to the left, while leaving the
/// anchor in place.
pub fn select_char_left(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_by(view, window, cx, false, |v, _, r| v.move_by_char(r, false, None))
}

/// Move the selection head one character to the right.
pub fn select_char_right(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_by(view, window, cx, true, |v, _, r| v.move_by_char(r, true, None))
}

/// Move the selection head one character forward.
pub fn select_char_forward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_char_right(view, window, cx)
}

/// Move the selection head one character backward.
pub fn select_char_backward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_char_left(view, window, cx)
}

/// Move the selection head one group to the left.
pub fn select_group_left(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_by(view, window, cx, false, |v, _, r| v.move_by_group(r, false))
}

/// Move the selection head one group to the right.
pub fn select_group_right(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_by(view, window, cx, true, |v, _, r| v.move_by_group(r, true))
}

/// Move the selection head one group forward.
pub fn select_group_forward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_group_right(view, window, cx)
}

/// Move the selection head one group backward.
pub fn select_group_backward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_group_left(view, window, cx)
}

/// Move the selection head one group forward in the default Windows style,
/// skipping to the start of the next group.
pub fn select_group_forward_win(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_by(view, window, cx, true, |v, _, r| group_forward_win(v, r))
}

/// Move the selection head one group or camel-case subword forward.
pub fn select_subword_forward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_by(view, window, cx, true, |v, _, r| move_by_subword(v, r, true))
}

/// Move the selection head one group or subword backward.
pub fn select_subword_backward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_by(view, window, cx, false, |v, _, r| move_by_subword(v, r, false))
}

/// Move the selection head one line up.
pub fn select_line_up(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_by(view, window, cx, false, |v, window, r| v.move_vertically(window, r, false, None))
}

/// Move the selection head one line down.
pub fn select_line_down(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_by(view, window, cx, true, |v, window, r| v.move_vertically(window, r, true, None))
}

/// Move the selection head one page up.
pub fn select_page_up(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    let h = page_height(view);
    select_by(view, window, cx, false, move |v, window, r| v.move_vertically(window, r, false, Some(h)))
}

/// Move the selection head one page down.
pub fn select_page_down(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    let h = page_height(view);
    select_by(view, window, cx, true, move |v, window, r| v.move_vertically(window, r, true, Some(h)))
}

/// Move the selection head to the next line boundary.
pub fn select_line_boundary_forward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_by(view, window, cx, true, |v, window, r| move_by_line_boundary(v, window, r, true))
}

/// Move the selection head to the previous line boundary.
pub fn select_line_boundary_backward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_by(view, window, cx, false, |v, window, r| move_by_line_boundary(v, window, r, false))
}

/// Move the selection head one line boundary to the left.
pub fn select_line_boundary_left(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_line_boundary_backward(view, window, cx)
}

/// Move the selection head one line boundary to the right.
pub fn select_line_boundary_right(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_line_boundary_forward(view, window, cx)
}

/// Move the selection head to the start of the line.
pub fn select_line_start(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_by(view, window, cx, false, |v, _, r| EditorSelection::cursor(v.line_block_at(r.head()).from))
}

/// Move the selection head to the end of the line.
pub fn select_line_end(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    select_by(view, window, cx, true, |v, _, r| EditorSelection::cursor(v.line_block_at(r.head()).to()))
}

fn add_cursor_vertically(
    view: &mut EditorView,
    window: &mut Window,
    cx: &mut Context<EditorView>,
    forward: bool,
) -> bool {
    let state = view.state().clone();
    let sel = state.selection();
    let mut ranges = sel.ranges.clone();
    for range in &sel.ranges {
        let line = state.doc().line_at(range.head());
        if if forward { line.to < state.doc().len() } else { line.from > 0 } {
            let mut cur = *range;
            loop {
                let next = view.move_vertically(window, &cur, forward, None);
                if next.head() < line.from || next.head() > line.to {
                    if !ranges.iter().any(|r| r.head() == next.head()) {
                        ranges.push(next);
                    }
                    break;
                } else if next.head() == cur.head() {
                    break;
                } else {
                    cur = next;
                }
            }
        }
    }
    if ranges.len() == sel.ranges.len() {
        return false;
    }
    let main = ranges.len() - 1;
    let tr = crate::commands::set_sel(&state, EditorSelection::create(ranges, main));
    view.dispatch(tr, cx);
    true
}

/// Expand the selection by adding a cursor above the heads of currently
/// selected ranges.
pub fn add_cursor_above(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    add_cursor_vertically(view, window, cx, false)
}

/// Expand the selection by adding a cursor below the heads of currently
/// selected ranges.
pub fn add_cursor_below(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    add_cursor_vertically(view, window, cx, true)
}

/// Delete selected lines.
pub fn delete_line(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    let state = view.state().clone();
    if state.read_only() {
        return false;
    }
    let len = state.doc().len();
    let specs: Vec<codemirror_state::ChangeSpec> = selected_line_blocks(&state)
        .iter()
        .map(|b| {
            let (mut from, mut to) = (b.from, b.to);
            if from > 0 {
                from -= 1;
            } else if to < len {
                to += 1;
            }
            codemirror_state::ChangeSpec::delete(from, to)
        })
        .collect();
    let changes = state.changes(specs);
    let selection = update_sel(state.selection(), |range| {
        let mut dist = None;
        if view.line_wrapping() {
            let block = view.line_block_at(range.head());
            let assoc = if range.assoc() != 0 { range.assoc() } else { 1 };
            let (_, _, bottom) = view.doc_coords_at_pos(window, range.head(), assoc);
            dist = Some((block.bottom() as f32 - bottom) + view.default_line_height() / 2.0);
        }
        view.move_vertically(window, range, true, dist)
    })
    .map(changes.desc(), -1);
    view.dispatch_spec(
        TransactionSpec::new().changes(changes).selection(selection).scroll_into_view().user_event("delete.line"),
        cx,
    );
    true
}

fn delete_by_view(
    view: &mut EditorView,
    window: &mut Window,
    cx: &mut Context<EditorView>,
    by: impl Fn(&EditorView, &mut Window, &SelectionRange) -> usize,
) -> bool {
    with_view(view, window, cx, |v, window, s, d| delete_by(s, d, |r| by(v, window, r)))
}

/// Delete the selection, or, if it is a cursor selection, delete to the end
/// of the line. If the cursor is directly at the end of the line, delete
/// the line break after it.
pub fn delete_to_line_end(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    delete_by_view(view, window, cx, |v, _, r| {
        let line_end = v.line_block_at(r.head()).to();
        if r.head() < line_end {
            line_end
        } else {
            v.state().doc().len().min(r.head() + 1)
        }
    })
}

/// Delete the selection, or, if it is a cursor selection, delete to the
/// start of the line. If the cursor is directly at the start of the line,
/// delete the line break before it.
pub fn delete_to_line_start(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    delete_by_view(view, window, cx, |v, _, r| {
        let line_start = v.line_block_at(r.head()).from;
        if r.head() > line_start {
            line_start
        } else {
            r.head().saturating_sub(1)
        }
    })
}

/// Delete the selection, or, if it is a cursor selection, delete to the
/// start of the line or the next line wrap before the cursor.
pub fn delete_line_boundary_backward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    delete_by_view(view, window, cx, |v, window, r| {
        let line_start = v.move_to_line_boundary(window, r, false, true).head();
        if r.head() > line_start {
            line_start
        } else {
            r.head().saturating_sub(1)
        }
    })
}

/// Delete the selection, or, if it is a cursor selection, delete to the end
/// of the line or the next line wrap after the cursor.
pub fn delete_line_boundary_forward(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    delete_by_view(view, window, cx, |v, window, r| {
        let line_end = v.move_to_line_boundary(window, r, true, true).head();
        if r.head() < line_end {
            line_end
        } else {
            v.state().doc().len().min(r.head() + 1)
        }
    })
}

/// Variant of [`delete_group_forward`] that uses the Windows convention of
/// also deleting the whitespace after a word.
pub fn delete_group_forward_win(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) -> bool {
    delete_by_view(view, window, cx, |v, _, r| group_forward_win(v, r).head())
}

/// Key bindings for the undo history.
///
/// - Mod-z: [`undo`].
/// - Mod-y (Mod-Shift-z on macOS) and Ctrl-Shift-z on Linux: [`redo`].
/// - Mod-u: [`undo_selection`].
/// - Alt-u (Mod-Shift-u on macOS): [`redo_selection`].
pub fn history_keymap() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("Mod-z").run_state(undo).prevent_default(),
        KeyBinding::new("Mod-y").mac("Mod-Shift-z").run_state(redo).prevent_default(),
        KeyBinding::linux_only("Ctrl-Shift-z").run_state(redo).prevent_default(),
        KeyBinding::new("Mod-u").run_state(undo_selection).prevent_default(),
        KeyBinding::new("Alt-u").mac("Mod-Shift-u").run_state(redo_selection).prevent_default(),
    ]
}

/// The Emacs-style bindings that are available on macOS by default.
pub fn emacs_style_keymap() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("Ctrl-b").run(cursor_char_left).shift(select_char_left).prevent_default(),
        KeyBinding::new("Ctrl-f").run(cursor_char_right).shift(select_char_right),
        KeyBinding::new("Ctrl-p").run(cursor_line_up).shift(select_line_up),
        KeyBinding::new("Ctrl-n").run(cursor_line_down).shift(select_line_down),
        KeyBinding::new("Ctrl-a").run(cursor_line_start).shift(select_line_start),
        KeyBinding::new("Ctrl-e").run(cursor_line_end).shift(select_line_end),
        KeyBinding::new("Ctrl-d").run_state(delete_char_forward),
        KeyBinding::new("Ctrl-h").run_state(delete_char_backward),
        KeyBinding::new("Ctrl-k").run(delete_to_line_end),
        KeyBinding::new("Ctrl-Alt-h").run_state(delete_group_backward),
        KeyBinding::new("Ctrl-o").run_state(split_line),
        KeyBinding::new("Ctrl-t").run_state(transpose_chars),
        KeyBinding::new("Ctrl-v").run(cursor_page_down),
    ]
}

/// Key bindings closely sticking to platform-standard or widely used
/// bindings: arrows, Home/End, PageUp/PageDown, Enter, Backspace, Delete,
/// select all, and the Emacs-style bindings on macOS.
pub fn standard_keymap() -> Vec<KeyBinding> {
    let mut map = vec![
        KeyBinding::new("ArrowLeft").run(cursor_char_left).shift(select_char_left).prevent_default(),
        KeyBinding::new("Mod-ArrowLeft")
            .mac("Alt-ArrowLeft")
            .run(cursor_group_left)
            .shift(select_group_left)
            .prevent_default(),
        KeyBinding::mac_only("Cmd-ArrowLeft")
            .run(cursor_line_boundary_left)
            .shift(select_line_boundary_left)
            .prevent_default(),
        KeyBinding::new("ArrowRight").run(cursor_char_right).shift(select_char_right).prevent_default(),
        KeyBinding::new("Mod-ArrowRight")
            .mac("Alt-ArrowRight")
            .run(cursor_group_right)
            .shift(select_group_right)
            .prevent_default(),
        KeyBinding::mac_only("Cmd-ArrowRight")
            .run(cursor_line_boundary_right)
            .shift(select_line_boundary_right)
            .prevent_default(),
        KeyBinding::new("ArrowUp").run(cursor_line_up).shift(select_line_up).prevent_default(),
        KeyBinding::mac_only("Cmd-ArrowUp").run_state(cursor_doc_start).shift_state(select_doc_start),
        KeyBinding::mac_only("Ctrl-ArrowUp").run(cursor_page_up).shift(select_page_up),
        KeyBinding::new("ArrowDown").run(cursor_line_down).shift(select_line_down).prevent_default(),
        KeyBinding::mac_only("Cmd-ArrowDown").run_state(cursor_doc_end).shift_state(select_doc_end),
        KeyBinding::mac_only("Ctrl-ArrowDown").run(cursor_page_down).shift(select_page_down),
        KeyBinding::new("PageUp").run(cursor_page_up).shift(select_page_up),
        KeyBinding::new("PageDown").run(cursor_page_down).shift(select_page_down),
        KeyBinding::new("Home")
            .run(cursor_line_boundary_backward)
            .shift(select_line_boundary_backward)
            .prevent_default(),
        KeyBinding::new("Mod-Home").run_state(cursor_doc_start).shift_state(select_doc_start),
        KeyBinding::new("End").run(cursor_line_boundary_forward).shift(select_line_boundary_forward).prevent_default(),
        KeyBinding::new("Mod-End").run_state(cursor_doc_end).shift_state(select_doc_end),
        KeyBinding::new("Enter").run_state(insert_newline_and_indent).shift_state(insert_newline_and_indent),
        KeyBinding::new("Mod-a").run_state(select_all),
        KeyBinding::new("Backspace")
            .run_state(delete_char_backward)
            .shift_state(delete_char_backward)
            .prevent_default(),
        KeyBinding::new("Delete").run_state(delete_char_forward).prevent_default(),
        KeyBinding::new("Mod-Backspace").mac("Alt-Backspace").run_state(delete_group_backward).prevent_default(),
        KeyBinding::new("Mod-Delete").mac("Alt-Delete").run_state(delete_group_forward).prevent_default(),
        KeyBinding::mac_only("Mod-Backspace").run(delete_line_boundary_backward).prevent_default(),
        KeyBinding::mac_only("Mod-Delete").run(delete_line_boundary_forward).prevent_default(),
    ];
    for b in emacs_style_keymap() {
        map.push(KeyBinding { mac: b.key, run: b.run, shift: b.shift, ..Default::default() });
    }
    map
}

/// The default keymap: [`standard_keymap`] plus syntax-free extras: move
/// and copy lines, add cursors, Escape, blank line, select line, indent,
/// delete line, matching bracket, and comments. (`cursorSyntax*`,
/// `selectParentSyntax` and tab-focus mode are not ported yet.)
pub fn default_keymap() -> Vec<KeyBinding> {
    let mut map = vec![
        KeyBinding::new("Alt-ArrowUp").run_state(move_line_up),
        KeyBinding::new("Shift-Alt-ArrowUp").run_state(copy_line_up),
        KeyBinding::new("Alt-ArrowDown").run_state(move_line_down),
        KeyBinding::new("Shift-Alt-ArrowDown").run_state(copy_line_down),
        KeyBinding::new("Mod-Alt-ArrowUp").run(add_cursor_above),
        KeyBinding::new("Mod-Alt-ArrowDown").run(add_cursor_below),
        KeyBinding::new("Escape").run_state(simplify_selection),
        KeyBinding::new("Mod-Enter").run_state(insert_blank_line),
        KeyBinding::new("Alt-l").mac("Ctrl-l").run_state(select_line),
        KeyBinding::new("Mod-[").run_state(indent_less),
        KeyBinding::new("Mod-]").run_state(indent_more),
        KeyBinding::new("Mod-Alt-\\").run_state(indent_selection),
        KeyBinding::new("Shift-Mod-k").run(delete_line),
        KeyBinding::new("Shift-Mod-\\").run_state(cursor_matching_bracket),
        KeyBinding::new("Mod-/").run_state(toggle_comment),
        KeyBinding::new("Alt-A").mac("Ctrl-A").run_state(toggle_block_comment),
    ];
    map.extend(standard_keymap());
    map
}

/// A binding that binds Tab to [`indent_more`] and Shift-Tab to
/// [`indent_less`].
pub fn indent_with_tab() -> KeyBinding {
    KeyBinding::new("Tab").run_state(indent_more).shift_state(indent_less)
}
