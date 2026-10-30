//! The search state, find and replace commands, match highlighting and key
//! bindings (`search.ts`).
//!
//! CodeMirror draws its search panel with the DOM. This port keeps the
//! panel's state (whether it is open, and the query) in the editor state,
//! and leaves drawing the panel to the host application: show it while
//! [`search_panel_open`] is true, read the query with [`get_search_query`],
//! and change it with the [`SET_SEARCH_QUERY`] effect. CodeMirror's screen
//! reader announcements are not ported.
//!
//! The commands are state commands, so they also work outside a view.

use std::sync::{Arc, LazyLock};

use codemirror_state::{
    ChangeSpec, EditorSelection, EditorState, Extension, Facet, FacetSpec, Prec, SelectionRange, StateCommand,
    StateEffect, StateEffectType, StateField, StateFieldSpec, Transaction, TransactionSpec,
};
use codemirror_view::{
    scroll_target, Decoration, DecorationSet, EditorView, KeyBinding, MarkSpec, MarkStyle, PluginValue, Theme,
    ViewPlugin, ViewUpdate, Viewport, SCROLL_INTO_VIEW, THEME,
};
use gpui::{rgba, Hsla};

use crate::query::{QueryMatch, SearchQuery, SearchQueryConfig};
use crate::selection_match::select_next_occurrence;

type Dispatch<'a> = &'a mut dyn FnMut(Transaction);

/// Makes the effect that scrolls a match into view.
pub type ScrollToMatch = Arc<dyn Fn(SelectionRange) -> StateEffect + Send + Sync>;

/// Configuration for [`search`]. Unset fields take their defaults.
#[derive(Clone, Default)]
pub struct SearchConfig {
    /// Whether the search panel goes at the top of the editor (the
    /// default is the bottom). Only a hint to the host that draws it.
    pub top: Option<bool>,
    /// Whether the default query is case sensitive (default false).
    pub case_sensitive: Option<bool>,
    /// Whether the default query is literal (default false).
    pub literal: Option<bool>,
    /// Whether the default query matches whole words (default false).
    pub whole_word: Option<bool>,
    /// Whether the default query is a regular expression (default false).
    pub regexp: Option<bool>,
    /// The background of matches (CodeMirror's `cm-searchMatch`).
    pub match_background: Option<Hsla>,
    /// The background of the selected match.
    pub selected_match_background: Option<Hsla>,
    /// Produces the effect that scrolls a match into view. The default
    /// scrolls it into view the nearest way.
    pub scroll_to_match: Option<ScrollToMatch>,
}

/// The combined search configuration.
#[derive(Clone)]
pub struct ResolvedSearchConfig {
    pub top: bool,
    pub case_sensitive: bool,
    pub literal: bool,
    pub whole_word: bool,
    pub regexp: bool,
    pub match_background: Option<Hsla>,
    pub selected_match_background: Option<Hsla>,
    pub scroll_to_match: ScrollToMatch,
}

static SEARCH_CONFIG: LazyLock<Facet<SearchConfig, ResolvedSearchConfig>> = LazyLock::new(|| {
    Facet::define(FacetSpec::combine(|configs: &[SearchConfig]| {
        fn first<T: Clone>(configs: &[SearchConfig], f: impl Fn(&SearchConfig) -> Option<T>) -> Option<T> {
            configs.iter().find_map(f)
        }
        ResolvedSearchConfig {
            top: first(configs, |c| c.top).unwrap_or(false),
            case_sensitive: first(configs, |c| c.case_sensitive).unwrap_or(false),
            literal: first(configs, |c| c.literal).unwrap_or(false),
            whole_word: first(configs, |c| c.whole_word).unwrap_or(false),
            regexp: first(configs, |c| c.regexp).unwrap_or(false),
            match_background: first(configs, |c| c.match_background),
            selected_match_background: first(configs, |c| c.selected_match_background),
            scroll_to_match: first(configs, |c| c.scroll_to_match.clone())
                .unwrap_or_else(|| Arc::new(|range| SCROLL_INTO_VIEW.of(scroll_target(range)))),
        }
    }))
});

/// Add search state to the editor configuration, and optionally configure
/// the search extension. ([`open_search_panel`] enables it when it isn't
/// on yet.)
pub fn search(config: Option<SearchConfig>) -> Extension {
    match config {
        Some(config) => Extension::list([SEARCH_CONFIG.of(config), search_extensions()]),
        None => search_extensions(),
    }
}

