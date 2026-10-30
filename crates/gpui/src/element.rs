//! The element that lays out and paints an [`EditorView`]: the lines in
//! view (measured into the line heights), selections, carets, the active
//! line, line numbers and a scrollbar.

use gpui::{
    fill, point, px, relative, size, App, Bounds, ContentMask, Element, ElementId, ElementInputHandler, Entity,
    GlobalElementId, InspectorElementId, IntoElement, LayoutId, PaintQuad, Pixels, Point, ShapedLine, SharedString,
    Style, TextAlign, TextRun, Window,
};

use crate::layout::LineLayout;
use crate::view::{EditorView, SCROLLBAR_WIDTH};

pub(crate) struct EditorElement {
    pub view: Entity<EditorView>,
}

pub(crate) struct Frame {
    lines: Vec<(Point<Pixels>, LineLayout)>,
    numbers: Vec<(Point<Pixels>, ShapedLine)>,
    backgrounds: Vec<PaintQuad>,
    selections: Vec<PaintQuad>,
    carets: Vec<PaintQuad>,
    gutter: Option<PaintQuad>,
    scrollbar: Option<PaintQuad>,
    text_area: Bounds<Pixels>,
    line_height: Pixels,
    background: PaintQuad,
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

const VIEWPORT_MARGIN: f32 = 100.0;

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Frame;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let focused = self.view.read(cx).focus_handle.is_focused(window);
        self.view.update(cx, |view, cx| {
            view.set_focus_state(focused, cx);
            view.frame(bounds, window, cx)
        })
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        frame: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.view.read(cx).focus_handle.clone();
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.view.clone()), cx);
        window.paint_quad(frame.background.clone());
        let line_height = frame.line_height;
        window.with_content_mask(Some(ContentMask { bounds: frame.text_area }), |window| {
            for q in frame.backgrounds.drain(..) {
                window.paint_quad(q);
            }
            for q in frame.selections.drain(..) {
                window.paint_quad(q);
            }
            for (origin, layout) in &frame.lines {
                let _ = layout.wrapped.paint(*origin, line_height, TextAlign::Left, None, window, cx);
            }
            for q in frame.carets.drain(..) {
                window.paint_quad(q);
            }
        });
        if let Some(g) = frame.gutter.take() {
            window.paint_quad(g);
            for (origin, line) in &frame.numbers {
                let _ = line.paint(*origin, line_height, window, cx);
            }
        }
        if let Some(s) = frame.scrollbar.take() {
            window.paint_quad(s);
        }
    }
}

