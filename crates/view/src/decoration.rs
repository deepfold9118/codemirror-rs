//! Decorations. Port of `decoration.ts`.
//!
//! CodeMirror styles marks and lines with DOM attributes and CSS classes.
//! Here a mark carries a [`MarkStyle`] that maps onto GPUI text runs, and a
//! line decoration a background color. The `class` field is kept as
//! an identifier that other code can match on.

use std::any::Any;
use std::sync::Arc;

use codemirror_state::{MapMode, Range, RangeSet, RangeValue};
use gpui::{FontStyle, FontWeight, Hsla, StrikethroughStyle, UnderlineStyle};

/// Widgets added to the content are described by implementations of this
/// trait. Drawing widgets is not ported yet; the height map already
/// accounts for their size.
pub trait WidgetType: Any + Send + Sync {
    /// Compare this widget to another widget of the same type. Used to avoid
    /// redrawing widgets when they are replaced by a new decoration of the
    /// same type. The default returns `false`.
    fn eq(&self, _other: &dyn WidgetType) -> bool {
        false
    }

    /// The estimated height this widget will have, to be used when
    /// estimating the height of content that hasn't been drawn. May return
    /// -1 to indicate you don't know. The default is -1.
    fn estimated_height(&self) -> f64 {
        -1.0
    }

    /// For inline widgets that introduce line breaks, the amount of line
    /// breaks they introduce. Defaults to 0.
    fn line_breaks(&self) -> usize {
        0
    }
}

fn widgets_eq(a: &Option<Arc<dyn WidgetType>>, b: &Option<Arc<dyn WidgetType>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            Arc::ptr_eq(a, b) || ((&**a as &dyn Any).type_id() == (&**b as &dyn Any).type_id() && a.eq(&**b))
        }
        _ => false,
    }
}

/// A decoration set represents a collection of decorated ranges, organized
/// for efficient access and mapping.
pub type DecorationSet = RangeSet<Decoration>;

pub(crate) mod side {
    pub const NON_INC_END: i64 = -600_000_000;
    pub const GAP_START: i64 = -500_000_000;
    pub const BLOCK_BEFORE: i64 = -400_000_000;
    pub const BLOCK_INC_START: i64 = -300_000_000;
    pub const LINE: i64 = -200_000_000;
    pub const INLINE_BEFORE: i64 = -100_000_000;
    pub const INLINE_INC_START: i64 = -1;
    pub const INLINE_INC_END: i64 = 1;
    pub const INLINE_AFTER: i64 = 100_000_000;
    pub const BLOCK_INC_END: i64 = 200_000_000;
    pub const BLOCK_AFTER: i64 = 300_000_000;
    pub const GAP_END: i64 = 400_000_000;
    pub const NON_INC_START: i64 = 500_000_000;
}

/// The different types of blocks that can occur in an editor view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockType {
    /// A line of text.
    Text,
    /// A block widget associated with the position after it.
    WidgetBefore,
    /// A block widget associated with the position before it.
    WidgetAfter,
    /// A block widget replacing a range of content.
    WidgetRange,
}

/// How a mark decoration styles its text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarkStyle {
    /// Text color.
    pub color: Option<Hsla>,
    /// Background color.
    pub background: Option<Hsla>,
    /// Underline (set `wavy` for a spelling squiggle).
    pub underline: Option<UnderlineStyle>,
    /// Strikethrough.
    pub strikethrough: Option<StrikethroughStyle>,
    /// Font style (italic).
    pub font_style: Option<FontStyle>,
    /// Font weight (bold).
    pub font_weight: Option<FontWeight>,
}

/// Options for [`Decoration::mark`].
#[derive(Clone, Debug, Default)]
pub struct MarkSpec {
    /// Whether the mark covers its start and end position or not. This
    /// influences whether content inserted at those positions becomes part
    /// of the mark. Defaults to false.
    pub inclusive: bool,
    /// Whether the start position of the marked range should be inclusive.
    /// Overrides `inclusive`.
    pub inclusive_start: Option<bool>,
    /// Whether the end should be inclusive. Overrides `inclusive`.
    pub inclusive_end: Option<bool>,
    /// An identifier for this kind of mark (CodeMirror's CSS class).
    pub class: Option<String>,
    /// How the marked text is drawn.
    pub style: MarkStyle,
}

