//! Laying out a document line with GPUI's text system, and mapping between
//! document positions and coordinates in that layout. This replaces
//! CodeMirror's tile tree and DOM measurement (`tile.ts`, `docview.ts`,
//! `buildtile.ts`).

use std::sync::Arc;

use codemirror_state::{RangeSet, SpanIterator};
use gpui::{px, Font, Hsla, Pixels, SharedString, TextRun, UnderlineStyle, Window, WrappedLine};

use crate::decoration::{Decoration, DecorationSet, Kind, MarkStyle};

/// The decorations that apply to one line.
#[derive(Default)]
pub(crate) struct LineDecorations {
    /// (from, to, styles), relative to the line start.
    pub marks: Vec<(usize, usize, Vec<MarkStyle>)>,
    pub background: Option<Hsla>,
}

struct Collector<'a> {
    line_from: usize,
    out: &'a mut LineDecorations,
}

impl SpanIterator<Decoration> for Collector<'_> {
    fn span(&mut self, from: usize, to: usize, active: &[Arc<Decoration>], _open_start: usize) {
        let styles: Vec<MarkStyle> = active
            .iter()
            .filter_map(|d| d.mark_spec())
            .map(|s| s.style.clone())
            .filter(|s| *s != MarkStyle::default())
            .collect();
        if !styles.is_empty() && from < to {
            self.out.marks.push((from - self.line_from, to - self.line_from, styles));
        }
    }

    fn point(
        &mut self,
        _from: usize,
        _to: usize,
        value: &Arc<Decoration>,
        _active: &[Arc<Decoration>],
        _open_start: usize,
        _index: usize,
    ) {
        // Widgets and replaced ranges are not drawn yet.
        if let Kind::Line(spec) = &value.kind {
            if spec.background.is_some() {
                self.out.background = spec.background;
            }
        }
    }
}

pub(crate) fn line_decorations(sets: &[DecorationSet], from: usize, to: usize) -> LineDecorations {
    let mut out = LineDecorations::default();
    if sets.iter().all(|s| s.is_empty()) {
        return out;
    }
    RangeSet::spans(sets, from, to, &mut Collector { line_from: from, out: &mut out }, 0);
    out
}

/// A laid-out line. Tabs are expanded to spaces up to the next tab stop,
/// so positions are mapped between the document line and the displayed
/// text.
pub(crate) struct LineLayout {
    pub wrapped: WrappedLine,
    // For lines with tabs: the display offset of each document offset
    // (length + 1 entries). Empty otherwise.
    disp_of_doc: Vec<usize>,
    doc_len: usize,
    pub line_height: f32,
}

pub(crate) struct Row {
    pub start: usize,
    pub end: usize,
}

/// What a line is laid out with.
pub(crate) struct LineStyle<'a> {
    pub font: &'a Font,
    pub font_size: Pixels,
    pub color: Hsla,
    pub line_height: f32,
    pub wrap_width: Option<f32>,
    pub tab_size: usize,
}

fn styled_run(base: &TextRun, styles: &[MarkStyle], len: usize) -> TextRun {
    let mut run = TextRun { len, ..base.clone() };
    for s in styles {
        if let Some(c) = s.color {
            run.color = c;
        }
        if s.background.is_some() {
            run.background_color = s.background;
        }
        if s.underline.is_some() {
            run.underline = s.underline;
        }
        if s.strikethrough.is_some() {
            run.strikethrough = s.strikethrough;
        }
        if let Some(st) = s.font_style {
            run.font.style = st;
        }
        if let Some(w) = s.font_weight {
            run.font.weight = w;
        }
    }
    run
}

