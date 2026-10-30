//! Editing commands. Port of the state-level commands in `commands.ts`.
//!
//! Waiting for the view crate: motion by character, group, subword, line,
//! page and line boundary (they use the view's layout and text direction),
//! `addCursorAbove`/`Below`, `deleteLine`, `deleteToLineStart`/`End`,
//! `deleteLineBoundary*`, `deleteGroupForwardWin`, tab-focus mode, the
//! screen-reader announcement on deletion, atomic ranges, and the keymaps.
//! Waiting for a syntax tree: `cursorSyntax*`, `selectSyntax*` and
//! `selectParentSyntax`.

use std::cell::RefCell;
use std::collections::HashMap;

use codemirror_language::{
    get_indent_unit, get_indentation, indent_string, match_brackets, BracketConfig, IndentContext, IndentOptions,
    INDENT_UNIT,
};
use codemirror_state::{
    count_column, find_cluster_break, ChangeByRange, ChangeSpec, CharCategory, EditorSelection, EditorState, Line,
    RangeChange, SelectionRange, Text, Transaction, TransactionSpec,
};

use crate::{is_space, leading_space};

pub(crate) type Dispatch<'a> = &'a mut dyn FnMut(Transaction);

pub(crate) fn update_sel(sel: &EditorSelection, by: impl FnMut(&SelectionRange) -> SelectionRange) -> EditorSelection {
    EditorSelection::create(sel.ranges.iter().map(by).collect(), sel.main_index)
}

pub(crate) fn set_sel(state: &EditorState, selection: EditorSelection) -> Transaction {
    state.update(TransactionSpec::new().selection(selection).scroll_into_view().user_event("select"))
}

pub(crate) fn move_sel(
    state: &EditorState,
    dispatch: Dispatch,
    how: impl FnMut(&SelectionRange) -> SelectionRange,
) -> bool {
    let selection = update_sel(state.selection(), how);
    if selection.eq(state.selection(), true) {
        return false;
    }
    dispatch(set_sel(state, selection));
    true
}

pub(crate) fn range_end(range: &SelectionRange, forward: bool) -> SelectionRange {
    EditorSelection::cursor(if forward { range.to } else { range.from })
}

fn by_char_logical(state: &EditorState, range: &SelectionRange, forward: bool) -> SelectionRange {
    let mut pos = range.head();
    let line = state.doc().line_at(pos);
    if pos == if forward { line.to } else { line.from } {
        pos = if forward { state.doc().len().min(line.to + 1) } else { line.from.saturating_sub(1) };
    } else {
        pos = line.from + find_cluster_break(&line.text, pos - line.from, forward, true);
    }
    EditorSelection::cursor_with(pos, if forward { -1 } else { 1 }, None, None)
}

fn move_by_char_logical(state: &EditorState, dispatch: Dispatch, forward: bool) -> bool {
    move_sel(state, dispatch, |range| {
        if range.is_empty() {
            by_char_logical(state, range, forward)
        } else {
            range_end(range, forward)
        }
    })
}

/// Move the selection one character forward, in logical
/// (non-text-direction-aware) string index order.
pub fn cursor_char_forward_logical(state: &EditorState, dispatch: Dispatch) -> bool {
    move_by_char_logical(state, dispatch, true)
}

/// Move the selection one character backward, in logical string index
/// order.
pub fn cursor_char_backward_logical(state: &EditorState, dispatch: Dispatch) -> bool {
    move_by_char_logical(state, dispatch, false)
}

// The position one character before or after `pos`.
fn char_step(state: &EditorState, pos: usize, forward: bool) -> usize {
    let rope = state.doc().rope();
    let c = if forward { rope.byte_slice(pos..).chars().next() } else { rope.byte_slice(..pos).chars().next_back() };
    match c {
        Some(c) if forward => pos + c.len_utf8(),
        Some(c) => pos - c.len_utf8(),
        None => pos,
    }
}

