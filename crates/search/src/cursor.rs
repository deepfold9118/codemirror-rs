//! Port of `cursor.ts`: plain-text search over a document.

use std::sync::Arc;

use codemirror_state::Text;
use unicode_normalization::UnicodeNormalization;

/// A function applied to the query and to each character of the text
/// before comparing them.
pub type Normalize = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// A filter on matches, given the match's `from` and `to`.
pub type CursorTest<'a> = Box<dyn Fn(usize, usize) -> bool + 'a>;

/// A match found by a [`SearchCursor`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub from: usize,
    pub to: usize,
    /// False when the match starts or ends _inside_ a character that,
    /// when normalized, expands to multiple characters. The `from`-`to`
    /// range then covers content that isn't part of the actual match.
    pub precise: bool,
}

struct Partial {
    from: usize,
    index: usize,
    precise: bool,
}

/// A search cursor iterates over the matches of a string in a document.
///
/// Where CodeMirror's cursor has `next()`, `value` and `done`, this one is
/// a Rust [`Iterator`]: `next()` returns the next match or `None`.
pub struct SearchCursor<'a> {
    chunks: Box<dyn Iterator<Item = &'a str> + 'a>,
    buffer: &'a str,
    buffer_pos: usize,
    buffer_start: usize,
    normalize: Option<Normalize>,
    query: Vec<char>,
    matches: Vec<Partial>,
    test: Option<CursorTest<'a>>,
}

/// NFKD, which CodeMirror applies to all text before comparing.
fn basic_normalize(s: &str) -> String {
    if s.is_ascii() {
        s.to_string()
    } else {
        s.nfkd().collect()
    }
}

impl<'a> SearchCursor<'a> {
    /// Create a text cursor. The query is the search string, `from` to
    /// `to` provides the region to search.
    ///
    /// When `normalize` is given, it is called, on both the query string
    /// and the content it is matched against, before comparing. For
    /// example, a case-insensitive search passes a lowercasing function.
    /// Text is always normalized with NFKD first.
    pub fn new(
        text: &'a Text,
        query: &str,
        from: usize,
        to: Option<usize>,
        normalize: Option<Normalize>,
        test: Option<CursorTest<'a>>,
    ) -> SearchCursor<'a> {
        let to = to.unwrap_or(text.len());
        let mut cursor = SearchCursor {
            chunks: Box::new(text.iter_range(from, to)),
            buffer: "",
            buffer_pos: 0,
            buffer_start: from.min(text.len()),
            normalize,
            query: Vec::new(),
            matches: Vec::new(),
            test,
        };
        cursor.query = cursor.normalize(query).chars().collect();
        cursor
    }

    fn normalize(&self, s: &str) -> String {
        let s = basic_normalize(s);
        match &self.normalize {
            Some(f) => f(&s),
            None => s,
        }
    }

    fn peek(&mut self) -> Option<char> {
        while self.buffer_pos == self.buffer.len() {
            self.buffer_start += self.buffer.len();
            self.buffer = self.chunks.next()?;
            self.buffer_pos = 0;
        }
        self.buffer[self.buffer_pos..].chars().next()
    }

    /// Like [`next`](Iterator::next), but also returns matches that
    /// partially overlap a previous match.
    pub fn next_overlapping(&mut self) -> Option<SearchMatch> {
        loop {
            let ch = self.peek()?;
            let start = self.buffer_start + self.buffer_pos;
            self.buffer_pos += ch.len_utf8();
            let end = self.buffer_start + self.buffer_pos;
            let mut tmp = [0u8; 4];
            let norm: Vec<char> = self.normalize(ch.encode_utf8(&mut tmp)).chars().collect();
            let (mut pos, mut pos_precise) = (start, true);
            for (i, &code) in norm.iter().enumerate() {
                let last = i == norm.len() - 1;
                if let Some(m) = self.match_char(code, pos, pos_precise, end, last) {
                    return Some(m);
                }
                if last {
                    break;
                }
                if pos_precise && i == 0 && ch == code {
                    pos = end;
                } else {
                    pos_precise = false;
                }
            }
        }
    }

    fn match_char(&mut self, code: char, pos: usize, pos_precise: bool, end: usize, end_precise: bool) -> Option<SearchMatch> {
        let mut found = None;
        let mut i = 0;
        while i < self.matches.len() {
            let partial = &mut self.matches[i];
            let mut keep = false;
            if self.query[partial.index] == code {
                if partial.index == self.query.len() - 1 {
                    found = Some(SearchMatch { from: partial.from, to: end, precise: end_precise && partial.precise });
                } else {
                    partial.index += 1;
                    keep = true;
                }
            }
            if keep {
                i += 1;
            } else {
                self.matches.remove(i);
            }
        }
        if self.query.first() == Some(&code) {
            if self.query.len() == 1 {
                found = Some(SearchMatch { from: pos, to: end, precise: pos_precise && end_precise });
            } else {
                self.matches.push(Partial { from: pos, index: 1, precise: pos_precise });
            }
        }
        match (found, &self.test) {
            (Some(m), Some(test)) if !test(m.from, m.to) => None,
            _ => found,
        }
    }
}

impl Iterator for SearchCursor<'_> {
    type Item = SearchMatch;

    /// Look for the next match. Matches that partially overlap a previous
    /// match are skipped.
    fn next(&mut self) -> Option<SearchMatch> {
        self.matches.clear();
        self.next_overlapping()
    }
}
