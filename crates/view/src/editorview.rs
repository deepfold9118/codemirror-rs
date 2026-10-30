//! The editor view. Port of `editorview.ts`, drawn with egui: instead of
//! keeping a DOM in sync, [`EditorView::ui`] lays out and paints the lines
//! in view each frame, and measures them into the height map.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use codemirror_state::{
    EditorState, Facet, FacetSpec, SelectionRange, StateCommand, StateEffect, Transaction, TransactionSpec,
};
use egui::{Pos2, Rect, Sense, Vec2};

use crate::decoration::{BlockType, DecorationSet};
use crate::extension::{
    scroll_target, update_flag, DecorationSource, PluginInstance, PluginValue, ScrollStrategy, ScrollTarget,
    ViewUpdate, DECORATIONS, LINE_WRAPPING, SCROLL_INTO_VIEW, THEME, UPDATE_LISTENER, VIEW_PLUGIN,
};
use crate::heightmap::{clear_height_change_flag, height_change_flag, BlockInfo, MeasuredHeights, QueryType};
use crate::input::InputState;
use crate::keymap::KeymapCache;
use crate::layout::{layout_line, line_decorations, LineLayout};
use crate::theme::Theme;
use crate::viewstate::{ViewState, Viewport};

/// Show line numbers in a gutter. See [`line_numbers`].
pub static LINE_NUMBERS: LazyLock<Facet<bool, bool>> =
    LazyLock::new(|| Facet::define(FacetSpec::combine(|values: &[bool]| values.iter().any(|v| *v)).compare_eq()));

/// Highlight the lines that hold a cursor. See [`highlight_active_line`].
pub static HIGHLIGHT_ACTIVE_LINE: LazyLock<Facet<bool, bool>> =
    LazyLock::new(|| Facet::define(FacetSpec::combine(|values: &[bool]| values.iter().any(|v| *v)).compare_eq()));

/// An extension that shows line numbers in a gutter.
pub fn line_numbers() -> codemirror_state::Extension {
    LINE_NUMBERS.of(true)
}

/// An extension that highlights the lines that hold a cursor.
pub fn highlight_active_line() -> codemirror_state::Extension {
    HIGHLIGHT_ACTIVE_LINE.of(true)
}

/// An extension that makes lines wrap at the editor width.
pub fn line_wrapping() -> codemirror_state::Extension {
    LINE_WRAPPING.of(true)
}

// Geometry from the last frame, in screen coordinates.
#[derive(Clone, Copy)]
pub(crate) struct Geometry {
    // Screen position of document position (0, 0).
    pub origin: Pos2,
    // The visible part of the editor.
    pub visible: Rect,
    pub wrap_width: Option<f32>,
    pub gutter_width: f32,
    pub row_height: f32,
    pub char_width: f32,
    pub text_height: f32,
    pub max_line_width: f32,
    pub valid: bool,
}

impl Default for Geometry {
    fn default() -> Self {
        Geometry {
            origin: Pos2::ZERO,
            visible: Rect::NOTHING,
            wrap_width: None,
            gutter_width: 0.0,
            row_height: 0.0,
            char_width: 0.0,
            text_height: 0.0,
            max_line_width: 0.0,
            valid: false,
        }
    }
}

const VIEWPORT_MARGIN: f64 = 300.0;

/// An editor view displays an [`EditorState`] and handles input. Call
/// [`ui`](EditorView::ui) every frame to draw it.
pub struct EditorView {
    state: EditorState,
    plugins: Vec<PluginInstance>,
    pub(crate) vs: ViewState,
    pub(crate) geo: Geometry,
    pub(crate) ctx: Option<egui::Context>,
    pub(crate) id: egui::Id,
    pub(crate) input: InputState,
    pub(crate) keymaps: KeymapCache,
    default_theme: Arc<Theme>,
    has_focus: bool,
    focus_requested: bool,
    // Time of the last selection change, to restart the caret blink.
    blink_start: f64,
    scroll_frames: u8,
}

static NEXT_VIEW_ID: AtomicU64 = AtomicU64::new(0);

impl EditorView {
    /// Create a view for the given state.
    pub fn new(state: EditorState) -> EditorView {
        let vs = ViewState::new(&state);
        let mut view = EditorView {
            state,
            plugins: Vec::new(),
            vs,
            geo: Geometry::default(),
            ctx: None,
            id: egui::Id::new(("codemirror-view", NEXT_VIEW_ID.fetch_add(1, Ordering::Relaxed))),
            input: InputState::default(),
            keymaps: KeymapCache::default(),
            default_theme: Arc::new(Theme::dark()),
            has_focus: false,
            focus_requested: false,
            blink_start: 0.0,
            scroll_frames: 0,
        };
        view.update_plugins_config();
        view
    }

