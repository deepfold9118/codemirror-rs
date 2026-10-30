//! Laying out a line of text with egui, and mapping between document
//! positions and coordinates in that layout. This replaces CodeMirror's
//! tile tree and DOM measurement (`tile.ts`, `docview.ts`, `buildtile.ts`).

use std::sync::Arc;

use codemirror_state::{RangeSet, SpanIterator};
use egui::text::{ByteIndex, LayoutJob, LayoutSection};
use egui::{Galley, TextFormat};

use crate::decoration::{Decoration, DecorationSet, Kind, MarkStyle};
use crate::theme::Theme;

/// A laid-out line.
pub(crate) struct LineLayout {
    pub galley: Arc<Galley>,
    // Byte offset (within the line) of each char, plus the line length.
    // Empty when the line is ASCII, where bytes and chars coincide.
    byte_of_char: Vec<usize>,
    len: usize,
}

/// The decorations that apply to one line.
#[derive(Default)]
pub(crate) struct LineDecorations {
    /// (from, to, styles), relative to the line start.
    pub marks: Vec<(usize, usize, Vec<MarkStyle>)>,
    pub background: Option<egui::Color32>,
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

fn apply_style(format: &mut TextFormat, style: &MarkStyle) {
    if let Some(c) = style.color {
        format.color = c;
    }
    if let Some(c) = style.background {
        format.background = c;
    }
    if let Some(s) = style.underline {
        format.underline = s;
    }
    if let Some(s) = style.strikethrough {
        format.strikethrough = s;
    }
    if style.italics {
        format.italics = true;
    }
}

/// Lay out a line. Tabs are drawn as spaces that reach the next tab stop
/// (a multiple of `tab_size` columns); everything else is egui's layout.
pub(crate) fn layout_line(
    fonts: &mut egui::epaint::text::FontsView<'_>,
    theme: &Theme,
    text: &str,
    deco: &LineDecorations,
    wrap_width: Option<f32>,
    tab_size: usize,
) -> LineLayout {
    let base = TextFormat {
        font_id: theme.font.clone(),
        color: theme.text,
        line_height: Some(theme.row_height()),
        ..Default::default()
    };
    // Style boundaries, from the marks.
    let mut bounds: Vec<usize> = vec![0, text.len()];
    for (from, to, _) in &deco.marks {
        bounds.push((*from).min(text.len()));
        bounds.push((*to).min(text.len()));
    }
    bounds.sort_unstable();
    bounds.dedup();

    let tab_size = tab_size.max(1);
    let space_w = fonts.glyph_width(&theme.font, ' ');
    let char_w = fonts.glyph_width(&theme.font, '0');
    let mut job = LayoutJob { text: text.replace('\t', " "), ..Default::default() };
    job.wrap.max_width = wrap_width.unwrap_or(f32::INFINITY);
    let mut col = 0usize;
    let mut pending_space = 0.0f32;
    for w in bounds.windows(2) {
        let (start, end) = (w[0], w[1]);
        if start >= end {
            continue;
        }
        let mut format = base.clone();
        for (from, to, styles) in &deco.marks {
            if *from <= start && *to >= end {
                for s in styles {
                    apply_style(&mut format, s);
                }
            }
        }
        let mut piece = start;
        for (i, c) in text[start..end].char_indices() {
            if c != '\t' {
                col += 1;
                continue;
            }
            let at = start + i;
            if at > piece {
                job.sections.push(LayoutSection {
                    leading_space: std::mem::take(&mut pending_space),
                    byte_range: ByteIndex(piece)..ByteIndex(at),
                    format: format.clone(),
                });
            }
            let width = tab_size - col % tab_size;
            col += width;
            // The tab's space glyph goes after the extra space, so the
            // position after the tab lands on the tab stop.
            job.sections.push(LayoutSection {
                leading_space: std::mem::take(&mut pending_space) + (width as f32 * char_w - space_w).max(0.0),
                byte_range: ByteIndex(at)..ByteIndex(at + 1),
                format: format.clone(),
            });
            piece = at + 1;
        }
        if piece < end {
            job.sections.push(LayoutSection {
                leading_space: std::mem::take(&mut pending_space),
                byte_range: ByteIndex(piece)..ByteIndex(end),
                format,
            });
        }
    }
    if job.sections.is_empty() {
        job.sections.push(LayoutSection { leading_space: 0.0, byte_range: ByteIndex(0)..ByteIndex(0), format: base });
    }
    let galley = fonts.layout_job(job);
    let byte_of_char = if text.is_ascii() {
        Vec::new()
    } else {
        let mut v: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        v.push(text.len());
        v
    };
    LineLayout { galley, byte_of_char, len: text.len() }
}

/// A visual row of a line layout: its char range and rectangle.
pub(crate) struct RowInfo {
    pub index: usize,
    pub start: usize,
    pub end: usize,
    pub rect: egui::Rect,
}

impl LineLayout {
    pub fn height(&self, row_height: f32) -> f32 {
        self.galley.size().y.max(row_height)
    }