fn to_matching_bracket(state: &EditorState, dispatch: Dispatch, extend: bool) -> bool {
    let mut found = false;
    let config = BracketConfig::default();
    let len = state.doc().len();
    let selection = update_sel(state.selection(), |range| {
        let head = range.head();
        let matching = match_brackets(state, head, -1, &config)
            .or_else(|| match_brackets(state, head, 1, &config))
            .or_else(|| if head > 0 { match_brackets(state, char_step(state, head, false), 1, &config) } else { None })
            .or_else(
                || if head < len { match_brackets(state, char_step(state, head, true), -1, &config) } else { None },
            );
        let Some((start, end)) = matching.and_then(|m| m.end.map(|end| (m.start, end))) else { return *range };
        found = true;
        let head = if start.from == head { end.to } else { end.from };
        if extend {
            EditorSelection::range(range.anchor(), head)
        } else {
            EditorSelection::cursor(head)
        }
    });
    if !found {
        return false;
    }
    dispatch(set_sel(state, selection));
    true
}

/// Move the selection to the bracket matching the one it is currently on,
/// if any.
pub fn cursor_matching_bracket(state: &EditorState, dispatch: Dispatch) -> bool {
    to_matching_bracket(state, dispatch, false)
}

/// Extend the selection to the bracket matching the one the selection head
/// is currently on, if any.
pub fn select_matching_bracket(state: &EditorState, dispatch: Dispatch) -> bool {
    to_matching_bracket(state, dispatch, true)
}

pub(crate) fn extend_sel(
    state: &EditorState,
    dispatch: Dispatch,
    forward: bool,
    mut how: impl FnMut(&SelectionRange) -> SelectionRange,
) -> bool {
    let selection = update_sel(state.selection(), |range| {
        let mut range = *range;
        if range.undirectional() && (range.head() >= range.anchor()) != forward {
            range = EditorSelection::range(range.head(), range.anchor());
        }
        let head = how(&range);
        EditorSelection::range_with(
            range.anchor(),
            head.head(),
            head.goal_column,
            head.bidi_level().filter(|&l| l != 0),
            head.assoc(),
        )
    });
    if selection.eq(state.selection(), false) {
        return false;
    }
    dispatch(set_sel(state, selection));
    true
}

/// Move the selection head one character forward by logical
/// (non-direction aware) string index order.
pub fn select_char_forward_logical(state: &EditorState, dispatch: Dispatch) -> bool {
    extend_sel(state, dispatch, true, |range| by_char_logical(state, range, true))
}

/// Move the selection head one character backward by logical string index
/// order.
pub fn select_char_backward_logical(state: &EditorState, dispatch: Dispatch) -> bool {
    extend_sel(state, dispatch, false, |range| by_char_logical(state, range, false))
}

/// Move the selection to the start of the document.
pub fn cursor_doc_start(state: &EditorState, dispatch: Dispatch) -> bool {
    dispatch(set_sel(state, EditorSelection::single(0, 0)));
    true
}

/// Move the selection to the end of the document.
pub fn cursor_doc_end(state: &EditorState, dispatch: Dispatch) -> bool {
    let len = state.doc().len();
    dispatch(set_sel(state, EditorSelection::single(len, len)));
    true
}

/// Move the selection head to the start of the document.
pub fn select_doc_start(state: &EditorState, dispatch: Dispatch) -> bool {
    dispatch(set_sel(state, EditorSelection::single(state.selection().main().anchor(), 0)));
    true
}

/// Move the selection head to the end of the document.
pub fn select_doc_end(state: &EditorState, dispatch: Dispatch) -> bool {
    dispatch(set_sel(state, EditorSelection::single(state.selection().main().anchor(), state.doc().len())));
    true
}

/// Select the entire document.
pub fn select_all(state: &EditorState, dispatch: Dispatch) -> bool {
    let selection = EditorSelection::single(0, state.doc().len());
    dispatch(state.update(TransactionSpec::new().selection(selection).user_event("select")));
    true
}

