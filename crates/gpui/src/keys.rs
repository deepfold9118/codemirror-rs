//! Key bindings, in CodeMirror's key notation, mapped from GPUI keystrokes.
//! The default set mirrors `@codemirror/commands`' standard and default
//! keymaps plus the history keymap and Tab indentation.

use std::collections::HashMap;

use codemirror_commands as cmds;
use gpui::{Context, Keystroke, Window};

use crate::view::{Cmd, EditorView};

pub(crate) struct Keymap {
    pub bindings: HashMap<String, Vec<Cmd>>,
}

fn normalize(name: &str) -> String {
    let (mods, key) = match name.rfind('-') {
        Some(i) if i + 1 < name.len() => (&name[..i], &name[i + 1..]),
        _ => ("", name),
    };
    let (mut alt, mut ctrl, mut shift, mut meta) = (false, false, false, false);
    for m in mods.split('-').filter(|m| !m.is_empty()) {
        match m.to_ascii_lowercase().as_str() {
            "alt" | "a" => alt = true,
            "ctrl" | "c" | "control" => ctrl = true,
            "shift" | "s" => shift = true,
            "cmd" | "meta" | "m" => meta = true,
            "mod" => {
                if cfg!(target_os = "macos") {
                    meta = true
                } else {
                    ctrl = true
                }
            }
            other => panic!("Unrecognized modifier name: {other}"),
        }
    }
    let mut result = if key == "Space" { " ".to_string() } else { key.to_string() };
    if alt {
        result = format!("Alt-{result}");
    }
    if ctrl {
        result = format!("Ctrl-{result}");
    }
    if meta {
        result = format!("Meta-{result}");
    }
    if shift {
        result = format!("Shift-{result}");
    }
    result
}

/// CodeMirror's name for a GPUI key.
pub(crate) fn key_name(key: &str) -> String {
    match key {
        "left" => "ArrowLeft".into(),
        "right" => "ArrowRight".into(),
        "up" => "ArrowUp".into(),
        "down" => "ArrowDown".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "pageup" => "PageUp".into(),
        "pagedown" => "PageDown".into(),
        "enter" => "Enter".into(),
        "escape" => "Escape".into(),
        "backspace" => "Backspace".into(),
        "delete" => "Delete".into(),
        "tab" => "Tab".into(),
        "insert" => "Insert".into(),
        "space" => " ".into(),
        k if k.len() > 1 && k.starts_with('f') && k[1..].chars().all(|c| c.is_ascii_digit()) => k.to_uppercase(),
        k => k.to_string(),
    }
}

fn with_mods(name: &str, ks: &Keystroke, shift: bool) -> String {
    let m = &ks.modifiers;
    let mut n = name.to_string();
    if m.alt {
        n = format!("Alt-{n}");
    }
    if m.control {
        n = format!("Ctrl-{n}");
    }
    if m.platform {
        n = format!("Meta-{n}");
    }
    if shift && m.shift {
        n = format!("Shift-{n}");
    }
    n
}

impl Keymap {
    fn add(&mut self, key: &str, cmd: Cmd) {
        self.bindings.entry(normalize(key)).or_default().push(cmd);
    }

    fn bind(&mut self, key: &str, run: Cmd, shift: Option<Cmd>) {
        self.add(key, run);
        if let Some(s) = shift {
            self.add(&format!("Shift-{key}"), s);
        }
    }

    /// Run the commands bound to a keystroke. Returns true if one handled it.
    pub fn run(
        &self,
        view: &mut EditorView,
        ks: &Keystroke,
        window: &mut Window,
        cx: &mut Context<EditorView>,
    ) -> bool {
        let name = key_name(&ks.key);
        let is_char = name.chars().count() == 1 && name != " ";
        let mut candidates = Vec::new();
        if is_char && ks.modifiers.shift && name.chars().all(|c| c.is_ascii_alphabetic()) {
            candidates.push(with_mods(&name.to_uppercase(), ks, false));
        }
        if !is_char || !ks.modifiers.shift || ks.modifiers.control || ks.modifiers.alt || ks.modifiers.platform {
            candidates.push(with_mods(&name, ks, true));
        }
        for c in candidates {
            if let Some(cmds) = self.bindings.get(&c) {
                for cmd in cmds {
                    let handled = match *cmd {
                        Cmd::State(f) => view.run_state_command(f, cx),
                        Cmd::View(f) => f(view, window, cx),
                    };
                    if handled {
                        return true;
                    }
                }
            }
        }
        false
    }
}

type V = fn(&mut EditorView, &mut Window, &mut Context<EditorView>) -> bool;

fn v(f: V) -> Cmd {
    Cmd::View(f)
}

fn s(f: codemirror_state::StateCommand) -> Cmd {
    Cmd::State(f)
}