impl EditorView {
    /// Lay out the visible lines for a frame and collect what to paint.
    pub(crate) fn frame(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut gpui::Context<Self>) -> Frame {
        let style = self.style.clone();
        let line_height = self.line_height();
        let font_size = px(style.font_size);
        let font_id = window.text_system().resolve_font(&style.font);
        let char_width =
            window.text_system().ch_width(font_id, font_size).map(f32::from).unwrap_or(style.font_size * 0.55);
        let gutter_width = if self.line_numbers() {
            let digits = self.state().doc().lines().to_string().len().max(2);
            digits as f32 * char_width + 16.0
        } else {
            0.0
        };
        let width = f32::from(bounds.size.width);
        let wrap_width = self
            .line_wrapping()
            .then(|| (width - gutter_width - style.padding_left - SCROLLBAR_WIDTH - 2.0).max(char_width * 4.0));
        let changed = !self.geo.valid
            || (self.geo.line_height - line_height).abs() > 0.1
            || match (wrap_width, self.geo.wrap_width) {
                (Some(a), Some(b)) => (a - b).abs() > char_width / 2.0,
                (a, b) => a.is_some() != b.is_some(),
            };
        // Keep the first visible line in place while heights change.
        let anchor = self.heights.line_at(self.scroll.1 - style.padding_y);
        let anchor_offset = self.scroll.1 - self.heights.top(anchor);
        self.geo = crate::view::Geometry { bounds, line_height, char_width, gutter_width, wrap_width, valid: true };
        if changed {
            self.reestimate();
        }

        let visible = f32::from(bounds.size.height);
        let measure = |view: &mut EditorView, window: &mut Window| {
            let top = view.scroll.1 - style.padding_y - VIEWPORT_MARGIN;
            let bottom = view.scroll.1 + visible + VIEWPORT_MARGIN;
            let first = view.heights.line_at(top.max(0.0));
            let mut lines = Vec::new();
            let mut n = first;
            while n < view.heights.len() && view.heights.top(n) < bottom {
                let layout = view.layout(window, n);
                view.max_line_width = view.max_line_width.max(layout.width());
                view.heights.set(n, layout.height());
                lines.push((n, layout));
                n += 1;
            }
            lines
        };
        let mut lines = measure(self, window);
        let anchored = self.heights.top(anchor) + anchor_offset;
        if (anchored - self.scroll.1).abs() > 0.5 && !self.scroll_to_cursor && self.mouse.is_none() {
            self.scroll.1 = anchored.clamp(0.0, self.max_scroll_y());
            lines = measure(self, window);
        }

        // Scroll the main cursor into view.
        if self.scroll_to_cursor {
            self.scroll_to_cursor = false;
            let main = self.state().selection().main();
            let (x, top, bot) = self.doc_coords(window, main.head(), if main.assoc() != 0 { main.assoc() } else { 1 });
            let (top, bot) = (top + style.padding_y, bot + style.padding_y);
            let mut scrolled = false;
            if top - 2.0 < self.scroll.1 {
                self.scroll.1 = (top - 2.0).max(0.0);
                scrolled = true;
            } else if bot + 2.0 > self.scroll.1 + visible {
                self.scroll.1 = bot + 2.0 - visible;
                scrolled = true;
            }
            if !self.line_wrapping() {
                let text_w = width - gutter_width - style.padding_left - SCROLLBAR_WIDTH;
                if x - char_width * 2.0 < self.scroll.0 {
                    self.scroll.0 = (x - char_width * 4.0).max(0.0);
                    scrolled = true;
                } else if x + char_width * 2.0 > self.scroll.0 + text_w {
                    self.scroll.0 = x + char_width * 4.0 - text_w;
                    scrolled = true;
                }
            }
            if scrolled {
                lines = measure(self, window);
            }
        }
        // Scroll while selecting with the pointer outside the editor.
        if self.mouse.is_some() {
            if let Some(p) = self.last_pointer {
                let dy = if p.y < bounds.top() {
                    f32::from(p.y - bounds.top())
                } else if p.y > bounds.bottom() {
                    f32::from(p.y - bounds.bottom())
                } else {
                    0.0
                };
                if dy != 0.0 {
                    self.scroll.1 = (self.scroll.1 + dy.clamp(-60.0, 60.0) / 3.0).clamp(0.0, self.max_scroll_y());
                    self.mouse_select(p, window, cx);
                    lines = measure(self, window);
                    window.request_animation_frame();
                }
            }
        }
        self.scroll.1 = self.scroll.1.clamp(0.0, self.max_scroll_y());
        self.scroll.0 = self.scroll.0.clamp(0.0, self.max_scroll_x());

        // Collect what to paint.
        let origin = self.origin();
        let text_left = bounds.left() + px(gutter_width);
        let text_area = Bounds::from_corners(
            point(text_left, bounds.top()),
            point(bounds.right() - px(SCROLLBAR_WIDTH), bounds.bottom()),
        );
        let right_edge = match wrap_width {
            Some(w) => w,
            None => (f32::from(text_area.right() - origin.x)).max(self.max_line_width),
        };
        let selection = self.state().selection().clone();
        let sel_color = if self.focused { style.selection } else { style.selection_unfocused };
        let mut frame = Frame {
            lines: Vec::new(),
            numbers: Vec::new(),
            backgrounds: Vec::new(),
            selections: Vec::new(),
            carets: Vec::new(),
            gutter: None,
            scrollbar: None,
            text_area,
            line_height: px(line_height),
            background: fill(bounds, style.background),
        };
        let doc = self.state().doc().clone();
        for (n, layout) in lines {
            let line = doc.line(n + 1);
            let top = origin.y + px(self.heights.top(n));
            let height = px(self.heights.height(n));
            let has_head = selection.ranges.iter().any(|r| r.head() >= line.from && r.head() <= line.to);
            if has_head && self.highlight_active_line() {
                frame.backgrounds.push(fill(
                    Bounds::new(point(text_area.left(), top), size(text_area.size.width, height)),
                    style.active_line,
                ));
            }
            for r in &selection.ranges {
                if r.is_empty() || r.to < line.from || r.from > line.to {
                    continue;
                }
                let (a, b) = (r.from.max(line.from) - line.from, r.to.min(line.to) - line.from);
                for (x0, y, x1) in layout.range_rects(a, b, r.to > line.to, right_edge) {
                    frame.selections.push(fill(
                        Bounds::from_corners(
                            point(origin.x + px(x0), top + px(y)),
                            point(origin.x + px(x1.max(x0 + 1.0)), top + px(y + line_height)),
                        ),
                        sel_color,
                    ));
                }
            }
            if self.focused && self.cursor_visible && self.marked.is_none() {
                for r in &selection.ranges {
                    if r.head() < line.from || r.head() > line.to {
                        continue;
                    }
                    let (x, y) = layout.coords(r.head() - line.from, if r.assoc() != 0 { r.assoc() } else { 1 });
                    frame.carets.push(fill(
                        Bounds::new(point(origin.x + px(x) - px(0.75), top + px(y)), size(px(1.5), px(line_height))),
                        style.caret,
                    ));
                }
            }
            if gutter_width > 0.0 {
                let text = SharedString::from(line.number.to_string());
                let run = TextRun {
                    len: text.len(),
                    font: style.font.clone(),
                    color: if has_head { style.gutter_active_text } else { style.gutter_text },
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = window.text_system().shape_line(text, font_size, &[run], None);
                let x = bounds.left() + px(gutter_width - 8.0) - shaped.width;
                frame.numbers.push((point(x, top), shaped));
            }
            frame.lines.push((point(origin.x, top), layout));
        }
        if gutter_width > 0.0 {
            frame.gutter = Some(fill(
                Bounds::new(bounds.origin, size(px(gutter_width), bounds.size.height)),
                style.gutter_background,
            ));
        }
        let total = self.heights.total() + style.padding_y * 2.0;
        if total > visible {
            let thumb = (visible / total * visible).max(20.0);
            let y = self.scroll.1 / self.max_scroll_y().max(1.0) * (visible - thumb);
            frame.scrollbar = Some(fill(
                Bounds::new(
                    point(bounds.right() - px(SCROLLBAR_WIDTH - 2.0), bounds.top() + px(y)),
                    size(px(SCROLLBAR_WIDTH - 4.0), px(thumb)),
                ),
                style.scrollbar,
            ));
        }
        frame
    }
}
