//! Grapheme clusters and columns. Port of `char.ts` and `column.ts`.
//!
//! CodeMirror's `codePointAt`, `fromCodePoint` and `codePointSize` exist to
//! work around UTF-16 strings in JavaScript; Rust's `char` covers them, so
//! they are not ported.

use unicode_segmentation::GraphemeCursor;

/// Returns the next grapheme cluster break _after_ (not equal to) `pos` if
/// `forward` is true, or before it otherwise. Returns `pos` itself if there
/// is no further cluster break in the string. `pos` is a byte offset on a
/// char boundary.
///
/// When `include_extending` is false, this moves by single code points
/// (CodeMirror still joins zero-width-joiner sequences and flags in that
/// mode; this port does not).
pub fn find_cluster_break(s: &str, pos: usize, forward: bool, include_extending: bool) -> usize {
    if !include_extending {
        return if forward {
            s[pos..].chars().next().map_or(pos, |c| pos + c.len_utf8())
        } else {
            s[..pos].chars().next_back().map_or(pos, |c| pos - c.len_utf8())
        };
    }
    let mut cursor = GraphemeCursor::new(pos, s.len(), true);
    let next = if forward { cursor.next_boundary(s, 0) } else { cursor.prev_boundary(s, 0) };
    match next {
        Ok(Some(p)) => p,
        _ => pos,
    }
}

/// Count the column position at the given byte offset into the string,
/// taking extending characters and tab size into account.
pub fn count_column(s: &str, tab_size: usize, to: Option<usize>) -> usize {
    let to = to.unwrap_or(s.len()).min(s.len());
    let mut n = 0;
    let mut i = 0;
    while i < to {
        if s.as_bytes()[i] == b'\t' {
            n += tab_size - (n % tab_size);
            i += 1;
        } else {
            n += 1;
            i = find_cluster_break(s, i, true, true);
        }
    }
    n
}

/// Find the byte offset that corresponds to the given column position in a
/// string, taking extending characters and tab size into account. When the
/// string is too short to reach the column, this returns the string length,
/// or `None` when `strict` is true.
pub fn find_column(s: &str, col: usize, tab_size: usize, strict: bool) -> Option<usize> {
    let mut i = 0;
    let mut n = 0;
    loop {
        if n >= col {
            return Some(i);
        }
        if i == s.len() {
            break;
        }
        n += if s.as_bytes()[i] == b'\t' { tab_size - (n % tab_size) } else { 1 };
        i = find_cluster_break(s, i, true, true);
    }
    if strict {
        None
    } else {
        Some(s.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_over_clusters() {
        let s = "ae\u{301}b👩‍👩‍👧x";
        assert_eq!(find_cluster_break(s, 1, true, true), 4);
        assert_eq!(find_cluster_break(s, 1, true, false), 2);
        assert_eq!(find_cluster_break(s, 4, false, true), 1);
        let family_end = s.len() - 1;
        assert_eq!(find_cluster_break(s, 5, true, true), family_end);
        assert_eq!(find_cluster_break(s, family_end, false, true), 5);
        assert_eq!(find_cluster_break(s, s.len(), true, true), s.len());
        assert_eq!(find_cluster_break(s, 0, false, true), 0);
    }

    #[test]
    fn counts_columns() {
        assert_eq!(count_column("\tab", 4, None), 6);
        assert_eq!(count_column("a\tb", 4, Some(2)), 4);
        assert_eq!(count_column("e\u{301}x", 4, None), 2);
        assert_eq!(find_column("\tab", 5, 4, false), Some(2));
        assert_eq!(find_column("ab", 5, 4, false), Some(2));
        assert_eq!(find_column("ab", 5, 4, true), None);
    }
}
