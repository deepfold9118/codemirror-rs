//! Port of `test/test-query.ts`.

use std::sync::Arc;

use codemirror_search::{QueryTest, SearchQuery, SearchQueryConfig};
use codemirror_state::{EditorState, Text};

/// `doc` marks the expected matches with `[` and `]`.
fn test(query: SearchQuery, doc: &str) {
    let mut doc = doc.to_string();
    let mut expected = Vec::new();
    while let Some(start) = doc.find('[') {
        let end = start + doc[start..].find(']').expect("closing ]");
        expected.push((start, end - 1));
        doc = format!("{}{}{}", &doc[..start], &doc[start + 1..end], &doc[end + 1..]);
    }
    assert_eq!(query.matches_in(&Text::from(doc.as_str())), expected);
}

fn query(search: &str) -> SearchQueryConfig {
    SearchQueryConfig { search: search.into(), ..Default::default() }
}

#[test]
fn can_match_plain_strings() {
    test(SearchQuery::new(query("abc")), "[abc] flakdj a[abc] aabbcc");
}

#[test]
fn skips_overlapping_matches() {
    test(SearchQuery::new(query("aba")), "[aba]b[aba].");
}

#[test]
fn can_match_case_insensitive_strings() {
    test(SearchQuery::new(SearchQueryConfig { case_sensitive: false, ..query("abC") }), "[aBc] flakdj a[ABC]");
}

#[test]
fn can_match_across_lines() {
    test(SearchQuery::new(query("a\\nb")), "a [a\nb] b");
}

#[test]
fn can_match_across_multiple_lines() {
    test(SearchQuery::new(query("a\\nb\\nc\\nd")), "a [a\nb\nc\nd] e");
}

#[test]
fn can_match_literally() {
    test(SearchQuery::new(SearchQueryConfig { literal: true, ..query("a\\nb") }), "a\nb [a\\nb]");
}

#[test]
fn can_match_by_word() {
    test(
        SearchQuery::new(SearchQueryConfig { whole_word: true, ..query("hello") }),
        "[hello] hellothere [hello]\nello ahello ohellop",
    );
}

#[test]
fn doesnt_match_non_words_by_word() {
    test(SearchQuery::new(SearchQueryConfig { whole_word: true, ..query("^_^") }), "x[^_^]y [^_^]");
}

#[test]
fn can_match_regular_expressions() {
    test(SearchQuery::new(SearchQueryConfig { regexp: true, ..query("a..b") }), "[appb] apb");
}

#[test]
fn can_match_case_insensitive_regular_expressions() {
    test(SearchQuery::new(SearchQueryConfig { regexp: true, case_sensitive: false, ..query("a..b") }), "[Appb] Apb");
}

#[test]
fn can_match_regular_expressions_by_word() {
    test(
        SearchQuery::new(SearchQueryConfig { regexp: true, whole_word: true, ..query("a..") }),
        "[aap] baap aapje [a--]w",
    );
}

fn checked(f: impl Fn(&str, usize) -> bool + Send + Sync + 'static) -> QueryTest {
    Arc::new(move |m: &str, state: &EditorState, from, to| {
        assert_eq!(m, state.doc().slice_string(from, to, "\n"));
        f(m, from)
    })
}

#[test]
fn can_filter_string_matches_with_a_test_function() {
    test(
        SearchQuery::new(SearchQueryConfig { test: Some(checked(|_, from| from > 0)), ..query("ab") }),
        "ab a[ab] [ab]",
    );
}

#[test]
fn combines_whole_word_matches_with_a_test_function() {
    test(
        SearchQuery::new(SearchQueryConfig { whole_word: true, test: Some(checked(|_, from| from > 0)), ..query("ab") }),
        "ab xab [ab]",
    );
}

#[test]
fn can_filter_regexp_matches_with_a_test_function() {
    test(
        SearchQuery::new(SearchQueryConfig { regexp: true, test: Some(checked(|m, _| m != "aa")), ..query("a.") }),
        "[ab] aa [ac]",
    );
}

// Not in CodeMirror's suite: replacement expansion.

#[test]
fn expands_regexp_replacements() {
    let q = SearchQuery::new(SearchQueryConfig {
        regexp: true,
        replace: "<$2|$1|$&|$$|$3|$10>".into(),
        ..query("(a)(b)")
    });
    let state = EditorState::create(codemirror_state::EditorStateConfig::new().doc("xab"));
    let m = q.get_cursor(&state, 0, None).next().unwrap();
    assert_eq!(q.get_replacement(&m), "<b|a|ab|$|$3|a0>");
}

#[test]
fn reports_invalid_regexps() {
    assert!(!SearchQuery::new(SearchQueryConfig { regexp: true, ..query("a(") }).valid);
    assert!(!SearchQuery::new(query("")).valid);
    assert!(SearchQuery::new(SearchQueryConfig { regexp: true, ..query("a[^]") }).valid);
}
