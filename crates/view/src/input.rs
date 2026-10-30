//! Input handling: keys (through the keymap), typed text and IME
//! composition (through GPUI's input handler), the clipboard, mouse
//! selection and scrolling. Port of the relevant parts of `input.ts` and
//! `rectangular-selection.ts`.

use std::ops::Range;

use codemirror_state::{ChangeSpec, EditorSelection, RangeChange, SelectionRange, TransactionSpec};
use gpui::{
    point, px, Bounds, ClipboardItem, Context, EntityInputHandler, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, UTF16Selection, Window,
};

use crate::cursor::{group_at, PosAssoc};
use crate::editorview::{EditorView, SCROLLBAR_WIDTH};
use crate::extension::EDITABLE;
use crate::keymap::{key_name, run_scope_handlers};

pub(crate) struct MouseSelection {
    start: PosAssoc,
    start_point: Point<Pixels>,
    count: usize,
    extend: bool,
    multiple: bool,
    rectangular: bool,
    start_sel: EditorSelection,
}

impl EditorView {
    fn editable(&self) -> bool {
        *self.state().facet(&EDITABLE)
    }

    pub(crate) fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if run_scope_handlers(self, &event.keystroke, window, cx, "editor") || self.clipboard_key(event, cx) {
            cx.stop_propagation();
        }
    }

    // The clipboard keys, which a browser handles for CodeMirror.
    fn clipboard_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let m = &event.keystroke.modifiers;
        let primary = if cfg!(target_os = "macos") { m.platform } else { m.control };
        let key = key_name(&event.keystroke.key);
        match (primary, m.shift, m.alt, key.as_str()) {
            (true, false, false, "c") | (true, false, false, "Insert") => self.copy(false, cx),
            (true, false, false, "x") | (false, true, false, "Delete") => self.copy(true, cx),
            (true, false, false, "v") | (false, true, false, "Insert") => self.paste_from_clipboard(cx),
            _ => false,
        }
    }

    /// Replace the selection with typed text.
    pub fn insert_text(&mut self, text: &str, user_event: &str, cx: &mut Context<Self>) {
        let spec = self.state().replace_selection(text);
        let tr = self.state().update_many(vec![spec, TransactionSpec::new().scroll_into_view().user_event(user_event)]);
        self.dispatch(tr, cx);
    }

    fn paste_from_clipboard(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.editable() {
            return false;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else { return false };
        self.paste(&text, cx);
        true
    }

    /// Paste text, the way CodeMirror does: one line per cursor when the
    /// line count matches, and above the cursor lines after a line-wise
    /// copy.
    pub fn paste(&mut self, input: &str, cx: &mut Context<Self>) {
        let state = self.state().clone();
        let text = state.to_text(input);
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
                        input.to_string()
                    };
                    let insert = state.to_text(&format!("{piece}{}", state.line_break()));
                    RangeChange::new(EditorSelection::cursor(range.from + insert.len()))
                        .changes(ChangeSpec::insert(line.from, insert))
                })
                .into()
        } else if by_line {
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
    }

    /// The text a copy would put on the clipboard, the ranges it covers,
    /// and whether it is a line-wise copy (nothing was selected).
    pub fn copied_range(&self) -> (String, Vec<(usize, usize)>, bool) {
        let state = self.state();
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
            // Nothing selected: copy whole lines.
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

    fn copy(&mut self, cut: bool, cx: &mut Context<Self>) -> bool {
        let (text, ranges, linewise) = self.copied_range();
        self.last_linewise_copy = linewise.then(|| text.clone());
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if cut && self.editable() {
            let changes: Vec<ChangeSpec> = ranges.iter().map(|&(a, b)| ChangeSpec::delete(a, b)).collect();
            self.dispatch_spec(TransactionSpec::new().changes(changes).scroll_into_view().user_event("delete.cut"), cx);
        }
        true
    }

    pub(crate) fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        let b = self.geo.bounds;
        if f32::from(event.position.x - b.origin.x) > f32::from(b.size.width) - SCROLLBAR_WIDTH {
            self.scrollbar_drag = Some((f32::from(event.position.y), self.scroll.1));
            return;
        }
        let start = self.pos_at_coords(window, event.position);
        let m = event.modifiers;
        self.mouse = Some(MouseSelection {
            start,
            start_point: event.position,
            count: (event.click_count.max(1) - 1) % 3 + 1,
            extend: m.shift,
            multiple: if cfg!(target_os = "macos") { m.platform } else { m.control } && !m.alt,
            rectangular: m.alt && !m.shift,
            start_sel: self.state().selection().clone(),
        });
        self.mouse_select(event.position, window, cx);
    }

    pub(crate) fn on_mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.last_pointer = Some(event.position);
        if event.pressed_button != Some(MouseButton::Left) {
            self.mouse = None;
            self.scrollbar_drag = None;
            return;
        }
        if let Some((y0, scroll0)) = self.scrollbar_drag {
            let track = self.visible_height();
            let total = self.content_height() as f32 + self.theme().padding_y * 2.0;
            let dy = f32::from(event.position.y) - y0;
            self.scroll.1 = (scroll0 + dy * total / track).clamp(0.0, self.max_scroll_y());
            cx.notify();
            return;
        }
        if self.mouse.is_some() {
            self.mouse_select(event.position, window, cx);
        }
    }

    pub(crate) fn on_mouse_up(&mut self, _event: &MouseUpEvent, _window: &mut Window, _cx: &mut Context<Self>) {
        self.mouse = None;
        self.scrollbar_drag = None;
    }

    pub(crate) fn on_scroll(&mut self, event: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(self.default_line_height()));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        let (dx, dy) = if event.modifiers.shift && dx == 0.0 { (dy, 0.0) } else { (dx, dy) };
        self.scroll.1 = (self.scroll.1 - dy).clamp(0.0, self.max_scroll_y());
        self.scroll.0 = (self.scroll.0 - dx).clamp(0.0, self.max_scroll_x());
        cx.notify();
    }

    fn range_for_click(&self, pos: usize, bias: i32, count: usize) -> SelectionRange {
        match count {
            1 => EditorSelection::cursor_with(pos, bias, None, None),
            2 => group_at(self.state(), pos, bias),
            _ => {
                let line = self.state().doc().line_at(pos);
                let to = if line.to < self.state().doc().len() { line.to + 1 } else { line.to };
                EditorSelection::range(line.from, to)
            }
        }
    }

    pub(crate) fn mouse_select(&mut self, p: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = &self.mouse else { return };
        let cur = self.pos_at_coords(window, p);
        let selection = if m.rectangular {
            self.rectangle_selection(window, m.start_point, p, m.start.pos, cur.pos)
        } else {
            let mut range = self.range_for_click(cur.pos, cur.assoc, m.count);
            if m.start.pos != cur.pos && !m.extend {
                let s = self.range_for_click(m.start.pos, m.start.assoc, m.count);
                let (from, to) = (s.from.min(range.from), s.to.max(range.to));
                range =
                    if from < range.from { EditorSelection::range(from, to) } else { EditorSelection::range(to, from) };
            }
            let start_sel = &m.start_sel;
            if m.extend {
                start_sel.replace_range(start_sel.main().extend(range.from, range.to, 0), None)
            } else if m.count == 1 && m.multiple {
                remove_range_around(start_sel, cur.pos).unwrap_or_else(|| start_sel.add_range(range, true))
            } else if m.multiple {
                start_sel.add_range(range, true)
            } else {
                EditorSelection::create(vec![range], 0)
            }
        };
        if !selection.eq(self.state().selection(), true) {
            self.dispatch_spec(TransactionSpec::new().selection(selection).user_event("select.pointer"), cx);
        }
    }

    // A rectangular (column) selection between two window points. Lines
    // between them get a range from the start column to the end column.
    fn rectangle_selection(
        &self,
        window: &mut Window,
        start: Point<Pixels>,
        end: Point<Pixels>,
        start_pos: usize,
        end_pos: usize,
    ) -> EditorSelection {
        let doc = self.state().doc();
        let (a, b) = (doc.line_at(start_pos).number, doc.line_at(end_pos).number);
        let o = self.origin();
        let (x0, x1) = (f32::from(start.x - o.x), f32::from(end.x - o.x));
        let lines: Vec<usize> = if a <= b { (a..=b).collect() } else { (b..=a).rev().collect() };
        let ranges: Vec<SelectionRange> = lines
            .into_iter()
            .map(|n| {
                let line = doc.line(n);
                let y = self.line_block_at(line.from).top + self.default_line_height() as f64 / 2.0;
                let anchor = self.pos_at_doc_coords(window, x0, y, None).pos.clamp(line.from, line.to);
                let head = self.pos_at_doc_coords(window, x1, y, None).pos.clamp(line.from, line.to);
                EditorSelection::range(anchor, head)
            })
            .collect();
        let main = ranges.len() - 1;
        EditorSelection::create(ranges, main)
    }

    // Replace a range (or the composition, or the selection) with text,
    // marking it as composition text when `mark` is set.
    fn replace_text(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        mark: bool,
        new_sel: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        let state = self.state().clone();
        let target = range.or_else(|| self.marked.clone());
        let start = target.as_ref().map_or(state.selection().main().from, |r| r.start);
        let tr = match &target {
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
        self.marked = (mark && !text.is_empty()).then(|| start..start + text.len());
        self.dispatch(tr, cx);
    }

    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.utf16_to_byte(r.start)..self.utf16_to_byte(r.end)
    }

    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.byte_to_utf16(r.start)..self.byte_to_utf16(r.end)
    }
}