    /// The current editor state.
    pub fn state(&self) -> &EditorState {
        &self.state
    }

    /// The egui id of the editor widget.
    pub fn id(&self) -> egui::Id {
        self.id
    }

    /// Dispatch a transaction. Its start state must be the view's current
    /// state.
    pub fn dispatch(&mut self, tr: Transaction) {
        self.update(vec![tr]);
    }

    /// Create a transaction from the given spec and dispatch it.
    pub fn dispatch_spec(&mut self, spec: impl Into<TransactionSpec>) {
        let tr = self.state.update(spec);
        self.dispatch(tr);
    }

    /// Run a state command against the view's state, dispatching what it
    /// produces.
    pub fn run_state_command(&mut self, command: StateCommand) -> bool {
        let state = self.state.clone();
        let mut trs = Vec::new();
        let result = command(&state, &mut |tr| trs.push(tr));
        for tr in trs {
            if tr.start_state().ptr_eq(&self.state) {
                self.dispatch(tr);
            }
        }
        result
    }

    /// Update the view for the given transactions, which must each start
    /// from the state produced by the one before (the first from the
    /// current state).
    pub fn update(&mut self, transactions: Vec<Transaction>) {
        let mut state = self.state.clone();
        for tr in &transactions {
            assert!(
                tr.start_state().ptr_eq(&state),
                "Trying to update state with a transaction that doesn't start from the previous state."
            );
            state = tr.state().clone();
        }
        let reconfigured = transactions.iter().any(|tr| tr.reconfigured());
        let selection_changed = transactions.iter().any(|tr| tr.selection().is_some() || tr.doc_changed());
        let mut update = ViewUpdate::new(self.state.clone(), state.clone(), transactions);
        self.state = state;
        if reconfigured {
            self.update_plugins_config();
        }
        self.vs.update(&mut update);
        if selection_changed {
            self.blink_start = self.now();
            self.scroll_frames = 0;
        }
        self.finish_update(update);
    }

    /// Replace the view's state without a transaction, as when loading a
    /// different document. Resets the view's layout information.
    pub fn set_state(&mut self, state: EditorState) {
        self.state = state;
        self.vs = ViewState::new(&self.state);
        self.update_plugins_config();
        self.geo.max_line_width = 0.0;
    }