/// Expand the selection to cover entire lines.
pub fn select_line(state: &EditorState, dispatch: Dispatch) -> bool {
    let len = state.doc().len();
    let ranges = selected_line_blocks(state)
        .iter()
        .map(|b| EditorSelection::undirectional_range(b.from, (b.to + 1).min(len)))
        .collect();
    dispatch(state.update(TransactionSpec::new().selection(EditorSelection::create(ranges, 0)).user_event("select")));
    true
}

/// Simplify the current selection. When multiple ranges are selected,
/// reduce it to its main range. Otherwise, if the selection is non-empty,
/// convert it to a cursor selection.
pub fn simplify_selection(state: &EditorState, dispatch: Dispatch) -> bool {
    let cur = state.selection();
    let selection = if cur.ranges.len() > 1 {
        EditorSelection::create(vec![cur.main()], 0)
    } else if !cur.main().is_empty() {
        EditorSelection::create(vec![EditorSelection::cursor(cur.main().head())], 0)
    } else {
        return false;
    };
    dispatch(set_sel(state, selection));
    true
}

pub(crate) fn delete_by(state: &EditorState, dispatch: Dispatch, mut by: impl FnMut(&SelectionRange) -> usize) -> bool {
    if state.read_only() {
        return false;
    }
    let mut event = "delete.selection";
    // CodeMirror also skips over atomic ranges here; those come with the
    // view.
    let changes = state.change_by_range(|range| {
        let (mut from, mut to) = (range.from, range.to);
        if from == to {
            let towards = by(&range);
            if towards < from {
                event = "delete.backward";
            } else if towards > from {
                event = "delete.forward";
            }
            from = from.min(towards);
            to = to.max(towards);
        }
        if from == to {
            RangeChange::new(range)
        } else {
            RangeChange::new(EditorSelection::cursor_with(from, if from < range.head() { -1 } else { 1 }, None, None))
                .changes(ChangeSpec::delete(from, to))
        }
    });
    if changes.changes.is_empty() {
        return false;
    }
    // The view adds a screen-reader announcement for "delete.selection".
    dispatch(state.update_many(vec![changes.into(), TransactionSpec::new().scroll_into_view().user_event(event)]));
    true
}

fn is_variation_selector(c: char) -> bool {
    ('\u{fe00}'..='\u{fe0f}').contains(&c)
}

fn delete_by_char(state: &EditorState, dispatch: Dispatch, forward: bool, by_indent_unit: bool) -> bool {
    delete_by(state, dispatch, |range| {
        let mut pos = range.from;
        let line = state.doc().line_at(pos);
        let before = &line.text[..pos - line.from];
        if by_indent_unit
            && !forward
            && pos > line.from
            && pos < line.from + 200
            && before.bytes().all(|b| b == b' ' || b == b'\t')
        {
            if before.ends_with('\t') {
                return pos - 1;
            }
            let col = count_column(before, state.tab_size(), None);
            let unit = get_indent_unit(state);
            let drop = match col % unit {
                0 => unit,
                d => d,
            };
            let bytes = before.as_bytes();
            let mut i = 0;
            while i < drop && i < bytes.len() && bytes[bytes.len() - 1 - i] == b' ' {
                pos -= 1;
                i += 1;
            }
            pos
        } else {
            let mut target = find_cluster_break(&line.text, pos - line.from, forward, forward) + line.from;
            if target == pos && line.number != if forward { state.doc().lines() } else { 1 } {
                target = if forward { pos + 1 } else { pos - 1 };
            } else if !forward && line.text[target - line.from..pos - line.from].chars().any(is_variation_selector) {
                target = find_cluster_break(&line.text, target - line.from, false, false) + line.from;
            }
            target
        }
    })
}

/// Delete the selection, or, for cursor selections, the character or
/// indentation unit before the cursor.
pub fn delete_char_backward(state: &EditorState, dispatch: Dispatch) -> bool {
    delete_by_char(state, dispatch, false, true)
}

/// Delete the selection or the character before the cursor. Does not
/// implement any extended behavior like deleting whole indentation units in
/// one go.
pub fn delete_char_backward_strict(state: &EditorState, dispatch: Dispatch) -> bool {
    delete_by_char(state, dispatch, false, false)
}

