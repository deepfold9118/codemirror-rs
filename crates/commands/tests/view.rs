//! Port of the cases in CodeMirror's `test/webtest-commands.ts` that don't
//! need widgets or CSS, run in GPUI's test context (whose text system gives
//! every character the same width). CodeMirror's "narrow" theme (13ch
//! wide) becomes a window sized for 13 columns. The CJK `Intl.Segmenter`
//! cases are not ported.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use codemirror_commands::*;
use codemirror_state::{AnyValue, EditorState, Extension, LanguageData, LanguageDataProvider, LANGUAGE_DATA};
use codemirror_view::{keymap, line_wrapping, EditorView};
use common::{mk_state, state_str};
use gpui::{px, size, Context, Entity, TestAppContext, VisualTestContext, Window};

type Cmd = dyn Fn(&mut EditorView, &mut Window, &mut Context<EditorView>) -> bool;

// Size the test window (the view fills it) and let it draw.
fn draw(cx: &mut VisualTestContext, view: &Entity<EditorView>, width: f32) {
    cx.simulate_resize(size(px(width), px(300.)));
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
}

fn test_cmd_with(before: &str, after: &str, command: &Cmd, ext: Extension, narrow: bool) {
    let ext = if narrow { Extension::list([ext, line_wrapping()]) } else { ext };
    let state = mk_state(before, ext);
    let mut app = TestAppContext::single();
    let (view, cx) = app.add_window_view(|_, cx| EditorView::new(state, cx));
    draw(cx, &view, 800.0);
    if narrow {
        let (char_w, pad) = view.read_with(cx, |v, _| (v.default_character_width(), v.theme().padding_left));
        // Padding, scrollbar (12) and the 2px gap before it, then 13 columns.
        let width = pad + 14.0 + 13.0 * char_w + 0.5;
        draw(cx, &view, width);
        draw(cx, &view, width);
    }
    view.update_in(cx, |v, window, cx| {
        command(v, window, cx);
    });
    let result = view.read_with(cx, |v, _| state_str(v.state()));
    assert_eq!(result, after, "from {before:?}");
}

fn test_cmd(before: &str, after: &str, command: codemirror_view::Command) {
    test_cmd_with(before, after, &command, Extension::empty(), false);
}

fn twice(command: codemirror_view::Command) -> impl Fn(&mut EditorView, &mut Window, &mut Context<EditorView>) -> bool {
    move |v, w, cx| {
        command(v, w, cx);
        command(v, w, cx);
        true
    }
}

fn dash_word_char() -> Extension {
    let data: LanguageData =
        Arc::new(HashMap::from([("wordChars".to_string(), Arc::new("-".to_string()) as AnyValue)]));
    let provider: LanguageDataProvider = Arc::new(move |_: &EditorState, _: usize, _: i32| vec![data.clone()]);
    LANGUAGE_DATA.of(provider)
}

#[test]
fn cursor_subword_forward_stops_at_boundaries() {
    for (before, after) in [
        ("|CamelCaseWord", "Camel|CaseWord"),
        ("Camel|CaseWord", "CamelCase|Word"),
        ("CamelCase|Word", "CamelCaseWord|"),
        ("Eat|CSSToken", "EatCSS|Token"),
        ("o|kay.", "okay|."),
        ("|snake_case", "snake|_case"),
        ("snake|_case", "snake_|case"),
    ] {
        test_cmd(before, after, cursor_subword_forward);
    }
    for (before, after) in [("|kebab-case", "kebab|-case"), ("kebab|-case", "kebab-|case"), ("one|--..", "one--|..")] {
        test_cmd_with(before, after, &cursor_subword_forward, dash_word_char(), false);
    }
}

#[test]
fn cursor_subword_backward_stops_at_boundaries() {
    for (before, after) in [
        ("CamelCaseWord|", "CamelCase|Word"),
        ("Camel|CaseWord", "|CamelCaseWord"),
        ("EatCSS|Token", "Eat|CSSToken"),
        (".o|kay", ".|okay"),
        ("snake_case|", "snake_|case"),
        ("snake_|case", "snake|_case"),
    ] {
        test_cmd(before, after, cursor_subword_backward);
    }
    for (before, after) in [("kebab-case|", "kebab-|case"), ("kebab--|case", "kebab|--case"), ("..--one|", "..--|one")]
    {
        test_cmd_with(before, after, &cursor_subword_backward, dash_word_char(), false);
    }
}

#[test]
fn moves_cursor_line_down() {
    test_cmd("on|e\ntwo", "one\ntw|o", cursor_line_down); // can move to the next line
    test_cmd("on|e\nt", "one\nt|", cursor_line_down); // can move to a shorter line
    test_cmd("on|e", "one|", cursor_line_down); // goes to the end on last line
    test_cmd_with("on|e\nt\nthree", "one\nt\nth|ree", &twice(cursor_line_down), Extension::empty(), false);
    // can move in a wrapped line
    test_cmd_with(
        "da|ndelion dandelion dandelion",
        "dandelion da|ndelion dandelion",
        &cursor_line_down,
        Extension::empty(),
        true,
    );
    // keeps assoc when moving through wrapped text
    test_cmd_with(
        "|dandelion dandelion dandelion",
        "dandelion dandelion |dandelion",
        &twice(cursor_line_down),
        Extension::empty(),
        true,
    );
}

