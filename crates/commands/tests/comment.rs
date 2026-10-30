//! Port of CodeMirror's `test/test-comment.ts`. The multi-language test
//! needs the HTML language and is not ported yet.

use std::collections::HashMap;
use std::sync::Arc;

use codemirror_commands::{
    toggle_block_comment, toggle_block_comment_by_line, toggle_line_comment, BlockCommentTokens, CommentTokens,
};
use codemirror_state::{
    AnyValue, EditorSelection, EditorState, EditorStateConfig, Extension, LanguageData, LanguageDataProvider,
    StateCommand, Transaction, ALLOW_MULTIPLE_SELECTIONS, LANGUAGE_DATA,
};

/// Create a state from `doc`, where `|` marks the ends of selection ranges
/// (a single `|` is a cursor).
fn s(doc: &str, config: &CommentTokens) -> EditorState {
    let mut doc = doc.to_string();
    let mut markers = Vec::new();
    while let Some(pos) = doc.find('|') {
        markers.push(pos);
        doc.remove(pos);
    }
    let ranges = if markers.len() == 1 {
        vec![EditorSelection::cursor(markers[0])]
    } else if markers.is_empty() {
        vec![EditorSelection::cursor(0)]
    } else {
        assert!(markers.len() % 2 == 0, "Markers for multiple selections need to be even.");
        markers.chunks(2).map(|m| EditorSelection::range(m[0], m[1])).collect()
    };
    let data: LanguageData =
        Arc::new(HashMap::from([("commentTokens".to_string(), Arc::new(config.clone()) as AnyValue)]));
    let provider: LanguageDataProvider = Arc::new(move |_: &EditorState, _: usize, _: i32| vec![data.clone()]);
    EditorState::create(
        EditorStateConfig::new()
            .doc(doc)
            .selection(EditorSelection::create(ranges, 0))
            .extensions(Extension::list([ALLOW_MULTIPLE_SELECTIONS.of(true), LANGUAGE_DATA.of(provider)])),
    )
}

fn same(actual: &EditorState, expected: &EditorState) {
    assert_eq!(actual.doc_string(), expected.doc_string());
    let sel = |s: &EditorState| {
        let ranges: Vec<(usize, usize)> = s.selection().ranges.iter().map(|r| (r.anchor(), r.head())).collect();
        (ranges, s.selection().main_index)
    };
    assert_eq!(sel(actual), sel(expected), "selection in {:?}", actual.doc_string());
}

fn check_toggle_chain(toggle: StateCommand, config: &CommentTokens, docs: &[String]) {
    let mut state = s(&docs[0], config);
    for i in 1..=docs.len() {
        let start = state.clone();
        toggle(&start, &mut |tr: Transaction| state = tr.state().clone());
        same(&state, &s(&docs[if i == docs.len() { docs.len() - 2 } else { i }], config));
    }
}

fn line_config(k: &str) -> CommentTokens {
    CommentTokens { line: Some(k.to_string()), block: None }
}

fn check_line(k: &str, docs: &[String]) {
    check_toggle_chain(toggle_line_comment, &line_config(k), docs);
}

const LINE_TOKENS: [&str; 2] = ["//", "#"];

#[test]
fn line_toggles_in_an_empty_single_selection() {
    for k in LINE_TOKENS {
        check_line(
            k,
            &[
                format!("\nline 1\n  {k} {k} {k} {k}line| 2\nline 3\n"),
                format!("\nline 1\n  {k} {k} {k}line| 2\nline 3\n"),
                format!("\nline 1\n  {k} {k}line| 2\nline 3\n"),
                format!("\nline 1\n  {k}line| 2\nline 3\n"),
                "\nline 1\n  line| 2\nline 3\n".to_string(),
                format!("\nline 1\n  {k} line| 2\nline 3\n"),
            ],
        );
        check_line(
            k,
            &[
                format!("\nline 1\n  {k}line 2|\nline 3\n"),
                "\nline 1\n  line 2|\nline 3\n".to_string(),
                format!("\nline 1\n  {k} line 2|\nline 3\n"),
            ],
        );
        check_line(
            k,
            &[
                format!("\nline 1\n|  {k}line 2\nline 3\n"),
                "\nline 1\n|  line 2\nline 3\n".to_string(),
                format!("\nline 1\n|  {k} line 2\nline 3\n"),
            ],
        );
        check_line(
            k,
            &[
                format!("\nline 1\n|{k}\nline 3\n"),
                "\nline 1\n|\nline 3\n".to_string(),
                format!("\nline 1\n{k} |\nline 3\n"),
            ],
        );
        check_line(
            k,
            &[
                format!("\nline 1\n line 2\nline 3\n|{k}"),
                "\nline 1\n line 2\nline 3\n|".to_string(),
                format!("\nline 1\n line 2\nline 3\n{k} |"),
            ],
        );
    }
}

#[test]
fn line_toggles_comments_in_a_single_line_when_the_cursor_is_at_the_beginning() {
    for k in LINE_TOKENS {
        check_line(k, &["line 1\n  |line 2\nline 3\n".to_string(), format!("line 1\n  {k} |line 2\nline 3\n")]);
    }
}

#[test]
fn line_toggles_comments_in_a_single_line_selection() {
    for k in LINE_TOKENS {
        check_line(
            k,
            &[
                format!("line 1\n  {k}li|ne |2\nline 3\n"),
                "line 1\n  li|ne |2\nline 3\n".to_string(),
                format!("line 1\n  {k} li|ne |2\nline 3\n"),
            ],
        );
    }
}