    fn finish_update(&mut self, update: ViewUpdate) {
        for p in &mut self.plugins {
            p.value.update(&update);
        }
        for listener in self.state.facet(&UPDATE_LISTENER) {
            listener(&update);
        }
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    fn notify(&mut self, flags: u8) {
        let mut update = ViewUpdate::new(self.state.clone(), self.state.clone(), Vec::new());
        update.flags = flags;
        self.finish_update(update);
    }

    fn update_plugins_config(&mut self) {
        let specs = self.state.facet(&VIEW_PLUGIN).clone();
        let mut old = std::mem::take(&mut self.plugins);
        let mut plugins = Vec::with_capacity(specs.len());
        for spec in &specs {
            match old.iter().position(|p| p.id == spec.id) {
                Some(i) => plugins.push(old.remove(i)),
                None => plugins.push(PluginInstance { id: spec.id, value: (spec.create)(self) }),
            }
        }
        for mut p in old {
            p.value.destroy();
        }
        self.plugins = plugins;
    }

    /// Get the value of a view plugin, if it is active in this view.
    pub fn plugin<V: PluginValue>(&self, plugin: &crate::ViewPlugin<V>) -> Option<&V> {
        self.plugin_by_id(plugin.id)
    }

    /// Mutable access to a view plugin's value.
    pub fn plugin_mut<V: PluginValue>(&mut self, plugin: &crate::ViewPlugin<V>) -> Option<&mut V> {
        let p = self.plugins.iter_mut().find(|p| p.id == plugin.id)?;
        (&mut *p.value as &mut dyn std::any::Any).downcast_mut::<V>()
    }

    pub(crate) fn plugin_by_id<V: PluginValue>(&self, id: u64) -> Option<&V> {
        let p = self.plugins.iter().find(|p| p.id == id)?;
        (&*p.value as &dyn std::any::Any).downcast_ref::<V>()
    }

    /// Whether the editor has keyboard focus.
    pub fn has_focus(&self) -> bool {
        self.has_focus
    }

    /// Move keyboard focus to the editor (takes effect in the next frame).
    pub fn focus(&mut self) {
        self.focus_requested = true;
        if let Some(ctx) = &self.ctx {
            ctx.memory_mut(|m| m.request_focus(self.id));
        }
    }

    /// Whether lines wrap.
    pub fn line_wrapping(&self) -> bool {
        *self.state.facet(&LINE_WRAPPING)
    }

    /// The theme in use.
    pub fn theme(&self) -> Arc<Theme> {
        self.state.facet(&THEME).clone().unwrap_or_else(|| self.default_theme.clone())
    }

    /// The height of a line of text.
    pub fn default_line_height(&self) -> f32 {
        if self.geo.valid {
            self.geo.row_height
        } else {
            self.theme().row_height()
        }
    }

    /// The width of a character (`0`) in the editor font.
    pub fn default_character_width(&self) -> f32 {
        if self.geo.valid {
            self.geo.char_width
        } else {
            self.theme().font.size * 0.6
        }
    }

    /// The range of the document that was laid out in the last frame
    /// (visible lines plus a margin).
    pub fn viewport(&self) -> Viewport {
        self.vs.viewport
    }

    /// The height of the visible part of the editor.
    pub fn visible_height(&self) -> f32 {
        if self.geo.valid {
            self.geo.visible.height()
        } else {
            self.default_line_height() * 20.0
        }
    }

    /// The screen rectangle of the visible part of the editor, from the last
    /// frame.
    pub fn visible_rect(&self) -> Rect {
        self.geo.visible
    }

    /// The total height of the document.
    pub fn content_height(&self) -> f64 {
        self.vs.height_map.height()
    }

    /// The screen position of the top of the document, from the last frame.
    pub fn document_top(&self) -> f32 {
        self.geo.origin.y
    }

    /// Find the line block (a line plus any block widgets attached to it)
    /// around the given position.
    pub fn line_block_at(&self, pos: usize) -> BlockInfo {
        self.vs.height_map.line_at(pos as f64, QueryType::ByPos, &self.vs.oracle, 0.0, 0)
    }

    /// Find the line block at the given height, relative to the top of the
    /// document.
    pub fn line_block_at_height(&self, height: f64) -> BlockInfo {
        self.vs.height_map.line_at(height, QueryType::ByHeight, &self.vs.oracle, 0.0, 0)
    }

    /// Find the block (line or block widget) at the given height, relative
    /// to the top of the document.
    pub fn element_at_height(&self, height: f64) -> BlockInfo {
        self.vs.height_map.block_at(height, &self.vs.oracle, 0.0, 0)
    }

    /// An effect that scrolls the given position into view.
    pub fn scroll_into_view(pos: usize, y: ScrollStrategy) -> StateEffect {
        SCROLL_INTO_VIEW.of(ScrollTarget { y, ..scroll_target(codemirror_state::EditorSelection::cursor(pos)) })
    }

    pub(crate) fn now(&self) -> f64 {
        self.ctx.as_ref().map_or(0.0, |c| c.input(|i| i.time))
    }

    /// All decoration sets, including the ones computed from the view.
    pub(crate) fn decoration_sets(&self) -> Vec<DecorationSet> {
        self.state
            .facet(&DECORATIONS)
            .iter()
            .map(|s| match s {
                DecorationSource::Set(set) => set.clone(),
                DecorationSource::Dynamic(f) => f(self),
            })
            .collect()
    }

    /// Lay out the text line starting at `line_from`.
    pub(crate) fn layout(&self, line_from: usize, decos: Option<&[DecorationSet]>) -> Option<LineLayout> {
        let ctx = self.ctx.as_ref()?;
        let line = self.state.doc().line_at(line_from);
        let owned;
        let decos = match decos {
            Some(d) => d,
            None => {
                owned = self.decoration_sets();
                &owned
            }
        };
        let deco = line_decorations(decos, line.from, line.to);
        let theme = self.theme();
        let tab_size = self.state.tab_size();
        Some(ctx.fonts_mut(|f| layout_line(f, &theme, &line.text, &deco, self.geo.wrap_width, tab_size)))
    }

    /// Draw the editor, handle its input, and return the response of the
    /// editor widget (for context menus and the like).
    pub fn ui(&mut self, ui: &mut egui::Ui) -> egui::Response {
        self.ctx = Some(ui.ctx().clone());
        let visuals_theme = Theme::from_visuals(ui.visuals());
        if *self.default_theme != visuals_theme {
            self.default_theme = Arc::new(visuals_theme);
        }
        let theme = self.theme();
        let row_height = theme.row_height();
        let (char_width, text_height) =
            ui.ctx().fonts_mut(|f| (f.glyph_width(&theme.font, '0'), f.row_height(&theme.font)));
        let wrapping = self.line_wrapping();
        let gutter_width = if *self.state.facet(&LINE_NUMBERS) {
            let digits = self.state.doc().lines().to_string().len().max(2);
            digits as f32 * char_width + 12.0
        } else {
            0.0
        };
        let text_left = gutter_width + theme.padding_left;
        let bar = ui.spacing().scroll.allocated_width();
        let avail = ui.available_size();
        let wrap_width =
            if wrapping { Some((avail.x - text_left - theme.padding_right - bar).max(char_width * 4.0)) } else { None };

        // Refresh the height estimates when the metrics changed.
        let line_length = wrap_width.map_or(1e9, |w| (w / char_width) as f64);
        let width_changed = match (wrap_width, self.geo.wrap_width) {
            (Some(a), Some(b)) => (a - b).abs() > char_width,
            (a, b) => a.is_some() != b.is_some(),
        };
        let changed = self.vs.oracle.refresh(
            wrapping,
            row_height as f64,
            char_width as f64,
            text_height as f64,
            line_length,
            &[],
        );
        let geometry_changed = changed || width_changed || !self.geo.valid;
        self.geo.wrap_width = wrap_width;
        self.geo.gutter_width = gutter_width;
        self.geo.row_height = row_height;
        self.geo.char_width = char_width;
        self.geo.text_height = text_height;
        if geometry_changed {
            let map = std::mem::replace(&mut self.vs.height_map, crate::heightmap::HeightMap::empty());
            self.vs.height_map = map.update_height(&self.vs.oracle, 0, true, None);
        }

        let content_width = if wrapping {
            avail.x - bar
        } else {
            (text_left + self.geo.max_line_width + theme.padding_right + char_width).max(avail.x - bar)
        };
        let total = Vec2::new(content_width, self.vs.height_map.height() as f32 + theme.padding_y * 2.0);
        let scroll = if wrapping { egui::ScrollArea::vertical() } else { egui::ScrollArea::both() };
        let output = scroll
            .id_salt(self.id.with("scroll"))
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| self.frame(ui, viewport, total, &theme, text_left));
        if geometry_changed {
            self.notify(update_flag::GEOMETRY);
        }
        output.inner
    }

