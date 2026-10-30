//! Port of `test/test-cursor.ts`. Positions are byte offsets, so the
//! expected ranges differ from CodeMirror's UTF-16 ones for non-ASCII
//! text.

use std::sync::Arc;

use codemirror_search::{Normalize, RegExpCursor, RegExpCursorOptions, SearchCursor};
use codemirror_state::Text;

fn doc(lines: &[&str]) -> Text {
    Text::of(lines)
}

fn test_matches(found: Vec<(usize, usize, bool)>, expected: &[(usize, usize, bool)]) {
    assert_eq!(found, expected);
}

fn search(text: &Text, query: &str, from: usize, to: Option<usize>, normalize: Option<Normalize>) -> Vec<(usize, usize, bool)> {
    SearchCursor::new(text, query, from, to, normalize, None).map(|m| (m.from, m.to, m.precise)).collect()
}

fn regexp(text: &Text, query: &str, ignore_case: bool, from: usize, to: Option<usize>) -> Vec<(usize, usize, bool)> {
    let options = RegExpCursorOptions { ignore_case, test: None };
    RegExpCursor::new(text, query, options, from, to).unwrap().map(|m| (m.from, m.to, true)).collect()
}

#[test]
fn finds_all_matches_in_a_simple_string() {
    let t = doc(&["one two one two one"]);
    test_matches(search(&t, "one", 0, None, None), &[(0, 3, true), (8, 11, true), (16, 19, true)]);
}

#[test]
fn finds_only_matches_in_the_given_region() {
    let t = doc(&["one two one two one"]);
    test_matches(search(&t, "one", 2, Some(17), None), &[(8, 11, true)]);
}

#[test]
fn can_cross_lines() {
    let t = doc(&["one two", "one two", "one"]);
    test_matches(search(&t, "one", 0, None, None), &[(0, 3, true), (8, 11, true), (16, 19, true)]);
}

#[test]
fn can_normalize_case() {
    let t = doc(&["ONE two oNe two one"]);
    let lower: Normalize = Arc::new(|s: &str| s.to_lowercase());
    test_matches(search(&t, "one", 0, Some(19), Some(lower)), &[(0, 3, true), (8, 11, true), (16, 19, true)]);
}

#[test]
fn doesnt_get_confused_by_expanding_transforms() {
    let t = doc(&["Auf die Straße"]);
    let upper: Normalize = Arc::new(|s: &str| s.to_uppercase());
    test_matches(search(&t, "straße", 0, Some(15), Some(upper)), &[(8, 15, true)]);
}

#[test]
fn normalizes_composed_chars() {
    // The document's é is composed, the query's decomposed.
    test_matches(search(&doc(&["h\u{e9}\u{e9}"]), "he\u{301}e\u{301}", 0, None, None), &[(0, 5, true)]);
    // The document's é is decomposed, the query's composed.
    test_matches(search(&doc(&["he\u{301}e\u{301}"]), "h\u{e9}\u{e9}", 0, None, None), &[(0, 7, true)]);
}

#[test]
fn can_match_across_lines() {
    test_matches(search(&doc(&["one two", "three four"]), "two\nthree", 0, None, None), &[(4, 13, true)]);
}

#[test]
fn can_search_an_empty_document() {
    test_matches(search(&Text::empty(), "aaaa", 0, None, None), &[]);
}

#[test]
fn doesnt_include_overlapping_results() {
    test_matches(search(&doc(&["fofofofo"]), "fofo", 0, None, None), &[(0, 4, true), (4, 8, true)]);
}

#[test]
fn includes_overlapping_results_with_next_overlapping() {
    let t = doc(&["fofofofo"]);
    let mut cursor = SearchCursor::new(&t, "fofo", 0, None, None, None);
    let mut found = Vec::new();
    while let Some(m) = cursor.next_overlapping() {
        found.push((m.from, m.to));
    }
    assert_eq!(found, [(0, 4), (2, 6), (4, 8)]);
}

#[test]
fn marks_matched_partial_normalized_content_as_imprecise() {
    test_matches(search(&doc(&["\u{b4}"]), " ", 0, None, None), &[(0, 2, false)]);
}

#[test]
fn produces_the_correct_range_for_astral_chars_normalized_to_non_astral() {
    test_matches(search(&doc(&["\u{1d70e}"]), "\u{1d70e}", 0, None, None), &[(0, 4, true)]);
}

