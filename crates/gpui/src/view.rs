//! The GPUI editor view: holds the CodeMirror state, handles input, and
//! lays out the visible lines for [`EditorElement`](crate::element).

use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use codemirror_state::{
    find_cluster_break, ChangeSpec, CharCategory, EditorSelection, EditorState, RangeChange, SelectionRange,
    StateCommand, Transaction, TransactionSpec,
};
use codemirror_view::{group_at, HIGHLIGHT_ACTIVE_LINE, LINE_NUMBERS, LINE_WRAPPING};
use gpui::{
    div, font, prelude::*, px, rgb, rgba, App, Bounds, ClipboardItem, Context, CursorStyle, FocusHandle, Focusable,
    Font, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollWheelEvent, Task, Window,
};

use crate::element::EditorElement;
use crate::heights::LineHeights;
use crate::keys::Keymap;
use crate::layout::{layout_line, LineLayout};

/// Colors and metrics.
#[derive(Clone)]
pub struct EditorStyle {
    pub font: Font,
    pub font_size: f32,
    pub line_height: f32,
    pub text: Hsla,
    pub background: Hsla,
    pub selection: Hsla,
    pub selection_unfocused: Hsla,
    pub caret: Hsla,
    pub active_line: Hsla,
    pub gutter_background: Hsla,
    pub gutter_text: Hsla,
    pub gutter_active_text: Hsla,
    pub scrollbar: Hsla,
    pub padding_left: f32,
    pub padding_y: f32,
}

impl Default for EditorStyle {
    fn default() -> Self {
        EditorStyle {
            font: font("Consolas"),
            font_size: 14.0,
            line_height: 1.4,
            text: rgb(0xd4d4d4).into(),
            background: rgb(0x1e1e1e).into(),
            selection: rgb(0x264f78).into(),
            selection_unfocused: rgb(0x3a3d41).into(),
            caret: rgb(0xaeafad).into(),
            active_line: rgba(0xffffff0a).into(),
            gutter_background: rgb(0x1e1e1e).into(),
            gutter_text: rgb(0x858585).into(),
            gutter_active_text: rgb(0xc6c6c6).into(),
            scrollbar: rgba(0x79797966).into(),
            padding_left: 6.0,
            padding_y: 4.0,
        }
    }
}

/// A command for a key binding.
#[derive(Clone, Copy)]
pub enum Cmd {
    State(StateCommand),
    View(fn(&mut EditorView, &mut Window, &mut Context<EditorView>) -> bool),
}

pub(crate) struct MouseSelection {
    start: (usize, i32),
    start_point: Point<Pixels>,
    count: usize,
    extend: bool,
    multiple: bool,
    rectangular: bool,
    start_sel: EditorSelection,
}

// Geometry from the last frame.
#[derive(Clone, Copy, Default)]
pub(crate) struct Geometry {
    pub bounds: Bounds<Pixels>,
    pub line_height: f32,
    pub char_width: f32,
    pub gutter_width: f32,
    pub wrap_width: Option<f32>,
    pub valid: bool,
}

pub(crate) const SCROLLBAR_WIDTH: f32 = 12.0;

/// An editor view for an [`EditorState`], drawn with GPUI.
pub struct EditorView {
    state: EditorState,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) style: EditorStyle,
    pub(crate) heights: LineHeights,
    pub(crate) scroll: (f32, f32),
    pub(crate) geo: Geometry,
    pub(crate) max_line_width: f32,
    pub(crate) marked: Option<Range<usize>>,
    pub(crate) cursor_visible: bool,
    pub(crate) focused: bool,
    pub(crate) scroll_to_cursor: bool,
    pub(crate) mouse: Option<MouseSelection>,
    pub(crate) scrollbar_drag: Option<(f32, f32)>,
    pub(crate) last_pointer: Option<Point<Pixels>>,
    last_linewise_copy: Option<String>,
    blink_epoch: usize,
    _blink: Option<Task<()>>,
    keymap: Rc<Keymap>,
}