    fn frame(
        &mut self,
        ui: &mut egui::Ui,
        viewport: Rect,
        total: Vec2,
        theme: &Theme,
        text_left: f32,
    ) -> egui::Response {
        let (rect, _) = ui.allocate_exact_size(total, Sense::hover());
        let resp = ui.interact(rect, self.id, Sense::click_and_drag());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Editor"));
        self.geo.origin = rect.min + Vec2::new(text_left, theme.padding_y);
        self.geo.visible = ui.clip_rect();
        self.geo.valid = true;

        if self.focus_requested || resp.clicked() || resp.drag_started() {
            self.focus_requested = false;
            ui.memory_mut(|m| m.request_focus(self.id));
        }
        let focused = ui.memory(|m| m.has_focus(self.id));
        if focused != self.has_focus {
            self.has_focus = focused;
            self.blink_start = self.now();
            self.notify(update_flag::FOCUS);
        }
        if focused {
            let filter = self.keymaps.event_filter(&self.state);
            ui.memory_mut(|m| m.set_focus_lock_filter(self.id, filter));
            self.handle_events(ui);
        }
        self.handle_mouse(ui, &resp);

        let lines = self.measure(viewport, theme);
        self.paint(ui, rect, &lines, focused, theme);

        if self.vs.scroll_target.is_some() {
            self.scroll_to_target(ui);
        }
        resp
    }

