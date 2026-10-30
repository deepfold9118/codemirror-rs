//! Port of CodeMirror's `test/state.ts`: build a state from a document with
//! `|` (cursor) and `<...>` (range) markers, and print a state back in the
//! same notation.

#![allow(dead_code)]

use codemirror_state::{
    EditorSelection, EditorState, EditorStateConfig, Extension, Transaction, ALLOW_MULTIPLE_SELECTIONS,
};

pub fn mk_state(doc: &str, extensions: Extension) -> EditorState {
    let mut text = String::new();
    let mut ranges = Vec::new();
    let mut rest = doc;
    while let Some(i) = rest.find(['|', '<']) {
        text.push_str(&rest[..i]);
        if rest.as_bytes()[i] == b'|' {
            ranges.push(EditorSelection::cursor(text.len()));
            rest = &rest[i + 1..];
        } else {
            let close = rest[i..].find('>').expect("unclosed range marker") + i;
            let from = text.len();
            text.push_str(&rest[i + 1..close]);
            ranges.push(EditorSelection::undirectional_range(from, text.len()));
            rest = &rest[close + 1..];
        }
    }
    text.push_str(rest);
    let mut config = EditorStateConfig::new()
        .doc(text)
        .extensions(Extension::list([extensions, ALLOW_MULTIPLE_SELECTIONS.of(true)]));
    if !ranges.is_empty() {
        config = config.selection(EditorSelection::create(ranges, 0));
    }
    EditorState::create(config)
}

pub fn state_str(state: &EditorState) -> String {
    let mut doc = state.doc_string();
    for range in state.selection().ranges.iter().rev() {
        if range.is_empty() {
            doc.insert(range.from, '|');
        } else {
            doc.insert(range.to, '>');
            doc.insert(range.from, '<');
        }
    }
    doc
}

/// Run a command and return the state it dispatched (or the original
/// state), plus whether it applied.
pub fn run(state: &EditorState, command: codemirror_state::StateCommand) -> (EditorState, bool) {
    let mut result = state.clone();
    let applied = command(state, &mut |tr: Transaction| result = tr.state().clone());
    (result, applied)
}
