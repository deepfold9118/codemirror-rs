//! A Rust port of [`@codemirror/language`](https://codemirror.net/docs/ref/#language).
//!
//! Only the parts that work without a syntax tree are ported so far:
//! indentation helpers ([`INDENT_UNIT`], [`INDENT_SERVICE`],
//! [`IndentContext`], [`get_indentation`]) and plain-text bracket matching
//! ([`match_brackets`]). Until a parser is chosen, these behave the way
//! CodeMirror behaves when no language is loaded (an empty syntax tree).
//!
//! Ported from CodeMirror, copyright (C) 2018-2021 by Marijn Haverbeke
//! <marijn@haverbeke.berlin> and others, MIT license.

#![allow(clippy::type_complexity)]

mod indent;
mod matchbrackets;

pub use indent::{
    get_indent_unit, get_indentation, indent_range, indent_string, IndentContext, IndentOptions, IndentService,
    INDENT_SERVICE, INDENT_UNIT,
};
pub use matchbrackets::{match_brackets, BracketConfig, MatchResult, TokenRange};

/// JavaScript's `\s`: Unicode white space plus the byte order mark.
pub(crate) fn is_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// The byte length of the white space at the start of `s` (`/^\s*/`).
pub(crate) fn leading_space(s: &str) -> usize {
    s.find(|c: char| !is_space(c)).unwrap_or(s.len())
}
