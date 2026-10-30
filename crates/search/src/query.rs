//! The search query (`SearchQuery` and the `QueryType` classes in
//! `search.ts`).

use std::sync::Arc;

use codemirror_state::{find_cluster_break, CharCategory, EditorState, EditorStateConfig, Text};

use crate::cursor::{CursorTest, Normalize, SearchCursor};
use crate::regexp::{valid_regexp, RegExpCursor, RegExpCursorOptions, RegExpMatch, RegExpTest};

/// A custom filter for a query: given the matched string, the state and
/// the match range, returns whether to keep the match.
pub type QueryTest = Arc<dyn Fn(&str, &EditorState, usize, usize) -> bool + Send + Sync>;

/// The options for [`SearchQuery::new`].
#[derive(Clone, Default)]
pub struct SearchQueryConfig {
    /// The search string (or regular expression).
    pub search: String,
    /// Whether the search is case-sensitive.
    pub case_sensitive: bool,
    /// By default, string search replaces `\n`, `\r`, and `\t` in the
    /// query with newline, return, and tab characters. When this is true,
    /// that behavior is disabled.
    pub literal: bool,
    /// Interpret the search string as a regular expression.
    pub regexp: bool,
    /// The replace text.
    pub replace: String,
    /// Enable whole-word matching.
    pub whole_word: bool,
    /// An optional filter on matches.
    pub test: Option<QueryTest>,
}

/// A search query. Part of the editor's search state.
#[derive(Clone)]
pub struct SearchQuery {
    /// The search string (or regular expression).
    pub search: String,
    /// Whether the search is case-sensitive.
    pub case_sensitive: bool,
    /// Whether `\n`, `\r` and `\t` in a string query are taken literally.
    pub literal: bool,
    /// Whether the search string is a regular expression.
    pub regexp: bool,
    /// The replace text, or the empty string if none was given.
    pub replace: String,
    /// Whether this query is non-empty and, for a regular expression,
    /// syntactically valid.
    pub valid: bool,
    /// When true, matches that contain words are ignored when there are
    /// further word characters around them.
    pub whole_word: bool,
    /// The optional match filter.
    pub test: Option<QueryTest>,
    pub(crate) unquoted: String,
}

impl PartialEq for SearchQuery {
    /// Compare two queries (CodeMirror's `eq`).
    fn eq(&self, other: &SearchQuery) -> bool {
        self.search == other.search
            && self.replace == other.replace
            && self.case_sensitive == other.case_sensitive
            && self.regexp == other.regexp
            && self.whole_word == other.whole_word
            && match (&self.test, &other.test) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }
}

impl std::fmt::Debug for SearchQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchQuery")
            .field("search", &self.search)
            .field("case_sensitive", &self.case_sensitive)
            .field("literal", &self.literal)
            .field("regexp", &self.regexp)
            .field("replace", &self.replace)
            .field("whole_word", &self.whole_word)
            .finish()
    }
}

/// A match of a query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryMatch {
    pub from: usize,
    pub to: usize,
    /// See [`SearchMatch::precise`](crate::SearchMatch::precise). Always
    /// true for regular expressions.
    pub precise: bool,
    /// For a regular expression, the matched text and the capture groups.
    pub groups: Vec<Option<String>>,
}

/// An iterator over the matches of a query; see [`SearchQuery::get_cursor`].
pub enum QueryCursor<'a> {
    String(SearchCursor<'a>),
    RegExp(RegExpCursor<'a>),
    /// An invalid regular expression has no matches.
    Empty,
}

impl Iterator for QueryCursor<'_> {
    type Item = QueryMatch;

    fn next(&mut self) -> Option<QueryMatch> {
        match self {
            QueryCursor::String(c) => {
                c.next().map(|m| QueryMatch { from: m.from, to: m.to, precise: m.precise, groups: Vec::new() })
            }
            QueryCursor::RegExp(c) => c.next().map(regexp_match),
            QueryCursor::Empty => None,
        }
    }
}

fn regexp_match(m: RegExpMatch) -> QueryMatch {
    QueryMatch { from: m.from, to: m.to, precise: true, groups: m.groups }
}

/// The chunk size for searching backwards (by searching chunks forwards).
const FIND_PREV_CHUNK: usize = 10000;
/// How far around the visible range regexp matches are looked for.
pub(crate) const HIGHLIGHT_MARGIN: usize = 250;

impl SearchQuery {
    /// Create a query object.
    pub fn new(config: SearchQueryConfig) -> SearchQuery {
        let valid = !config.search.is_empty() && (!config.regexp || valid_regexp(&config.search));
        let mut query = SearchQuery {
            search: config.search,
            case_sensitive: config.case_sensitive,
            literal: config.literal,
            regexp: config.regexp,
            replace: config.replace,
            valid,
            whole_word: config.whole_word,
            test: config.test,
            unquoted: String::new(),
        };
        query.unquoted = query.unquote(&query.search);
        query
    }

