//! Input handling: keys, typed text, IME composition, the clipboard and
//! mouse selection. Port of the relevant parts of `input.ts` and
//! `rectangular-selection.ts`, reading egui events instead of DOM events.

use codemirror_state::{ChangeSpec, EditorSelection, RangeChange, SelectionRange, TransactionSpec};
use egui::{Event, Pos2, Vec2};

use crate::cursor::{group_at, PosAssoc};
use crate::editorview::EditorView;
use crate::extension::EDITABLE;
use crate::keymap::run_scope_handlers;

#[derive(Default)]
pub(crate) struct InputState {
    /// IME text being composed.
    pub composition: Option<String>,
    /// The text of the last line-wise copy, to paste it as whole lines.
    last_linewise_copy: Option<String>,
    mouse: Option<MouseSelection>,
    last_click: Option<(f64, Pos2, u8)>,
}

struct MouseSelection {
    start: PosAssoc,
    start_point: Pos2,
    count: u8,
    extend: bool,
    multiple: bool,
    rectangular: bool,
    start_sel: EditorSelection,
}

impl EditorView {
    fn editable(&self) -> bool {
        *self.state().facet(&EDITABLE)
    }

    pub(crate) fn handle_events(&mut self, ui: &egui::Ui) {
        let events = ui.input(|i| i.events.clone());
        let mut skip_text = false;
        for event in events {
            match event {
                Event::Key { key, pressed: true, modifiers, .. } => {
                    if run_scope_handlers(self, key, modifiers, "editor") {
                        // AltGr (Ctrl-Alt) also produces text; don't insert
                        // it when a binding handled the key.
                        skip_text = modifiers.ctrl && modifiers.alt;
                    }
                }
                Event::Text(text) => {
                    if std::mem::take(&mut skip_text) || !self.editable() || text.chars().all(char::is_control) {
                        continue;
                    }
                    self.insert_text(&text, "input.type");
                }
                Event::Paste(text) => {
                    if self.editable() {
                        self.paste(&text);
                    }
                }
                Event::Copy => self.copy(ui, false),
                Event::Cut => self.copy(ui, true),
                Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                    self.input.composition = (!text.is_empty()).then_some(text);
                }
                Event::Ime(egui::ImeEvent::Commit(text)) => {
                    self.input.composition = None;
                    if self.editable() && !text.is_empty() {
                        self.insert_text(&text, "input.type");
                    }
                }
                _ => {}
            }
        }
    }

    /// Replace the selection with typed text.
    pub fn insert_text(&mut self, text: &str, user_event: &str) {
        let spec = self.state().replace_selection(text);
        self.dispatch_spec_many(vec![spec, TransactionSpec::new().scroll_into_view().user_event(user_event)]);
    }

    fn dispatch_spec_many(&mut self, specs: Vec<TransactionSpec>) {
        let tr = self.state().update_many(specs);
        self.dispatch(tr);
    }

    /// Paste text, the way CodeMirror does: one line per cursor when the
    /// line count matches, and above the cursor lines after a line-wise
    /// copy.
    pub fn paste(&mut self, input: &str) {
        let state = self.state().clone();
        let text = state.to_text(input);
        let ranges = &state.selection().ranges;
        let by_line = text.lines() == ranges.len();
        let linewise = self.input.last_linewise_copy.as_deref() == Some(text.to_string().as_str())
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
        self.dispatch_spec_many(vec![spec, TransactionSpec::new().scroll_into_view().user_event("input.paste")]);
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

    fn copy(&mut self, ui: &egui::Ui, cut: bool) {
        let (text, ranges, linewise) = self.copied_range();
        if text.is_empty() && !linewise {
            return;
        }
        self.input.last_linewise_copy = linewise.then(|| text.clone());
        ui.ctx().copy_text(text);
        if cut && self.editable() {
            let changes: Vec<ChangeSpec> = ranges.iter().map(|&(a, b)| ChangeSpec::delete(a, b)).collect();
            self.dispatch_spec(TransactionSpec::new().changes(changes).scroll_into_view().user_event("delete.cut"));
        }
    }

    pub(crate) fn handle_mouse(&mut self, ui: &egui::Ui, resp: &egui::Response) {
        let (pressed, down, latest, origin, mods, now) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.latest_pos(),
                i.pointer.press_origin(),
                i.modifiers,
                i.time,
            )
        });
        if pressed && resp.contains_pointer() {
            let point = origin.or(latest).unwrap_or_default();
            let count = match self.input.last_click {
                Some((t, p, c)) if now - t < 0.45 && (p - point).length() < 6.0 => c % 3 + 1,
                _ => 1,
            };
            self.input.last_click = Some((now, point, count));
            let start = self.pos_at_coords(point);
            self.input.mouse = Some(MouseSelection {
                start,
                start_point: point,
                count,
                extend: mods.shift,
                multiple: mods.command && !mods.alt,
                rectangular: mods.alt && !mods.shift,
                start_sel: self.state().selection().clone(),
            });
            self.focus();
            self.mouse_select(point);
        } else if self.input.mouse.is_some() {
            if !down {
                self.input.mouse = None;
                return;
            }
            if let Some(point) = latest {
                self.mouse_select(point);
            }
            scroll_while_selecting(ui, !self.line_wrapping());
        }
    }

    fn range_for_click(&self, pos: usize, bias: i32, count: u8) -> SelectionRange {
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

    fn mouse_select(&mut self, point: Pos2) {
        let Some(m) = &self.input.mouse else { return };
        let cur = self.pos_at_coords(point);
        let selection = if m.rectangular {
            self.rectangle_selection(m.start_point, point, m.start.pos, cur.pos)
        } else {
            let mut range = self.range_for_click(cur.pos, cur.assoc, m.count);
            if m.start.pos != cur.pos && !m.extend {
                let start_range = self.range_for_click(m.start.pos, m.start.assoc, m.count);
                let from = start_range.from.min(range.from);
                let to = start_range.to.max(range.to);
                range =
                    if from < range.from { EditorSelection::range(from, to) } else { EditorSelection::range(to, from) };
            }
            let start_sel = &m.start_sel;
            if m.extend {
                let main = start_sel.main().extend(range.from, range.to, 0);
                start_sel.replace_range(main, None)
            } else if m.count == 1 && m.multiple {
                match remove_range_around(start_sel, cur.pos) {
                    Some(sel) => sel,
                    None => start_sel.add_range(range, true),
                }
            } else if m.multiple {
                start_sel.add_range(range, true)
            } else {
                EditorSelection::create(vec![range], 0)
            }
        };
        if !selection.eq(self.state().selection(), true) {
            self.dispatch_spec(TransactionSpec::new().selection(selection).user_event("select.pointer"));
        }
    }

    // A rectangular (column) selection between two screen points. Lines
    // between them get a range from the start column to the end column.
    fn rectangle_selection(&self, start: Pos2, end: Pos2, start_pos: usize, end_pos: usize) -> EditorSelection {
        let doc = self.state().doc();
        let (a, b) = (doc.line_at(start_pos).number, doc.line_at(end_pos).number);
        let origin = self.geo.origin;
        let (x0, x1) = (start.x - origin.x, end.x - origin.x);
        let mut ranges = Vec::new();
        let range: Box<dyn Iterator<Item = usize>> = if a <= b { Box::new(a..=b) } else { Box::new((b..=a).rev()) };
        for n in range {
            let line = doc.line(n);
            let block = self.line_block_at(line.from);
            let y = block.top + self.default_line_height() as f64 / 2.0;
            let anchor = self.pos_at_doc_coords(x0, y, None).pos.clamp(line.from, line.to);
            let head = self.pos_at_doc_coords(x1, y, None).pos.clamp(line.from, line.to);
            ranges.push(EditorSelection::range(anchor, head));
        }
        let main = ranges.len() - 1;
        EditorSelection::create(ranges, main)
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

/// Scroll while the user drags a selection past the edge of the view.
/// egui's scroll area does not scroll while a widget is dragged, so this
/// passes the wheel through, and scrolls faster the farther the pointer is
/// past an edge.
fn scroll_while_selecting(ui: &egui::Ui, horizontal: bool) {
    let view = ui.clip_rect();
    let wheel = ui.input(|i| i.smooth_scroll_delta);
    let mut delta = wheel;
    if wheel != Vec2::ZERO {
        ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
    }
    if let Some(pos) = ui.input(|i| i.pointer.latest_pos()) {
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        let speed = |dist: f32| (60.0 + dist.min(300.0) * 12.0) * dt;
        let mut edge = Vec2::ZERO;
        if pos.y < view.top() {
            edge.y = speed(view.top() - pos.y);
        } else if pos.y > view.bottom() {
            edge.y = -speed(pos.y - view.bottom());
        }
        if horizontal {
            if pos.x < view.left() {
                edge.x = speed(view.left() - pos.x);
            } else if pos.x > view.right() {
                edge.x = -speed(pos.x - view.right());
            }
        }
        if edge != Vec2::ZERO {
            delta += edge;
            ui.ctx().request_repaint();
        }
    }
    if delta != Vec2::ZERO {
        ui.scroll_with_delta_animation(delta, egui::style::ScrollAnimation::none());
    }
}