#[test]
fn line_toggles_comments_in_a_multi_line_selection() {
    for k in LINE_TOKENS {
        check_line(
            k,
            &[
                format!("\n  {k}lin|e 1\n  {k}  line 2\n  {k} line |3\n"),
                "\n  lin|e 1\n   line 2\n  line |3\n".to_string(),
                format!("\n  {k} lin|e 1\n  {k}  line 2\n  {k} line |3\n"),
            ],
        );
        check_line(
            k,
            &[
                format!("\n  {k}lin|e 1\n  {k}  line 2\n   line 3\n  {k} li|ne 4\n"),
                format!("\n  {k} {k}lin|e 1\n  {k} {k}  line 2\n  {k}  line 3\n  {k} {k} li|ne 4\n"),
            ],
        );
        check_line(k, &[format!("\n  {k} lin|e 1\n\n  {k} line |3\n"), "\n  lin|e 1\n\n  line |3\n".to_string()]);
        check_line(
            k,
            &[format!("\n  {k} lin|e 1\n     \n  {k} line |3\n"), "\n  lin|e 1\n     \n  line |3\n".to_string()],
        );
        check_line(k, &[format!("\n|\n  {k} line 2\n    | \n"), "\n|\n  line 2\n    | \n".to_string()]);
        check_line(k, &["\n|\n\n    | \n".to_string(), "\n|\n\n    | \n".to_string()]);
    }
}

#[test]
fn line_toggles_comments_in_a_multi_line_multi_range_selection() {
    for k in LINE_TOKENS {
        check_line(
            k,
            &[
                "\n  lin|e 1\n  line |2\n  line 3\n  l|ine 4\n  line| 5\n".to_string(),
                format!("\n  {k} lin|e 1\n  {k} line |2\n  line 3\n  {k} l|ine 4\n  {k} line| 5\n"),
            ],
        );
    }
}

#[test]
fn line_can_handle_multiple_selections_on_one_line() {
    for k in LINE_TOKENS {
        check_line(k, &["|line| |with| |ranges|".to_string(), format!("{k} |line| |with| |ranges|")]);
    }
}

#[test]
fn line_doesnt_include_lines_in_which_a_selection_range_ends() {
    for k in LINE_TOKENS {
        check_line(k, &["line| 1\nline 2\n|line 3".to_string(), format!("{k} line| 1\n{k} line 2\n|line 3")]);
    }
}

#[test]
fn line_leaves_empty_lines_alone() {
    for k in LINE_TOKENS {
        check_line(k, &["line| 1\n\nline 3|".to_string(), format!("{k} line| 1\n\n{k} line 3|")]);
    }
}

#[test]
fn line_does_comment_empty_lines_with_a_cursor() {
    for k in LINE_TOKENS {
        check_line(k, &["|\nline 2".to_string(), format!("{k} |\nline 2")]);
    }
}

const BLOCK_TOKENS: [(&str, &str); 2] = [("/*", "*/"), ("<!--", "-->")];

fn block_config(o: &str, c: &str) -> CommentTokens {
    CommentTokens { line: None, block: Some(BlockCommentTokens { open: o.to_string(), close: c.to_string() }) }
}

#[test]
fn block_toggles_block_comment_in_multi_line_selection() {
    for (o, c) in BLOCK_TOKENS {
        check_toggle_chain(
            toggle_block_comment,
            &block_config(o, c),
            &[
                "\n  lin|e 1\n  line 2\n  line 3\n  line |4\n  line 5\n".to_string(),
                format!("\n  lin{o} |e 1\n  line 2\n  line 3\n  line | {c}4\n  line 5\n"),
            ],
        );
    }
}

#[test]
fn block_toggles_block_comment_in_multi_line_multi_range_selection() {
    for (o, c) in BLOCK_TOKENS {
        check_toggle_chain(
            toggle_block_comment,
            &block_config(o, c),
            &[
                "\n  lin|e 1\n  line |2\n  l|ine 3\n  line 4\n  line |5\n".to_string(),
                format!("\n  lin{o} |e 1\n  line | {c}2\n  l{o} |ine 3\n  line 4\n  line | {c}5\n"),
            ],
        );
    }
}

#[test]
fn block_can_toggle_comments_inside_the_selection() {
    for (o, c) in BLOCK_TOKENS {
        check_toggle_chain(
            toggle_block_comment,
            &block_config(o, c),
            &[format!("|{o} one\ntwo {c}| three"), "|one\ntwo| three".to_string(), format!("{o} |one\ntwo| {c} three")],
        );
    }
}

fn check_block_line(o: &str, c: &str, docs: &[String]) {
    check_toggle_chain(toggle_block_comment_by_line, &block_config(o, c), docs);
}

#[test]
fn block_comments_the_entire_line() {
    for (o, c) in BLOCK_TOKENS {
        check_block_line(o, c, &["one|\ntwo".to_string(), format!("{o} one| {c}\ntwo")]);
    }
}

#[test]
fn block_comments_multiple_lines() {
    for (o, c) in BLOCK_TOKENS {
        check_block_line(o, c, &["on|e\nt|wo".to_string(), format!("{o} on|e\nt|wo {c}")]);
    }
}

#[test]
fn block_joins_selected_blocks_of_lines() {
    for (o, c) in BLOCK_TOKENS {
        check_block_line(o, c, &["on|e\nt|w|o\nth|ree".to_string(), format!("{o} on|e\nt|w|o\nth|ree {c}")]);
    }
}

#[test]
fn block_doesnt_include_lines_that_the_selection_stops_at_the_start_of() {
    for (o, c) in BLOCK_TOKENS {
        check_block_line(o, c, &["|one\n|two".to_string(), format!("{o} |one {c}\n|two")]);
    }
}

#[test]
fn block_does_include_lines_with_cursor_selection_at_the_start() {
    for (o, c) in BLOCK_TOKENS {
        check_block_line(o, c, &["|one\ntwo".to_string(), format!("|{o} one {c}\ntwo")]);
    }
}