/// Delete the selection or the character after the cursor.
pub fn delete_char_forward(state: &EditorState, dispatch: Dispatch) -> bool {
    delete_by_char(state, dispatch, true, false)
}

fn delete_by_group(state: &EditorState, dispatch: Dispatch, forward: bool) -> bool {
    delete_by(state, dispatch, |range| {
        let mut pos = range.head();
        let line = state.doc().line_at(pos);
        let categorize = state.char_categorizer(pos);
        let mut cat: Option<CharCategory> = None;
        loop {
            if pos == if forward { line.to } else { line.from } {
                if pos == range.head() && line.number != if forward { state.doc().lines() } else { 1 } {
                    pos = if forward { pos + 1 } else { pos - 1 };
                }
                break;
            }
            let next = find_cluster_break(&line.text, pos - line.from, forward, true) + line.from;
            let next_char = &line.text[pos.min(next) - line.from..pos.max(next) - line.from];
            let next_cat = categorize(next_char);
            if cat.is_some_and(|c| c != next_cat) {
                break;
            }
            if next_char != " " || pos != range.head() {
                cat = Some(next_cat);
            }
            pos = next;
        }
        pos
    })
}

/// Delete the selection or backward until the end of the next group, only
/// skipping groups of whitespace when they consist of a single space.
pub fn delete_group_backward(state: &EditorState, dispatch: Dispatch) -> bool {
    delete_by_group(state, dispatch, false)
}

/// Delete the selection or forward until the end of the next group.
pub fn delete_group_forward(state: &EditorState, dispatch: Dispatch) -> bool {
    delete_by_group(state, dispatch, true)
}

/// Delete all whitespace directly before a line end from the document.
pub fn delete_trailing_whitespace(state: &EditorState, dispatch: Dispatch) -> bool {
    if state.read_only() {
        return false;
    }
    let doc = state.doc();
    let mut changes = Vec::new();
    for n in 1..=doc.lines() {
        let line = doc.line(n);
        let trimmed = line.text.trim_end_matches(is_space).len();
        if trimmed < line.text.len() {
            changes.push(ChangeSpec::delete(line.from + trimmed, line.to));
        }
    }
    if changes.is_empty() {
        return false;
    }
    dispatch(state.update(TransactionSpec::new().changes(changes).user_event("delete")));
    true
}

/// Replace each selection range with a line break, leaving the cursor on
/// the line before the break.
pub fn split_line(state: &EditorState, dispatch: Dispatch) -> bool {
    if state.read_only() {
        return false;
    }
    let changes = state.change_by_range(|range| {
        RangeChange::new(EditorSelection::cursor(range.from)).changes(ChangeSpec::replace(
            range.from,
            range.to,
            Text::of(&["", ""]),
        ))
    });
    dispatch(state.update_many(vec![changes.into(), TransactionSpec::new().scroll_into_view().user_event("input")]));
    true
}

/// Flip the characters before and after the cursor(s).
pub fn transpose_chars(state: &EditorState, dispatch: Dispatch) -> bool {
    if state.read_only() {
        return false;
    }
    let doc = state.doc();
    let changes = state.change_by_range(|range| {
        if !range.is_empty() || range.from == 0 || range.from == doc.len() {
            return RangeChange::new(range);
        }
        let pos = range.from;
        let line = doc.line_at(pos);
        let from = if pos == line.from {
            pos - 1
        } else {
            find_cluster_break(&line.text, pos - line.from, false, true) + line.from
        };
        let to = if pos == line.to {
            pos + 1
        } else {
            find_cluster_break(&line.text, pos - line.from, true, true) + line.from
        };
        RangeChange::new(EditorSelection::cursor(to)).changes(ChangeSpec::replace(
            from,
            to,
            doc.slice(pos, to).append(&doc.slice(from, pos)),
        ))
    });
    if changes.changes.is_empty() {
        return false;
    }
    dispatch(
        state.update_many(vec![changes.into(), TransactionSpec::new().scroll_into_view().user_event("move.character")]),
    );
    true
}

pub(crate) struct LineBlock {
    pub from: usize,
    pub to: usize,
    pub ranges: Vec<SelectionRange>,
}

