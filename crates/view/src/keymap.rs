//! Key bindings. Port of `keymap.ts`, reading egui key events.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use codemirror_state::{EditorState, Extension, Facet, StateCommand};
use egui::{Key, Modifiers};

use crate::editorview::EditorView;
use crate::extension::{Cmd, Command};

/// Key bindings associate key names with [commands](Cmd).
///
/// Key names are strings like `"Shift-Ctrl-Enter"`: a key identifier
/// prefixed with zero or more modifiers. Key identifiers are based on the
/// strings that can appear in `KeyboardEvent.key` in browsers: `"a"`,
/// `"ArrowLeft"`, `"Enter"`, `"["`. Use lowercase letters to refer to
/// letter keys (or uppercase letters if you want shift to be held). You
/// may use `"Space"` as an alias for `" "`.
///
/// Modifiers can be given in any order. `Shift-` (or `s-`), `Alt-` (or
/// `a-`), `Ctrl-` (or `c-` or `Control-`) and `Cmd-` (or `m-` or `Meta-`)
/// are recognized. `Mod-` is `Cmd-` on macOS and `Ctrl-` elsewhere.
///
/// When a key binding contains multiple key names separated by spaces, it
/// represents a multi-stroke binding, which fires when the user presses
/// the given keys after each other.
#[derive(Clone, Default)]
pub struct KeyBinding {
    /// The key name to use for this binding. A platform-specific name
    /// (`mac`, `win`, `linux`) takes precedence on that platform.
    pub key: Option<String>,
    /// Key to use on macOS.
    pub mac: Option<String>,
    /// Key to use on Windows.
    pub win: Option<String>,
    /// Key to use on Linux.
    pub linux: Option<String>,
    /// The command to run. When it returns `false`, further bindings are
    /// tried for the key.
    pub run: Option<Cmd>,
    /// A second binding, on the key name prefixed with `Shift-`.
    pub shift: Option<Cmd>,
    /// The scopes the binding applies in, separated by spaces. Defaults to
    /// `"editor"`.
    pub scope: Option<String>,
    /// Always treat the key as handled, even if the commands return false.
    pub prevent_default: bool,
    /// Stop other handlers when the key is handled.
    pub stop_propagation: bool,
}

impl KeyBinding {
    /// A binding for the given key name.
    pub fn new(key: &str) -> KeyBinding {
        KeyBinding { key: Some(key.to_string()), ..Default::default() }
    }

    /// A binding that only applies on macOS.
    pub fn mac_only(key: &str) -> KeyBinding {
        KeyBinding { mac: Some(key.to_string()), ..Default::default() }
    }

    /// A binding that only applies on Linux.
    pub fn linux_only(key: &str) -> KeyBinding {
        KeyBinding { linux: Some(key.to_string()), ..Default::default() }
    }

    /// Use a different key name on macOS.
    pub fn mac(mut self, key: &str) -> KeyBinding {
        self.mac = Some(key.to_string());
        self
    }

    /// Use a different key name on Windows.
    pub fn win(mut self, key: &str) -> KeyBinding {
        self.win = Some(key.to_string());
        self
    }

    /// Run a view command.
    pub fn run(mut self, cmd: Command) -> KeyBinding {
        self.run = Some(Cmd::View(cmd));
        self
    }

    /// Run a state command.
    pub fn run_state(mut self, cmd: StateCommand) -> KeyBinding {
        self.run = Some(Cmd::State(cmd));
        self
    }

    /// Run a view command with Shift held.
    pub fn shift(mut self, cmd: Command) -> KeyBinding {
        self.shift = Some(Cmd::View(cmd));
        self
    }

    /// Run a state command with Shift held.
    pub fn shift_state(mut self, cmd: StateCommand) -> KeyBinding {
        self.shift = Some(Cmd::State(cmd));
        self
    }

    /// Always treat the key as handled.
    pub fn prevent_default(mut self) -> KeyBinding {
        self.prevent_default = true;
        self
    }