fn search_extensions() -> Extension {
    Extension::list([Extension::from(&*SEARCH_STATE), Prec::low(SEARCH_HIGHLIGHTER.extension())])
}

/// A state effect that updates the current search query. It only has an
/// effect when the search state is in the editor (through [`search`] or a
/// first [`open_search_panel`]).
pub static SET_SEARCH_QUERY: LazyLock<StateEffectType<SearchQuery>> = LazyLock::new(StateEffectType::define);

static TOGGLE_PANEL: LazyLock<StateEffectType<bool>> = LazyLock::new(StateEffectType::define);

#[derive(Clone)]
struct SearchState {
    query: SearchQuery,
    panel: bool,
}

impl PartialEq for SearchState {
    fn eq(&self, other: &SearchState) -> bool {
        self.panel == other.panel && self.query == other.query
    }
}

static SEARCH_STATE: LazyLock<StateField<SearchState>> = LazyLock::new(|| {
    StateField::define(StateFieldSpec::new(
        |state| SearchState { query: default_query(state, None), panel: false },
        |value: &SearchState, tr: &Transaction| {
            let mut value = value.clone();
            for effect in tr.effects() {
                if let Some(query) = effect.value(&SET_SEARCH_QUERY) {
                    value.query = query.clone();
                } else if let Some(open) = effect.value(&TOGGLE_PANEL) {
                    value.panel = *open;
                }
            }
            value
        },
    ))
});

/// The current search query of an editor state.
pub fn get_search_query(state: &EditorState) -> SearchQuery {
    match state.field_opt(&SEARCH_STATE) {
        Some(s) => s.query.clone(),
        None => default_query(state, None),
    }
}

/// Whether the search panel is open in the given editor state.
pub fn search_panel_open(state: &EditorState) -> bool {
    state.field_opt(&SEARCH_STATE).is_some_and(|s| s.panel)
}

/// The configured placement of the search panel: true for the top.
pub fn search_panel_at_top(state: &EditorState) -> bool {
    state.facet(&SEARCH_CONFIG).top
}

// ---------- highlighting ----------

/// Highlights the matches of the query in the viewport while the panel is
/// open.
pub struct SearchHighlighter {
    decorations: DecorationSet,
}

impl PluginValue for SearchHighlighter {
    fn update(&mut self, update: &ViewUpdate) {
        let state = update.state.field_opt(&SEARCH_STATE);
        if state != update.start_state.field_opt(&SEARCH_STATE)
            || update.doc_changed()
            || update.selection_set()
            || update.viewport_changed()
        {
            self.decorations = highlight(&update.state, update.viewport);
        }
    }
}

static SEARCH_HIGHLIGHTER: LazyLock<ViewPlugin<SearchHighlighter>> = LazyLock::new(|| {
    ViewPlugin::define(|view: &EditorView| SearchHighlighter { decorations: highlight(view.state(), view.viewport()) })
        .decorations(|v| v.decorations.clone())
});

/// Whether the editor theme is dark (CodeMirror's `&dark` theme rules).
pub(crate) fn dark_theme(state: &EditorState) -> bool {
    let theme = state.facet(&THEME).clone().unwrap_or_else(|| Arc::new(Theme::dark()));
    theme.background.l < 0.5
}

fn mark(class: &str, background: Hsla) -> Decoration {
    Decoration::mark(MarkSpec {
        class: Some(class.into()),
        style: MarkStyle { background: Some(background), ..Default::default() },
        ..Default::default()
    })
}

fn highlight(state: &EditorState, viewport: Viewport) -> DecorationSet {
    let Some(SearchState { query, panel: true }) = state.field_opt(&SEARCH_STATE) else { return Decoration::none() };
    if !query.valid {
        return Decoration::none();
    }
    let config = state.facet(&SEARCH_CONFIG);
    let dark = dark_theme(state);
    let match_mark = mark(
        "cm-searchMatch",
        config.match_background.unwrap_or_else(|| if dark { rgba(0x00ffff8a) } else { rgba(0xffff0054) }.into()),
    );
    let selected_mark = mark(
        "cm-searchMatch cm-searchMatch-selected",
        config
            .selected_match_background
            .unwrap_or_else(|| if dark { rgba(0xff00ff8a) } else { rgba(0xff6a0054) }.into()),
    );
    let mut ranges = Vec::new();
    query.highlight(state, viewport.from, viewport.to, |from, to| {
        let selected = state.selection().ranges.iter().any(|r| r.from == from && r.to == to);
        ranges.push(if selected { &selected_mark } else { &match_mark }.range(from, to));
    });
    Decoration::set(ranges, true)
}

