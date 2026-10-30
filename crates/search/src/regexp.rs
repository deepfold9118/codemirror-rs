//! Port of `regexp.ts`: regular expression search over a document.
//!
//! CodeMirror uses JavaScript regular expressions; this port uses
//! [`fancy_regex`], which also supports look-around and backreferences.
//! Patterns are compiled in multi-line mode, so `^` and `$` match at line
//! boundaries, and JavaScript's `[^]` (any character) is accepted.

use codemirror_state::Text;
use fancy_regex::{Captures, Regex, RegexBuilder};

/// A match found by a [`RegExpCursor`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegExpMatch {
    pub from: usize,
    pub to: usize,
    /// The matched text (index 0) and the capture groups.
    pub groups: Vec<Option<String>>,
}

/// A filter on matches.
pub type RegExpTest<'a> = Box<dyn Fn(usize, usize, &RegExpMatch) -> bool + 'a>;

/// Options for [`RegExpCursor::new`].
#[derive(Default)]
pub struct RegExpCursorOptions<'a> {
    pub ignore_case: bool,
    pub test: Option<RegExpTest<'a>>,
}

enum Mode {
    /// Matches line by line.
    Line { cur_line: String, cur_line_start: usize },
    /// The pattern can match line breaks: match against a growing,
    /// flattened piece of the document.
    Multi { flat_from: usize, flat: String },
}

/// Like [`SearchCursor`](crate::SearchCursor), but searches for a regular
/// expression pattern instead of a plain string. Iterate it to get the
/// matches.
pub struct RegExpCursor<'a> {
    text: &'a Text,
    re: Regex,
    test: Option<RegExpTest<'a>>,
    to: usize,
    match_pos: usize,
    /// The end of the last match returned.
    last_to: Option<usize>,
    mode: Mode,
}

/// Translate the JavaScript-only syntax that CodeMirror users write.
fn translate(source: &str) -> String {
    source.replace("[^]", r"[\s\S]")
}

fn compile(source: &str, ignore_case: bool) -> Result<Regex, Box<fancy_regex::Error>> {
    let flags = if ignore_case { "(?mi)" } else { "(?m)" };
    RegexBuilder::new(&format!("{flags}{}", translate(source))).build().map_err(Box::new)
}

/// Whether `source` is a valid pattern.
pub fn valid_regexp(source: &str) -> bool {
    compile(source, false).is_ok()
}

/// Whether the pattern may match line breaks, which needs the multi-line
/// cursor (the same test as CodeMirror's).
fn is_multiline(source: &str) -> bool {
    source.contains('\n')
        || source.contains('\r')
        || source.contains("[^")
        || ["\\s", "\\W", "\\D", "\\n", "\\r"].iter().any(|p| source.contains(p))
}

/// The position after the character at `pos` (one past `pos` at the end
/// of the document).
fn next_char_pos(text: &Text, pos: usize) -> usize {
    if pos >= text.len() {
        pos + 1
    } else {
        text.ceil_char_boundary(pos + 1)
    }
}

fn to_match(from_base: usize, caps: &Captures) -> RegExpMatch {
    let whole = caps.get(0).expect("group 0");
    RegExpMatch {
        from: from_base + whole.start(),
        to: from_base + whole.end(),
        groups: caps.iter().map(|g| g.map(|g| g.as_str().to_string())).collect(),
    }
}

/// Find a match in `s` at or after byte `pos`.
fn exec<'t>(re: &Regex, s: &'t str, pos: usize) -> Option<Captures<'t>> {
    if pos > s.len() {
        return None;
    }
    re.captures_from_pos(s, pos).ok().flatten()
}

const CHUNK_BASE: usize = 5000;

