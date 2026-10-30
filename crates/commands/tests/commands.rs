//! Port of CodeMirror's `test/test-commands.ts`.
//!
//! The `indentSelection` tests and the `insertNewlineAndIndent` tests that
//! load the JavaScript language need a parser and are not ported yet.
//! The tests at the end cover commands that CodeMirror only tests in the
//! browser, or not at all.

mod common;

use codemirror_commands::*;
use codemirror_language::INDENT_UNIT;
use codemirror_state::{Extension, StateCommand};
use common::{mk_state, run, state_str};

fn check(from: &str, to: &str, command: StateCommand) {
    check_with(from, to, command, Extension::empty());
}

fn check_with(from: &str, to: &str, command: StateCommand, ext: Extension) {
    assert_eq!(state_str(&run(&mk_state(from, ext), command).0), to, "from {from:?}");
}

#[test]
fn indent_more_adds_indentation() {
    check("one\ntwo|\nthree", "one\n  two|\nthree", indent_more);
}

#[test]
fn indent_more_indents_all_lines_in_a_range() {
    check("one\n<two\nthree>", "one\n  <two\n  three>", indent_more);
}

#[test]
fn indent_more_doesnt_double_indent_a_given_line() {
    check("on|e|\n<two\nth><ree\nfour>", "  on|e|\n  <two\n  th><ree\n  four>", indent_more);
}

#[test]
fn indent_more_ignores_lines_if_a_range_selection_ends_directly_at_their_start() {
    check("on<e\ntwo\n>three", "  on<e\n  two\n>three", indent_more);
}

#[test]
fn indent_less_removes_indentation() {
    check("one\n  two|\nthree", "one\ntwo|\nthree", indent_less);
}

#[test]
fn indent_less_removes_one_unit_of_indentation() {
    check("one\n    two|\n     three|", "one\n  two|\n   three|", indent_less);
}

#[test]
fn indent_less_dedents_all_lines_in_a_range() {
    check("one\n  <two\n  three>", "one\n<two\nthree>", indent_less);
}

#[test]
fn indent_less_takes_tabs_into_account() {
    check("   \tone|\n  \ttwo|", "  one|\n  two|", indent_less);
}

#[test]
fn indent_less_can_split_tabs() {
    check("\tone|", "  one|", indent_less);
}

#[test]
fn insert_newline_keep_indent_keeps_indentation() {
    check("  one|", "  one\n  |", insert_newline_keep_indent);
}

#[test]
fn insert_newline_keep_indent_keeps_zero_indentation() {
    check("one|two", "one\n|two", insert_newline_keep_indent);
}

#[test]
fn insert_newline_keep_indent_deletes_the_selection() {
    check("if x:\n  one<two\n  three>four", "if x:\n  one\n  |four", insert_newline_keep_indent);
}

#[test]
fn insert_newline_and_indent_doesnt_indent_when_previous_line_lacks_indentation() {
    check("foo|", "foo\n|", insert_newline_and_indent);
}

#[test]
fn insert_newline_and_indent_indents_when_previous_line_uses_two_space_indentation() {
    check("  foo|", "  foo\n  |", insert_newline_and_indent);
}

#[test]
fn insert_newline_and_indent_indents_when_previous_line_uses_four_space_indentation() {
    check("    foo|", "    foo\n    |", insert_newline_and_indent);
}

#[test]
fn insert_newline_and_indent_indents_when_previous_line_uses_tab_indentation() {
    check_with("\tfoo|", "\tfoo\n\t|", insert_newline_and_indent, INDENT_UNIT.of("\t".into()));
}

#[test]
fn insert_newline_and_indent_indents_when_previous_line_uses_tab_indentation_and_short_alignment() {
    check_with("\t  foo|", "\t  foo\n\t  |", insert_newline_and_indent, INDENT_UNIT.of("\t".into()));
}

fn check_doc(from: &str, to: &str, command: StateCommand) {
    assert_eq!(run(&mk_state(from, Extension::empty()), command).0.doc_string(), to);
}

#[test]
fn delete_trailing_whitespace_deletes_trailing_whitespace() {
    check_doc("foo   ", "foo", delete_trailing_whitespace);
}

#[test]
fn delete_trailing_whitespace_checks_multiple_lines() {
    check_doc("one\ntwo \nthree   \n   ", "one\ntwo\nthree\n", delete_trailing_whitespace);
}

#[test]
fn delete_trailing_whitespace_can_handle_empty_lines() {
    check_doc("one  \n\ntwo ", "one\n\ntwo", delete_trailing_whitespace);
}

