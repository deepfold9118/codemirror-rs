//! The editor view. Port of `editorview.ts`, drawn with GPUI: the view is
//! a GPUI entity, and [`EditorElement`](crate::element) lays out and paints
//! the lines in view each frame, measuring them into the height map.

use std::ops::Range;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use codemirror_state::{
    EditorSelection, EditorState, Extension, Facet, FacetSpec, StateCommand, StateEffect, Transaction, TransactionSpec,
};
use gpui::{
    div, prelude::*, px, App, Bounds, Context, CursorStyle, FocusHandle, Focusable, MouseButton, Pixels, Point, Task,
    Window,
};

use crate::decoration::DecorationSet;
use crate::element::EditorElement;
use crate::extension::{
    scroll_target, update_flag, DecorationSource, PluginInstance, PluginValue, ScrollStrategy, ScrollTarget,
    ViewPlugin, ViewUpdate, DECORATIONS, LINE_WRAPPING, SCROLL_INTO_VIEW, THEME, UPDATE_LISTENER, VIEW_PLUGIN,
};
use crate::heightmap::{BlockInfo, QueryType};
use crate::input::MouseSelection;
use crate::keymap::KeymapCache;
use crate::layout::{layout_line, line_decorations, LineLayout, LineStyle};
use crate::theme::Theme;
use crate::viewstate::{ViewState, Viewport};

/// Show line numbers in a gutter. See [`line_numbers`].
pub static LINE_NUMBERS: LazyLock<Facet<bool, bool>> =
    LazyLock::new(|| Facet::define(FacetSpec::combine(|values: &[bool]| values.iter().any(|v| *v)).compare_eq()));

/// Highlight the lines that hold a cursor. See [`highlight_active_line`].
pub static HIGHLIGHT_ACTIVE_LINE: LazyLock<Facet<bool, bool>> =
    LazyLock::new(|| Facet::define(FacetSpec::combine(|values: &[bool]| values.iter().any(|v| *v)).compare_eq()));

/// An extension that shows line numbers in a gutter.
pub fn line_numbers() -> Extension {
    LINE_NUMBERS.of(true)
}

/// An extension that highlights the lines that hold a cursor.
pub fn highlight_active_line() -> Extension {
    HIGHLIGHT_ACTIVE_LINE.of(true)
}

/// An extension that makes lines wrap at the editor width.
pub fn line_wrapping() -> Extension {
    LINE_WRAPPING.of(true)
}

// Geometry from the last frame.
#[derive(Clone, Copy, Default)]
pub(crate) struct Geometry {
    pub bounds: Bounds<Pixels>,
    pub char_width: f32,
    pub gutter_width: f32,
    pub wrap_width: Option<f32>,
    pub valid: bool,
}

pub(crate) const SCROLLBAR_WIDTH: f32 = 12.0;
const BLINK_INTERVAL: Duration = Duration::from_millis(530);

/// An editor view displays an [`EditorState`] and handles input. It is a
/// GPUI entity: create it with `cx.new(|cx| EditorView::new(state, cx))`
/// and render it as a child element.
pub struct EditorView {
    state: EditorState,
    plugins: Vec<PluginInstance>,
    pub(crate) vs: ViewState,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) geo: Geometry,
    pub(crate) scroll: (f32, f32),
    pub(crate) max_line_width: f32,
    pub(crate) marked: Option<Range<usize>>,
    pub(crate) cursor_visible: bool,
    pub(crate) focused: bool,
    pub(crate) mouse: Option<MouseSelection>,
    pub(crate) scrollbar_drag: Option<(f32, f32)>,
    pub(crate) last_pointer: Option<Point<Pixels>>,
    pub(crate) last_linewise_copy: Option<String>,
    pub(crate) keymaps: KeymapCache,
    blink_epoch: usize,
    _blink: Option<Task<()>>,
}

impl EditorView {
    /// Create a view for the given state.
    pub fn new(state: EditorState, cx: &mut Context<Self>) -> EditorView {
        let vs = ViewState::new(&state);
        let mut view = EditorView {
            state,
            plugins: Vec::new(),
            vs,
            focus_handle: cx.focus_handle(),
            geo: Geometry::default(),
            scroll: (0.0, 0.0),
            max_line_width: 0.0,
            marked: None,
            cursor_visible: true,
            focused: false,
            mouse: None,
            scrollbar_drag: None,
            last_pointer: None,
            last_linewise_copy: None,
            keymaps: KeymapCache::default(),
            blink_epoch: 0,
            _blink: None,
        };
        view.update_plugins_config();
        view
    }