#[test]
fn moves_cursor_line_up() {
    test_cmd("one\ntwo|", "one|\ntwo", cursor_line_up);
    test_cmd("o\ntwo|", "o|\ntwo", cursor_line_up);
    test_cmd("on|e", "|one", cursor_line_up);
    test_cmd_with("one\nt\nthr|ee", "one|\nt\nthree", &twice(cursor_line_up), Extension::empty(), false);
    test_cmd_with(
        "dandelion dandel|ion dandelion",
        "dandel|ion dandelion dandelion",
        &cursor_line_up,
        Extension::empty(),
        true,
    );
}

#[test]
fn extends_select_line_down() {
    test_cmd("|one\ntwo\nthree", "<one\n>two\nthree", select_line_down);
    test_cmd("on|e\ntwo\nthree", "on<e\ntw>o\nthree", select_line_down);
    test_cmd_with("on|e\nt\nthree", "on<e\nt\nth>ree", &twice(select_line_down), Extension::empty(), false);
    test_cmd("one\nt|wo", "one\nt<wo>", select_line_down);
    let down_up = |v: &mut EditorView, w: &mut Window, cx: &mut Context<EditorView>| {
        select_line_down(v, w, cx) && select_line_up(v, w, cx)
    };
    test_cmd_with("one\ntwo\nth|ree", "one\ntwo<\nth>ree", &down_up, Extension::empty(), false);
    test_cmd_with(
        "|dandelion dandelion dandelion",
        "<dandelion dandelion >dandelion",
        &twice(select_line_down),
        Extension::empty(),
        true,
    );
}

#[test]
fn extends_select_line_up() {
    test_cmd("one\ntwo\n|three", "one\n<two\n>three", select_line_up);
    test_cmd("one\ntwo\nt|hree", "one\nt<wo\nt>hree", select_line_up);
    test_cmd_with("one\nt\nthr|ee", "one<\nt\nthr>ee", &twice(select_line_up), Extension::empty(), false);
    test_cmd("on|e\ntwo", "<on>e\ntwo", select_line_up);
    let up_down = |v: &mut EditorView, w: &mut Window, cx: &mut Context<EditorView>| {
        select_line_up(v, w, cx) && select_line_down(v, w, cx)
    };
    test_cmd_with("on|e\ntwo\nthree", "on<e\n>two\nthree", &up_down, Extension::empty(), false);
}

#[test]
fn select_char_left_and_right() {
    test_cmd("ab|cd", "a<b>cd", select_char_left);
    test_cmd("a<bc>d", "<abc>d", select_char_left);
    test_cmd("a<bc>d", "a<bcd>", select_char_right);
}

// Not in CodeMirror's suite.

#[test]
fn moves_by_group_and_line_boundary() {
    test_cmd("|one two", "one| two", cursor_group_forward);
    test_cmd("one two|", "one |two", cursor_group_backward);
    test_cmd("|one two", "one |two", cursor_group_forward_win);
    test_cmd("  on|e", "  |one", cursor_line_boundary_backward);
    test_cmd("  |one", "|  one", cursor_line_boundary_backward);
    test_cmd("o|ne\ntwo", "one|\ntwo", cursor_line_boundary_forward);
    test_cmd("on|e\ntwo", "tw|o", delete_line);
    test_cmd("o|ne\ntwo", "o|\ntwo", delete_to_line_end);
    test_cmd("o|ne\ntwo|", "o|ne|\ntwo|", add_cursor_above);
}

#[test]
fn types_and_runs_key_bindings_from_events() {
    let mut bindings = default_keymap();
    bindings.extend(history_keymap());
    let ext = Extension::list([history(HistoryConfig::default()), keymap(bindings)]);
    let state = mk_state("|", ext);
    let mut app = TestAppContext::single();
    let (view, cx) = app.add_window_view(|_, cx| EditorView::new(state, cx));
    view.update_in(cx, |v, window, _| v.focus(window));
    draw(cx, &view, 800.0);
    let doc = |cx: &mut VisualTestContext| view.read_with(cx, |v, _| state_str(v.state()));
    cx.simulate_input("abc");
    assert_eq!(doc(cx), "abc|");
    cx.simulate_keystrokes("left backspace");
    assert_eq!(doc(cx), "a|c");
    cx.simulate_keystrokes("enter");
    assert_eq!(doc(cx), "a\n|c");
    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(view.read_with(cx, |v, _| v.state().doc_string()), "ac");
    cx.simulate_keystrokes("shift-left");
    assert!(view.read_with(cx, |v, _| !v.state().selection().main().is_empty()));
}
