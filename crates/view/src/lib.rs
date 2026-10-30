//! A Rust port of [`@codemirror/view`](https://codemirror.net/docs/ref/#view),
//! drawn with [egui](https://github.com/emilk/egui).
//!
//! The DOM-based parts of CodeMirror (the tile tree, DOM observation and
//! reading) are replaced by an immediate-mode [`EditorView::ui`] that lays
//! out the lines in view with egui each frame, measures them into the
//! height map, and paints text, selections, carets and the gutter. The
//! state-facing parts (view plugins, view updates, decorations, the height
//! map, key bindings, cursor motion) follow CodeMirror.
//!
//! Not ported yet: drawing widgets and replaced ranges (the height map
//! already accounts for them), bidirectional text, atomic ranges, panels,
//! tooltips, the generic gutter API (line numbers are built in), drag and
//! drop, and scaling for documents taller than about 16 million points.
//!
//! Ported from CodeMirror, copyright (C) 2018-2021 by Marijn Haverbeke
//! <marijn@haverbeke.berlin> and others, MIT license.

#![allow(clippy::type_complexity)]

mod cursor;
mod decoration;
mod editorview;
mod extension;
mod heightmap;
mod input;
mod keymap;
mod layout;
mod theme;
mod viewstate;

pub use cursor::{group_at, ByChar, PosAssoc};
pub use decoration::{
    BlockType, Decoration, DecorationSet, LineSpec, MarkSpec, MarkStyle, ReplaceSpec, WidgetSpec, WidgetType,
};
pub use editorview::{
    highlight_active_line, line_numbers, line_wrapping, EditorView, HIGHLIGHT_ACTIVE_LINE, LINE_NUMBERS,
};
pub use extension::{
    scroll_target, Cmd, Command, DecorationSource, PluginValue, ScrollStrategy, ScrollTarget, UpdateListener,
    ViewPlugin, ViewUpdate, DECORATIONS, EDITABLE, LINE_WRAPPING, SCROLL_INTO_VIEW, THEME, UPDATE_LISTENER,
};
pub use heightmap::BlockInfo;
pub use keymap::{keymap, run_scope_handlers, KeyBinding, KEYMAP};
pub use theme::Theme;
pub use viewstate::Viewport;