pub(crate) fn selected_line_blocks(state: &EditorState) -> Vec<LineBlock> {
    let doc = state.doc();
    let mut blocks: Vec<LineBlock> = Vec::new();
    let mut upto = 0;
    for range in &state.selection().ranges {
        let start_line = doc.line_at(range.from);
        let mut end_line = doc.line_at(range.to);
        if !range.is_empty() && range.to == end_line.from {
            end_line = doc.line_at(range.to - 1);
        }
        match blocks.last_mut() {
            Some(prev) if upto >= start_line.number => {
                prev.to = end_line.to;
                prev.ranges.push(*range);
            }
            _ => blocks.push(LineBlock { from: start_line.from, to: end_line.to, ranges: vec![*range] }),
        }
        upto = end_line.number + 1;
    }
    blocks
}

fn move_line(state: &EditorState, dispatch: Dispatch, forward: bool) -> bool {
    if state.read_only() {
        return false;
    }
    let doc = state.doc();
    let len = doc.len();
    let mut changes = Vec::new();
    let mut ranges = Vec::new();
    for block in selected_line_blocks(state) {
        if if forward { block.to == len } else { block.from == 0 } {
            continue;
        }
        let next_line = doc.line_at(if forward { block.to + 1 } else { block.from - 1 });
        let size = next_line.length() + 1;
        if forward {
            changes.push(ChangeSpec::delete(block.to, next_line.to));
            changes.push(ChangeSpec::insert(block.from, format!("{}{}", next_line.text, state.line_break())));
            for r in &block.ranges {
                ranges.push(EditorSelection::range(len.min(r.anchor() + size), len.min(r.head() + size)));
            }
        } else {
            changes.push(ChangeSpec::delete(next_line.from, block.from));
            changes.push(ChangeSpec::insert(block.to, format!("{}{}", state.line_break(), next_line.text)));
            for r in &block.ranges {
                ranges.push(EditorSelection::range(r.anchor() - size, r.head() - size));
            }
        }
    }
    if changes.is_empty() {
        return false;
    }
    // CodeMirror keeps the main index even when blocks at the document edge
    // were skipped; clamp it so the selection stays valid.
    let main = state.selection().main_index.min(ranges.len() - 1);
    dispatch(
        state.update(
            TransactionSpec::new()
                .changes(changes)
                .scroll_into_view()
                .selection(EditorSelection::create(ranges, main))
                .user_event("move.line"),
        ),
    );
    true
}

/// Move the selected lines up one line.
pub fn move_line_up(state: &EditorState, dispatch: Dispatch) -> bool {
    move_line(state, dispatch, false)
}

/// Move the selected lines down one line.
pub fn move_line_down(state: &EditorState, dispatch: Dispatch) -> bool {
    move_line(state, dispatch, true)
}

fn copy_line(state: &EditorState, dispatch: Dispatch, forward: bool) -> bool {
    if state.read_only() {
        return false;
    }
    let mut changes = Vec::new();
    for block in selected_line_blocks(state) {
        let text = state.slice_doc(block.from, block.to);
        if forward {
            changes.push(ChangeSpec::insert(block.from, format!("{}{}", text, state.line_break())));
        } else {
            changes.push(ChangeSpec::insert(block.to, format!("{}{}", state.line_break(), text)));
        }
    }
    let change_set = state.changes(changes);
    let selection = state.selection().map(change_set.desc(), if forward { 1 } else { -1 });
    dispatch(state.update(
        TransactionSpec::new().changes(change_set).selection(selection).scroll_into_view().user_event("input.copyline"),
    ));
    true
}

/// Create a copy of the selected lines. Keep the selection in the top copy.
pub fn copy_line_up(state: &EditorState, dispatch: Dispatch) -> bool {
    copy_line(state, dispatch, false)
}

/// Create a copy of the selected lines. Keep the selection in the bottom
/// copy.
pub fn copy_line_down(state: &EditorState, dispatch: Dispatch) -> bool {
    copy_line(state, dispatch, true)
}