    /// The current editor state.
    pub fn state(&self) -> &EditorState {
        &self.state
    }

    /// Dispatch a transaction. Its start state must be the view's current
    /// state.
    pub fn dispatch(&mut self, tr: Transaction, cx: &mut Context<Self>) {
        self.update(vec![tr], cx);
    }

    /// Create a transaction from the given spec and dispatch it.
    pub fn dispatch_spec(&mut self, spec: impl Into<TransactionSpec>, cx: &mut Context<Self>) {
        let tr = self.state.update(spec);
        self.dispatch(tr, cx);
    }

    /// Run a state command against the view's state, dispatching what it
    /// produces.
    pub fn run_state_command(&mut self, command: StateCommand, cx: &mut Context<Self>) -> bool {
        let state = self.state.clone();
        let mut trs = Vec::new();
        let result = command(&state, &mut |tr| trs.push(tr));
        for tr in trs {
            if tr.start_state().ptr_eq(&self.state) {
                self.dispatch(tr, cx);
            }
        }
        result
    }

    /// Update the view for the given transactions, which must each start
    /// from the state produced by the one before (the first from the
    /// current state).
    pub fn update(&mut self, transactions: Vec<Transaction>, cx: &mut Context<Self>) {
        let mut state = self.state.clone();
        for tr in &transactions {
            assert!(
                tr.start_state().ptr_eq(&state),
                "Trying to update state with a transaction that doesn't start from the previous state."
            );
            state = tr.state().clone();
        }
        let reconfigured = transactions.iter().any(|tr| tr.reconfigured());
        let selection_moved = transactions.iter().any(|tr| tr.selection().is_some() || tr.doc_changed());
        let mut update = ViewUpdate::new(self.state.clone(), state.clone(), transactions);
        self.state = state;
        if reconfigured {
            self.update_plugins_config();
        }
        self.vs.update(&mut update);
        if selection_moved {
            self.restart_blink(cx);
        }
        self.finish_update(update, cx);
    }

    /// Replace the view's state without a transaction, as when loading a
    /// different document. Resets the view's layout information.
    pub fn set_state(&mut self, state: EditorState, cx: &mut Context<Self>) {
        self.state = state;
        self.vs = ViewState::new(&self.state);
        self.update_plugins_config();
        self.max_line_width = 0.0;
        self.scroll = (0.0, 0.0);
        self.marked = None;
        self.geo.valid = false;
        cx.notify();
    }

    fn finish_update(&mut self, update: ViewUpdate, cx: &mut Context<Self>) {
        for p in &mut self.plugins {
            p.value.update(&update);
        }
        for listener in self.state.facet(&UPDATE_LISTENER) {
            listener(&update);
        }
        cx.notify();
    }