// ---------- commands ----------

fn search_command(
    state: &EditorState,
    dispatch: Dispatch,
    f: impl FnOnce(&EditorState, &SearchQuery, Dispatch) -> bool,
) -> bool {
    match state.field_opt(&SEARCH_STATE) {
        Some(s) if s.query.valid => f(state, &s.query, dispatch),
        _ => open_search_panel(state, dispatch),
    }
}

fn select_match(state: &EditorState, m: &QueryMatch, dispatch: Dispatch) {
    let selection = EditorSelection::single(m.from, m.to);
    let scroll = (state.facet(&SEARCH_CONFIG).scroll_to_match)(selection.main());
    dispatch(state.update(TransactionSpec::new().selection(selection).effect(scroll).user_event("select.search")));
}

/// Open the search panel if it isn't already open, and move the selection
/// to the first match after the current main selection. Wraps around to
/// the start of the document at the end.
pub fn find_next(state: &EditorState, dispatch: Dispatch) -> bool {
    search_command(state, dispatch, |state, query, dispatch| {
        let to = state.selection().main().to;
        let Some(next) = query.next_match(state, to, to) else { return false };
        select_match(state, &next, dispatch);
        true
    })
}

/// Move the selection to the previous instance of the search query, before
/// the current main selection. Wraps past the start of the document to the
/// end.
pub fn find_previous(state: &EditorState, dispatch: Dispatch) -> bool {
    search_command(state, dispatch, |state, query, dispatch| {
        let from = state.selection().main().from;
        let Some(prev) = query.prev_match(state, from, from) else { return false };
        select_match(state, &prev, dispatch);
        true
    })
}

/// Select all instances of the search query.
pub fn select_matches(state: &EditorState, dispatch: Dispatch) -> bool {
    search_command(state, dispatch, |state, query, dispatch| {
        let ranges = match query.match_all(state, 1000) {
            Some(ranges) if !ranges.is_empty() => ranges,
            _ => return false,
        };
        let selection = EditorSelection::create(ranges.iter().map(|r| EditorSelection::range(r.from, r.to)).collect(), 0);
        dispatch(state.update(TransactionSpec::new().selection(selection).user_event("select.search.matches")));
        true
    })
}

/// Select all instances of the currently selected text.
pub fn select_selection_matches(state: &EditorState, dispatch: Dispatch) -> bool {
    let sel = state.selection();
    let main_range = sel.main();
    if sel.ranges.len() > 1 || main_range.is_empty() {
        return false;
    }
    let query = state.slice_doc(main_range.from, main_range.to);
    let mut ranges = Vec::new();
    let mut main = 0;
    for m in crate::SearchCursor::new(state.doc(), &query, 0, None, None, None) {
        if ranges.len() > 1000 {
            return false;
        }
        if m.from == main_range.from {
            main = ranges.len();
        }
        ranges.push(EditorSelection::range(m.from, m.to));
    }
    dispatch(state.update(
        TransactionSpec::new()
            .selection(EditorSelection::create(ranges, main))
            .user_event("select.search.matches"),
    ));
    true
}

/// Replace the current match of the search query, and select the next.
pub fn replace_next(state: &EditorState, dispatch: Dispatch) -> bool {
    search_command(state, dispatch, |state, query, dispatch| {
        let main = state.selection().main();
        if state.read_only() {
            return false;
        }
        let Some(m) = query.next_match(state, main.from, main.from) else { return false };
        let mut changes = Vec::new();
        let next = if !m.precise {
            query.next_match(state, m.from, m.to)
        } else if m.from == main.from && m.to == main.to {
            changes.push(ChangeSpec::replace(m.from, m.to, state.to_text(&query.get_replacement(&m))));
            query.next_match(state, m.from, m.to)
        } else {
            Some(m)
        };
        let change_set = state.changes(changes);
        let mut spec = TransactionSpec::new().changes(change_set.clone()).user_event("input.replace");
        if let Some(next) = next {
            let selection = EditorSelection::single(next.from, next.to).map(change_set.desc(), -1);
            let scroll = (state.facet(&SEARCH_CONFIG).scroll_to_match)(selection.main());
            spec = spec.selection(selection).effect(scroll);
        }
        dispatch(state.update(spec));
        true
    })
}