    // Lay out the lines around the visible part of the document, update the
    // height map with their sizes, and return them with their positions.
    fn measure(&mut self, viewport: Rect, theme: &Theme) -> Vec<(BlockInfo, Option<LineLayout>)> {
        let decos = self.decoration_sets();
        let top = (viewport.min.y - theme.padding_y) as f64;
        let bottom = (viewport.max.y - theme.padding_y) as f64;
        let row_height = theme.row_height();
        let mut height_changed = false;
        let mut result = Vec::new();
        let mut range = Viewport::default();
        for attempt in 0..3 {
            let oracle = &self.vs.oracle;
            let first =
                self.vs.height_map.line_at((top - VIEWPORT_MARGIN).max(0.0), QueryType::ByHeight, oracle, 0.0, 0);
            let last = self.vs.height_map.line_at(bottom + VIEWPORT_MARGIN, QueryType::ByHeight, oracle, 0.0, 0);
            range = Viewport { from: first.from, to: last.to() };
            let mut blocks = Vec::new();
            self.vs.height_map.for_each_line(range.from, range.to, oracle, 0.0, 0, &mut |b| blocks.push(b));
            let mut heights = Vec::new();
            result.clear();
            for block in blocks {
                let mut layout = None;
                let parts: Vec<BlockInfo> = match block.children() {
                    Some(c) => c.to_vec(),
                    None => vec![block.clone()],
                };
                for part in &parts {
                    if part.block_type() == BlockType::Text {
                        let l = self.layout(part.from, Some(&decos));
                        if let Some(l) = &l {
                            self.geo.max_line_width = self.geo.max_line_width.max(l.width());
                            heights.push(l.height(row_height) as f64);
                        } else {
                            heights.push(part.height);
                        }
                        layout = l;
                    } else {
                        heights.push(part.height);
                    }
                }
                result.push((block, layout));
            }
            clear_height_change_flag();
            let map = std::mem::replace(&mut self.vs.height_map, crate::heightmap::HeightMap::empty());
            let mut measured = MeasuredHeights::new(range.from, heights);
            self.vs.height_map = map.update_height(&self.vs.oracle, 0, false, Some(&mut measured));
            if !height_change_flag() {
                break;
            }
            height_changed = true;
            if attempt == 2 {
                // Recompute the positions without laying out again.
                let mut blocks = Vec::new();
                let oracle = &self.vs.oracle;
                self.vs.height_map.for_each_line(range.from, range.to, oracle, 0.0, 0, &mut |b| blocks.push(b));
                for ((old, _), new) in result.iter_mut().zip(blocks) {
                    *old = new;
                }
            }
        }
        let mut flags = 0;
        if height_changed {
            flags |= update_flag::HEIGHT;
        }
        if range != self.vs.viewport {
            self.vs.viewport = range;
            flags |= update_flag::VIEWPORT | update_flag::VIEWPORT_MOVED;
        }
        if flags != 0 {
            self.notify(flags);
        }
        result
    }

    fn paint(
        &mut self,
        ui: &egui::Ui,
        rect: Rect,
        lines: &[(BlockInfo, Option<LineLayout>)],
        focused: bool,
        theme: &Theme,
    ) {
        let clip = ui.clip_rect();
        let painter = ui.painter_at(clip);
        painter.rect_filled(clip, 0.0, theme.background);
        let origin = self.geo.origin;
        let selection = self.state.selection().clone();
        let active_lines = *self.state.facet(&HIGHLIGHT_ACTIVE_LINE);
        let right = match self.geo.wrap_width {
            Some(w) => w,
            None => (clip.right() - origin.x).max(self.geo.max_line_width),
        };
        let sel_color = if focused { theme.selection } else { theme.selection_unfocused };
        let now = self.now();
        let blink_period = 1.2;
        let phase = (now - self.blink_start).rem_euclid(blink_period);
        let caret_on = phase < blink_period / 2.0;
        if focused {
            let wait = if caret_on { blink_period / 2.0 - phase } else { blink_period - phase };
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(wait.max(0.01)));
        }
        let decos = self.decoration_sets();
        let mut ime_rect = None;