    pub(crate) fn notify_flags(&mut self, flags: u8, cx: &mut Context<Self>) {
        let mut update = ViewUpdate::new(self.state.clone(), self.state.clone(), Vec::new());
        update.flags = flags;
        for p in &mut self.plugins {
            p.value.update(&update);
        }
        for listener in self.state.facet(&UPDATE_LISTENER) {
            listener(&update);
        }
        // Plugins may have changed their decorations: draw again.
        if flags & (update_flag::VIEWPORT | update_flag::FOCUS) != 0 && !self.plugins.is_empty() {
            cx.notify();
        }
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
    pub fn plugin<V: PluginValue>(&self, plugin: &ViewPlugin<V>) -> Option<&V> {
        self.plugin_by_id(plugin.id)
    }

    /// Mutable access to a view plugin's value.
    pub fn plugin_mut<V: PluginValue>(&mut self, plugin: &ViewPlugin<V>) -> Option<&mut V> {
        let p = self.plugins.iter_mut().find(|p| p.id == plugin.id)?;
        (&mut *p.value as &mut dyn std::any::Any).downcast_mut::<V>()
    }

    pub(crate) fn plugin_by_id<V: PluginValue>(&self, id: u64) -> Option<&V> {
        let p = self.plugins.iter().find(|p| p.id == id)?;
        (&*p.value as &dyn std::any::Any).downcast_ref::<V>()
    }

    /// Whether the editor has keyboard focus.
    pub fn has_focus(&self) -> bool {
        self.focused
    }

    /// Move keyboard focus to the editor.
    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus_handle);
    }

    /// Whether lines wrap.
    pub fn line_wrapping(&self) -> bool {
        *self.state.facet(&LINE_WRAPPING)
    }

    pub(crate) fn line_numbers(&self) -> bool {
        *self.state.facet(&LINE_NUMBERS)
    }

    pub(crate) fn highlight_active_line(&self) -> bool {
        *self.state.facet(&HIGHLIGHT_ACTIVE_LINE)
    }

    /// The theme in use.
    pub fn theme(&self) -> Arc<Theme> {
        static DEFAULT: LazyLock<Arc<Theme>> = LazyLock::new(|| Arc::new(Theme::dark()));
        self.state.facet(&THEME).clone().unwrap_or_else(|| DEFAULT.clone())
    }

    /// The height of a line of text.
    pub fn default_line_height(&self) -> f32 {
        self.theme().row_height()
    }

    /// The width of a character (`0`) in the editor font, from the last
    /// frame.
    pub fn default_character_width(&self) -> f32 {
        if self.geo.valid {
            self.geo.char_width
        } else {
            self.theme().font_size * 0.55
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
            f32::from(self.geo.bounds.size.height)
        } else {
            self.default_line_height() * 20.0
        }
    }

    /// The window bounds of the editor, from the last frame.
    pub fn visible_rect(&self) -> Bounds<Pixels> {
        self.geo.bounds
    }

    /// The total height of the document.
    pub fn content_height(&self) -> f64 {
        self.vs.height_map.height()
    }

    /// The window position of the top of the document, from the last
    /// frame.
    pub fn document_top(&self) -> Pixels {
        self.origin().y
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
        SCROLL_INTO_VIEW.of(ScrollTarget { y, ..scroll_target(EditorSelection::cursor(pos)) })
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
    pub(crate) fn layout(&self, window: &mut Window, line_from: usize, decos: Option<&[DecorationSet]>) -> LineLayout {
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
        let marked = self
            .marked
            .as_ref()
            .and_then(|m| (m.start >= line.from && m.end <= line.to).then(|| (m.start - line.from, m.end - line.from)));
        let theme = self.theme();
        let font = theme.font();
        let style = LineStyle {
            font: &font,
            font_size: px(theme.font_size),
            color: theme.text,
            line_height: theme.row_height(),
            wrap_width: if self.line_wrapping() { self.geo.wrap_width } else { None },
            tab_size: self.state.tab_size(),
        };
        layout_line(window, &line.text, &style, &deco, marked)
    }

    /// Window position of document coordinates (0, 0).
    pub(crate) fn origin(&self) -> Point<Pixels> {
        let b = self.geo.bounds;
        let theme = self.theme();
        Point::new(
            b.origin.x + px(self.geo.gutter_width + theme.padding_left - self.scroll.0),
            b.origin.y + px(theme.padding_y - self.scroll.1),
        )
    }

    pub(crate) fn max_scroll_y(&self) -> f32 {
        (self.content_height() as f32 + self.theme().padding_y * 2.0 - self.visible_height()).max(0.0)
    }

    pub(crate) fn max_scroll_x(&self) -> f32 {
        if self.line_wrapping() {
            return 0.0;
        }
        let text_w =
            f32::from(self.geo.bounds.size.width) - self.geo.gutter_width - self.theme().padding_left - SCROLLBAR_WIDTH;
        (self.max_line_width + self.geo.char_width * 2.0 - text_w).max(0.0)
    }

    pub(crate) fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.cursor_visible = true;
        self.blink_epoch += 1;
        let epoch = self.blink_epoch;
        self._blink = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(BLINK_INTERVAL).await;
            let alive = this
                .update(cx, |view, cx| {
                    if view.blink_epoch != epoch {
                        return false;
                    }
                    if view.focused {
                        view.cursor_visible = !view.cursor_visible;
                        cx.notify();
                    }
                    true
                })
                .unwrap_or(false);
            if !alive {
                break;
            }
        }));
    }

    pub(crate) fn set_focus_state(&mut self, focused: bool, cx: &mut Context<Self>) {
        if focused != self.focused {
            self.focused = focused;
            if focused {
                self.restart_blink(cx);
            }
            self.notify_flags(update_flag::FOCUS, cx);
        }
    }

    pub(crate) fn utf16_to_byte(&self, off: usize) -> usize {
        let rope = self.state.doc().rope();
        rope.byte_of_utf16_code_unit(off.min(rope.utf16_len()))
    }

    pub(crate) fn byte_to_utf16(&self, off: usize) -> usize {
        self.state.doc().rope().utf16_code_unit_of_byte(off.min(self.state.doc().len()))
    }
}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("CodeMirror")
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(EditorElement { view: cx.entity() })
    }
}
