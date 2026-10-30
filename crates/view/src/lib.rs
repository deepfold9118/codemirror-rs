//! A Rust port of [`@codemirror/view`](https://codemirror.net/docs/ref/#view),
//! drawn with [GPUI](https://gpui.rs), Zed's UI framework.
//!
//! The DOM-based parts of CodeMirror (the tile tree, DOM observation and
//! reading) are replaced by a GPUI element that lays out the lines in view
//! each frame (GPUI caches line layouts between frames), measures them into
//! the height map, and paints text, selections, carets and the gutter.
//! Text input and IME composition go through GPUI's input handler. The
//! state-facing parts (view plugins, view updates, decorations, the height
//! map, key bindings, cursor motion) follow CodeMirror.
//!
//! Not ported yet: drawing widgets and replaced ranges (the height map
//! already accounts for them), bidirectional text, atomic ranges, panels,
//! tooltips, the generic gutter API (line numbers are built in), and drag
//! and drop.
//!
//! Ported from CodeMirror, copyright (C) 2018-2021 by Marijn Haverbeke
//! <marijn@haverbeke.berlin> and others, MIT license.

#![allow(clippy::type_complexity)]

mod cursor;
mod decoration;
mod editorview;
mod element;
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