        for (block, layout) in lines {
            let y = origin.y + block.top as f32;
            let height = block.height as f32;
            let has_head = selection.ranges.iter().any(|r| r.head() >= block.from && r.head() <= block.to());
            let deco = line_decorations(&decos, block.from, block.to());
            let line_rect =
                Rect::from_min_size(Pos2::new(rect.left(), y), Vec2::new(rect.width().max(clip.width()), height));
            if let Some(bg) = deco.background {
                painter.rect_filled(line_rect, 0.0, bg);
            }
            if active_lines && has_head {
                painter.rect_filled(line_rect, 0.0, theme.active_line);
            }
            let Some(layout) = layout else { continue };
            let text_top = match block.children() {
                Some(c) => c.iter().find(|p| p.block_type() == BlockType::Text).map_or(block.top, |p| p.top),
                None => block.top,
            };
            let text_pos = Pos2::new(origin.x, origin.y + text_top as f32);
            let line = self.state.doc().line_at(block.from);
            for r in &selection.ranges {
                if r.is_empty() || r.to < line.from || r.from > line.to {
                    continue;
                }
                let a = r.from.max(line.from) - line.from;
                let b = r.to.min(line.to) - line.from;
                for rr in layout.range_rects(a, b, r.to > line.to, right) {
                    painter.rect_filled(rr.translate(text_pos.to_vec2()), 0.0, sel_color);
                }
            }
            painter.galley(text_pos, layout.galley.clone(), theme.text);

            for (i, r) in selection.ranges.iter().enumerate() {
                let head = r.head();
                if head < line.from || head > line.to {
                    continue;
                }
                let c = layout.coords(head - line.from, cursor_assoc(r)).translate(text_pos.to_vec2());
                let caret = Rect::from_min_max(
                    Pos2::new(c.left() - theme.caret_width / 2.0, c.top()),
                    Pos2::new(c.left() + theme.caret_width / 2.0, c.bottom()),
                );
                if i == selection.main_index {
                    ime_rect = Some(caret);
                    if let Some(text) = self.input.composition.as_deref().filter(|t| !t.is_empty()) {
                        let galley = ui.ctx().fonts_mut(|f| {
                            let mut job = egui::text::LayoutJob::default();
                            job.append(
                                text,
                                0.0,
                                egui::TextFormat {
                                    font_id: theme.font.clone(),
                                    color: theme.text,
                                    background: theme.background,
                                    underline: egui::Stroke::new(1.0, theme.composition),
                                    line_height: Some(theme.row_height()),
                                    ..Default::default()
                                },
                            );
                            f.layout_job(job)
                        });
                        painter.galley(Pos2::new(c.left(), c.top()), galley, theme.text);
                        continue;
                    }
                }
                if focused && caret_on {
                    painter.rect_filled(caret, 0.0, theme.caret);
                }
            }
        }

        if self.geo.gutter_width > 0.0 {
            let gutter =
                Rect::from_min_max(clip.left_top(), Pos2::new(clip.left() + self.geo.gutter_width, clip.bottom()));
            painter.rect_filled(gutter, 0.0, theme.gutter_background);
            for (block, layout) in lines {
                if layout.is_none() {
                    continue;
                }
                let line = self.state.doc().line_at(block.from);
                let active = selection.ranges.iter().any(|r| r.head() >= line.from && r.head() <= line.to);
                painter.text(
                    Pos2::new(
                        gutter.right() - 7.0,
                        origin.y + block.top as f32 + (self.geo.row_height - self.geo.text_height) / 2.0,
                    ),
                    egui::Align2::RIGHT_TOP,
                    line.number.to_string(),
                    theme.font.clone(),
                    if active { theme.gutter_active_text } else { theme.gutter_text },
                );
            }
        }

        if focused {
            if let Some(r) = ime_rect {
                ui.ctx().output_mut(|o| {
                    o.ime =
                        Some(egui::output::IMEOutput { rect: r, cursor_rect: r, should_interrupt_composition: false });
                });
            }
        }
    }

    fn scroll_to_target(&mut self, ui: &egui::Ui) {
        let Some(target) = self.vs.scroll_target.clone() else { return };
        let Some(r) = self.coords_at_pos(target.range.head(), cursor_assoc(&target.range)) else { return };
        let r = r.expand2(Vec2::new(target.x_margin + self.geo.char_width, target.y_margin));
        let align = match target.y {
            ScrollStrategy::Nearest => None,
            ScrollStrategy::Start => Some(egui::Align::Min),
            ScrollStrategy::End => Some(egui::Align::Max),
            ScrollStrategy::Center => Some(egui::Align::Center),
        };
        ui.scroll_to_rect(r, align);
        // Heights measured after scrolling can move the target; try again in
        // the next frame.
        self.scroll_frames += 1;
        if self.scroll_frames >= 2 {
            self.vs.scroll_target = None;
            self.scroll_frames = 0;
        } else {
            ui.ctx().request_repaint();
        }
    }
}

/// The association to use when drawing a range's head (CodeMirror's
/// `range.assoc || 1`).
pub(crate) fn cursor_assoc(r: &SelectionRange) -> i32 {
    match r.assoc() {
        0 => 1,
        a => a,
    }
}