    /// Set the scope.
    pub fn scope(mut self, scope: &str) -> KeyBinding {
        self.scope = Some(scope.to_string());
        self
    }
}

/// Facet used for registering keymaps. Keymaps with higher precedence (or
/// given earlier) are checked first. When a handler has returned `true`
/// for a given key, no further handlers are called.
pub static KEYMAP: LazyLock<Facet<Arc<Vec<KeyBinding>>>> = LazyLock::new(Facet::list);

/// Create an extension that adds the given key bindings.
pub fn keymap(bindings: Vec<KeyBinding>) -> Extension {
    KEYMAP.of(Arc::new(bindings))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Platform {
    Mac,
    Win,
    Linux,
    Other,
}

pub(crate) const PLATFORM: Platform = if cfg!(target_os = "macos") {
    Platform::Mac
} else if cfg!(target_os = "windows") {
    Platform::Win
} else if cfg!(target_os = "linux") {
    Platform::Linux
} else {
    Platform::Other
};

pub(crate) fn normalize_key_name(name: &str, platform: Platform) -> String {
    // Split on dashes, except a trailing one (the "-" key).
    let (mods, key) = if name.len() > 1 && name.ends_with("--") {
        (&name[..name.len() - 2], "-")
    } else if name == "-" {
        ("", "-")
    } else {
        match name.rfind('-') {
            Some(i) => (&name[..i], &name[i + 1..]),
            None => ("", name),
        }
    };
    let mut result = if key == "Space" { " ".to_string() } else { key.to_string() };
    let (mut alt, mut ctrl, mut shift, mut meta) = (false, false, false, false);
    if !mods.is_empty() {
        for m in mods.split('-') {
            match m.to_ascii_lowercase().as_str() {
                "cmd" | "meta" | "m" => meta = true,
                "a" | "alt" => alt = true,
                "c" | "ctrl" | "control" => ctrl = true,
                "s" | "shift" => shift = true,
                "mod" => {
                    if platform == Platform::Mac {
                        meta = true
                    } else {
                        ctrl = true
                    }
                }
                _ => panic!("Unrecognized modifier name: {m}"),
            }
        }
    }
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

fn modifiers(name: &str, mods: Modifiers, shift: bool) -> String {
    let mut name = name.to_string();
    if mods.alt {
        name = format!("Alt-{name}");
    }
    if mods.ctrl {
        name = format!("Ctrl-{name}");
    }
    if mods.mac_cmd {
        name = format!("Meta-{name}");
    }
    if shift && mods.shift {
        name = format!("Shift-{name}");
    }
    name
}

/// The key name (as in `KeyboardEvent.key`, without shift applied) for an
/// egui key.
pub(crate) fn key_name(key: Key) -> String {
    match key {
        Key::ArrowLeft => "ArrowLeft".into(),
        Key::ArrowRight => "ArrowRight".into(),
        Key::ArrowUp => "ArrowUp".into(),
        Key::ArrowDown => "ArrowDown".into(),
        Key::Space => " ".into(),
        Key::Minus => "-".into(),
        Key::PageUp => "PageUp".into(),
        Key::PageDown => "PageDown".into(),
        Key::Quote => "'".into(),
        _ => {
            let s = key.symbol_or_name();
            if s.chars().count() == 1 {
                s.to_lowercase()
            } else {
                s.to_string()
            }
        }
    }
}

#[derive(Clone)]
enum Action {
    Cmd(Cmd),
    Prefix(String),
}

#[derive(Clone, Default)]
struct Binding {
    prevent_default: bool,
    stop_propagation: bool,
    run: Vec<Action>,
}

type ScopeMap = HashMap<String, Binding>;

/// A built keymap: scope → key → binding.
pub(crate) struct Keymap {
    scopes: HashMap<String, ScopeMap>,
}

pub(crate) fn build_keymap(bindings: &[KeyBinding], platform: Platform) -> Keymap {
    let mut bound: HashMap<String, ScopeMap> = HashMap::new();
    let mut is_prefix: HashMap<String, bool> = HashMap::new();
    let mut check_prefix = |name: &str, is: bool| match is_prefix.get(name) {
        None => {
            is_prefix.insert(name.to_string(), is);
        }
        Some(current) if *current != is => {
            panic!("Key binding {name} is used both as a regular binding and as a multi-stroke prefix")
        }
        _ => {}
    };
    let mut add = |scope: &str, key: &str, command: Option<Cmd>, prevent_default: bool, stop_propagation: bool| {
        let scope_obj = bound.entry(scope.to_string()).or_default();
        let parts: Vec<String> = split_strokes(key).iter().map(|k| normalize_key_name(k, platform)).collect();
        for i in 1..parts.len() {
            let prefix = parts[..i].join(" ");
            check_prefix(&prefix, true);
            scope_obj.entry(prefix.clone()).or_insert_with(|| Binding {
                prevent_default: true,
                stop_propagation: false,
                run: vec![Action::Prefix(prefix)],
            });
        }
        let full = parts.join(" ");
        check_prefix(&full, false);
        let binding = scope_obj.entry(full).or_default();
        if let Some(c) = command {
            binding.run.push(Action::Cmd(c));
        }
        if prevent_default {
            binding.prevent_default = true;
        }
        if stop_propagation {
            binding.stop_propagation = true;
        }
    };
    for b in bindings {
        let scopes: Vec<&str> = b.scope.as_deref().map_or(vec!["editor"], |s| s.split(' ').collect());
        let platform_name = match platform {
            Platform::Mac => b.mac.as_ref(),
            Platform::Win => b.win.as_ref(),
            Platform::Linux => b.linux.as_ref(),
            Platform::Other => None,
        };
        let Some(name) = platform_name.or(b.key.as_ref()) else { continue };
        for scope in scopes {
            add(scope, name, b.run, b.prevent_default, b.stop_propagation);
            if b.shift.is_some() {
                add(scope, &format!("Shift-{name}"), b.shift, b.prevent_default, b.stop_propagation);
            }
        }
    }
    Keymap { scopes: bound }
}

// Split a binding into strokes on spaces, except a trailing space (the
// space key).
fn split_strokes(key: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let bytes = key.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b' ' && i + 1 < bytes.len() && i > start {
            parts.push(&key[start..i]);
            start = i + 1;
        }
    }
    parts.push(&key[start..]);
    parts
}

const PREFIX_TIMEOUT: f64 = 4.0;

/// Caches the built keymap for a view, and holds a pending multi-stroke
/// prefix.
#[derive(Default)]
pub(crate) struct KeymapCache {
    source: Vec<Arc<Vec<KeyBinding>>>,
    built: Option<Arc<Keymap>>,
    stored_prefix: Option<(String, String, f64)>,
}

impl KeymapCache {
    fn get(&mut self, state: &EditorState) -> Arc<Keymap> {
        let maps = state.facet(&KEYMAP);
        let same = self.built.is_some()
            && maps.len() == self.source.len()
            && maps.iter().zip(&self.source).all(|(a, b)| Arc::ptr_eq(a, b));
        if !same {
            let all: Vec<KeyBinding> = maps.iter().flat_map(|m| m.iter().cloned()).collect();
            self.built = Some(Arc::new(build_keymap(&all, PLATFORM)));
            self.source = maps.clone();
        }
        self.built.clone().unwrap()
    }

    /// Which navigation keys the editor should keep from egui's focus
    /// handling.
    pub(crate) fn event_filter(&mut self, state: &EditorState) -> egui::EventFilter {
        let map = self.get(state);
        let has = |k: &str| map.scopes.get("editor").is_some_and(|s| s.contains_key(k));
        egui::EventFilter {
            tab: has("Tab") || has("Shift-Tab"),
            horizontal_arrows: true,
            vertical_arrows: true,
            escape: has("Escape"),
        }
    }
}

/// Run the key handlers registered for a given scope. Returns true if any
/// of the handlers handled the key.
pub fn run_scope_handlers(view: &mut EditorView, key: Key, mods: Modifiers, scope: &str) -> bool {
    let state = view.state().clone();
    let map = view.keymaps.get(&state);
    let now = view.now();
    let name = key_name(key);
    let is_char = name.chars().count() == 1 && name != " ";
    let mut prefix = String::new();
    let mut prevented = false;
    if let Some((stored, stored_scope, time)) = view.keymaps.stored_prefix.take() {
        if stored_scope == scope && now - time < PREFIX_TIMEOUT {
            prefix = format!("{stored} ");
            prevented = true;
        }
    }
    let Some(scope_obj) = map.scopes.get(scope) else { return prevented };

    let mut ran: Vec<Cmd> = Vec::new();
    let mut run_for = |view: &mut EditorView, key: &str| -> bool {
        let Some(binding) = scope_obj.get(key) else { return false };
        for action in &binding.run {
            match action {
                Action::Prefix(p) => {
                    view.keymaps.stored_prefix = Some((p.clone(), scope.to_string(), now));
                    return true;
                }
                Action::Cmd(cmd) => {
                    if ran.iter().any(|c| c.same(*cmd)) {
                        continue;
                    }
                    ran.push(*cmd);
                    if cmd.run(view) {
                        return true;
                    }
                }
            }
        }
        if binding.prevent_default {
            prevented = true;
        }
        false
    };

    // The name of a character key as typed: egui reports the base key, so
    // shifted letters become uppercase and other shifted keys are unknown.
    let typed = if is_char && mods.shift {
        if name.chars().all(|c| c.is_ascii_alphabetic()) {
            Some(name.to_uppercase())
        } else {
            None
        }
    } else {
        Some(name.clone())
    };
    let mut handled = false;
    if let Some(typed) = &typed {
        if run_for(view, &format!("{prefix}{}", modifiers(typed, mods, !is_char))) {
            handled = true;
        }
    }
    if !handled
        && is_char
        && (mods.alt || mods.mac_cmd || mods.ctrl)
        // Ctrl-Alt may be used for AltGr on Windows.
        && !(PLATFORM == Platform::Win && mods.ctrl && mods.alt)
        // Alt combinations on macOS tend to be typed characters.
        && !(PLATFORM == Platform::Mac && mods.alt && !(mods.ctrl || mods.mac_cmd))
        && typed.as_deref() != Some(name.as_str())
    {
        if run_for(view, &format!("{prefix}{}", modifiers(&name, mods, true))) {
            handled = true;
        }
    } else if !handled && is_char && mods.shift {
        if let Some(typed) = &typed {
            if run_for(view, &format!("{prefix}{}", modifiers(typed, mods, true))) {
                handled = true;
            }
        }
    }
    handled || prevented
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_key_names() {
        assert_eq!(normalize_key_name("Mod-z", Platform::Win), "Ctrl-z");
        assert_eq!(normalize_key_name("Mod-z", Platform::Mac), "Meta-z");
        assert_eq!(normalize_key_name("Shift-Mod-k", Platform::Win), "Shift-Ctrl-k");
        assert_eq!(normalize_key_name("Ctrl-Alt-Shift-x", Platform::Win), "Shift-Ctrl-Alt-x");
        assert_eq!(normalize_key_name("Ctrl--", Platform::Win), "Ctrl--");
        assert_eq!(normalize_key_name("Space", Platform::Win), " ");
        assert_eq!(split_strokes("Ctrl-k Ctrl-c"), ["Ctrl-k", "Ctrl-c"]);
        assert_eq!(split_strokes("Ctrl- "), ["Ctrl- "]);
    }

    #[test]
    fn names_egui_keys() {
        assert_eq!(key_name(Key::A), "a");
        assert_eq!(key_name(Key::ArrowLeft), "ArrowLeft");
        assert_eq!(key_name(Key::OpenBracket), "[");
        assert_eq!(key_name(Key::Backslash), "\\");
        assert_eq!(key_name(Key::Num1), "1");
        assert_eq!(key_name(Key::Enter), "Enter");
        assert_eq!(key_name(Key::Home), "Home");
        assert_eq!(key_name(Key::F5), "F5");
    }
}