/// Options for [`Decoration::widget`].
#[derive(Clone)]
pub struct WidgetSpec {
    /// The widget to draw here.
    pub widget: Arc<dyn WidgetType>,
    /// Which side of the given position the widget is on. Between -10000
    /// and 10000. Defaults to 0.
    pub side: i64,
    /// Render a block widget between inline widgets, ordered by `side`.
    pub inline_order: bool,
    /// Whether this is a block widget, drawn between lines.
    pub block: bool,
}

/// Options for [`Decoration::replace`].
#[derive(Clone, Default)]
pub struct ReplaceSpec {
    /// An optional widget to draw in place of the replaced content.
    pub widget: Option<Arc<dyn WidgetType>>,
    /// Whether this range covers the positions on its sides. Defaults to
    /// false for inline replacements, and true for block replacements.
    pub inclusive: Option<bool>,
    /// Set inclusivity at the start.
    pub inclusive_start: Option<bool>,
    /// Set inclusivity at the end.
    pub inclusive_end: Option<bool>,
    /// Whether this is a block-level decoration. Defaults to false.
    pub block: bool,
    /// Internal: a gap between blocks of lines.
    pub is_block_gap: bool,
}

/// Options for [`Decoration::line`].
#[derive(Clone, Debug, Default)]
pub struct LineSpec {
    /// An identifier for this kind of line decoration.
    pub class: Option<String>,
    /// Background color for the line.
    pub background: Option<Hsla>,
}

#[derive(Clone)]
pub(crate) enum Kind {
    Mark(MarkSpec),
    Line(LineSpec),
    Point { block: bool, is_replace: bool },
}

/// A decoration provides information on how to draw or style a piece of
/// content. You'll usually use it wrapped in a [`Range`], which adds a
/// start and end position.
#[derive(Clone)]
pub struct Decoration {
    pub(crate) start_side: i64,
    pub(crate) end_side: i64,
    pub(crate) widget: Option<Arc<dyn WidgetType>>,
    pub(crate) kind: Kind,
}

impl RangeValue for Decoration {
    fn value_eq(&self, other: &Self) -> bool {
        match (&self.kind, &other.kind) {
            (Kind::Mark(a), Kind::Mark(b)) => a.class == b.class && a.style == b.style,
            (Kind::Line(a), Kind::Line(b)) => a.class == b.class && a.background == b.background,
            (Kind::Point { block: a, .. }, Kind::Point { block: b, .. }) => {
                widgets_eq(&self.widget, &other.widget)
                    && a == b
                    && self.start_side == other.start_side
                    && self.end_side == other.end_side
            }
            _ => false,
        }
    }

    fn start_side(&self) -> i64 {
        self.start_side
    }

    fn end_side(&self) -> i64 {
        self.end_side
    }

    fn map_mode(&self) -> MapMode {
        match self.kind {
            Kind::Mark(_) => MapMode::TrackDel,
            Kind::Line(_) => MapMode::TrackBefore,
            Kind::Point { block: false, .. } => MapMode::TrackDel,
            Kind::Point { .. } if self.start_side <= 0 => MapMode::TrackBefore,
            Kind::Point { .. } => MapMode::TrackAfter,
        }
    }

    fn point(&self) -> bool {
        !matches!(self.kind, Kind::Mark(_))
    }
}

fn get_inclusive(
    inclusive: Option<bool>,
    inclusive_start: Option<bool>,
    inclusive_end: Option<bool>,
    block: bool,
) -> (bool, bool) {
    (inclusive_start.or(inclusive).unwrap_or(block), inclusive_end.or(inclusive).unwrap_or(block))
}

impl Decoration {
    /// Create a mark decoration, which influences the styling of the
    /// content in its range.
    pub fn mark(spec: MarkSpec) -> Decoration {
        let (start, end) = get_inclusive(Some(spec.inclusive), spec.inclusive_start, spec.inclusive_end, false);
        Decoration {
            start_side: if start { side::INLINE_INC_START } else { side::NON_INC_START },
            end_side: if end { side::INLINE_INC_END } else { side::NON_INC_END },
            widget: None,
            kind: Kind::Mark(spec),
        }
    }

    /// Create a widget decoration, which displays a widget at the given
    /// position.
    pub fn widget(spec: WidgetSpec) -> Decoration {
        let mut s = spec.side.clamp(-10000, 10000);
        s += if spec.block && !spec.inline_order {
            if s > 0 {
                side::BLOCK_AFTER
            } else {
                side::BLOCK_BEFORE
            }
        } else if s > 0 {
            side::INLINE_AFTER
        } else {
            side::INLINE_BEFORE
        };
        Decoration {
            start_side: s,
            end_side: s,
            widget: Some(spec.widget),
            kind: Kind::Point { block: spec.block, is_replace: false },
        }
    }