impl EditorView {
    /// Create a view for the given state, with the default key bindings.
    pub fn new(state: EditorState, cx: &mut Context<Self>) -> EditorView {
        let style = EditorStyle::default();
        let line_height = (style.font_size * style.line_height).round();
        let heights = LineHeights::new(vec![line_height; state.doc().lines()]);
        EditorView {
            state,
            focus_handle: cx.focus_handle(),
            style,
            heights,
            scroll: (0.0, 0.0),
            geo: Geometry::default(),
            max_line_width: 0.0,
            marked: None,
            cursor_visible: true,
            focused: false,
            scroll_to_cursor: false,
            mouse: None,
            scrollbar_drag: None,
            last_pointer: None,
            last_linewise_copy: None,
            blink_epoch: 0,
            _blink: None,
            keymap: Rc::new(crate::keys::default_keymap()),
        }
    }

    /// The current state.
    pub fn state(&self) -> &EditorState {
        &self.state
    }

    pub fn line_wrapping(&self) -> bool {
        *self.state.facet(&LINE_WRAPPING)
    }

    pub(crate) fn line_numbers(&self) -> bool {
        *self.state.facet(&LINE_NUMBERS)
    }

    pub(crate) fn highlight_active_line(&self) -> bool {
        *self.state.facet(&HIGHLIGHT_ACTIVE_LINE)
    }

    pub(crate) fn line_height(&self) -> f32 {
        (self.style.font_size * self.style.line_height).round()
    }

    // ---- updates ----

    /// Apply a transaction.
    pub fn dispatch(&mut self, tr: Transaction, cx: &mut Context<Self>) {
        if !tr.start_state().ptr_eq(&self.state) {
            return;
        }
        if tr.doc_changed() {
            let old = tr.start_state().doc().clone();
            let new = tr.state().doc().clone();
            let mut ranges = Vec::new();
            tr.changes().iter_changed_ranges(|fa, ta, fb, tb| ranges.push((fa, ta, fb, tb)), false);
            for &(fa, ta, fb, tb) in ranges.iter().rev() {
                let (a0, a1) = (old.line_at(fa).number - 1, old.line_at(ta).number);
                let (b0, b1) = (new.line_at(fb).number - 1, new.line_at(tb).number);
                let estimates = (b0..b1).map(|i| self.estimate(&new, i)).collect();
                self.heights.splice(a0, a1, estimates);
            }
        }
        if tr.scroll_into_view() {
            self.scroll_to_cursor = true;
        }
        let selection_moved = tr.doc_changed() || tr.selection().is_some();
        self.state = tr.state().clone();
        if selection_moved {
            self.restart_blink(cx);
        }
        cx.notify();
    }

    /// Create a transaction from a spec and apply it.
    pub fn dispatch_spec(&mut self, spec: impl Into<TransactionSpec>, cx: &mut Context<Self>) {
        let tr = self.state.update(spec);
        self.dispatch(tr, cx);
    }

    /// Run a state command.
    pub fn run_state_command(&mut self, command: StateCommand, cx: &mut Context<Self>) -> bool {
        let state = self.state.clone();
        let mut trs = Vec::new();
        let result = command(&state, &mut |tr| trs.push(tr));
        for tr in trs {
            self.dispatch(tr, cx);
        }
        result
    }

    fn estimate(&self, doc: &codemirror_state::Text, line: usize) -> f32 {
        let lh = self.line_height();
        match (self.geo.wrap_width, self.line_wrapping()) {
            (Some(w), true) if self.geo.char_width > 0.0 => {
                let cols = (w / self.geo.char_width).max(1.0);
                let len = doc.line(line + 1).length() as f32;
                lh * (1.0 + ((len - cols) / cols).ceil().max(0.0))
            }
            _ => lh,
        }
    }

    pub(crate) fn reestimate(&mut self) {
        let doc = self.state.doc().clone();
        let estimates: Vec<f32> = (0..self.heights.len()).map(|i| self.estimate(&doc, i)).collect();
        self.heights.reestimate(|i| estimates[i]);
    }

