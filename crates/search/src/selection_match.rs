//! Port of `selection-match.ts`: highlighting the text that matches the
//! selection, and selecting the next occurrence.

use std::sync::LazyLock;

use codemirror_state::{
    CharCategory, EditorSelection, EditorState, Extension, Facet, FacetSpec, Transaction, TransactionSpec,
};
use codemirror_view::{
    scroll_target, Decoration, DecorationSet, EditorView, MarkSpec, MarkStyle, PluginValue, ViewPlugin, ViewUpdate,
    Viewport, SCROLL_INTO_VIEW,
};
use gpui::{rgba, Hsla};

use crate::cursor::SearchCursor;

type Categorizer = Box<dyn Fn(&str) -> CharCategory>;

/// Options for [`highlight_selection_matches`]. Unset fields take their
/// defaults.
#[derive(Clone, Default)]
pub struct HighlightOptions {
    /// When nothing is selected, highlight the word around the cursor.
    /// Defaults to false.
    pub highlight_word_around_cursor: Option<bool>,
    /// The minimum length of the selection before it is highlighted.
    /// Defaults to 1 (always highlight non-cursor selections).
    pub min_selection_length: Option<usize>,
    /// The number of matches (in the viewport) at which to disable
    /// highlighting. Defaults to 100.
    pub max_matches: Option<usize>,
    /// Whether to only highlight whole words.
    pub whole_words: Option<bool>,
    /// The match background (CodeMirror's `cm-selectionMatch`).
    pub background: Option<Hsla>,
}

#[derive(Clone)]
struct HighlightConfig {
    highlight_word_around_cursor: bool,
    min_selection_length: usize,
    max_matches: usize,
    whole_words: bool,
    background: Hsla,
}

static HIGHLIGHT_CONFIG: LazyLock<Facet<HighlightOptions, HighlightConfig>> = LazyLock::new(|| {
    Facet::define(FacetSpec::combine(|options: &[HighlightOptions]| HighlightConfig {
        highlight_word_around_cursor: options.iter().any(|o| o.highlight_word_around_cursor == Some(true)),
        min_selection_length: options.iter().filter_map(|o| o.min_selection_length).min().unwrap_or(1),
        max_matches: options.iter().filter_map(|o| o.max_matches).min().unwrap_or(100),
        whole_words: options.iter().find_map(|o| o.whole_words).unwrap_or(false),
        background: options.iter().find_map(|o| o.background).unwrap_or_else(|| rgba(0x99ff7780).into()),
    }))
});

/// Highlight the text that matches the selection (class
/// `cm-selectionMatch`). With `highlight_word_around_cursor`, the word at
/// the cursor itself gets `cm-selectionMatch-main`.
pub fn highlight_selection_matches(options: Option<HighlightOptions>) -> Extension {
    let mut exts = vec![MATCH_HIGHLIGHTER.extension()];
    if let Some(options) = options {
        exts.push(HIGHLIGHT_CONFIG.of(options));
    }
    Extension::list(exts)
}

/// Whether the characters directly outside the given positions are
/// non-word characters.
fn inside_word_boundaries(check: &dyn Fn(&str) -> CharCategory, state: &EditorState, from: usize, to: usize) -> bool {
    let doc = state.doc();
    (from == 0 || check(&crate::query::char_before(doc, from)) != CharCategory::Word)
        && (to == doc.len() || check(&crate::query::char_after(doc, to)) != CharCategory::Word)
}

/// Whether the characters directly at the given positions are word
/// characters.
fn inside_word(check: &dyn Fn(&str) -> CharCategory, state: &EditorState, from: usize, to: usize) -> bool {
    let doc = state.doc();
    check(&crate::query::char_after(doc, from)) == CharCategory::Word
        && check(&crate::query::char_before(doc, to)) == CharCategory::Word
}

/// The plugin behind [`highlight_selection_matches`].
pub struct MatchHighlighter {
    decorations: DecorationSet,
}

impl PluginValue for MatchHighlighter {
    fn update(&mut self, update: &ViewUpdate) {
        if update.selection_set() || update.doc_changed() || update.viewport_changed() {
            self.decorations = get_deco(&update.state, update.viewport);
        }
    }
}

static MATCH_HIGHLIGHTER: LazyLock<ViewPlugin<MatchHighlighter>> = LazyLock::new(|| {
    ViewPlugin::define(|view: &EditorView| MatchHighlighter { decorations: get_deco(view.state(), view.viewport()) })
        .decorations(|v| v.decorations.clone())
});