fn remove_range_around(sel: &EditorSelection, pos: usize) -> Option<EditorSelection> {
    if sel.ranges.len() < 2 {
        return None;
    }
    let i = sel.ranges.iter().position(|r| r.from <= pos && r.to >= pos)?;
    let mut ranges = sel.ranges.clone();
    ranges.remove(i);
    let main = if sel.main_index == i { 0 } else { sel.main_index - usize::from(sel.main_index > i) };
    Some(EditorSelection::create(ranges, main))
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.state().slice_doc(range.start, range.end))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let main = self.state().selection().main();
        Some(UTF16Selection {
            range: self.range_to_utf16(&(main.from..main.to)),
            reversed: main.head() < main.anchor(),
        })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.editable() || self.state().read_only() {
            return;
        }
        let range = range_utf16.map(|r| self.range_from_utf16(&r));
        self.replace_text(range, text, false, None, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.editable() || self.state().read_only() {
            return;
        }
        let range = range_utf16.map(|r| self.range_from_utf16(&r));
        // The new selection is in UTF-16 units within the new text.
        let to_byte = |u: usize| {
            let mut units = 0;
            for (i, c) in new_text.char_indices() {
                if units >= u {
                    return i;
                }
                units += c.len_utf16();
            }
            new_text.len()
        };
        let sel = new_selected_range_utf16.map(|r| to_byte(r.start)..to_byte(r.end));
        self.replace_text(range, new_text, true, sel, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let pos = self.utf16_to_byte(range_utf16.start);
        let b = self.coords_at_pos(window, pos, 1);
        Some(Bounds::from_corners(b.origin, point(b.origin.x + px(1.0), b.bottom())))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let pos = self.pos_at_coords(window, point);
        Some(self.byte_to_utf16(pos.pos))
    }
}