    pub(crate) fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.cursor_visible = true;
        self.blink_epoch += 1;
        let epoch = self.blink_epoch;
        self._blink = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(530)).await;
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

    // ---- layout queries ----

    /// Lay out the line with the given 0-based index.
    pub(crate) fn layout(&self, window: &mut Window, line: usize) -> LineLayout {
        let l = self.state.doc().line(line + 1);
        let marked = self
            .marked
            .as_ref()
            .and_then(|m| (m.start >= l.from && m.end <= l.to).then(|| (m.start - l.from, m.end - l.from)));
        layout_line(
            window,
            &l.text,
            &self.style.font,
            px(self.style.font_size),
            self.style.text,
            self.line_height(),
            if self.line_wrapping() { self.geo.wrap_width } else { None },
            self.state.tab_size(),
            marked,
        )
    }

    /// Document coordinates (x from the text start, y from the document top)
    /// of a position: (x, row top, row bottom).
    pub(crate) fn doc_coords(&self, window: &mut Window, pos: usize, assoc: i32) -> (f32, f32, f32) {
        let line = self.state.doc().line_at(pos);
        let layout = self.layout(window, line.number - 1);
        let (x, y) = layout.coords(pos - line.from, assoc);
        let top = self.heights.top(line.number - 1) + y;
        (x, top, top + self.line_height())
    }

    /// The position at document coordinates.
    pub(crate) fn pos_at_doc(&self, window: &mut Window, x: f32, y: f32) -> (usize, i32) {
        let doc = self.state.doc();
        if y < 0.0 {
            return (0, 1);
        }
        if y >= self.heights.total() {
            return (doc.len(), -1);
        }
        let n = self.heights.line_at(y);
        let line = doc.line(n + 1);
        let layout = self.layout(window, n);
        let (off, assoc) = layout.pos_at(x, y - self.heights.top(n));
        (line.from + off, assoc)
    }

    /// Screen position of document coordinates (0, 0).
    pub(crate) fn origin(&self) -> Point<Pixels> {
        let b = self.geo.bounds;
        Point::new(
            b.origin.x + px(self.geo.gutter_width + self.style.padding_left - self.scroll.0),
            b.origin.y + px(self.style.padding_y - self.scroll.1),
        )
    }

    pub(crate) fn pos_at_point(&self, window: &mut Window, p: Point<Pixels>) -> (usize, i32) {
        let o = self.origin();
        self.pos_at_doc(window, f32::from(p.x - o.x), f32::from(p.y - o.y))
    }

    pub(crate) fn visible_height(&self) -> f32 {
        f32::from(self.geo.bounds.size.height)
    }

    pub(crate) fn max_scroll_y(&self) -> f32 {
        (self.heights.total() + self.style.padding_y * 2.0 - self.visible_height()).max(0.0)
    }

    pub(crate) fn max_scroll_x(&self) -> f32 {
        if self.line_wrapping() {
            return 0.0;
        }
        let text_w =
            f32::from(self.geo.bounds.size.width) - self.geo.gutter_width - self.style.padding_left - SCROLLBAR_WIDTH;
        (self.max_line_width + self.geo.char_width * 2.0 - text_w).max(0.0)
    }

    // ---- motion (ports of cursor.ts, logical order) ----

    fn move_by_char(
        &self,
        start: &SelectionRange,
        forward: bool,
        by: Option<&dyn Fn(&str) -> Box<dyn FnMut(&str) -> bool>>,
    ) -> SelectionRange {
        let doc = self.state.doc();
        let mut line = doc.line_at(start.head());
        let mut cur = *start;
        let mut check: Option<Box<dyn FnMut(&str) -> bool>> = None;
        loop {
            let off = cur.head() - line.from;
            let next_off = find_cluster_break(&line.text, off, forward, true);
            let (next, ch) = if next_off == off {
                if line.number == if forward { doc.lines() } else { 1 } {
                    return cur;
                }
                line = doc.line(if forward { line.number + 1 } else { line.number - 1 });
                let pos = if forward { line.from } else { line.to };
                (EditorSelection::cursor_with(pos, if forward { -1 } else { 1 }, None, None), "\n".to_string())
            } else {
                let (a, b) = if forward { (off, next_off) } else { (next_off, off) };
                let next = EditorSelection::cursor_with(line.from + next_off, if forward { -1 } else { 1 }, None, None);
                (next, line.text[a..b].to_string())
            };
            match &mut check {
                None => {
                    let Some(by) = by else { return next };
                    check = Some(by(&ch));
                }
                Some(f) => {
                    if !f(&ch) {
                        return cur;
                    }
                }
            }
            cur = next;
        }
    }

    fn move_by_group(&self, start: &SelectionRange, forward: bool) -> SelectionRange {
        let categorize = Rc::new(self.state.char_categorizer(start.head()));
        let by = move |initial: &str| -> Box<dyn FnMut(&str) -> bool> {
            let categorize = categorize.clone();
            let mut cat = categorize(initial);
            Box::new(move |next: &str| {
                let next_cat = categorize(next);
                if cat == CharCategory::Space {
                    cat = next_cat;
                }
                cat == next_cat
            })
        };
        self.move_by_char(start, forward, Some(&by))
    }

    fn move_to_line_boundary(
        &self,
        window: &mut Window,
        start: &SelectionRange,
        forward: bool,
        include_wrap: bool,
    ) -> SelectionRange {
        let line = self.state.doc().line_at(start.head());
        if include_wrap && self.line_wrapping() {
            let layout = self.layout(window, line.number - 1);
            let assoc = if start.assoc() < 0 && start.head() > line.from { -1 } else { 1 };
            let (a, b) = layout.row_range(start.head() - line.from, assoc);
            return if forward {
                EditorSelection::cursor_with(line.from + b, if line.from + b == line.to { 1 } else { -1 }, None, None)
            } else {
                EditorSelection::cursor_with(line.from + a, 1, None, None)
            };
        }
        EditorSelection::cursor_with(
            if forward { line.to } else { line.from },
            if forward { 1 } else { -1 },
            None,
            None,
        )
    }

    fn move_vertically(
        &self,
        window: &mut Window,
        start: &SelectionRange,
        forward: bool,
        distance: Option<f32>,
    ) -> SelectionRange {
        let pos = start.head();
        if pos == if forward { self.state.doc().len() } else { 0 } {
            return EditorSelection::cursor_with(pos, start.assoc(), None, None);
        }
        let side = if start.assoc() != 0 {
            start.assoc()
        } else if if start.is_empty() { forward } else { pos == start.from } {
            1
        } else {
            -1
        };
        let (x, top, bottom) = self.doc_coords(window, pos, side);
        let goal = start.goal_column.map_or(x, |g| g as f32);
        let start_y = if forward { bottom } else { top };
        let half = self.line_height() / 2.0;
        let dist = distance.unwrap_or(half);
        let dir = if forward { 1.0 } else { -1.0 };
        let mut scan = 0.0;
        loop {
            let y = start_y + (dist + scan) * dir;
            let (p, assoc) = self.pos_at_doc(window, goal, y);
            if if forward { y > self.heights.total() } else { y < 0.0 } {
                return EditorSelection::cursor_with(p, assoc, None, None);
            }
            let (_, t, b) = self.doc_coords(window, p, assoc);
            let mid = (t + b) / 2.0;
            if if forward { mid > start_y } else { mid < start_y } {
                return EditorSelection::cursor_with(p, assoc, None, Some(goal as f64));
            }
            scan += half;
        }
    }

    // ---- command helpers ----

    fn move_sel(&mut self, cx: &mut Context<Self>, how: impl FnMut(&SelectionRange) -> SelectionRange) -> bool {
        let sel = self.state.selection();
        let next = EditorSelection::create(sel.ranges.iter().map(how).collect(), sel.main_index);
        if next.eq(sel, true) {
            return false;
        }
        self.dispatch_spec(TransactionSpec::new().selection(next).scroll_into_view().user_event("select"), cx);
        true
    }

    fn extend_sel(
        &mut self,
        cx: &mut Context<Self>,
        forward: bool,
        mut how: impl FnMut(&SelectionRange) -> SelectionRange,
    ) -> bool {
        let sel = self.state.selection();
        let next = EditorSelection::create(
            sel.ranges
                .iter()
                .map(|r| {
                    let mut r = *r;
                    if r.undirectional() && (r.head() >= r.anchor()) != forward {
                        r = EditorSelection::range(r.head(), r.anchor());
                    }
                    let head = how(&r);
                    EditorSelection::range_with(r.anchor(), head.head(), head.goal_column, None, head.assoc())
                })
                .collect(),
            sel.main_index,
        );
        if next.eq(sel, false) {
            return false;
        }
        self.dispatch_spec(TransactionSpec::new().selection(next).scroll_into_view().user_event("select"), cx);
        true
    }

    fn range_end(r: &SelectionRange, forward: bool) -> SelectionRange {
        EditorSelection::cursor(if forward { r.to } else { r.from })
    }

    pub(crate) fn cursor_char(&mut self, forward: bool, cx: &mut Context<Self>) -> bool {
        let next: Vec<SelectionRange> = self
            .state
            .selection()
            .ranges
            .iter()
            .map(|r| if r.is_empty() { self.move_by_char(r, forward, None) } else { Self::range_end(r, forward) })
            .collect();
        let mut it = next.into_iter();
        self.move_sel(cx, |_| it.next().unwrap())
    }

    pub(crate) fn select_char(&mut self, forward: bool, cx: &mut Context<Self>) -> bool {
        let snapshot = self.state.clone();
        let this = &*self;
        let heads: Vec<SelectionRange> = snapshot
            .selection()
            .ranges
            .iter()
            .map(|r| {
                let mut r = *r;
                if r.undirectional() && (r.head() >= r.anchor()) != forward {
                    r = EditorSelection::range(r.head(), r.anchor());
                }
                this.move_by_char(&r, forward, None)
            })
            .collect();
        let mut it = heads.into_iter();
        self.extend_sel(cx, forward, |_| it.next().unwrap())
    }

    pub(crate) fn cursor_group(&mut self, forward: bool, select: bool, cx: &mut Context<Self>) -> bool {
        let heads: Vec<SelectionRange> = self
            .state
            .selection()
            .ranges
            .iter()
            .map(|r| {
                if select || r.is_empty() {
                    self.move_by_group(&EditorSelection::cursor(r.head()), forward)
                } else {
                    Self::range_end(r, forward)
                }
            })
            .collect();
        let mut it = heads.into_iter();
        if select {
            self.extend_sel(cx, forward, |_| it.next().unwrap())
        } else {
            self.move_sel(cx, |_| it.next().unwrap())
        }
    }

    pub(crate) fn cursor_line(
        &mut self,
        forward: bool,
        select: bool,
        page: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let distance = page.then(|| (self.visible_height() - 5.0).max(self.line_height()));
        let heads: Vec<SelectionRange> = self
            .state
            .selection()
            .ranges
            .iter()
            .map(|r| {
                if !select && !r.is_empty() {
                    return Self::range_end(r, forward);
                }
                let moved = self.move_vertically(window, r, forward, distance);
                if !select && !page && moved.head() == r.head() {
                    self.move_to_line_boundary(window, r, forward, true)
                } else {
                    moved
                }
            })
            .collect();
        if page {
            self.scroll.1 =
                (self.scroll.1 + distance.unwrap() * if forward { 1.0 } else { -1.0 }).clamp(0.0, self.max_scroll_y());
        }
        let mut it = heads.into_iter();
        if select {
            self.extend_sel(cx, forward, |_| it.next().unwrap())
        } else {
            self.move_sel(cx, |_| it.next().unwrap())
        }
    }

    pub(crate) fn line_boundary(
        &mut self,
        forward: bool,
        select: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let heads: Vec<SelectionRange> = self
            .state
            .selection()
            .ranges
            .iter()
            .map(|r| {
                let line = self.state.doc().line_at(r.head());
                let mut moved = self.move_to_line_boundary(window, r, forward, true);
                if moved.head() == r.head() && moved.head() != if forward { line.to } else { line.from } {
                    moved = self.move_to_line_boundary(window, r, forward, false);
                }
                if !forward && moved.head() == line.from && line.length() > 0 {
                    let space = line.text.len() - line.text.trim_start().len();
                    if space > 0 && r.head() != line.from + space {
                        moved = EditorSelection::cursor(line.from + space);
                    }
                }
                moved
            })
            .collect();
        let mut it = heads.into_iter();
        if select {
            self.extend_sel(cx, forward, |_| it.next().unwrap())
        } else {
            self.move_sel(cx, |_| it.next().unwrap())
        }
    }

    pub(crate) fn add_cursor(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let state = self.state.clone();
        let sel = state.selection();
        let mut ranges = sel.ranges.clone();
        for range in &sel.ranges {
            let line = state.doc().line_at(range.head());
            if if forward { line.to < state.doc().len() } else { line.from > 0 } {
                let mut cur = *range;
                loop {
                    let next = self.move_vertically(window, &cur, forward, None);
                    if next.head() < line.from || next.head() > line.to {
                        if !ranges.iter().any(|r| r.head() == next.head()) {
                            ranges.push(next);
                        }
                        break;
                    } else if next.head() == cur.head() {
                        break;
                    }
                    cur = next;
                }
            }
        }
        if ranges.len() == sel.ranges.len() {
            return false;
        }
        let main = ranges.len() - 1;
        self.dispatch_spec(
            TransactionSpec::new()
                .selection(EditorSelection::create(ranges, main))
                .scroll_into_view()
                .user_event("select"),
            cx,
        );
        true
    }

    pub(crate) fn delete_line(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let state = self.state.clone();
        if state.read_only() {
            return false;
        }
        let len = state.doc().len();
        let mut specs = Vec::new();
        let mut upto = 0;
        for r in &state.selection().ranges {
            let a = state.doc().line_at(r.from);
            let mut b = state.doc().line_at(r.to);
            if !r.is_empty() && r.to == b.from {
                b = state.doc().line_at(r.to - 1);
            }
            if a.number <= upto {
                continue;
            }
            upto = b.number;
            let (mut from, mut to) = (a.from, b.to);
            if from > 0 {
                from -= 1;
            } else if to < len {
                to += 1;
            }
            specs.push(ChangeSpec::delete(from, to));
        }
        let changes = state.changes(specs);
        let moved: Vec<SelectionRange> =
            state.selection().ranges.iter().map(|r| self.move_vertically(window, r, true, None)).collect();
        let selection = EditorSelection::create(moved, state.selection().main_index).map(changes.desc(), -1);
        self.dispatch_spec(
            TransactionSpec::new().changes(changes).selection(selection).scroll_into_view().user_event("delete.line"),
            cx,
        );
        true
    }

    // ---- clipboard ----

    fn copied(&self) -> (String, Vec<(usize, usize)>, bool) {
        let state = &self.state;
        let mut content = Vec::new();
        let mut ranges = Vec::new();
        for r in &state.selection().ranges {
            if !r.is_empty() {
                content.push(state.slice_doc(r.from, r.to));
                ranges.push((r.from, r.to));
            }
        }
        let mut linewise = false;
        if content.is_empty() {
            let mut upto = 0;
            for r in &state.selection().ranges {
                let line = state.doc().line_at(r.from);
                if line.number > upto {
                    content.push(line.text.clone());
                    ranges.push((line.from, state.doc().len().min(line.to + 1)));
                }
                upto = line.number;
            }
            linewise = true;
        }
        (content.join(&state.line_break()), ranges, linewise)
    }

    pub(crate) fn copy(&mut self, cut: bool, cx: &mut Context<Self>) -> bool {
        let (text, ranges, linewise) = self.copied();
        self.last_linewise_copy = linewise.then(|| text.clone());
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if cut {
            let changes: Vec<ChangeSpec> = ranges.iter().map(|&(a, b)| ChangeSpec::delete(a, b)).collect();
            self.dispatch_spec(TransactionSpec::new().changes(changes).scroll_into_view().user_event("delete.cut"), cx);
        }
        true
    }

    pub(crate) fn paste(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(input) = cx.read_from_clipboard().and_then(|item| item.text()) else { return false };
        let state = self.state.clone();
        let text = state.to_text(&input);
        let ranges = &state.selection().ranges;
        let by_line = text.lines() == ranges.len();
        let linewise = self.last_linewise_copy.as_deref() == Some(text.to_string().as_str())
            && ranges.iter().all(|r| r.is_empty());
        let mut i = 1;
        let spec: TransactionSpec = if linewise {
            let mut last_line = None;
            state
                .change_by_range(|range| {
                    let line = state.doc().line_at(range.from);
                    if last_line == Some(line.from) {
                        return RangeChange::new(range);
                    }
                    last_line = Some(line.from);
                    let piece = if by_line {
                        let l = text.line(i).text;
                        i += 1;
                        l
                    } else {
                        input.clone()
                    };
                    let insert = state.to_text(&format!("{piece}{}", state.line_break()));
                    RangeChange::new(EditorSelection::cursor(range.from + insert.len()))
                        .changes(ChangeSpec::insert(line.from, insert))
                })
                .into()
        } else if by_line && ranges.len() > 1 {
            state
                .change_by_range(|range| {
                    let line = text.line(i);
                    i += 1;
                    RangeChange::new(EditorSelection::cursor(range.from + line.length()))
                        .changes(ChangeSpec::replace(range.from, range.to, line.text))
                })
                .into()
        } else {
            state.replace_selection(text)
        };
        let tr = state.update_many(vec![spec, TransactionSpec::new().scroll_into_view().user_event("input.paste")]);
        self.dispatch(tr, cx);
        true
    }

    // ---- event handlers ----

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keymap = self.keymap.clone();
        if keymap.run(self, &event.keystroke, window, cx) {
            cx.stop_propagation();
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        let b = self.geo.bounds;
        if f32::from(event.position.x - b.origin.x) > f32::from(b.size.width) - SCROLLBAR_WIDTH {
            self.scrollbar_drag = Some((f32::from(event.position.y), self.scroll.1));
            return;
        }
        let start = self.pos_at_point(window, event.position);
        let m = event.modifiers;
        let count = (event.click_count.max(1) - 1) % 3 + 1;
        self.mouse = Some(MouseSelection {
            start,
            start_point: event.position,
            count,
            extend: m.shift,
            multiple: if cfg!(target_os = "macos") { m.platform } else { m.control },
            rectangular: m.alt && !m.shift,
            start_sel: self.state.selection().clone(),
        });
        self.mouse_select(event.position, window, cx);
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.last_pointer = Some(event.position);
        if event.pressed_button != Some(MouseButton::Left) {
            self.mouse = None;
            self.scrollbar_drag = None;
            return;
        }
        if let Some((y0, scroll0)) = self.scrollbar_drag {
            let track = self.visible_height();
            let total = self.heights.total() + self.style.padding_y * 2.0;
            let dy = f32::from(event.position.y) - y0;
            self.scroll.1 = (scroll0 + dy * total / track).clamp(0.0, self.max_scroll_y());
            cx.notify();
            return;
        }
        if self.mouse.is_some() {
            self.mouse_select(event.position, window, cx);
        }
    }

    fn on_mouse_up(&mut self, _event: &MouseUpEvent, _window: &mut Window, _cx: &mut Context<Self>) {
        self.mouse = None;
        self.scrollbar_drag = None;
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(self.line_height()));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        let (dx, dy) = if event.modifiers.shift && dx == 0.0 { (dy, 0.0) } else { (dx, dy) };
        self.scroll.1 = (self.scroll.1 - dy).clamp(0.0, self.max_scroll_y());
        self.scroll.0 = (self.scroll.0 - dx).clamp(0.0, self.max_scroll_x());
        cx.notify();
    }

    pub(crate) fn mouse_select(&mut self, point: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = &self.mouse else { return };
        let cur = self.pos_at_point(window, point);
        let selection = if m.rectangular {
            let doc = self.state.doc();
            let (a, b) = (doc.line_at(m.start.0).number, doc.line_at(cur.0).number);
            let o = self.origin();
            let (x0, x1) = (f32::from(m.start_point.x - o.x), f32::from(point.x - o.x));
            let lines: Vec<usize> = if a <= b { (a..=b).collect() } else { (b..=a).rev().collect() };
            let ranges: Vec<SelectionRange> = lines
                .into_iter()
                .map(|n| {
                    let line = doc.line(n);
                    let y = self.heights.top(n - 1) + self.line_height() / 2.0;
                    let anchor = self.pos_at_doc(window, x0, y).0.clamp(line.from, line.to);
                    let head = self.pos_at_doc(window, x1, y).0.clamp(line.from, line.to);
                    EditorSelection::range(anchor, head)
                })
                .collect();
            let main = ranges.len() - 1;
            EditorSelection::create(ranges, main)
        } else {
            let range_for = |pos: usize, bias: i32| match m.count {
                1 => EditorSelection::cursor_with(pos, bias, None, None),
                2 => group_at(&self.state, pos, bias),
                _ => {
                    let line = self.state.doc().line_at(pos);
                    let to = if line.to < self.state.doc().len() { line.to + 1 } else { line.to };
                    EditorSelection::range(line.from, to)
                }
            };
            let mut range = range_for(cur.0, cur.1);
            if m.start.0 != cur.0 && !m.extend {
                let s = range_for(m.start.0, m.start.1);
                let (from, to) = (s.from.min(range.from), s.to.max(range.to));
                range =
                    if from < range.from { EditorSelection::range(from, to) } else { EditorSelection::range(to, from) };
            }
            if m.extend {
                m.start_sel.replace_range(m.start_sel.main().extend(range.from, range.to, 0), None)
            } else if m.multiple {
                m.start_sel.add_range(range, true)
            } else {
                EditorSelection::create(vec![range], 0)
            }
        };
        if !selection.eq(self.state.selection(), true) {
            self.dispatch_spec(TransactionSpec::new().selection(selection).user_event("select.pointer"), cx);
        }
    }

    // ---- text input (used by the input handler) ----

    pub(crate) fn utf16_to_byte(&self, off: usize) -> usize {
        let rope = self.state.doc().rope();
        rope.byte_of_utf16_code_unit(off.min(rope.utf16_len()))
    }

    pub(crate) fn byte_to_utf16(&self, off: usize) -> usize {
        self.state.doc().rope().utf16_code_unit_of_byte(off)
    }

    pub(crate) fn replace_text(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        mark: bool,
        new_sel: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.clone();
        let target = range.or_else(|| self.marked.clone());
        let tr = match target {
            None if !mark => {
                let spec = state.replace_selection(text);
                state.update_many(vec![spec, TransactionSpec::new().scroll_into_view().user_event("input.type")])
            }
            _ => {
                let main = state.selection().main();
                let r = target.clone().unwrap_or(main.from..main.to);
                let sel = match &new_sel {
                    Some(s) => EditorSelection::single(r.start + s.start, r.start + s.end),
                    None => EditorSelection::single(r.start + text.len(), r.start + text.len()),
                };
                state.update(
                    TransactionSpec::new()
                        .changes(ChangeSpec::replace(r.start, r.end, text))
                        .selection(sel)
                        .scroll_into_view()
                        .user_event(if mark { "input.type.compose" } else { "input.type" }),
                )
            }
        };
        let start = target.as_ref().map_or(state.selection().main().from, |r| r.start);
        self.marked = (mark && !text.is_empty()).then(|| start..start + text.len());
        self.dispatch(tr, cx);
    }

    pub(crate) fn set_focus_state(&mut self, focused: bool, cx: &mut Context<Self>) {
        if focused != self.focused {
            self.focused = focused;
            if focused {
                self.restart_blink(cx);
            }
        }
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