/// Replace the selection with a newline.
pub fn insert_newline(state: &EditorState, dispatch: Dispatch) -> bool {
    dispatch(state.update_many(vec![
        state.replace_selection(state.line_break()),
        TransactionSpec::new().scroll_into_view().user_event("input"),
    ]));
    true
}

/// Replace the selection with a newline and the same amount of indentation
/// as the line above.
pub fn insert_newline_keep_indent(state: &EditorState, dispatch: Dispatch) -> bool {
    let changes = state.change_by_range(|range| {
        let text = state.doc().line_at(range.from).text;
        let indent = &text[..leading_space(&text)];
        RangeChange::new(EditorSelection::cursor(range.from + indent.len() + 1)).changes(ChangeSpec::replace(
            range.from,
            range.to,
            format!("{}{}", state.line_break(), indent),
        ))
    });
    dispatch(state.update_many(vec![changes.into(), TransactionSpec::new().scroll_into_view().user_event("input")]));
    true
}

fn is_between_brackets(state: &EditorState, pos: usize) -> Option<(usize, usize)> {
    let rope = state.doc().rope();
    let before = rope.byte_slice(..pos).chars().next_back();
    let after = rope.byte_slice(pos..).chars().next();
    if matches!((before, after), (Some('('), Some(')')) | (Some('['), Some(']')) | (Some('{'), Some('}'))) {
        return Some((pos, pos));
    }
    // CodeMirror then looks for bracket tokens in the syntax tree, which
    // finds nothing until there is one.
    None
}

/// Replace the selection with a newline and indent the newly created
/// line(s). If the current line consists only of whitespace, this will also
/// delete that whitespace. When the cursor is between matching brackets, an
/// additional newline will be inserted after the cursor.
pub fn insert_newline_and_indent(state: &EditorState, dispatch: Dispatch) -> bool {
    newline_and_indent(state, dispatch, false)
}

/// Create a blank, indented line below the current line.
pub fn insert_blank_line(state: &EditorState, dispatch: Dispatch) -> bool {
    newline_and_indent(state, dispatch, true)
}

fn newline_and_indent(state: &EditorState, dispatch: Dispatch, at_eof: bool) -> bool {
    if state.read_only() {
        return false;
    }
    let doc = state.doc();
    let changes = state.change_by_range(|range| {
        let (mut from, mut to) = (range.from, range.to);
        let line = doc.line_at(from);
        let explode = if !at_eof && from == to { is_between_brackets(state, from) } else { None };
        if at_eof {
            to = if to <= line.to { line.to } else { doc.line_at(to).to };
            from = to;
        }
        let cx = IndentContext::with_options(
            state,
            IndentOptions {
                simulate_break: Some(from),
                simulate_double_break: explode.is_some(),
                ..Default::default()
            },
        );
        let indent = get_indentation(&cx, from).unwrap_or_else(|| {
            let text = doc.line_at(from).text;
            count_column(&text[..leading_space(&text)], state.tab_size(), None)
        });

        while to < line.to && line.text[to - line.from..].chars().next().is_some_and(is_space) {
            to += line.text[to - line.from..].chars().next().unwrap().len_utf8();
        }
        if let Some((ex_from, ex_to)) = explode {
            (from, to) = (ex_from, ex_to);
        } else if from > line.from && from < line.from + 100 {
            // CodeMirror slices the line with the absolute position here
            // (`line.text.slice(0, from)`); kept as is.
            let mut end = from.min(line.text.len());
            while !line.text.is_char_boundary(end) {
                end -= 1;
            }
            if line.text[..end].chars().all(is_space) {
                from = line.from;
            }
        }
        let mut insert = vec![String::new(), indent_string(state, indent)];
        if explode.is_some() {
            insert.push(indent_string(state, cx.line_indent(line.from, -1)));
        }
        RangeChange::new(EditorSelection::cursor(from + 1 + insert[1].len())).changes(ChangeSpec::replace(
            from,
            to,
            Text::of(&insert),
        ))
    });
    dispatch(state.update_many(vec![changes.into(), TransactionSpec::new().scroll_into_view().user_event("input")]));
    true
}