    pub fn width(&self) -> f32 {
        self.galley.size().x
    }

    fn char_of(&self, byte: usize) -> usize {
        if self.byte_of_char.is_empty() {
            byte.min(self.len)
        } else {
            match self.byte_of_char.binary_search(&byte) {
                Ok(i) => i,
                Err(i) => i.saturating_sub(1),
            }
        }
    }

    fn byte_of(&self, ci: usize) -> usize {
        if self.byte_of_char.is_empty() {
            ci.min(self.len)
        } else {
            self.byte_of_char[ci.min(self.byte_of_char.len() - 1)]
        }
    }

    pub fn rows(&self) -> Vec<RowInfo> {
        let mut start = 0;
        self.galley
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let end = start + row.glyphs.len();
                let info = RowInfo { index, start, end, rect: row.rect() };
                start = end + usize::from(row.ends_with_newline);
                info
            })
            .collect()
    }

    // The x position of the boundary before char `ci` in a row.
    fn x_in_row(&self, row: &RowInfo, ci: usize) -> f32 {
        let placed = &self.galley.rows[row.index];
        if ci <= row.start || placed.glyphs.is_empty() {
            placed.pos.x
        } else {
            let g = &placed.glyphs[(ci - row.start - 1).min(placed.glyphs.len() - 1)];
            placed.pos.x + g.max_x()
        }
    }

    // The row that shows the position `ci`, given its association: at the
    // boundary between two wrapped rows, -1 picks the end of the first.
    fn row_for(&self, rows: &[RowInfo], ci: usize, assoc: i32) -> usize {
        let last = rows.len() - 1;
        for (k, row) in rows.iter().enumerate() {
            if ci > row.end || (ci == row.end && k != last && assoc >= 0) {
                continue;
            }
            return k;
        }
        last
    }

    /// The rectangle (relative to the layout) of the position `byte`
    /// within the line.
    pub fn coords(&self, byte: usize, assoc: i32) -> egui::Rect {
        let rows = self.rows();
        let ci = self.char_of(byte);
        let row = &rows[self.row_for(&rows, ci, assoc)];
        let x = self.x_in_row(row, ci);
        egui::Rect::from_min_max(egui::pos2(x, row.rect.top()), egui::pos2(x, row.rect.bottom()))
    }

    /// The byte offset and association at the given point (relative to the
    /// layout).
    pub fn pos_at(&self, x: f32, y: f32) -> (usize, i32) {
        let rows = self.rows();
        let last = rows.len() - 1;
        let k = rows.iter().position(|r| y < r.rect.bottom()).unwrap_or(last);
        let row = &rows[k];
        for ci in row.start..row.end {
            let mid = (self.x_in_row(row, ci) + self.x_in_row(row, ci + 1)) / 2.0;
            if x < mid {
                return (self.byte_of(ci), 1);
            }
        }
        (self.byte_of(row.end), if k == last { 1 } else { -1 })
    }

    /// The byte range of the visual row that shows `byte`.
    pub fn row_range(&self, byte: usize, assoc: i32) -> (usize, usize) {
        let rows = self.rows();
        let row = &rows[self.row_for(&rows, self.char_of(byte), assoc)];
        (self.byte_of(row.start), self.byte_of(row.end))
    }

    /// Rectangles (relative to the layout) covering the byte range
    /// `from..to`, one per visual row. `to_edge` extends the last one to
    /// `right`, for selections that continue past the line.
    pub fn range_rects(&self, from: usize, to: usize, to_edge: bool, right: f32) -> Vec<egui::Rect> {
        let rows = self.rows();
        let (a, b) = (self.char_of(from), self.char_of(to));
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
                out.push(egui::Rect::from_min_max(egui::pos2(x0, row.rect.top()), egui::pos2(x1, row.rect.bottom())));
            }
            if b <= row.end {
                break;
            }
        }
        out
    }
}