pub(crate) fn default_keymap() -> Keymap {
    let mut k = Keymap { bindings: HashMap::new() };
    // standardKeymap
    k.bind("ArrowLeft", v(|e, _, cx| e.cursor_char(false, cx)), Some(v(|e, _, cx| e.select_char(false, cx))));
    k.bind("ArrowRight", v(|e, _, cx| e.cursor_char(true, cx)), Some(v(|e, _, cx| e.select_char(true, cx))));
    k.bind(
        "Mod-ArrowLeft",
        v(|e, _, cx| e.cursor_group(false, false, cx)),
        Some(v(|e, _, cx| e.cursor_group(false, true, cx))),
    );
    k.bind(
        "Mod-ArrowRight",
        v(|e, _, cx| e.cursor_group(true, false, cx)),
        Some(v(|e, _, cx| e.cursor_group(true, true, cx))),
    );
    k.bind(
        "ArrowUp",
        v(|e, w, cx| e.cursor_line(false, false, false, w, cx)),
        Some(v(|e, w, cx| e.cursor_line(false, true, false, w, cx))),
    );
    k.bind(
        "ArrowDown",
        v(|e, w, cx| e.cursor_line(true, false, false, w, cx)),
        Some(v(|e, w, cx| e.cursor_line(true, true, false, w, cx))),
    );
    k.bind(
        "PageUp",
        v(|e, w, cx| e.cursor_line(false, false, true, w, cx)),
        Some(v(|e, w, cx| e.cursor_line(false, true, true, w, cx))),
    );
    k.bind(
        "PageDown",
        v(|e, w, cx| e.cursor_line(true, false, true, w, cx)),
        Some(v(|e, w, cx| e.cursor_line(true, true, true, w, cx))),
    );
    k.bind(
        "Home",
        v(|e, w, cx| e.line_boundary(false, false, w, cx)),
        Some(v(|e, w, cx| e.line_boundary(false, true, w, cx))),
    );
    k.bind(
        "End",
        v(|e, w, cx| e.line_boundary(true, false, w, cx)),
        Some(v(|e, w, cx| e.line_boundary(true, true, w, cx))),
    );
    k.bind("Mod-Home", s(cmds::cursor_doc_start), Some(s(cmds::select_doc_start)));
    k.bind("Mod-End", s(cmds::cursor_doc_end), Some(s(cmds::select_doc_end)));
    k.bind("Enter", s(cmds::insert_newline_and_indent), Some(s(cmds::insert_newline_and_indent)));
    k.bind("Mod-a", s(cmds::select_all), None);
    k.bind("Backspace", s(cmds::delete_char_backward), Some(s(cmds::delete_char_backward)));
    k.bind("Delete", s(cmds::delete_char_forward), None);
    k.bind("Mod-Backspace", s(cmds::delete_group_backward), None);
    k.bind("Mod-Delete", s(cmds::delete_group_forward), None);
    // defaultKeymap
    k.bind("Alt-ArrowUp", s(cmds::move_line_up), None);
    k.bind("Shift-Alt-ArrowUp", s(cmds::copy_line_up), None);
    k.bind("Alt-ArrowDown", s(cmds::move_line_down), None);
    k.bind("Shift-Alt-ArrowDown", s(cmds::copy_line_down), None);
    k.bind("Mod-Alt-ArrowUp", v(|e, w, cx| e.add_cursor(false, w, cx)), None);
    k.bind("Mod-Alt-ArrowDown", v(|e, w, cx| e.add_cursor(true, w, cx)), None);
    k.bind("Escape", s(cmds::simplify_selection), None);
    k.bind("Mod-Enter", s(cmds::insert_blank_line), None);
    k.bind("Alt-l", s(cmds::select_line), None);
    k.bind("Mod-[", s(cmds::indent_less), None);
    k.bind("Mod-]", s(cmds::indent_more), None);
    k.bind("Mod-Alt-\\", s(cmds::indent_selection), None);
    k.bind("Shift-Mod-k", v(|e, w, cx| e.delete_line(w, cx)), None);
    k.bind("Shift-Mod-\\", s(cmds::cursor_matching_bracket), None);
    k.bind("Mod-/", s(cmds::toggle_comment), None);
    k.bind("Alt-A", s(cmds::toggle_block_comment), None);
    // historyKeymap
    k.bind("Mod-z", s(cmds::undo), None);
    k.bind("Mod-y", s(cmds::redo), None);
    k.bind("Mod-Shift-z", s(cmds::redo), None);
    k.bind("Mod-u", s(cmds::undo_selection), None);
    k.bind("Alt-u", s(cmds::redo_selection), None);
    // Tab inserts a tab, or indents a selection; Shift-Tab dedents
    k.bind("Tab", s(cmds::insert_tab), Some(s(cmds::indent_less)));
    // The clipboard (handled by the browser in CodeMirror)
    k.bind("Mod-c", v(|e, _, cx| e.copy(false, cx)), None);
    k.bind("Mod-x", v(|e, _, cx| e.copy(true, cx)), None);
    k.bind("Mod-v", v(|e, _, cx| e.paste(cx)), None);
    k.bind("Shift-Delete", v(|e, _, cx| e.copy(true, cx)), None);
    k.bind("Shift-Insert", v(|e, _, cx| e.paste(cx)), None);
    k.bind("Mod-Insert", v(|e, _, cx| e.copy(false, cx)), None);
    k
}
