//! Bracket matching. Port of `matchBrackets` from `matchbrackets.ts`.
//!
//! Without a syntax tree, only CodeMirror's plain-text fallback applies:
//! brackets are matched character by character. The `bracketMatching`
//! highlighting extension belongs to the view and is not ported yet.

use codemirror_state::EditorState;

/// Configuration for [`match_brackets`].
#[derive(Clone, Debug)]
pub struct BracketConfig {
    /// The bracket characters to match, as a string of pairs. Defaults to
    /// `"()[]{}"`.
    pub brackets: String,
    /// The maximum distance, in bytes, to scan for matching brackets.
    /// Defaults to 10 000.
    pub max_scan_distance: usize,
}

impl Default for BracketConfig {
    fn default() -> Self {
        BracketConfig { brackets: "()[]{}".to_string(), max_scan_distance: 10000 }
    }
}

/// A document range covering a token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenRange {
    pub from: usize,
    pub to: usize,
}

/// The result returned from [`match_brackets`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchResult {
    /// The extent of the bracket token found.
    pub start: TokenRange,
    /// The extent of the matched token, if any was found.
    pub end: Option<TokenRange>,
    /// Whether the tokens match. This can be false even when `end` has a
    /// value, if that token doesn't match the opening token.
    pub matched: bool,
}

/// Find the matching bracket for the token at `pos`, scanning direction
/// `dir` (-1 or 1). Returns `None` if no bracket was found at `pos`, or a
/// match result otherwise.
pub fn match_brackets(state: &EditorState, pos: usize, dir: i32, config: &BracketConfig) -> Option<MatchResult> {
    let doc = state.doc();
    let brackets: Vec<char> = config.brackets.chars().collect();
    let rope = doc.rope();
    if if dir < 0 { pos == 0 } else { pos == doc.len() } {
        return None;
    }
    let start_ch =
        if dir < 0 { rope.byte_slice(..pos).chars().next_back() } else { rope.byte_slice(pos..).chars().next() }?;
    let bracket = brackets.iter().position(|&c| c == start_ch)?;
    if (bracket % 2 == 0) != (dir > 0) {
        return None;
    }
    let len = start_ch.len_utf8();
    let start = if dir < 0 { TokenRange { from: pos - len, to: pos } } else { TokenRange { from: pos, to: pos + len } };

    let mut depth = 0;
    let mut distance = 0;
    let mut scan = |c: char, at: usize| -> Option<MatchResult> {
        let found = brackets.iter().position(|&b| b == c)?;
        if (found % 2 == 0) == (dir > 0) {
            depth += 1;
        } else if depth == 1 {
            let end = TokenRange { from: at, to: at + c.len_utf8() };
            return Some(MatchResult { start, end: Some(end), matched: found >> 1 == bracket >> 1 });
        } else {
            depth -= 1;
        }
        None
    };
    if dir > 0 {
        let mut at = pos;
        for c in rope.byte_slice(pos..).chars() {
            if distance > config.max_scan_distance {
                return None;
            }
            if let Some(result) = scan(c, at) {
                return Some(result);
            }
            at += c.len_utf8();
            distance += c.len_utf8();
        }
    } else {
        let mut at = pos;
        for c in rope.byte_slice(..pos).chars().rev() {
            at -= c.len_utf8();
            distance += c.len_utf8();
            if distance > config.max_scan_distance {
                return None;
            }
            if let Some(result) = scan(c, at) {
                return Some(result);
            }
        }
    }
    Some(MatchResult { start, end: None, matched: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use codemirror_state::EditorStateConfig;

    fn m(doc: &str, pos: usize, dir: i32) -> Option<MatchResult> {
        let state = EditorState::create(EditorStateConfig::new().doc(doc));
        match_brackets(&state, pos, dir, &BracketConfig::default())
    }

    #[test]
    fn matches_plain_brackets() {
        let r = m("a(b[c]d)e", 1, 1).unwrap();
        assert_eq!((r.start.from, r.end.unwrap().from, r.matched), (1, 7, true));
        let r = m("a(b[c]d)e", 8, -1).unwrap();
        assert_eq!((r.start.from, r.end.unwrap().from, r.matched), (7, 1, true));
        let r = m("(é]", 0, 1).unwrap();
        assert_eq!((r.end.unwrap().from, r.matched), (3, false));
        assert_eq!(m("(abc", 0, 1).unwrap().end, None);
        assert_eq!(m("abc", 1, 1), None);
        assert_eq!(m(")", 0, 1), None);
    }
}
