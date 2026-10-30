//! Port of `test/test-selection-match.ts`, and tests for the find and
//! replace commands (which CodeMirror tests in the browser).

use codemirror_search::{
    close_search_panel, find_next, find_previous, get_search_query, goto_line_selection, open_search_panel,
    replace_all, replace_next, search, search_panel_open, select_matches, select_next_occurrence,
    select_selection_matches, SearchQuery, SearchQueryConfig, SET_SEARCH_QUERY,
};
use codemirror_state::{
    EditorSelection, EditorState, EditorStateConfig, SelectionRange, StateCommand, TransactionSpec,
    ALLOW_MULTIPLE_SELECTIONS,
};

/// A state from `doc`, where `|` marks a cursor and `<...>` a range.
fn mk_state(doc: &str) -> EditorState {
    let mut ranges: Vec<SelectionRange> = Vec::new();
    let mut text = String::new();
    let mut chars = doc.chars();
    while let Some(c) = chars.next() {
        match c {
            '|' => ranges.push(EditorSelection::cursor(text.len())),
            '<' => {
                let start = text.len();
                for c in chars.by_ref() {
                    if c == '>' {
                        break;
                    }
                    text.push(c);
                }
                ranges.push(EditorSelection::range(start, text.len()));
            }
            c => text.push(c),
        }
    }
    if ranges.is_empty() {
        ranges.push(EditorSelection::cursor(0));
    }
    EditorState::create(
        EditorStateConfig::new()
            .doc(text)
            .selection(EditorSelection::create(ranges, 0))
            .extensions(ALLOW_MULTIPLE_SELECTIONS.of(true)),
    )
}

fn state_str(state: &EditorState) -> String {
    let mut doc = state.doc().to_string();
    for range in state.selection().ranges.iter().rev() {
        if range.is_empty() {
            doc.insert(range.from, '|');
        } else {
            doc.insert(range.to, '>');
            doc.insert(range.from, '<');
        }
    }
    doc
}

fn run(state: &EditorState, cmd: StateCommand) -> (EditorState, bool) {
    let mut out = state.clone();
    let ran = cmd(state, &mut |tr| out = tr.state().clone());
    (out, ran)
}

fn next_occurrence(doc: &str, expected: &str) {
    let (state, _) = run(&mk_state(doc), select_next_occurrence);
    assert_eq!(state_str(&state), expected, "from {doc}");
}

#[test]
fn expands_to_the_surrounding_word() {
    next_occurrence("one| two", "<one> two");
    next_occurrence("|one two", "<one> two");
    next_occurrence("o|ne two", "<one> two");
    next_occurrence("one |two", "one <two>");
}

#[test]
fn selects_the_next_occurrence() {
    next_occurrence("<one> one one", "<one> <one> one");
    next_occurrence("<one> <one> two one", "<one> <one> two <one>");
    next_occurrence("one <one> one", "one <one> <one>");
    next_occurrence("one <one> <one>", "<one> <one> <one>");
    next_occurrence("<one> <one> <one>", "<one> <one> <one>");
}

#[test]
fn matches_full_words() {
    next_occurrence("<one> two onetwo one", "<one> two onetwo <one>");
}

#[test]
fn matches_subwords_if_a_subword_is_selected() {
    next_occurrence("<one>two onethree", "<one>two <one>three");
    next_occurrence("<one>two <one> onethree", "<one>two <one> <one>three");
}

// Find and replace.

/// A state with the search extension and an open panel searching `query`.
fn searching(doc: &str, config: SearchQueryConfig) -> EditorState {
    let state = mk_state(doc);
    let state = state.update(TransactionSpec::new().effect(codemirror_state::StateEffect::append_config(search(None)))).state().clone();
    let (state, _) = run(&state, open_search_panel);
    state.update(TransactionSpec::new().effect(SET_SEARCH_QUERY.of(SearchQuery::new(config)))).state().clone()
}

fn q(search: &str) -> SearchQueryConfig {
    SearchQueryConfig { search: search.into(), ..Default::default() }
}

