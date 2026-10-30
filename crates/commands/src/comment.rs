//! Comment toggling. Port of `comment.ts`.

use codemirror_state::{ChangeSpec, EditorState, Line, Transaction, TransactionSpec};

use crate::{is_space, leading_space, trailing_space};

/// Block comment delimiters, for example `/*` and `*/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockCommentTokens {
    pub open: String,
    pub close: String,
}

/// A value of this type can be provided as [language
/// data](EditorState::language_data_at) under a `"commentTokens"` property
/// to configure comment syntax for a language.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommentTokens {
    /// The block comment syntax, if any. For example, for HTML you'd
    /// provide `<!--` and `-->`.
    pub block: Option<BlockCommentTokens>,
    /// The line comment syntax. For example `"//"`.
    pub line: Option<String>,
}

/// Comment or uncomment the current selection. Will use line comments if
/// available, otherwise falling back to block comments.
pub fn toggle_comment(state: &EditorState, dispatch: &mut dyn FnMut(Transaction)) -> bool {
    let line = state.doc().line_at(state.selection().main().from);
    let config = get_config(state, line.from);
    if config.line.is_some() {
        toggle_line_comment(state, dispatch)
    } else if config.block.is_some() {
        toggle_block_comment_by_line(state, dispatch)
    } else {
        false
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CommentOption {
    Toggle,
    Comment,
    Uncomment,
}

fn command(
    state: &EditorState,
    dispatch: &mut dyn FnMut(Transaction),
    f: impl FnOnce(&EditorState) -> Option<TransactionSpec>,
) -> bool {
    if state.read_only() {
        return false;
    }
    let Some(tr) = f(state) else { return false };
    dispatch(state.update(tr));
    true
}

fn selection_ranges(state: &EditorState) -> Vec<(usize, usize)> {
    state.selection().ranges.iter().map(|r| (r.from, r.to)).collect()
}

/// Comment or uncomment the current selection using line comments. The
/// line comment syntax is taken from the [`CommentTokens`] language data.
pub fn toggle_line_comment(state: &EditorState, dispatch: &mut dyn FnMut(Transaction)) -> bool {
    command(state, dispatch, |s| change_line_comment(CommentOption::Toggle, s, &selection_ranges(s)))
}

/// Comment the current selection using line comments.
pub fn line_comment(state: &EditorState, dispatch: &mut dyn FnMut(Transaction)) -> bool {
    command(state, dispatch, |s| change_line_comment(CommentOption::Comment, s, &selection_ranges(s)))
}

/// Uncomment the current selection using line comments.
pub fn line_uncomment(state: &EditorState, dispatch: &mut dyn FnMut(Transaction)) -> bool {
    command(state, dispatch, |s| change_line_comment(CommentOption::Uncomment, s, &selection_ranges(s)))
}

/// Comment or uncomment the current selection using block comments. The
/// block comment syntax is taken from the [`CommentTokens`] language data.
pub fn toggle_block_comment(state: &EditorState, dispatch: &mut dyn FnMut(Transaction)) -> bool {
    command(state, dispatch, |s| change_block_comment(CommentOption::Toggle, s, &selection_ranges(s)))
}

/// Comment the current selection using block comments.
pub fn block_comment(state: &EditorState, dispatch: &mut dyn FnMut(Transaction)) -> bool {
    command(state, dispatch, |s| change_block_comment(CommentOption::Comment, s, &selection_ranges(s)))
}

/// Uncomment the current selection using block comments.
pub fn block_uncomment(state: &EditorState, dispatch: &mut dyn FnMut(Transaction)) -> bool {
    command(state, dispatch, |s| change_block_comment(CommentOption::Uncomment, s, &selection_ranges(s)))
}

/// Comment or uncomment the lines around the current selection using block
/// comments.
pub fn toggle_block_comment_by_line(state: &EditorState, dispatch: &mut dyn FnMut(Transaction)) -> bool {
    command(state, dispatch, |s| change_block_comment(CommentOption::Toggle, s, &selected_line_ranges(s)))
}

fn get_config(state: &EditorState, pos: usize) -> CommentTokens {
    state.language_data_at::<CommentTokens>("commentTokens", pos, 1).into_iter().next().unwrap_or_default()
}

struct CommentEnd {
    pos: usize,
    margin: usize,
}

struct BlockComment {
    open: CommentEnd,
    close: CommentEnd,
}

const SEARCH_MARGIN: usize = 50;

// Move `pos` back (or forward) to the nearest character boundary.
fn boundary(state: &EditorState, mut pos: usize, forward: bool) -> usize {
    let rope = state.doc().rope();
    while !rope.is_char_boundary(pos) {
        if forward {
            pos += 1;
        } else {
            pos -= 1;
        }
    }
    pos
}

fn ends_in_space(s: &str) -> bool {
    s.chars().next_back().is_some_and(is_space)
}

/// Determines if the given range is block-commented in the given state.
fn find_block_comment(
    state: &EditorState,
    tokens: &BlockCommentTokens,
    from: usize,
    to: usize,
) -> Option<BlockComment> {
    let (open, close) = (&tokens.open, &tokens.close);
    let text_before = state.slice_doc(boundary(state, from.saturating_sub(SEARCH_MARGIN), true), from);
    let text_after = state.slice_doc(to, boundary(state, (to + SEARCH_MARGIN).min(state.doc().len()), false));
    let space_before = trailing_space(&text_before);
    let space_after = leading_space(&text_after);
    let before_off = text_before.len() - space_before;
    if text_before[..before_off].ends_with(open.as_str()) && text_after[space_after..].starts_with(close.as_str()) {
        return Some(BlockComment {
            open: CommentEnd { pos: from - space_before, margin: usize::from(space_before > 0) },
            close: CommentEnd { pos: to + space_after, margin: usize::from(space_after > 0) },
        });
    }

    let (start_text, end_text) = if to - from <= 2 * SEARCH_MARGIN {
        let text = state.slice_doc(from, to);
        (text.clone(), text)
    } else {
        (
            state.slice_doc(from, boundary(state, from + SEARCH_MARGIN, false)),
            state.slice_doc(boundary(state, to - SEARCH_MARGIN, true), to),
        )
    };
    let start_space = leading_space(&start_text);
    let end_space = trailing_space(&end_text);
    let end_trimmed = &end_text[..end_text.len() - end_space];
    if start_text[start_space..].starts_with(open.as_str()) && end_trimmed.ends_with(close.as_str()) {
        let end_off = end_trimmed.len() - close.len();
        let after_open = &start_text[start_space + open.len()..];
        return Some(BlockComment {
            open: CommentEnd {
                pos: from + start_space + open.len(),
                margin: usize::from(after_open.chars().next().is_some_and(is_space)),
            },
            close: CommentEnd {
                pos: to - end_space - close.len(),
                margin: usize::from(ends_in_space(&end_text[..end_off])),
            },
        });
    }
    None
}

fn selected_line_ranges(state: &EditorState) -> Vec<(usize, usize)> {
    let doc = state.doc();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for r in &state.selection().ranges {
        let from_line = doc.line_at(r.from);
        let mut to_line = if r.to <= from_line.to { from_line.clone() } else { doc.line_at(r.to) };
        if to_line.from > from_line.from && to_line.from == r.to {
            to_line = if r.to == from_line.to + 1 { from_line.clone() } else { doc.line_at(r.to - 1) };
        }
        match ranges.last_mut() {
            Some(last) if last.1 > from_line.from => last.1 = to_line.to,
            _ => ranges.push((from_line.from + leading_space(&from_line.text), to_line.to)),
        }
    }
    ranges
}

// Performs toggle, comment and uncomment of block comments in languages
// that support them.
fn change_block_comment(
    option: CommentOption,
    state: &EditorState,
    ranges: &[(usize, usize)],
) -> Option<TransactionSpec> {
    let tokens = ranges.iter().map(|r| get_config(state, r.0).block).collect::<Option<Vec<_>>>()?;
    let comments: Vec<Option<BlockComment>> =
        ranges.iter().zip(&tokens).map(|(r, t)| find_block_comment(state, t, r.0, r.1)).collect();
    if option != CommentOption::Uncomment && !comments.iter().all(Option::is_some) {
        let mut changes = Vec::new();
        for ((range, token), comment) in ranges.iter().zip(&tokens).zip(&comments) {
            if comment.is_none() {
                changes.push(ChangeSpec::insert(range.0, format!("{} ", token.open)));
                changes.push(ChangeSpec::insert(range.1, format!(" {}", token.close)));
            }
        }
        Some(TransactionSpec::new().changes(state.changes(changes)))
    } else if option != CommentOption::Comment && comments.iter().any(Option::is_some) {
        let mut changes = Vec::new();
        for (token, comment) in tokens.iter().zip(&comments) {
            if let Some(BlockComment { open, close }) = comment {
                changes.push(ChangeSpec::delete(open.pos - token.open.len(), open.pos + open.margin));
                changes.push(ChangeSpec::delete(close.pos - close.margin, close.pos + token.close.len()));
            }
        }
        Some(TransactionSpec::new().changes(changes))
    } else {
        None
    }
}

struct LineInfo {
    line: Line,
    token: String,
    comment: Option<usize>,
    empty: bool,
    indent: usize,
    single: bool,
}

// Performs toggle, comment and uncomment of line comments.
fn change_line_comment(
    option: CommentOption,
    state: &EditorState,
    ranges: &[(usize, usize)],
) -> Option<TransactionSpec> {
    let doc = state.doc();
    let mut lines: Vec<LineInfo> = Vec::new();
    let mut prev_line: Option<usize> = None;
    'ranges: for &(from, to) in ranges {
        let start_i = lines.len();
        let mut min_indent = usize::MAX;
        let mut token: Option<String> = None;
        let mut pos = from;
        while pos <= to {
            let line = doc.line_at(pos);
            let tok = match &token {
                Some(t) => t.clone(),
                None => match get_config(state, line.from).line {
                    Some(t) => {
                        token = Some(t.clone());
                        t
                    }
                    None => continue 'ranges,
                },
            };
            // CodeMirror starts `prevLine` at -1.
            if prev_line.is_none_or(|p| line.from > p) && (from == to || to > line.from) {
                prev_line = Some(line.from);
                let indent = leading_space(&line.text);
                let empty = indent == line.length();
                let comment = if line.text[indent..].starts_with(tok.as_str()) { Some(indent) } else { None };
                if indent < line.text.len() && indent < min_indent {
                    min_indent = indent;
                }
                let next = line.to + 1;
                lines.push(LineInfo { line, comment, token: tok, indent, empty, single: false });
                pos = next;
            } else {
                pos = line.to + 1;
            }
        }
        if min_indent < usize::MAX {
            for l in &mut lines[start_i..] {
                if l.indent < l.line.text.len() {
                    l.indent = min_indent;
                }
            }
        }
        if lines.len() == start_i + 1 {
            lines[start_i].single = true;
        }
    }

    if option != CommentOption::Uncomment && lines.iter().any(|l| l.comment.is_none() && (!l.empty || l.single)) {
        let mut changes = Vec::new();
        for l in &lines {
            if l.single || !l.empty {
                changes.push(ChangeSpec::insert(l.line.from + l.indent, format!("{} ", l.token)));
            }
        }
        let change_set = state.changes(changes);
        let selection = state.selection().map(change_set.desc(), 1);
        Some(TransactionSpec::new().changes(change_set).selection(selection))
    } else if option != CommentOption::Comment && lines.iter().any(|l| l.comment.is_some()) {
        let mut changes = Vec::new();
        for l in &lines {
            if let Some(comment) = l.comment {
                let from = l.line.from + comment;
                let mut to = from + l.token.len();
                if l.line.text.as_bytes().get(to - l.line.from) == Some(&b' ') {
                    to += 1;
                }
                changes.push(ChangeSpec::delete(from, to));
            }
        }
        Some(TransactionSpec::new().changes(changes))
    } else {
        None
    }
}