#[test]
fn delete_group_forward() {
    for (from, to) in [
        ("one |two three", "one | three"), // deletes a word
        ("one| two three", "one| three"),  // deletes a word with leading space
        ("one|...two", "one|two"),         // deletes a group of punctuation
        ("one|  \ttwo", "one|two"),        // deletes a group of space
        ("one|\ntwo", "one|two"),          // deletes a newline
        ("one| \n two", "one|\n two"),     // stops deleting at a newline
        ("one|\n two", "one| two"),        // stops deleting after a newline
        ("one|two", "one|"),               // deletes up to the end of the doc
        ("one|", "one|"),                  // does nothing at the end of the doc
    ] {
        check(from, to, codemirror_commands::delete_group_forward);
    }
}

#[test]
fn delete_group_backward() {
    for (from, to) in [
        ("one two| three", "one | three"), // deletes a word
        ("one two |three", "one |three"),  // deletes a word with trailing space
        ("one...|two", "one|two"),         // deletes a group of punctuation
        ("one \t |two", "one|two"),        // deletes a group of space
        ("one\n|two", "one|two"),          // deletes a newline
        ("one \n |two", "one \n|two"),     // stops deleting at a newline
        ("one \n|two", "one |two"),        // stops deleting after a newline
        ("one|two", "|two"),               // deletes up to the start of the doc
    ] {
        check(from, to, codemirror_commands::delete_group_backward);
    }
}

#[test]
fn move_line_up() {
    for (from, to) in [
        ("one\ntwo|\nthree", "two|\none\nthree"), // can move a line up
        ("one\nt|w|o|\n", "t|w|o|\none\n"),       // preserves multiple cursors
        ("one\ntwo\nthr<ee\nfour\nfive>\n", "one\nthr<ee\nfour\nfive>\ntwo\n"), // moves selected blocks as one
        ("one\n<two\nth>ree\nfo|u<r\nfive>\n", "<two\nth>ree\nfo|u<r\nfive>\none\n"), // multiple ranges as one
        ("one\n<two\nthree\n>four", "<two\nthree\n>one\nfour"), // no trailing line after a range
    ] {
        check(from, to, codemirror_commands::move_line_up);
    }
}

#[test]
fn move_line_down() {
    for (from, to) in [
        ("one\ntwo|\nthree", "one\nthree\ntwo|"),     // can move a line down
        ("one\nt|w|o|\nthree", "one\nthree\nt|w|o|"), // preserves multiple cursors
        ("one\ntwo\nthr<ee\nfour\nfive>\nsix", "one\ntwo\nsix\nthr<ee\nfour\nfive>"), // moves selected blocks as one
        ("one\n<two\nth>ree\nfo|u<r\nfive>\nsix\n", "one\nsix\n<two\nth>ree\nfo|u<r\nfive>\n"), // multiple ranges
        ("one\n<two\nthree\n>four\n", "one\nfour\n<two\nthree\n>"), // no trailing line after a range
        ("one\n<two\nthree\n>four", "one\nfour\n<two\nthree>"), // clips at the end of the doc
    ] {
        check(from, to, codemirror_commands::move_line_down);
    }
}

// Not in CodeMirror's state-level suite.

#[test]
fn delete_char_backward_deletes_an_indent_unit_or_a_character() {
    check("    |x", "  |x", delete_char_backward);
    check("   |x", "  |x", delete_char_backward);
    check("ab|c", "a|c", delete_char_backward);
    check("a\n|b", "a|b", delete_char_backward);
    check("é|", "|", delete_char_backward);
    check("<ab>c", "|c", delete_char_backward);
    check("a|b", "a|", delete_char_forward);
}

#[test]
fn transpose_chars_swaps_around_the_cursor() {
    check("ab|c", "acb|", transpose_chars);
    check("ab\n|cd", "abc\n|d", transpose_chars);
}

#[test]
fn cursor_matching_bracket_jumps_to_the_partner() {
    check("|(a[b]c)", "(a[b]c)|", cursor_matching_bracket);
    check("(a[b]|c)", "(a|[b]c)", cursor_matching_bracket);
    check("x|y", "x|y", cursor_matching_bracket);
    check("|(ab)", "<(ab)>", select_matching_bracket);
}

#[test]
fn copy_and_select_lines() {
    check("one|\ntwo", "one\none|\ntwo", copy_line_down);
    check("one|\ntwo", "one|\none\ntwo", copy_line_up);
    check("o|ne\ntwo", "<one\n>two", select_line);
    check("a|b", "a\t|b", insert_tab);
}
