//! A window with the editor, for trying it out: `cargo run -p codemirror-demo`.

use codemirror_commands::{default_keymap, history, history_keymap, indent_with_tab, HistoryConfig};
use codemirror_state::{EditorState, EditorStateConfig, Extension, ALLOW_MULTIPLE_SELECTIONS};
use codemirror_view::{highlight_active_line, keymap, line_numbers, line_wrapping, EditorView};
use eframe::egui;

const SAMPLE: &str = "Welcome to the Rust port of CodeMirror, drawn with egui.

Try the usual keys: arrows, Home and End, PageUp and PageDown, Ctrl+Arrow
to move by word, Shift to select, Ctrl+Z and Ctrl+Y to undo and redo.
Alt+Up and Alt+Down move lines; Ctrl+Alt+Up and Down add cursors.
Ctrl+Click adds a cursor, Alt+drag selects a rectangle, double-click selects
a word and triple-click a line. Ctrl+/ toggles a comment (none configured
here, so it does nothing), Tab indents and Shift+Tab dedents.

\tA line indented with a tab.
\t\tTwo tabs.
Unicode: héllo wörld, 中文字符, emoji 👍🏽 and Ελληνικά.

This line is long enough to wrap when line wrapping is on, which it is in this demo, so you can try moving up and down through the wrapped rows of a single document line, and Home and End go to the wrap points first.
";

struct Demo {
    view: EditorView,
}

impl Demo {
    fn new() -> Demo {
        let mut doc = String::from(SAMPLE);
        for i in 1..=2000 {
            doc.push_str(&format!("Filler line {i} to make the document long enough to scroll.\n"));
        }
        let mut bindings = default_keymap();
        bindings.extend(history_keymap());
        bindings.push(indent_with_tab());
        let extensions = Extension::list([
            history(HistoryConfig::default()),
            keymap(bindings),
            line_numbers(),
            highlight_active_line(),
            line_wrapping(),
            ALLOW_MULTIPLE_SELECTIONS.of(true),
        ]);
        let state = EditorState::create(EditorStateConfig::new().doc(doc).extensions(extensions));
        let mut view = EditorView::new(state);
        view.focus();
        Demo { view }
    }
}

impl eframe::App for Demo {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Frame::NONE.show(ui, |ui| {
            self.view.ui(ui);
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 640.0]).with_title("codemirror-rs demo"),
        ..Default::default()
    };
    eframe::run_native("codemirror-rs demo", options, Box::new(|_| Ok(Box::new(Demo::new()))))
}
