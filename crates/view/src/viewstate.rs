//! Vertical layout bookkeeping: the height map, the viewport and pending
//! scroll targets. A reduced port of `viewstate.ts`; the measuring itself
//! happens in [`EditorView::ui`](crate::EditorView::ui).

use codemirror_state::{EditorSelection, EditorState, Text};

use crate::decoration::DecorationSet;
use crate::extension::{
    update_flag, ChangedRange, DecorationSource, ScrollTarget, ViewUpdate, DECORATIONS, LINE_WRAPPING, SCROLL_INTO_VIEW,
};
use crate::heightmap::{clear_height_change_flag, height_change_flag, height_relevant_deco_changes};
use crate::heightmap::{HeightMap, HeightOracle};

/// A range of the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Viewport {
    pub from: usize,
    pub to: usize,
}

/// The decoration sets that come straight from the state (not from
/// functions of the view). Only these can change the vertical layout.
pub(crate) fn static_deco(state: &EditorState) -> Vec<DecorationSet> {
    state
        .facet(&DECORATIONS)
        .iter()
        .filter_map(|s| match s {
            DecorationSource::Set(set) => Some(set.clone()),
            DecorationSource::Dynamic(_) => None,
        })
        .collect()
}

pub(crate) struct ViewState {
    pub oracle: HeightOracle,
    pub height_map: Box<HeightMap>,
    pub state_deco: Vec<DecorationSet>,
    pub viewport: Viewport,
    pub scroll_target: Option<ScrollTarget>,
}

impl ViewState {
    pub fn new(state: &EditorState) -> ViewState {
        let mut oracle = HeightOracle::new(*state.facet(&LINE_WRAPPING));
        oracle.set_doc(state.doc());
        let state_deco = static_deco(state);
        let height_map = HeightMap::empty().apply_changes(
            &state_deco,
            &Text::empty(),
            &mut oracle,
            &[ChangedRange::new(0, 0, 0, state.doc().len())],
        );
        ViewState { oracle, height_map, state_deco, viewport: Viewport::default(), scroll_target: None }
    }

    pub fn update(&mut self, update: &mut ViewUpdate) {
        let prev_deco = std::mem::replace(&mut self.state_deco, static_deco(&update.state));
        let height_changes = ChangedRange::extend_with_ranges(
            &update.changed_ranges,
            &height_relevant_deco_changes(&prev_deco, &self.state_deco, update.changes.desc()),
        );
        let prev_height = self.height_map.height();
        clear_height_change_flag();
        self.oracle.set_doc(update.state.doc());
        self.oracle.line_wrapping = *update.state.facet(&LINE_WRAPPING);
        let map = std::mem::replace(&mut self.height_map, HeightMap::empty());
        self.height_map =
            map.apply_changes(&self.state_deco, update.start_state.doc(), &mut self.oracle, &height_changes);
        if self.height_map.height() != prev_height || height_change_flag() {
            update.flags |= update_flag::HEIGHT;
        }
        let mapped = Viewport {
            from: update.changes.map_pos(self.viewport.from, -1),
            to: update.changes.map_pos(self.viewport.to, 1),
        };
        if mapped != self.viewport {
            update.flags |= update_flag::VIEWPORT;
            self.viewport = mapped;
        }

        for tr in &update.transactions {
            if let Some(target) = &self.scroll_target {
                self.scroll_target = Some(target.map(tr.changes().desc()));
            }
            if tr.scroll_into_view() {
                let main = tr.state().selection().main();
                let range = if main.is_empty() {
                    main
                } else {
                    EditorSelection::cursor_with(
                        main.head(),
                        if main.head() > main.anchor() { -1 } else { 1 },
                        None,
                        None,
                    )
                };
                self.scroll_target = Some(crate::extension::scroll_target(range));
            }
            for e in tr.effects() {
                if let Some(target) = e.value(&SCROLL_INTO_VIEW) {
                    self.scroll_target = Some(target.clone());
                }
            }
        }
        if let Some(target) = &mut self.scroll_target {
            let len = update.state.doc().len();
            if target.range.to > len {
                target.range = EditorSelection::cursor(len);
            }
        }
    }
}
