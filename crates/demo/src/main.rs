//! A window with the editor view: `cargo run --release -p codemirror-demo`.

#![windows_subsystem = "windows"]

use codemirror_commands::{default_keymap, history, history_keymap, indent_less, insert_tab, HistoryConfig};
use codemirror_state::{EditorState, EditorStateConfig, Extension, ALLOW_MULTIPLE_SELECTIONS};
use codemirror_view::{highlight_active_line, keymap, line_numbers, line_wrapping, EditorView, KeyBinding};
use gpui::{prelude::*, px, size, App, Application, Bounds, Focusable, WindowBounds, WindowOptions};

const SAMPLE: &str = "Welcome to the Rust port of CodeMirror, drawn with GPUI.

Try the usual keys: arrows, Home and End, PageUp and PageDown, Ctrl+Arrow
to move by word, Shift to select, Ctrl+Z and Ctrl+Y to undo and redo.
Alt+Up and Alt+Down move lines; Ctrl+Alt+Up and Down add cursors.
Ctrl+Click adds a cursor, Alt+drag selects a rectangle, double-click selects
a word and triple-click a line. Tab indents and Shift+Tab dedents.

\tA line indented with a tab.
\t\tTwo tabs.
Unicode: héllo wörld, 中文字符, emoji 👍🏽 and Ελληνικά.

This line is long enough to wrap when line wrapping is on, which it is in this demo, so you can try moving up and down through the wrapped rows of a single document line, and Home and End go to the wrap points first.
";

fn main() {
    let mut doc = String::from(SAMPLE);
    for i in 1..=20000 {
        doc.push_str(&format!("Filler line {i} to make the document long enough to scroll.\n"));
    }
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(900.), px(640.)), cx);
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
        let window = cx
            .open_window(options, |_, cx| {
                let mut bindings = default_keymap();
                bindings.extend(history_keymap());
                // Tab inserts a tab, or indents a selection; Shift-Tab dedents.
                bindings.push(KeyBinding::new("Tab").run_state(insert_tab).shift_state(indent_less));
                let extensions = Extension::list([
                    history(HistoryConfig::default()),
                    keymap(bindings),
                    line_numbers(),
                    highlight_active_line(),
                    line_wrapping(),
                    ALLOW_MULTIPLE_SELECTIONS.of(true),
                ]);
                let state = EditorState::create(EditorStateConfig::new().doc(doc).extensions(extensions));
                cx.new(|cx| EditorView::new(state, cx))
            })
            .unwrap();
        window
            .update(cx, |view, window, cx| {
                window.focus(&view.focus_handle(cx));
                cx.activate(true);
            })
            .unwrap();
    });
}