fn get_deco(state: &EditorState, viewport: Viewport) -> DecorationSet {
    let conf = state.facet(&HIGHLIGHT_CONFIG);
    let sel = state.selection();
    if sel.ranges.len() > 1 {
        return Decoration::none();
    }
    let range = sel.main();
    let mut check: Option<Categorizer> = None;
    let query = if range.is_empty() {
        if !conf.highlight_word_around_cursor {
            return Decoration::none();
        }
        let Some(word) = state.word_at(range.head()) else { return Decoration::none() };
        check = Some(Box::new(state.char_categorizer(range.head())));
        state.slice_doc(word.from, word.to)
    } else {
        let len = range.to - range.from;
        if len < conf.min_selection_length || len > 200 {
            return Decoration::none();
        }
        let query = state.slice_doc(range.from, range.to);
        if conf.whole_words {
            let cat = state.char_categorizer(range.head());
            if !(inside_word_boundaries(&cat, state, range.from, range.to)
                && inside_word(&cat, state, range.from, range.to))
            {
                return Decoration::none();
            }
            check = Some(Box::new(cat));
        } else if query.is_empty() {
            return Decoration::none();
        }
        query
    };
    let style = |class: &str| {
        Decoration::mark(MarkSpec {
            class: Some(class.into()),
            style: MarkStyle { background: Some(conf.background), ..Default::default() },
            ..Default::default()
        })
    };
    let (match_deco, main_deco) = (style("cm-selectionMatch"), style("cm-selectionMatch cm-selectionMatch-main"));
    let mut deco = Vec::new();
    for m in SearchCursor::new(state.doc(), &query, viewport.from, Some(viewport.to), None, None) {
        if check.as_ref().is_none_or(|c| inside_word_boundaries(c, state, m.from, m.to)) {
            if range.is_empty() && m.from <= range.from && m.to >= range.to {
                deco.push(main_deco.range(m.from, m.to));
            } else if m.from >= range.to || m.to <= range.from {
                deco.push(match_deco.range(m.from, m.to));
            }
            if deco.len() > conf.max_matches {
                return Decoration::none();
            }
        }
    }
    Decoration::set(deco, false)
}

/// Select the words around the cursors.
fn select_word(state: &EditorState, dispatch: &mut dyn FnMut(Transaction)) -> bool {
    let selection = state.selection();
    let ranges = selection
        .ranges
        .iter()
        .map(|r| state.word_at(r.head()).unwrap_or_else(|| EditorSelection::cursor(r.head())))
        .collect();
    let new_sel = EditorSelection::create(ranges, selection.main_index);
    if new_sel.eq(selection, false) {
        return false;
    }
    dispatch(state.update(TransactionSpec::new().selection(new_sel)));
    true
}

/// Find the next occurrence of `query` after the last cursor, wrapping
/// around the document.
fn find_next_occurrence(state: &EditorState, query: &str) -> Option<(usize, usize)> {
    let sel = state.selection();
    let (main, ranges) = (sel.main(), &sel.ranges);
    let last = ranges.last().expect("a selection has a range");
    let full_word = state.word_at(main.head()).is_some_and(|w| w.from == main.from && w.to == main.to);
    let mut cursor = SearchCursor::new(state.doc(), query, last.to, None, None, None);
    let mut cycled = false;
    loop {
        match cursor.next() {
            None => {
                if cycled {
                    return None;
                }
                cursor = SearchCursor::new(state.doc(), query, 0, Some(last.from.saturating_sub(1)), None, None);
                cycled = true;
            }
            Some(m) => {
                if cycled && ranges.iter().any(|r| r.from == m.from) {
                    continue;
                }
                if full_word {
                    let word = state.word_at(m.from);
                    if word.is_none_or(|w| w.from != m.from || w.to != m.to) {
                        continue;
                    }
                }
                return Some((m.from, m.to));
            }
        }
    }
}

/// Select the next occurrence of the current selection. Expands the
/// selection to the surrounding word when it is empty.
pub fn select_next_occurrence(state: &EditorState, dispatch: &mut dyn FnMut(Transaction)) -> bool {
    let ranges = &state.selection().ranges;
    if ranges.iter().any(|r| r.from == r.to) {
        return select_word(state, dispatch);
    }
    let searched = state.slice_doc(ranges[0].from, ranges[0].to);
    if ranges.iter().any(|r| state.slice_doc(r.from, r.to) != searched) {
        return false;
    }
    let Some((from, to)) = find_next_occurrence(state, &searched) else { return false };
    let selection = state.selection().add_range(EditorSelection::range(from, to), false);
    let scroll = SCROLL_INTO_VIEW.of(scroll_target(EditorSelection::cursor(to)));
    dispatch(state.update(TransactionSpec::new().selection(selection).effect(scroll)));
    true
}