fn change_by_selected_line(
    state: &EditorState,
    mut f: impl FnMut(&Line, &mut Vec<ChangeSpec>, &SelectionRange),
) -> ChangeByRange {
    let mut at_line = 0;
    state.change_by_range(|range| {
        let mut changes = Vec::new();
        let mut pos = range.from;
        while pos <= range.to {
            let line = state.doc().line_at(pos);
            if line.number > at_line && (range.is_empty() || range.to > line.from) {
                f(&line, &mut changes, &range);
                at_line = line.number;
            }
            pos = line.to + 1;
        }
        let change_set = state.changes(changes.clone());
        RangeChange::new(EditorSelection::range(
            change_set.map_pos(range.anchor(), 1),
            change_set.map_pos(range.head(), 1),
        ))
        .changes(changes)
    })
}

/// Auto-indent the selected lines. This uses the [indentation service
/// facet](codemirror_language::INDENT_SERVICE) as source for auto-indent
/// information.
pub fn indent_selection(state: &EditorState, dispatch: Dispatch) -> bool {
    if state.read_only() {
        return false;
    }
    let updated = RefCell::new(HashMap::<usize, usize>::new());
    let context = IndentContext::with_options(
        state,
        IndentOptions {
            override_indentation: Some(Box::new(|start| updated.borrow().get(&start).copied())),
            ..Default::default()
        },
    );
    let changes = change_by_selected_line(state, |line, changes, range| {
        let Some(mut indent) = get_indentation(&context, line.from) else { return };
        if line.text.chars().all(is_space) {
            indent = 0;
        }
        let cur = &line.text[..leading_space(&line.text)];
        let norm = indent_string(state, indent);
        if cur != norm || range.from < line.from + cur.len() {
            updated.borrow_mut().insert(line.from, indent);
            changes.push(ChangeSpec::replace(line.from, line.from + cur.len(), norm));
        }
    });
    if !changes.changes.is_empty() {
        dispatch(state.update_many(vec![changes.into(), TransactionSpec::new().user_event("indent")]));
    }
    true
}

/// Add a [unit](codemirror_language::INDENT_UNIT) of indentation to all
/// selected lines.
pub fn indent_more(state: &EditorState, dispatch: Dispatch) -> bool {
    if state.read_only() {
        return false;
    }
    let unit = state.facet(&INDENT_UNIT).clone();
    let changes = change_by_selected_line(state, |line, changes, _| {
        changes.push(ChangeSpec::insert(line.from, unit.clone()));
    });
    dispatch(state.update_many(vec![changes.into(), TransactionSpec::new().user_event("input.indent")]));
    true
}

/// Remove a [unit](codemirror_language::INDENT_UNIT) of indentation from
/// all selected lines.
pub fn indent_less(state: &EditorState, dispatch: Dispatch) -> bool {
    if state.read_only() {
        return false;
    }
    let changes = change_by_selected_line(state, |line, changes, _| {
        let space = &line.text[..leading_space(&line.text)];
        if space.is_empty() {
            return;
        }
        let col = count_column(space, state.tab_size(), None);
        let insert = indent_string(state, col.saturating_sub(get_indent_unit(state)));
        let keep = space.bytes().zip(insert.bytes()).take_while(|(a, b)| a == b).count();
        changes.push(ChangeSpec::replace(line.from + keep, line.from + space.len(), &insert[keep..]));
    });
    dispatch(state.update_many(vec![changes.into(), TransactionSpec::new().user_event("delete.dedent")]));
    true
}

/// Insert a tab character at the cursor or, if something is selected, use
/// [`indent_more`] to indent the entire selection.
pub fn insert_tab(state: &EditorState, dispatch: Dispatch) -> bool {
    if state.selection().ranges.iter().any(|r| !r.is_empty()) {
        return indent_more(state, dispatch);
    }
    dispatch(state.update_many(vec![
        state.replace_selection("\t"),
        TransactionSpec::new().scroll_into_view().user_event("input"),
    ]));
    true
}