    /// Create a replace decoration which replaces the given range with a
    /// widget, or simply hides it.
    pub fn replace(spec: ReplaceSpec) -> Decoration {
        let block = spec.block;
        let (start_side, end_side) = if spec.is_block_gap {
            (side::GAP_START, side::GAP_END)
        } else {
            let (start, end) = get_inclusive(spec.inclusive, spec.inclusive_start, spec.inclusive_end, block);
            (
                (if start {
                    if block {
                        side::BLOCK_INC_START
                    } else {
                        side::INLINE_INC_START
                    }
                } else {
                    side::NON_INC_START
                }) - 1,
                (if end {
                    if block {
                        side::BLOCK_INC_END
                    } else {
                        side::INLINE_INC_END
                    }
                } else {
                    side::NON_INC_END
                }) + 1,
            )
        };
        Decoration { start_side, end_side, widget: spec.widget, kind: Kind::Point { block, is_replace: true } }
    }

    /// Create a line decoration, which styles the line starting at the
    /// given position.
    pub fn line(spec: LineSpec) -> Decoration {
        Decoration { start_side: side::LINE, end_side: side::LINE, widget: None, kind: Kind::Line(spec) }
    }

    /// Build a [`DecorationSet`] from the given decorated ranges. If the
    /// ranges aren't already sorted, pass `true` for `sort`.
    pub fn set(ranges: Vec<Range<Decoration>>, sort: bool) -> DecorationSet {
        RangeSet::of(ranges, sort)
    }

    /// The empty set of decorations.
    pub fn none() -> DecorationSet {
        RangeSet::empty()
    }

    /// Create a range for this decoration. Panics for ranges the decoration
    /// type doesn't allow (empty marks, non-empty line decorations or
    /// widgets, inverted replacements).
    pub fn range(&self, from: usize, to: usize) -> Range<Decoration> {
        match self.kind {
            Kind::Mark(_) => assert!(from < to, "Mark decorations may not be empty"),
            Kind::Line(_) => assert!(from == to, "Line decoration ranges must be zero-length"),
            Kind::Point { is_replace: true, .. } => assert!(
                !(from > to || (from == to && self.start_side > 0 && self.end_side <= 0)),
                "Invalid range for replacement decoration"
            ),
            Kind::Point { .. } => assert!(from == to, "Widget decorations can only have zero-length ranges"),
        }
        Range::new(from, to, self.clone())
    }

    /// Create a zero-length range at `pos`.
    pub fn at(&self, pos: usize) -> Range<Decoration> {
        self.range(pos, pos)
    }

    /// The widget of this decoration, if any.
    pub fn widget_type(&self) -> Option<&Arc<dyn WidgetType>> {
        self.widget.as_ref()
    }

    /// The mark options, for a mark decoration.
    pub fn mark_spec(&self) -> Option<&MarkSpec> {
        match &self.kind {
            Kind::Mark(spec) => Some(spec),
            _ => None,
        }
    }

    /// The line options, for a line decoration.
    pub fn line_spec(&self) -> Option<&LineSpec> {
        match &self.kind {
            Kind::Line(spec) => Some(spec),
            _ => None,
        }
    }

    pub(crate) fn is_block(&self) -> bool {
        matches!(self.kind, Kind::Point { block: true, .. })
    }

    // Only relevant for block decorations.
    pub(crate) fn block_type(&self) -> BlockType {
        if self.start_side != self.end_side {
            BlockType::WidgetRange
        } else if self.start_side <= 0 {
            BlockType::WidgetBefore
        } else {
            BlockType::WidgetAfter
        }
    }

    pub(crate) fn height_relevant(&self) -> bool {
        match self.kind {
            Kind::Point { block, .. } => {
                block || self.widget.as_ref().is_some_and(|w| w.estimated_height() >= 5.0 || w.line_breaks() > 0)
            }
            _ => false,
        }
    }
}

pub(crate) fn add_range(from: usize, to: usize, ranges: &mut Vec<usize>, margin: usize) {
    match ranges.last_mut() {
        Some(last) if *last + margin >= from => *last = (*last).max(to),
        _ => {
            ranges.push(from);
            ranges.push(to);
        }
    }
}
