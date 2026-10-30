//! A Rust port of [`@codemirror/commands`](https://codemirror.net/docs/ref/#commands):
//! the undo history, comment toggling, and editing commands.
//!
//! Every command here is a [`StateCommand`](codemirror_state::StateCommand):
//! it takes a state and a dispatch function and returns whether it applied.
//! Commands that need layout (moving by visual line, page or line wrap, bidi
//! text direction, subwords via the view's `moveByChar`) and the key
//! bindings arrive with the view crate.
//!
//! Ported from CodeMirror, copyright (C) 2018-2021 by Marijn Haverbeke
//! <marijn@haverbeke.berlin> and others, MIT license.

#![allow(clippy::type_complexity)]

mod commands;
mod comment;
mod history;
mod view_commands;

pub use commands::*;
pub use comment::{
    block_comment, block_uncomment, line_comment, line_uncomment, toggle_block_comment, toggle_block_comment_by_line,
    toggle_comment, toggle_line_comment, BlockCommentTokens, CommentTokens,
};
pub use history::{
    can_redo_selection, can_undo_selection, history, redo, redo_depth, redo_selection, undo, undo_depth,
    undo_selection, HistoryConfig, HistoryState, InvertedEffects, IsolateHistory, HISTORY_FIELD, INVERTED_EFFECTS,
    ISOLATE_HISTORY,
};
pub use view_commands::*;

/// JavaScript's `\s`: Unicode white space plus the byte order mark.
pub(crate) fn is_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// The byte length of the white space at the start of `s` (`/^\s*/`).
pub(crate) fn leading_space(s: &str) -> usize {
    s.find(|c: char| !is_space(c)).unwrap_or(s.len())
}

/// The byte length of the white space at the end of `s` (`/\s*$/`).
pub(crate) fn trailing_space(s: &str) -> usize {
    s.len() - s.trim_end_matches(is_space).len()
}

/// Whether `event` is `prefix` or a more specific event under it.
pub(crate) fn event_matches(event: &str, prefix: &str) -> bool {
    event == prefix
        || (event.len() > prefix.len() && event.starts_with(prefix) && event.as_bytes()[prefix.len()] == b'.')
}