    /// The query as a [`SearchQueryConfig`], to make a changed copy.
    pub fn config(&self) -> SearchQueryConfig {
        SearchQueryConfig {
            search: self.search.clone(),
            case_sensitive: self.case_sensitive,
            literal: self.literal,
            regexp: self.regexp,
            replace: self.replace.clone(),
            whole_word: self.whole_word,
            test: self.test.clone(),
        }
    }

    pub(crate) fn unquote(&self, text: &str) -> String {
        if self.literal {
            return text.to_string();
        }
        let mut out = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.peek() {
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('\\') => out.push('\\'),
                    _ => {
                        out.push(c);
                        continue;
                    }
                }
                chars.next();
            } else {
                out.push(c);
            }
        }
        out
    }

    /// Get a cursor over the query's matches in `from` to `to` (the end
    /// of the document when `None`) in `state`.
    pub fn get_cursor<'a>(&'a self, state: &'a EditorState, from: usize, to: Option<usize>) -> QueryCursor<'a> {
        let to = to.unwrap_or(state.doc().len());
        if self.regexp {
            regexp_cursor(self, state, from, to)
        } else {
            QueryCursor::String(string_cursor(self, state, from, to))
        }
    }

    /// Search a bare document, as CodeMirror's `getCursor` does when given
    /// a `Text`. Returns the match ranges.
    pub fn matches_in(&self, doc: &Text) -> Vec<(usize, usize)> {
        let state = EditorState::create(EditorStateConfig::new().doc(doc.clone()));
        self.get_cursor(&state, 0, None).map(|m| (m.from, m.to)).collect()
    }

    /// The first match after `cur_to`, wrapping around to the start of
    /// the document. Doesn't return the `cur_from`-`cur_to` range itself.
    pub(crate) fn next_match(&self, state: &EditorState, cur_from: usize, cur_to: usize) -> Option<QueryMatch> {
        let len = state.doc().len();
        if self.regexp {
            let mut found = regexp_cursor(self, state, cur_to, len).next();
            if found.is_none() {
                found = regexp_cursor(self, state, 0, cur_from).next();
            }
            return found;
        }
        let mut found = string_cursor(self, state, cur_to, len).next_overlapping();
        if found.is_none() {
            let end = state.doc().ceil_char_boundary(cur_from + self.unquoted.len());
            found = string_cursor(self, state, 0, end).next_overlapping();
        }
        found
            .filter(|m| !(m.from == cur_from && m.to == cur_to))
            .map(|m| QueryMatch { from: m.from, to: m.to, precise: m.precise, groups: Vec::new() })
    }

    fn prev_match_in_range(&self, state: &EditorState, from: usize, to: usize) -> Option<QueryMatch> {
        let doc = state.doc();
        if self.regexp {
            let mut size = 1;
            loop {
                let start = doc.floor_char_boundary(from.max(to.saturating_sub(size * FIND_PREV_CHUNK)));
                let range = regexp_cursor(self, state, start, to).last();
                if let Some(range) = range {
                    if start == from || range.from > start + 10 {
                        return Some(range);
                    }
                }
                if start <= from {
                    return None;
                }
                size += 1;
            }
        }
        let mut pos = to;
        loop {
            let start =
                doc.floor_char_boundary(from.max(pos.saturating_sub(FIND_PREV_CHUNK + self.unquoted.len())));
            let mut cursor = string_cursor(self, state, start, pos);
            let mut range = None;
            while let Some(m) = cursor.next_overlapping() {
                range = Some(m);
            }
            if let Some(m) = range {
                return Some(QueryMatch { from: m.from, to: m.to, precise: m.precise, groups: Vec::new() });
            }
            if start <= from {
                return None;
            }
            pos = doc.floor_char_boundary(pos.saturating_sub(FIND_PREV_CHUNK));
        }
    }

    /// The last match before `cur_from`, wrapping around to the end of the
    /// document. Doesn't return the `cur_from`-`cur_to` range itself.
    pub(crate) fn prev_match(&self, state: &EditorState, cur_from: usize, cur_to: usize) -> Option<QueryMatch> {
        let len = state.doc().len();
        if self.regexp {
            return self
                .prev_match_in_range(state, 0, cur_from)
                .or_else(|| self.prev_match_in_range(state, cur_to, len));
        }
        let found = self.prev_match_in_range(state, 0, cur_from).or_else(|| {
            let from = state.doc().floor_char_boundary(cur_to.saturating_sub(self.unquoted.len()));
            self.prev_match_in_range(state, from, len)
        });
        found.filter(|m| m.from != cur_from || m.to != cur_to)
    }

    /// The text that replaces `m`: the replace string, with `$&`, `$1`
    /// and so on expanded for regular expressions.
    pub fn get_replacement(&self, m: &QueryMatch) -> String {
        let replace = self.unquote(&self.replace);
        if !self.regexp {
            return replace;
        }
        let mut out = String::with_capacity(replace.len());
        let mut rest = replace.as_str();
        while let Some(i) = rest.find('$') {
            out.push_str(&rest[..i]);
            let after = &rest[i + 1..];
            if let Some(stripped) = after.strip_prefix('&') {
                out.push_str(m.groups.first().and_then(|g| g.as_deref()).unwrap_or(""));
                rest = stripped;
            } else if let Some(stripped) = after.strip_prefix('$') {
                out.push('$');
                rest = stripped;
            } else {
                let digits = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
                // The longest prefix of the digits that names a group.
                let group = (1..=digits).rev().find_map(|l| {
                    let n: usize = after[..l].parse().ok()?;
                    (n > 0 && n < m.groups.len()).then_some((n, l))
                });
                match group {
                    Some((n, l)) => {
                        out.push_str(m.groups[n].as_deref().unwrap_or(""));
                        rest = &after[l..];
                    }
                    None => {
                        out.push('$');
                        rest = after;
                    }
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// All matches, or `None` when there are more than `limit`.
    pub(crate) fn match_all(&self, state: &EditorState, limit: usize) -> Option<Vec<QueryMatch>> {
        let mut ranges = Vec::new();
        for m in self.get_cursor(state, 0, None) {
            if ranges.len() >= limit {
                return None;
            }
            ranges.push(m);
        }
        Some(ranges)
    }

    /// Call `add` for the matches in and around `from`-`to`.
    pub(crate) fn highlight(&self, state: &EditorState, from: usize, to: usize, mut add: impl FnMut(usize, usize)) {
        let doc = state.doc();
        let margin = if self.regexp { HIGHLIGHT_MARGIN } else { self.unquoted.len() };
        let from = doc.floor_char_boundary(from.saturating_sub(margin));
        let to = doc.ceil_char_boundary(to + margin);
        for m in self.get_cursor(state, from, Some(to)) {
            add(m.from, m.to);
        }
    }
}

fn lowercase() -> Normalize {
    Arc::new(|s: &str| s.to_lowercase())
}

/// The grapheme cluster before `pos`, or `"\n"` at a line start.
pub(crate) fn char_before(doc: &Text, pos: usize) -> String {
    let line = doc.line_at(pos);
    if pos == line.from {
        return if pos == 0 { String::new() } else { "\n".into() };
    }
    let i = pos - line.from;
    line.text[find_cluster_break(&line.text, i, false, true)..i].to_string()
}

/// The grapheme cluster after `pos`, or `"\n"` at a line end.
pub(crate) fn char_after(doc: &Text, pos: usize) -> String {
    let line = doc.line_at(pos);
    if pos == line.to {
        return if pos == doc.len() { String::new() } else { "\n".into() };
    }
    let i = pos - line.from;
    line.text[i..find_cluster_break(&line.text, i, true, true)].to_string()
}

/// Whether `from`-`to` doesn't have word characters on both sides of
/// either end.
fn word_bounded(doc: &Text, cat: &dyn Fn(&str) -> CharCategory, from: usize, to: usize) -> bool {
    let word = |s: String| cat(&s) == CharCategory::Word;
    (!word(char_before(doc, from)) || !word(char_after(doc, from)))
        && (!word(char_after(doc, to)) || !word(char_before(doc, to)))
}

fn string_cursor<'a>(spec: &'a SearchQuery, state: &'a EditorState, from: usize, to: usize) -> SearchCursor<'a> {
    let doc = state.doc();
    let mut test: Option<CursorTest<'a>> = None;
    if spec.whole_word {
        let cat = state.char_categorizer(state.selection().main().head());
        test = Some(Box::new(move |from, to| word_bounded(doc, &cat, from, to)));
    }
    if let Some(query_test) = spec.test.clone() {
        let inner = test.take();
        test = Some(Box::new(move |from, to| {
            inner.as_ref().is_none_or(|t| t(from, to)) && query_test(&doc.slice_string(from, to, "\n"), state, from, to)
        }));
    }
    let normalize = if spec.case_sensitive { None } else { Some(lowercase()) };
    SearchCursor::new(doc, &spec.unquoted, from, Some(to), normalize, test)
}

fn regexp_cursor<'a>(spec: &'a SearchQuery, state: &'a EditorState, from: usize, to: usize) -> QueryCursor<'a> {
    let doc = state.doc();
    let mut test: Option<RegExpTest<'a>> = None;
    if spec.whole_word {
        let cat = state.char_categorizer(state.selection().main().head());
        test = Some(Box::new(move |from, to, _| from == to || word_bounded(doc, &cat, from, to)));
    }
    if let Some(query_test) = spec.test.clone() {
        let inner = test.take();
        test = Some(Box::new(move |from, to, m: &RegExpMatch| {
            inner.as_ref().is_none_or(|t| t(from, to, m))
                && query_test(m.groups.first().and_then(|g| g.as_deref()).unwrap_or(""), state, from, to)
        }));
    }
    let options = RegExpCursorOptions { ignore_case: !spec.case_sensitive, test };
    match RegExpCursor::new(doc, &spec.search, options, from, Some(to)) {
        Ok(cursor) => QueryCursor::RegExp(cursor),
        Err(_) => QueryCursor::Empty,
    }
}