/// Replace all instances of the search query with the replacement.
pub fn replace_all(state: &EditorState, dispatch: Dispatch) -> bool {
    search_command(state, dispatch, |state, query, dispatch| {
        if state.read_only() {
            return false;
        }
        let changes: Vec<ChangeSpec> = query
            .match_all(state, usize::MAX)
            .unwrap_or_default()
            .into_iter()
            .filter(|m| m.precise)
            .map(|m| ChangeSpec::replace(m.from, m.to, query.get_replacement(&m)))
            .collect();
        if changes.is_empty() {
            return false;
        }
        dispatch(state.update(TransactionSpec::new().changes(changes).user_event("input.replace.all")));
        true
    })
}

/// The query to open the panel with: the selected text (up to 100 bytes on
/// one range), else `fallback`, else an empty query.
fn default_query(state: &EditorState, fallback: Option<&SearchQuery>) -> SearchQuery {
    let sel = state.selection().main();
    let sel_text = if sel.is_empty() || sel.to > sel.from + 100 { String::new() } else { state.slice_doc(sel.from, sel.to) };
    if let Some(fallback) = fallback {
        if sel_text.is_empty() {
            return fallback.clone();
        }
    }
    let config = state.facet(&SEARCH_CONFIG);
    let literal = fallback.map_or(config.literal, |f| f.literal);
    SearchQuery::new(SearchQueryConfig {
        search: if literal { sel_text } else { sel_text.replace('\n', "\\n") },
        case_sensitive: fallback.map_or(config.case_sensitive, |f| f.case_sensitive),
        literal,
        regexp: fallback.map_or(config.regexp, |f| f.regexp),
        whole_word: fallback.map_or(config.whole_word, |f| f.whole_word),
        ..Default::default()
    })
}

/// Open the search panel, taking the query from the selection when text
/// is selected. When the panel is already open, only updates the query.
/// Focusing the panel's search field is up to the host that draws it.
pub fn open_search_panel(state: &EditorState, dispatch: Dispatch) -> bool {
    match state.field_opt(&SEARCH_STATE) {
        Some(current) if current.panel => {
            let query = default_query(state, Some(&current.query));
            if query.valid && query != current.query {
                dispatch(state.update(TransactionSpec::new().effect(SET_SEARCH_QUERY.of(query))));
            }
        }
        Some(current) => {
            let query = default_query(state, Some(&current.query));
            dispatch(state.update(
                TransactionSpec::new().effect(TOGGLE_PANEL.of(true)).effect(SET_SEARCH_QUERY.of(query)),
            ));
        }
        None => {
            // A field added by a transaction starts from its `create`
            // value, so the panel opens in a second transaction.
            let tr = state.update(TransactionSpec::new().effect(StateEffect::append_config(search_extensions())));
            let next = tr.state().clone();
            dispatch(tr);
            dispatch(next.update(TransactionSpec::new().effect(TOGGLE_PANEL.of(true))));
        }
    }
    true
}

/// Close the search panel.
pub fn close_search_panel(state: &EditorState, dispatch: Dispatch) -> bool {
    if !search_panel_open(state) {
        return false;
    }
    dispatch(state.update(TransactionSpec::new().effect(TOGGLE_PANEL.of(false))));
    true
}

/// The default search key bindings:
///
///  - Mod-f: [`open_search_panel`]
///  - F3, Mod-g: [`find_next`]
///  - Shift-F3, Shift-Mod-g: [`find_previous`]
///  - Escape: [`close_search_panel`]
///  - Mod-Shift-l: [`select_selection_matches`]
///  - Mod-d: [`select_next_occurrence`]
///
/// CodeMirror's Mod-Alt-g (go to line) opens a dialog, which this port
/// leaves to the host; see [`goto_line_selection`](crate::goto_line_selection).
pub fn search_keymap() -> Vec<KeyBinding> {
    let scope = "editor search-panel";
    let open: StateCommand = open_search_panel;
    vec![
        KeyBinding::new("Mod-f").run_state(open).scope(scope),
        KeyBinding::new("F3").run_state(find_next).shift_state(find_previous).scope(scope).prevent_default(),
        KeyBinding::new("Mod-g").run_state(find_next).shift_state(find_previous).scope(scope).prevent_default(),
        KeyBinding::new("Escape").run_state(close_search_panel).scope(scope),
        KeyBinding::new("Mod-Shift-l").run_state(select_selection_matches),
        KeyBinding::new("Mod-d").run_state(select_next_occurrence).prevent_default(),
    ]
}
