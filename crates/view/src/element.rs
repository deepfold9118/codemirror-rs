//! The element that lays out and paints an [`EditorView`]: the lines in
//! view (measured into the height map), selections, carets, the active
//! line, line numbers and a scrollbar.

use gpui::{
    fill, point, px, relative, size, App, Bounds, ContentMask, Context, Element, ElementId, ElementInputHandler,
    Entity, GlobalElementId, InspectorElementId, IntoElement, LayoutId, PaintQuad, Pixels, Point, ShapedLine,
    SharedString, Style, TextAlign, TextRun, Window,
};

use crate::decoration::{BlockType, DecorationSet};
use crate::editorview::{EditorView, Geometry, SCROLLBAR_WIDTH};
use crate::extension::{update_flag, ScrollStrategy};
use crate::heightmap::{
    clear_height_change_flag, height_change_flag, BlockInfo, HeightMap, MeasuredHeights, QueryType,
};
use crate::layout::{line_decorations, LineLayout};
use crate::viewstate::Viewport;

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

// How far beyond the visible area lines are laid out.
const VIEWPORT_MARGIN: f64 = 100.0;

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
            // Mark decoration backgrounds. As in CodeMirror, where they are
            // inline styles and the selection is a layer behind the text,
            // they go over the selection.
            for (origin, layout) in &frame.lines {
                let _ = layout.wrapped.paint_background(*origin, line_height, TextAlign::Left, None, window, cx);
            }
            for (origin, layout) in &frame.lines {
                let _ = layout.wrapped.paint(*origin, line_height, TextAlign::Left, None, window, cx);
            }
            for q in frame.carets.drain(..) {
                window.paint_quad(q);
            }
        });
        // The lines laid out past the top and bottom edges have numbers
        // too, which must not be drawn over what is around the editor.
        if let Some(g) = frame.gutter.take() {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                window.paint_quad(g);
                for (origin, line) in &frame.numbers {
                    let _ = line.paint(*origin, line_height, window, cx);
                }
            });
        }
        if let Some(s) = frame.scrollbar.take() {
            window.paint_quad(s);
        }
    }
}

type Measured = Vec<(BlockInfo, Option<(f64, LineLayout)>)>;

impl EditorView {
    // Lay out the lines around the visible area and feed their heights to
    // the height map. Returns each line block with the top and layout of
    // its text line, and whether any height changed.
    fn measure(&mut self, window: &mut Window, decos: &[DecorationSet]) -> (Measured, bool, Viewport) {
        let pad = self.theme().padding_y as f64;
        let visible = self.visible_height() as f64;
        let top = (self.scroll.1 as f64 - pad - VIEWPORT_MARGIN).max(0.0);
        let bottom = self.scroll.1 as f64 + visible + VIEWPORT_MARGIN;
        let oracle = &self.vs.oracle;
        let first = self.vs.height_map.line_at(top, QueryType::ByHeight, oracle, 0.0, 0);
        let last = self.vs.height_map.line_at(bottom, QueryType::ByHeight, oracle, 0.0, 0);
        let range = Viewport { from: first.from, to: last.to() };
        let mut blocks = Vec::new();
        self.vs.height_map.for_each_line(range.from, range.to, oracle, 0.0, 0, &mut |b| blocks.push(b));
        let mut heights = Vec::new();
        let mut result = Vec::with_capacity(blocks.len());
        for block in blocks {
            let parts: Vec<BlockInfo> = block.children().map_or_else(|| vec![block.clone()], |c| c.to_vec());
            let mut text = None;
            for part in &parts {
                if part.block_type() == BlockType::Text {
                    let layout = self.layout(window, part.from, Some(decos));
                    self.max_line_width = self.max_line_width.max(layout.width());
                    heights.push(layout.height() as f64);
                    text = Some((part.top, layout));
                } else {
                    heights.push(part.height);
                }
            }
            result.push((block, text));
        }
        clear_height_change_flag();
        let map = std::mem::replace(&mut self.vs.height_map, HeightMap::empty());
        let mut measured = MeasuredHeights::new(range.from, heights);
        self.vs.height_map = map.update_height(&self.vs.oracle, 0, false, Some(&mut measured));
        let changed = height_change_flag();
        if changed {
            // Positions moved: recompute them without laying out again.
            let mut blocks = Vec::new();
            let oracle = &self.vs.oracle;
            self.vs.height_map.for_each_line(range.from, range.to, oracle, 0.0, 0, &mut |b| blocks.push(b));
            for ((old, text), new) in result.iter_mut().zip(blocks) {
                if let Some((top, _)) = text {
                    let text_top = new
                        .children()
                        .and_then(|c| c.iter().find(|p| p.block_type() == BlockType::Text).map(|p| p.top))
                        .unwrap_or(new.top);
                    *top = text_top;
                }
                *old = new;
            }
        }
        (result, changed, range)
    }