#[test]
fn opens_and_closes_the_panel() {
    let state = mk_state("one <two> three");
    assert!(!search_panel_open(&state));
    let (state, ran) = run(&state, open_search_panel);
    assert!(ran && search_panel_open(&state));
    assert_eq!(get_search_query(&state).search, "two");
    let (state, ran) = run(&state, close_search_panel);
    assert!(ran && !search_panel_open(&state));
    assert!(!run(&state, close_search_panel).1);
}

#[test]
fn reopening_takes_the_new_selection_but_keeps_the_options() {
    let state = searching("a <b> a", SearchQueryConfig { case_sensitive: true, ..q("x") });
    let state = state.update(TransactionSpec::new().selection(EditorSelection::single(0, 1))).state().clone();
    let (state, _) = run(&state, open_search_panel);
    let query = get_search_query(&state);
    assert_eq!(query.search, "a");
    assert!(query.case_sensitive);
}

#[test]
fn finds_next_and_previous_with_wrapping() {
    let state = searching("|ab ab ab", q("ab"));
    let (state, _) = run(&state, find_next);
    assert_eq!(state_str(&state), "<ab> ab ab");
    let (state, _) = run(&state, find_next);
    assert_eq!(state_str(&state), "ab <ab> ab");
    let (state, _) = run(&state, find_next);
    let (state, _) = run(&state, find_next);
    assert_eq!(state_str(&state), "<ab> ab ab");
    let (state, _) = run(&state, find_previous);
    assert_eq!(state_str(&state), "ab ab <ab>");
}

#[test]
fn find_next_opens_the_panel_without_a_valid_query() {
    let state = mk_state("|ab");
    let (state, ran) = run(&state, find_next);
    assert!(ran && search_panel_open(&state));
}

#[test]
fn selects_all_matches() {
    let state = searching("|x ab x ab", q("ab"));
    let (state, _) = run(&state, select_matches);
    assert_eq!(state_str(&state), "x <ab> x <ab>");
}

#[test]
fn replaces_the_current_match_then_selects_the_next() {
    let state = searching("|ab ab ab", SearchQueryConfig { replace: "X".into(), ..q("ab") });
    // The first replace selects the first match.
    let (state, _) = run(&state, replace_next);
    assert_eq!(state_str(&state), "<ab> ab ab");
    let (state, _) = run(&state, replace_next);
    assert_eq!(state_str(&state), "X <ab> ab");
    let (state, _) = run(&state, replace_next);
    assert_eq!(state_str(&state), "X X <ab>");
    let (state, _) = run(&state, replace_next);
    // No match is left: the selection stays on the replaced text, as in CodeMirror.
    assert_eq!(state_str(&state), "X X <X>");
}

#[test]
fn replaces_all_with_regexp_groups() {
    let state = searching(
        "|a1 b22 c3",
        SearchQueryConfig { regexp: true, replace: "$2=$1".into(), ..q(r"([a-z])(\d+)") },
    );
    let (state, ran) = run(&state, replace_all);
    assert!(ran);
    assert_eq!(state.doc().to_string(), "1=a 22=b 3=c");
}

#[test]
fn replaces_all_whole_words_case_insensitively() {
    let state = searching("|Cat cat concat CAT", SearchQueryConfig { whole_word: true, replace: "dog".into(), ..q("cat") });
    let (state, _) = run(&state, replace_all);
    assert_eq!(state.doc().to_string(), "dog dog concat dog");
}

#[test]
fn selects_selection_matches() {
    let (state, _) = run(&mk_state("ab <ab> abc"), select_selection_matches);
    assert_eq!(state_str(&state), "<ab> <ab> <ab>c");
    assert_eq!(state.selection().main_index, 1);
}

#[test]
fn goes_to_lines() {
    let state = mk_state("one\ntwo|\nthree\nfour");
    let pos = |input: &str| goto_line_selection(&state, input).map(|s| s.main().head());
    assert_eq!(pos("3"), Some(8));
    assert_eq!(pos("3:2"), Some(10));
    assert_eq!(pos("+1"), Some(8));
    assert_eq!(pos("-1"), Some(0));
    assert_eq!(pos("100"), Some(14));
    assert_eq!(pos("50%"), Some(4));
    assert_eq!(pos(":2"), Some(6));
    assert_eq!(pos("x"), None);
}