#[test]
fn can_handle_normalizers_that_remove_text() {
    let strip: Normalize = Arc::new(|s: &str| s.chars().filter(|c| !"aeuoi".contains(*c)).collect());
    test_matches(search(&doc(&["hello"]), "halal", 0, Some(5), Some(strip)), &[(0, 4, true)]);
}

// "㌢" (bytes 9 to 12) normalizes to "センチ".
#[test]
fn matches_full_compatibility_character_expansions() {
    test_matches(search(&doc(&["高さ180㌢の棚"]), "センチ", 0, None, None), &[(9, 12, true)]);
}

#[test]
fn matches_prefixes_inside_compatibility_character_expansions() {
    test_matches(search(&doc(&["高さ180㌢の棚"]), "セン", 0, None, None), &[(9, 12, false)]);
}

#[test]
fn matches_suffixes_inside_compatibility_character_expansions() {
    test_matches(search(&doc(&["高さ180㌢の棚"]), "ンチ", 0, None, None), &[(9, 12, false)]);
}

#[test]
fn matches_characters_completely_inside_compatibility_character_expansions() {
    test_matches(search(&doc(&["高さ180㌢の棚"]), "ン", 0, None, None), &[(9, 12, false)]);
}

#[test]
fn matches_across_compatibility_character_expansion_boundaries() {
    // "㌢㍍" (bytes 9 to 15) normalizes to "センチメートル".
    test_matches(search(&doc(&["高さ180㌢㍍の棚"]), "チメ", 0, None, None), &[(9, 15, false)]);
}

// RegExpCursor

#[test]
fn regexp_finds_all_matches_in_a_simple_string() {
    let t = doc(&["one two one two one"]);
    test_matches(regexp(&t, "one", false, 0, None), &[(0, 3, true), (8, 11, true), (16, 19, true)]);
}

#[test]
fn regexp_matches_by_line() {
    let t = doc(&["one two", "three four five", "six"]);
    test_matches(
        regexp(&t, r"^\w+|\w+$", false, 0, None),
        &[(0, 3, true), (4, 7, true), (8, 13, true), (19, 23, true), (24, 27, true)],
    );
}

#[test]
fn regexp_handles_empty_lines() {
    let t = doc(&["one", "", "two"]);
    test_matches(regexp(&t, ".*", false, 0, None), &[(0, 3, true), (4, 4, true), (5, 8, true)]);
}

#[test]
fn regexp_handles_empty_documents() {
    test_matches(regexp(&Text::empty(), ".*", false, 0, None), &[(0, 0, true)]);
    test_matches(regexp(&Text::empty(), "okay", false, 0, None), &[]);
}

#[test]
fn regexp_properly_cuts_off_long_matches() {
    test_matches(regexp(&doc(&["abcdefghi"]), ".*", false, 3, Some(6)), &[(3, 6, true)]);
}

#[test]
fn regexp_can_match_case_insensitively() {
    test_matches(regexp(&doc(&["abcdefghi"]), "DEF", true, 0, None), &[(3, 6, true)]);
}

#[test]
fn regexp_matches_across_lines() {
    test_matches(regexp(&doc(&["abc", "def"]), "c\nd", false, 0, None), &[(2, 5, true)]);
}

#[test]
fn regexp_detects_multi_line_regexps() {
    let t = doc(&["abc", "def"]);
    for pattern in [r"c\sd", r"c\Wd", r"c\Dd", "c[^x]d"] {
        test_matches(regexp(&t, pattern, false, 0, None), &[(2, 5, true)]);
    }
}

#[test]
fn regexp_can_match_a_large_document() {
    let line = "1234567890".repeat(10);
    let lines: Vec<&str> = (0..100).map(|_| line.as_str()).collect();
    let t = Text::of(&lines);
    let m = RegExpCursor::new(&t, "[^]*", RegExpCursorOptions::default(), 0, None).unwrap().next().unwrap();
    assert_eq!((m.from, m.to), (0, t.len()));
    assert_eq!(m.groups[0].as_ref().unwrap().len(), t.len());
}

#[test]
fn regexp_matches_line_starts_properly_in_multiline_mode() {
    test_matches(regexp(&doc(&["x", "a1111", "111a1111"]), r"^a(1|\s)*", false, 0, None), &[(2, 11, true)]);
}

#[test]
fn regexp_matches_line_ends_properly_in_multiline_mode() {
    test_matches(regexp(&doc(&["x", "111p111", "1111p"]), r"(1|\s)*p$", false, 0, None), &[(6, 15, true)]);
}
