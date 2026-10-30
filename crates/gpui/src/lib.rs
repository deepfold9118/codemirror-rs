//! Spike: an editor view for the Rust CodeMirror port, drawn with
//! [GPUI](https://gpui.rs), Zed's UI framework.
//!
//! It runs the same `codemirror-state` and `codemirror-commands` as the
//! egui view, with GPUI doing text shaping (DirectWrite on Windows), line
//! layout caching, input and IME. Not ported to this view yet: decorations,
//! view plugins, update listeners, subword motion, and CodeMirror's height
//! map (a simpler line-height index stands in).

#![allow(clippy::type_complexity)]

mod element;
mod heights;
mod input;
mod keys;
mod layout;
mod view;

pub use view::{Cmd, EditorStyle, EditorView};