    /// Lay out the visible lines for a frame and collect what to paint.
    pub(crate) fn frame(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) -> Frame {
        let theme = self.theme();
        let line_height = theme.row_height();
        let font = theme.font();
        let font_size = px(theme.font_size);
        let font_id = window.text_system().resolve_font(&font);
        let char_width = window
            .text_system()
            .advance(font_id, font_size, '0')
            .map(|s| f32::from(s.width))
            .unwrap_or(theme.font_size * 0.55);
        let gutter_width = if self.line_numbers() {
            let digits = self.state().doc().lines().to_string().len().max(2);
            digits as f32 * char_width + 16.0
        } else {
            0.0
        };
        let width = f32::from(bounds.size.width);
        let wrapping = self.line_wrapping();
        let wrap_width =
            wrapping.then(|| (width - gutter_width - theme.padding_left - SCROLLBAR_WIDTH - 2.0).max(char_width * 4.0));

        // Refresh the height estimates when the metrics changed.
        let line_length = wrap_width.map_or(1e9, |w| (w / char_width) as f64);
        let width_changed = match (wrap_width, self.geo.wrap_width) {
            (Some(a), Some(b)) => (a - b).abs() > char_width / 2.0,
            (a, b) => a.is_some() != b.is_some(),
        };
        let refreshed = self.vs.oracle.refresh(
            wrapping,
            line_height as f64,
            char_width as f64,
            theme.font_size as f64,
            line_length,
            &[],
        );
        let geometry_changed = refreshed || width_changed || !self.geo.valid;
        self.geo = Geometry { bounds, char_width, gutter_width, wrap_width, valid: true };
        if geometry_changed {
            let map = std::mem::replace(&mut self.vs.height_map, HeightMap::empty());
            self.vs.height_map = map.update_height(&self.vs.oracle, 0, true, None);
        }

        // Keep the first visible line in place while heights change.
        let pad = theme.padding_y as f64;
        let anchor = self.line_block_at_height((self.scroll.1 as f64 - pad).max(0.0));
        let anchor_offset = self.scroll.1 as f64 - anchor.top;
        let mut decos = self.decoration_sets();
        let mut height_changed = false;
        let (mut lines, mut range) = (Vec::new(), Viewport::default());
        for _ in 0..3 {
            let (l, changed, r) = self.measure(window, &decos);
            (lines, range) = (l, r);
            if !changed {
                break;
            }
            height_changed = true;
            if self.mouse.is_none() && self.vs.scroll_target.is_none() {
                self.scroll.1 = (self.line_block_at(anchor.from).top + anchor_offset).max(0.0) as f32;
            }
        }

        // Scroll a target (usually the main cursor) into view.
        if let Some(target) = self.vs.scroll_target.take() {
            let head = target.range.head();
            let assoc = if target.range.assoc() != 0 { target.range.assoc() } else { 1 };
            let (x, top, bot) = self.doc_coords_at_pos(window, head, assoc);
            let visible = self.visible_height();
            let (top, bot) = (top + theme.padding_y - target.y_margin, bot + theme.padding_y + target.y_margin);
            let old = self.scroll;
            self.scroll.1 = match target.y {
                ScrollStrategy::Nearest if top < self.scroll.1 => top,
                ScrollStrategy::Nearest if bot > self.scroll.1 + visible => bot - visible,
                ScrollStrategy::Nearest => self.scroll.1,
                ScrollStrategy::Start => top,
                ScrollStrategy::End => bot - visible,
                ScrollStrategy::Center => (top + bot - visible) / 2.0,
            };
            if !wrapping {
                let text_w = width - gutter_width - theme.padding_left - SCROLLBAR_WIDTH;
                let margin = target.x_margin + char_width;
                if x - margin < self.scroll.0 {
                    self.scroll.0 = (x - margin).max(0.0);
                } else if x + margin > self.scroll.0 + text_w {
                    self.scroll.0 = x + margin - text_w;
                }
            }
            self.scroll.1 = self.scroll.1.clamp(0.0, self.max_scroll_y());
            if self.scroll != old {
                let (l, changed, r) = self.measure(window, &decos);
                (lines, range) = (l, r);
                height_changed |= changed;
            }
        }
        // Scroll while selecting with the pointer above or below the editor.
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
                    let (l, _, r) = self.measure(window, &decos);
                    (lines, range) = (l, r);
                    window.request_animation_frame();
                }
            }
        }
        self.scroll.1 = self.scroll.1.clamp(0.0, self.max_scroll_y());
        self.scroll.0 = self.scroll.0.clamp(0.0, self.max_scroll_x());

        let mut flags = 0;
        if height_changed {
            flags |= update_flag::HEIGHT;
        }
        if geometry_changed {
            flags |= update_flag::GEOMETRY;
        }
        if range != self.vs.viewport {
            self.vs.viewport = range;
            flags |= update_flag::VIEWPORT | update_flag::VIEWPORT_MOVED;
        }
        if flags != 0 {
            self.notify_flags(flags, cx);
            // View plugins recompute their decorations for the new viewport
            // (a redraw requested from here would be dropped): lay the lines
            // out again with them, in this frame.
            if flags & update_flag::VIEWPORT != 0 && !self.plugins.is_empty() {
                decos = self.decoration_sets();
                lines = self.measure(window, &decos).0;
            }
        }

        // Collect what to paint.
        let origin = self.origin();
        let text_area = Bounds::from_corners(
            point(bounds.left() + px(gutter_width), bounds.top()),
            point(bounds.right() - px(SCROLLBAR_WIDTH), bounds.bottom()),
        );
        let right_edge = match wrap_width {
            Some(w) => w,
            None => f32::from(text_area.right() - origin.x).max(self.max_line_width),
        };
        let selection = self.state().selection().clone();
        let sel_color = if self.focused { theme.selection } else { theme.selection_unfocused };
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
            background: fill(bounds, theme.background),
        };
        let doc = self.state().doc().clone();
        let active_lines = self.highlight_active_line();
        for (block, text) in lines {
            let Some((text_top, layout)) = text else { continue };
            let line = doc.line_at(block.from);
            let block_top = origin.y + px(block.top as f32);
            let block_rect =
                Bounds::new(point(text_area.left(), block_top), size(text_area.size.width, px(block.height as f32)));
            let top = origin.y + px(text_top as f32);
            if let Some(bg) = line_decorations(&decos, line.from, line.to).background {
                frame.backgrounds.push(fill(block_rect, bg));
            }
            let has_head = selection.ranges.iter().any(|r| r.head() >= line.from && r.head() <= line.to);
            if has_head && active_lines {
                frame.backgrounds.push(fill(block_rect, theme.active_line));
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
                    let assoc = if r.assoc() != 0 { r.assoc() } else { 1 };
                    let (x, y) = layout.coords(r.head() - line.from, assoc);
                    let w = theme.caret_width;
                    frame.carets.push(fill(
                        Bounds::new(point(origin.x + px(x - w / 2.0), top + px(y)), size(px(w), px(line_height))),
                        theme.caret,
                    ));
                }
            }
            if gutter_width > 0.0 {
                let text = SharedString::from(line.number.to_string());
                let run = TextRun {
                    len: text.len(),
                    font: font.clone(),
                    color: if has_head { theme.gutter_active_text } else { theme.gutter_text },
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
                theme.gutter_background,
            ));
        }
        let visible = self.visible_height();
        let total = self.content_height() as f32 + theme.padding_y * 2.0;
        if total > visible {
            let thumb = (visible / total * visible).max(20.0);
            let y = self.scroll.1 / self.max_scroll_y().max(1.0) * (visible - thumb);
            frame.scrollbar = Some(fill(
                Bounds::new(
                    point(bounds.right() - px(SCROLLBAR_WIDTH - 2.0), bounds.top() + px(y)),
                    size(px(SCROLLBAR_WIDTH - 4.0), px(thumb)),
                ),
                theme.scrollbar,
            ));
        }
        frame
    }
}