/// Shape a line. `marked` is a composition range (relative to the line) to
/// underline.
pub(crate) fn layout_line(
    window: &mut Window,
    text: &str,
    style: &LineStyle,
    deco: &LineDecorations,
    marked: Option<(usize, usize)>,
) -> LineLayout {
    let (display, disp_of_doc) = if text.contains('\t') {
        let mut display = String::with_capacity(text.len() + 8);
        let mut map = Vec::with_capacity(text.len() + 1);
        let mut col = 0;
        let tab_size = style.tab_size.max(1);
        for c in text.chars() {
            for _ in 0..c.len_utf8() {
                map.push(display.len());
            }
            if c == '\t' {
                let width = tab_size - col % tab_size;
                display.extend(std::iter::repeat_n(' ', width));
                col += width;
            } else {
                display.push(c);
                col += 1;
            }
        }
        map.push(display.len());
        (display, map)
    } else {
        (text.to_string(), Vec::new())
    };
    let disp = |doc: usize| map_off(&disp_of_doc, doc.min(text.len()));

    // Style boundaries (document offsets) from the marks and composition.
    let mut bounds = vec![0, text.len()];
    for (from, to, _) in &deco.marks {
        bounds.push((*from).min(text.len()));
        bounds.push((*to).min(text.len()));
    }
    if let Some((a, b)) = marked {
        bounds.push(a.min(text.len()));
        bounds.push(b.min(text.len()));
    }
    bounds.sort_unstable();
    bounds.dedup();
    let base = TextRun {
        len: 0,
        font: style.font.clone(),
        color: style.color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let mut runs: Vec<TextRun> = Vec::new();
    for w in bounds.windows(2) {
        let (start, end) = (w[0], w[1]);
        let len = disp(end) - disp(start);
        if len == 0 {
            continue;
        }
        let styles: Vec<MarkStyle> = deco
            .marks
            .iter()
            .filter(|(from, to, _)| *from <= start && *to >= end)
            .flat_map(|(_, _, s)| s.iter().cloned())
            .collect();
        let mut run = styled_run(&base, &styles, len);
        if marked.is_some_and(|(a, b)| a <= start && b >= end) {
            run.underline = Some(UnderlineStyle { thickness: px(1.0), color: Some(run.color), wavy: false });
        }
        runs.push(run);
    }
    if runs.is_empty() {
        runs.push(TextRun { len: display.len(), ..base });
    }
    let wrapped = window
        .text_system()
        .shape_text(SharedString::from(display), style.font_size, &runs, style.wrap_width.map(px), None)
        .ok()
        .and_then(|mut lines| lines.pop())
        .unwrap_or_default();
    LineLayout { wrapped, disp_of_doc, doc_len: text.len(), line_height: style.line_height }
}

fn map_off(disp_of_doc: &[usize], doc: usize) -> usize {
    if disp_of_doc.is_empty() {
        doc
    } else {
        disp_of_doc[doc.min(disp_of_doc.len() - 1)]
    }
}

impl LineLayout {
    fn disp(&self, doc: usize) -> usize {
        map_off(&self.disp_of_doc, doc.min(self.doc_len))
    }

    fn doc(&self, disp: usize) -> usize {
        if self.disp_of_doc.is_empty() {
            disp.min(self.doc_len)
        } else {
            // The first document offset at or after this display offset.
            match self.disp_of_doc.binary_search(&disp) {
                Ok(mut i) => {
                    while i > 0 && self.disp_of_doc[i - 1] == disp {
                        i -= 1;
                    }
                    i
                }
                Err(i) => i.saturating_sub(1),
            }
        }
    }

    pub fn rows(&self) -> Vec<Row> {
        let layout = &self.wrapped.unwrapped_layout;
        let mut rows = Vec::new();
        let mut start = 0;
        for b in self.wrapped.wrap_boundaries() {
            let end = layout.runs[b.run_ix].glyphs[b.glyph_ix].index;
            rows.push(Row { start, end });
            start = end;
        }
        rows.push(Row { start, end: layout.len });
        rows
    }

    pub fn height(&self) -> f32 {
        (self.wrapped.wrap_boundaries().len() + 1) as f32 * self.line_height
    }

    pub fn width(&self) -> f32 {
        f32::from(self.wrapped.unwrapped_layout.width)
    }

    fn x_in_row(&self, row: &Row, disp: usize) -> f32 {
        let layout = &self.wrapped.unwrapped_layout;
        f32::from(layout.x_for_index(disp) - layout.x_for_index(row.start))
    }

    // The row that shows display offset `disp`. At the boundary between
    // two wrapped rows, assoc -1 picks the end of the first.
    fn row_for(&self, rows: &[Row], disp: usize, assoc: i32) -> usize {
        let last = rows.len() - 1;
        for (k, row) in rows.iter().enumerate() {
            if disp > row.end || (disp == row.end && k != last && assoc >= 0) {
                continue;
            }
            return k;
        }
        last
    }

    /// The x position and row top (relative to the line) of a document
    /// offset within the line.
    pub fn coords(&self, doc: usize, assoc: i32) -> (f32, f32) {
        let rows = self.rows();
        let disp = self.disp(doc);
        let k = self.row_for(&rows, disp, assoc);
        (self.x_in_row(&rows[k], disp), k as f32 * self.line_height)
    }

    /// The document offset and association at a point relative to the line.
    pub fn pos_at(&self, x: f32, y: f32) -> (usize, i32) {
        let rows = self.rows();
        let last = rows.len() - 1;
        let k = ((y / self.line_height).floor().max(0.0) as usize).min(last);
        let row = &rows[k];
        let layout = &self.wrapped.unwrapped_layout;
        let abs = px(x) + layout.x_for_index(row.start);
        let disp = layout.closest_index_for_x(abs).clamp(row.start, row.end);
        let assoc = if disp == row.end && k != last { -1 } else { 1 };
        (self.doc(disp), assoc)
    }

    /// The document range of the visual row that shows `doc`.
    pub fn row_range(&self, doc: usize, assoc: i32) -> (usize, usize) {
        let rows = self.rows();
        let row = &rows[self.row_for(&rows, self.disp(doc), assoc)];
        (self.doc(row.start), self.doc(row.end))
    }

    /// Rectangles (x0, top, x1) covering the document range `from..to`, one
    /// per visual row. `to_edge` extends the last one to `right`.
    pub fn range_rects(&self, from: usize, to: usize, to_edge: bool, right: f32) -> Vec<(f32, f32, f32)> {
        let rows = self.rows();
        let (a, b) = (self.disp(from), self.disp(to));
        let last = rows.len() - 1;
        let mut out = Vec::new();
        for (k, row) in rows.iter().enumerate() {
            if row.end < a || row.start > b || (row.end == a && k != last && a != b) {
                continue;
            }
            let x0 = self.x_in_row(row, a.max(row.start));
            let continues = b > row.end || (k != last && b == row.end && a != b && rows[k + 1].start < b);
            let x1 =
                if continues || (k == last && to_edge) { right.max(x0) } else { self.x_in_row(row, b.min(row.end)) };
            if x1 > x0 || (to_edge && k == last) {
                out.push((x0, k as f32 * self.line_height, x1));
            }
            if b <= row.end {
                break;
            }
        }
        out
    }
}