impl<'a> RegExpCursor<'a> {
    /// Create a cursor that searches `from` to `to` (the end of the
    /// document when `None`) in `text`. `query` is the raw pattern.
    /// Returns an error when the pattern is invalid.
    pub fn new(
        text: &'a Text,
        query: &str,
        options: RegExpCursorOptions<'a>,
        from: usize,
        to: Option<usize>,
    ) -> Result<RegExpCursor<'a>, Box<fancy_regex::Error>> {
        let to = to.unwrap_or(text.len()).min(text.len());
        let re = compile(query, options.ignore_case)?;
        let from = text.floor_char_boundary(from);
        let mode = if is_multiline(query) {
            Mode::Multi { flat_from: from, flat: String::new() }
        } else {
            Mode::Line { cur_line: String::new(), cur_line_start: text.line_at(from).from }
        };
        let mut cursor =
            RegExpCursor { text, re, test: options.test, to, match_pos: from, last_to: None, mode };
        match &mut cursor.mode {
            Mode::Multi { flat_from, flat } => {
                *flat = text.slice_string(*flat_from, chunk_end(text, to, from + CHUNK_BASE), "\n");
            }
            Mode::Line { cur_line_start, .. } => {
                let start = *cur_line_start;
                cursor.get_line(start);
            }
        }
        Ok(cursor)
    }

    fn get_line(&mut self, start: usize) {
        let line = self.text.line_at(start);
        let mut text = line.text;
        if start + text.len() > self.to {
            text.truncate(self.text.floor_char_boundary(self.to.max(start)) - start);
        }
        self.mode = Mode::Line { cur_line: text, cur_line_start: start };
    }

    fn next_line(&mut self) {
        let Mode::Line { cur_line, cur_line_start } = &mut self.mode else { return };
        let start = *cur_line_start + cur_line.len() + 1;
        if start > self.to || start > self.text.len() {
            *cur_line_start = start;
            cur_line.clear();
        } else {
            self.get_line(start);
        }
    }

    fn accept(&self, m: &RegExpMatch) -> bool {
        self.test.as_ref().is_none_or(|test| test(m.from, m.to, m))
    }

    fn next_by_line(&mut self) -> Option<RegExpMatch> {
        let Mode::Line { cur_line_start, .. } = &self.mode else { unreachable!() };
        let mut off = self.match_pos - cur_line_start;
        loop {
            let Mode::Line { cur_line, cur_line_start } = &self.mode else { unreachable!() };
            let (line_start, line_len) = (*cur_line_start, cur_line.len());
            let found = if self.match_pos <= self.to {
                exec(&self.re, cur_line, off).map(|caps| to_match(line_start, &caps))
            } else {
                None
            };
            if let Some(m) = found {
                self.match_pos = if m.from == m.to { next_char_pos(self.text, m.to) } else { m.to };
                if m.from == line_start + line_len {
                    self.next_line();
                }
                if (m.from < m.to || self.last_to.is_none_or(|t| m.from > t)) && self.accept(&m) {
                    self.last_to = Some(m.to);
                    return Some(m);
                }
                let Mode::Line { cur_line_start, .. } = &self.mode else { unreachable!() };
                off = self.match_pos.saturating_sub(*cur_line_start);
            } else if line_start + line_len < self.to {
                self.next_line();
                off = 0;
            } else {
                return None;
            }
        }
    }

    fn next_multi(&mut self) -> Option<RegExpMatch> {
        loop {
            let Mode::Multi { flat_from, flat } = &self.mode else { unreachable!() };
            let off = self.match_pos - flat_from;
            let mut caps = exec(&self.re, flat, off);
            // Skip empty matches directly after the last match.
            if caps.as_ref().is_some_and(|c| c.get(0).is_some_and(|g| g.start() == off && g.as_str().is_empty())) {
                caps = flat[off..].chars().next().and_then(|c| exec(&self.re, flat, off + c.len_utf8()));
            }
            let flat_to = flat_from + flat.len();
            if let Some(caps) = caps {
                let m = to_match(*flat_from, &caps);
                let near_end = m.to - flat_from > flat.len().saturating_sub(10);
                if (flat_to >= self.to || !near_end) && self.accept(&m) {
                    self.match_pos = if m.from == m.to { next_char_pos(self.text, m.to) } else { m.to };
                    return Some(m);
                }
            }
            if flat_to == self.to {
                return None;
            }
            // Grow the flattened document.
            let (from, len) = (*flat_from, flat.len());
            let mut end = chunk_end(self.text, self.to, from + len * 2);
            if end <= from + len {
                // An empty line at the end of the chunk: take the next line.
                end = chunk_end(self.text, self.to, from + len + 1);
            }
            self.mode = Mode::Multi { flat_from: from, flat: self.text.slice_string(from, end, "\n") };
        }
    }
}

fn chunk_end(text: &Text, to: usize, pos: usize) -> usize {
    if pos >= to {
        to
    } else {
        text.line_at(text.floor_char_boundary(pos)).to
    }
}

impl Iterator for RegExpCursor<'_> {
    type Item = RegExpMatch;

    /// Move to the next match, if there is one.
    fn next(&mut self) -> Option<RegExpMatch> {
        match self.mode {
            Mode::Line { .. } => self.next_by_line(),
            Mode::Multi { .. } => self.next_multi(),
        }
    }
}
