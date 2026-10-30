//! The height map: a balanced tree that stores the vertical layout of the
//! document, using measured heights for lines that were drawn and
//! estimates for the rest. Port of `heightmap.ts`.

use std::cell::Cell;
use std::collections::HashSet;
use std::sync::Arc;

use codemirror_state::{ChangeDesc, RangeComparator, RangeSet, SpanIterator, Text};

use crate::decoration::{add_range, BlockType, Decoration, DecorationSet, ReplaceSpec, WidgetType};
use crate::extension::ChangedRange;

thread_local! {
    // Used to track, during update_height, if any actual heights changed.
    static HEIGHT_CHANGED: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn clear_height_change_flag() {
    HEIGHT_CHANGED.with(|f| f.set(false));
}

pub(crate) fn height_change_flag() -> bool {
    HEIGHT_CHANGED.with(|f| f.get())
}

fn flag_height_change() {
    HEIGHT_CHANGED.with(|f| f.set(true));
}

/// Estimates heights for content that hasn't been measured.
#[derive(Clone)]
pub struct HeightOracle {
    pub(crate) doc: Text,
    height_samples: HashSet<i64>,
    /// The height of an entire line.
    pub line_height: f64,
    /// The width of a character.
    pub char_width: f64,
    /// The height of the font.
    pub text_height: f64,
    /// The number of characters that fit on a line.
    pub line_length: f64,
    /// Whether lines wrap.
    pub line_wrapping: bool,
}

impl HeightOracle {
    pub fn new(line_wrapping: bool) -> HeightOracle {
        HeightOracle {
            doc: Text::empty(),
            height_samples: HashSet::new(),
            line_height: 14.0,
            char_width: 7.0,
            text_height: 14.0,
            line_length: 30.0,
            line_wrapping,
        }
    }

    pub(crate) fn height_for_gap(&self, from: usize, to: usize) -> f64 {
        let mut lines = (self.doc.line_at(to).number - self.doc.line_at(from).number + 1) as f64;
        if self.line_wrapping {
            lines += (((to - from) as f64 - lines * self.line_length * 0.5) / self.line_length).ceil().max(0.0);
        }
        self.line_height * lines
    }

    pub(crate) fn height_for_line(&self, length: usize) -> f64 {
        if !self.line_wrapping {
            return self.line_height;
        }
        let lines = 1.0 + ((length as f64 - self.line_length) / (self.line_length - 5.0).max(1.0)).ceil().max(0.0);
        lines * self.line_height
    }

    pub(crate) fn set_doc(&mut self, doc: &Text) -> &mut Self {
        self.doc = doc.clone();
        self
    }

    /// Update the metrics. Returns true when the estimates changed enough
    /// that all estimated heights must be recomputed.
    pub(crate) fn refresh(
        &mut self,
        line_wrapping: bool,
        line_height: f64,
        char_width: f64,
        text_height: f64,
        line_length: f64,
        known_heights: &[f64],
    ) -> bool {
        let changed = (line_height - self.line_height).abs() > 0.3 || self.line_wrapping != line_wrapping;
        self.line_wrapping = line_wrapping;
        self.line_height = line_height;
        self.char_width = char_width;
        self.text_height = text_height;
        self.line_length = line_length;
        if changed {
            self.height_samples.clear();
            let mut i = 0;
            while i < known_heights.len() {
                let h = known_heights[i];
                if h < 0.0 {
                    i += 1;
                } else {
                    self.height_samples.insert((h * 10.0).floor() as i64);
                }
                i += 1;
            }
        }
        changed
    }
}

/// Used by `update_height` to make measurements arrive at the right nodes.
/// The `heights` array is a sequence of block heights, starting from
/// position `from`. A negative entry gives the space above the block that
/// follows it.
pub struct MeasuredHeights {
    pub from: usize,
    pub heights: Vec<f64>,
    pub index: usize,
}

impl MeasuredHeights {
    pub fn new(from: usize, heights: Vec<f64>) -> MeasuredHeights {
        MeasuredHeights { from, heights, index: 0 }
    }

    fn more(&self) -> bool {
        self.index < self.heights.len()
    }

    fn next(&mut self) -> f64 {
        let h = self.heights[self.index];
        self.index += 1;
        h
    }
}

#[derive(Clone)]
pub(crate) enum BlockContent {
    Children(Vec<BlockInfo>),
    Deco(Arc<Decoration>),
    Breaks(usize),
}

/// Information about a block-level element in the editor view: a line of
/// text or a block widget.
#[derive(Clone)]
pub struct BlockInfo {
    /// The start of the element in the document.
    pub from: usize,
    /// The length of the element.
    pub length: usize,
    /// The top position of the element (relative to the top of the
    /// document).
    pub top: f64,
    /// Its height.
    pub height: f64,
    pub(crate) content: BlockContent,
}

impl BlockInfo {
    fn new(from: usize, length: usize, top: f64, height: f64, content: BlockContent) -> BlockInfo {
        BlockInfo { from, length, top, height, content }
    }

    /// The type of element this is.
    pub fn block_type(&self) -> BlockType {
        match &self.content {
            BlockContent::Breaks(_) => BlockType::Text,
            BlockContent::Deco(d) => d.block_type(),
            // A line made of several blocks counts as text.
            BlockContent::Children(_) => BlockType::Text,
        }
    }

    /// When querying lines, the blocks that make up the line, if there are
    /// more than one.
    pub fn children(&self) -> Option<&[BlockInfo]> {
        match &self.content {
            BlockContent::Children(c) => Some(c),
            _ => None,
        }
    }

    /// The end of the element as a document position.
    pub fn to(&self) -> usize {
        self.from + self.length
    }

    /// The bottom position of the element.
    pub fn bottom(&self) -> f64 {
        self.top + self.height
    }

    /// If this is a widget block, the widget associated with it.
    pub fn widget(&self) -> Option<&Arc<dyn WidgetType>> {
        match &self.content {
            BlockContent::Deco(d) => d.widget.as_ref(),
            _ => None,
        }
    }

    /// If this is a text block, the number of line breaks that appear in
    /// widgets inside the block.
    pub fn widget_line_breaks(&self) -> usize {
        match self.content {
            BlockContent::Breaks(n) => n,
            _ => 0,
        }
    }

    pub(crate) fn join(self, other: BlockInfo) -> BlockInfo {
        let (from, top, length, height) = (self.from, self.top, self.length + other.length, self.height + other.height);
        let mut content = match self.content {
            BlockContent::Children(c) => c,
            content => vec![BlockInfo { content, ..self }],
        };
        match other.content {
            BlockContent::Children(c) => content.extend(c),
            c => content.push(BlockInfo { content: c, ..other }),
        }
        BlockInfo::new(from, length, top, height, BlockContent::Children(content))
    }
}

/// How [`HeightMap::line_at`] interprets its value.
// CodeMirror's names.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QueryType {
    ByPos,
    ByHeight,
    ByPosNoHeight,
}

const FLAG_BREAK: u8 = 1;
const FLAG_OUTDATED: u8 = 2;
const FLAG_SINGLE_LINE: u8 = 4;

const EPSILON: f64 = 1e-3;

/// A line of text (`is_text`) or a block widget.
pub(crate) struct LineNode {
    length: usize,
    height: f64,
    flags: u8,
    space_above: f64,
    deco: Option<Arc<Decoration>>,
    // Text lines only:
    collapsed: usize,
    widget_height: f64,
    breaks: usize,
}

pub(crate) struct BranchNode {
    left: Box<HeightMap>,
    right: Box<HeightMap>,
    length: usize,
    height: f64,
    flags: u8,
    size: usize,
}

/// A node in the height map tree.
pub(crate) enum HeightMap {
    Text(LineNode),
    Block(LineNode),
    Gap { length: usize, height: f64, flags: u8 },
    Branch(BranchNode),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    Text,
    Block,
    Gap,
    Branch,
}

type Nodes = Vec<Option<Box<HeightMap>>>;

impl HeightMap {
    fn new_text(length: usize, height: f64, above: f64) -> Box<HeightMap> {
        Box::new(HeightMap::Text(LineNode {
            length,
            height,
            flags: FLAG_OUTDATED,
            space_above: above,
            deco: None,
            collapsed: 0,
            widget_height: 0.0,
            breaks: 0,
        }))
    }

    fn new_block(length: usize, height: f64, deco: Option<Arc<Decoration>>) -> Box<HeightMap> {
        Box::new(HeightMap::Block(LineNode {
            length,
            height,
            flags: FLAG_OUTDATED,
            space_above: 0.0,
            deco,
            collapsed: 0,
            widget_height: 0.0,
            breaks: 0,
        }))
    }

    fn new_gap(length: usize) -> Box<HeightMap> {
        Box::new(HeightMap::Gap { length, height: 0.0, flags: FLAG_OUTDATED })
    }

    fn new_branch(left: Box<HeightMap>, brk: bool, right: Box<HeightMap>) -> Box<HeightMap> {
        let flags =
            (if brk { FLAG_BREAK } else { 0 }) | (if left.outdated() || right.outdated() { FLAG_OUTDATED } else { 0 });
        Box::new(HeightMap::Branch(BranchNode {
            length: left.length() + usize::from(brk) + right.length(),
            height: left.height() + right.height(),
            flags,
            size: left.size() + right.size(),
            left,
            right,
        }))
    }

    /// An empty height map.
    pub(crate) fn empty() -> Box<HeightMap> {
        HeightMap::new_text(0, 0.0, 0.0)
    }

    fn kind(&self) -> NodeKind {
        match self {
            HeightMap::Text(_) => NodeKind::Text,
            HeightMap::Block(_) => NodeKind::Block,
            HeightMap::Gap { .. } => NodeKind::Gap,
            HeightMap::Branch(_) => NodeKind::Branch,
        }
    }

    /// The number of characters covered.
    pub(crate) fn length(&self) -> usize {
        match self {
            HeightMap::Text(n) | HeightMap::Block(n) => n.length,
            HeightMap::Gap { length, .. } => *length,
            HeightMap::Branch(b) => b.length,
        }
    }

    /// The height of this part of the document.
    pub(crate) fn height(&self) -> f64 {
        match self {
            HeightMap::Text(n) | HeightMap::Block(n) => n.height,
            HeightMap::Gap { height, .. } => *height,
            HeightMap::Branch(b) => b.height,
        }
    }

    fn height_mut(&mut self) -> &mut f64 {
        match self {
            HeightMap::Text(n) | HeightMap::Block(n) => &mut n.height,
            HeightMap::Gap { height, .. } => height,
            HeightMap::Branch(b) => &mut b.height,
        }
    }

    fn flags_mut(&mut self) -> &mut u8 {
        match self {
            HeightMap::Text(n) | HeightMap::Block(n) => &mut n.flags,
            HeightMap::Gap { flags, .. } => flags,
            HeightMap::Branch(b) => &mut b.flags,
        }
    }

    fn flags(&self) -> u8 {
        match self {
            HeightMap::Text(n) | HeightMap::Block(n) => n.flags,
            HeightMap::Gap { flags, .. } => *flags,
            HeightMap::Branch(b) => b.flags,
        }
    }

    /// The number of leaves in this tree.
    pub(crate) fn size(&self) -> usize {
        match self {
            HeightMap::Branch(b) => b.size,
            _ => 1,
        }
    }

    pub(crate) fn outdated(&self) -> bool {
        self.flags() & FLAG_OUTDATED > 0
    }

    fn set_outdated(&mut self, value: bool) {
        let flags = self.flags_mut();
        *flags = (if value { FLAG_OUTDATED } else { 0 }) | (*flags & !FLAG_OUTDATED);
    }

    fn set_height(&mut self, height: f64) {
        let h = self.height_mut();
        if *h != height {
            if (*h - height).abs() > EPSILON {
                flag_height_change();
            }
            *h = height;
        }
    }

    fn brk(&self) -> usize {
        usize::from(self.flags() & FLAG_BREAK > 0)
    }

    // ---- queries ----

    pub(crate) fn block_at(&self, height: f64, oracle: &HeightOracle, top: f64, offset: usize) -> BlockInfo {
        match self {
            HeightMap::Text(n) | HeightMap::Block(n) => {
                if n.space_above > 0.0 && height < top + n.space_above {
                    space_block(offset, top, n.space_above)
                } else {
                    self.main_block(top, offset)
                }
            }
            HeightMap::Gap { length, height: gap_height, .. } => {
                let (first_line, last_line, per_line, per_char) = self.height_metrics(oracle, offset);
                if oracle.line_wrapping {
                    let guess = offset
                        + if height < oracle.line_height {
                            0
                        } else {
                            (((height - top) / gap_height).clamp(0.0, 1.0) * *length as f64).round() as usize
                        };
                    let line = oracle.doc.line_at(guess);
                    let line_height = per_line + line.length() as f64 * per_char;
                    let line_top = top.max(height - line_height / 2.0);
                    BlockInfo::new(line.from, line.length(), line_top, line_height, BlockContent::Breaks(0))
                } else {
                    let line = ((height - top) / per_line).floor().clamp(0.0, (last_line - first_line) as f64) as usize;
                    let l = oracle.doc.line(first_line + line);
                    BlockInfo::new(l.from, l.length(), top + per_line * line as f64, per_line, BlockContent::Breaks(0))
                }
            }
            HeightMap::Branch(b) => {
                let mid = top + b.left.height();
                if height < mid {
                    b.left.block_at(height, oracle, top, offset)
                } else {
                    b.right.block_at(height, oracle, mid, offset + b.left.length() + self.brk())
                }
            }
        }
    }

    fn main_block(&self, top: f64, offset: usize) -> BlockInfo {
        match self {
            HeightMap::Text(n) => BlockInfo::new(
                offset,
                n.length,
                top + n.space_above,
                n.height - n.space_above,
                BlockContent::Breaks(n.breaks),
            ),
            HeightMap::Block(n) => BlockInfo::new(
                offset,
                n.length,
                top + n.space_above,
                n.height - n.space_above,
                match &n.deco {
                    Some(d) => BlockContent::Deco(d.clone()),
                    None => BlockContent::Breaks(0),
                },
            ),
            _ => unreachable!("main_block on a non-leaf node"),
        }
    }

    pub(crate) fn line_at(
        &self,
        value: f64,
        query: QueryType,
        oracle: &HeightOracle,
        top: f64,
        offset: usize,
    ) -> BlockInfo {
        match self {
            HeightMap::Text(n) | HeightMap::Block(n) => {
                let main = self.main_block(top, offset);
                if n.space_above > 0.0 {
                    self.block_at(0.0, oracle, top, offset).join(main)
                } else {
                    main
                }
            }
            HeightMap::Gap { height, .. } => {
                if query == QueryType::ByHeight {
                    return self.block_at(value, oracle, top, offset);
                }
                if query == QueryType::ByPosNoHeight {
                    let line = oracle.doc.line_at(value as usize);
                    return BlockInfo::new(line.from, line.to - line.from, 0.0, 0.0, BlockContent::Breaks(0));
                }
                let (first_line, _, per_line, per_char) = self.height_metrics(oracle, offset);
                let line = oracle.doc.line_at(value as usize);
                let line_height = per_line + line.length() as f64 * per_char;
                let lines_above = line.number - first_line;
                let line_top =
                    top + per_line * lines_above as f64 + per_char * (line.from - offset - lines_above) as f64;
                BlockInfo::new(
                    line.from,
                    line.length(),
                    top.max(line_top.min(top + height - line_height)),
                    line_height,
                    BlockContent::Breaks(0),
                )
            }
            HeightMap::Branch(b) => {
                let right_top = top + b.left.height();
                let right_offset = offset + b.left.length() + self.brk();
                let left = if query == QueryType::ByHeight { value < right_top } else { value < right_offset as f64 };
                let base = if left {
                    b.left.line_at(value, query, oracle, top, offset)
                } else {
                    b.right.line_at(value, query, oracle, right_top, right_offset)
                };
                if self.brk() > 0 || (if left { base.to() < right_offset } else { base.from > right_offset }) {
                    return base;
                }
                let sub_query =
                    if query == QueryType::ByPosNoHeight { QueryType::ByPosNoHeight } else { QueryType::ByPos };
                if left {
                    base.join(b.right.line_at(right_offset as f64, sub_query, oracle, right_top, right_offset))
                } else {
                    b.left.line_at(right_offset as f64, sub_query, oracle, top, offset).join(base)
                }
            }
        }
    }

    pub(crate) fn for_each_line(
        &self,
        from: usize,
        to: usize,
        oracle: &HeightOracle,
        top: f64,
        offset: usize,
        f: &mut dyn FnMut(BlockInfo),
    ) {
        match self {
            HeightMap::Text(_) | HeightMap::Block(_) => {
                if from <= offset + self.length() && to >= offset {
                    f(self.line_at(0.0, QueryType::ByPos, oracle, top, offset));
                }
            }
            HeightMap::Gap { length, .. } => {
                let from = from.max(offset);
                let to = to.min(offset + length);
                let (first_line, _, per_line, per_char) = self.height_metrics(oracle, offset);
                let mut pos = from;
                let mut line_top = top;
                while pos <= to {
                    let line = oracle.doc.line_at(pos);
                    if pos == from {
                        let lines_above = line.number - first_line;
                        line_top += per_line * lines_above as f64 + per_char * (from - offset - lines_above) as f64;
                    }
                    let line_height = per_line + per_char * line.length() as f64;
                    f(BlockInfo::new(line.from, line.length(), line_top, line_height, BlockContent::Breaks(0)));
                    line_top += line_height;
                    pos = line.to + 1;
                }
            }
            HeightMap::Branch(b) => {
                let right_top = top + b.left.height();
                let right_offset = offset + b.left.length() + self.brk();
                if self.brk() > 0 {
                    if from < right_offset {
                        b.left.for_each_line(from, to, oracle, top, offset, f);
                    }
                    if to >= right_offset {
                        b.right.for_each_line(from, to, oracle, right_top, right_offset, f);
                    }
                } else {
                    let mid = self.line_at(right_offset as f64, QueryType::ByPos, oracle, top, offset);
                    if from < mid.from {
                        b.left.for_each_line(from, to.min(mid.from - 1), oracle, top, offset, f);
                    }
                    let mid_to = mid.to();
                    if mid_to >= from && mid.from <= to {
                        f(mid);
                    }
                    if to > mid_to {
                        b.right.for_each_line(from.max(mid_to + 1), to, oracle, right_top, right_offset, f);
                    }
                }
            }
        }
    }

    // (first_line, last_line, per_line, per_char) for a gap node.
    fn height_metrics(&self, oracle: &HeightOracle, offset: usize) -> (usize, usize, f64, f64) {
        let (length, height) = (self.length(), self.height());
        let first_line = oracle.doc.line_at(offset).number;
        let last_line = oracle.doc.line_at(offset + length).number;
        let lines = (last_line - first_line + 1) as f64;
        let mut per_char = 0.0;
        let per_line = if oracle.line_wrapping {
            let total_per_line = height.min(oracle.line_height * lines);
            if length as f64 > lines + 1.0 {
                per_char = (height - total_per_line) / (length as f64 - lines - 1.0);
            }
            total_per_line / lines
        } else {
            height / lines
        };
        (first_line, last_line, per_line, per_char)
    }

    // ---- updates ----

    pub(crate) fn update_height(
        mut self: Box<Self>,
        oracle: &HeightOracle,
        offset: usize,
        force: bool,
        mut measured: Option<&mut MeasuredHeights>,
    ) -> Box<HeightMap> {
        match &mut *self {
            HeightMap::Block(n) => {
                if let Some(m) = measured.filter(|m| m.from <= offset && m.more()) {
                    set_measured_height(n, m);
                }
                self.set_outdated(false);
                self
            }
            HeightMap::Text(n) => {
                if let Some(m) = measured.filter(|m| m.from <= offset && m.more()) {
                    set_measured_height(n, m);
                } else if force || n.flags & FLAG_OUTDATED > 0 {
                    n.space_above = 0.0;
                    let h = n.widget_height.max(oracle.height_for_line(n.length - n.collapsed))
                        + n.breaks as f64 * oracle.line_height;
                    self.set_height(h);
                }
                self.set_outdated(false);
                self
            }
            HeightMap::Gap { length, .. } => {
                let length = *length;
                let end = offset + length;
                if let Some(m) = measured.as_deref_mut().filter(|m| m.from <= end && m.more()) {
                    // Fill in part of this gap with measured lines. We know
                    // there can't be widgets or collapsed ranges in those
                    // lines, because they would already have been added to
                    // the heightmap (gaps only contain plain text).
                    let mut nodes: Nodes = Vec::new();
                    let mut pos = offset.max(m.from);
                    let mut single_height = -1.0;
                    if m.from > offset {
                        nodes.push(Some(
                            HeightMap::new_gap(m.from - offset - 1).update_height(oracle, offset, false, None),
                        ));
                    }
                    while pos <= end && m.more() {
                        let len = oracle.doc.line_at(pos).length();
                        if !nodes.is_empty() {
                            nodes.push(None);
                        }
                        let mut height = m.next();
                        let mut above = 0.0;
                        if height < 0.0 {
                            above = -height;
                            height = m.next();
                        }
                        if single_height == -1.0 {
                            single_height = height;
                        } else if (height - single_height).abs() >= EPSILON {
                            single_height = -2.0;
                        }
                        let mut line = HeightMap::new_text(len, height, above);
                        line.set_outdated(false);
                        nodes.push(Some(line));
                        pos += len + 1;
                    }
                    if pos <= end {
                        nodes.push(None);
                        nodes.push(Some(HeightMap::new_gap(end - pos).update_height(oracle, pos, false, None)));
                    }
                    let result = HeightMap::of(nodes);
                    if single_height < 0.0
                        || (result.height() - self.height()).abs() >= EPSILON
                        || (single_height - self.height_metrics(oracle, offset).2).abs() >= EPSILON
                    {
                        flag_height_change();
                    }
                    return replace_node(NodeKind::Gap, result);
                } else if force || self.outdated() {
                    let h = oracle.height_for_gap(offset, offset + length);
                    self.set_height(h);
                    self.set_outdated(false);
                }
                self
            }
            HeightMap::Branch(_) => {
                let HeightMap::Branch(b) = *self else { unreachable!() };
                let BranchNode { left, right, flags, .. } = b;
                let brk = usize::from(flags & FLAG_BREAK > 0);
                let right_start = offset + left.length() + brk;
                let (left_kind, right_kind) = (left.kind(), right.kind());
                let mut rebalance = false;
                let left = if measured.as_deref().is_some_and(|m| m.from <= offset + left.length() && m.more()) {
                    rebalance = true;
                    left.update_height(oracle, offset, force, measured.as_deref_mut())
                } else {
                    left.update_height(oracle, offset, force, None)
                };
                let right = if measured.as_deref().is_some_and(|m| m.from <= right_start + right.length() && m.more()) {
                    rebalance = true;
                    right.update_height(oracle, right_start, force, measured)
                } else {
                    right.update_height(oracle, right_start, force, None)
                };
                if rebalance {
                    return balanced(brk > 0, left, left_kind, right, right_kind);
                }
                let height = left.height() + right.height();
                let size = left.size() + right.size();
                let length = left.length() + brk + right.length();
                Box::new(HeightMap::Branch(BranchNode {
                    left,
                    right,
                    length,
                    height,
                    flags: flags & !FLAG_OUTDATED,
                    size,
                }))
            }
        }
    }

    #[allow(clippy::boxed_local)]
    // Replace the given range (in this node's coordinates) with the given
    // nodes.
    fn replace(self: Box<Self>, from: usize, to: usize, mut nodes: Nodes) -> Box<HeightMap> {
        match *self {
            HeightMap::Text(n) => {
                let fits = nodes.len() == 1
                    && match nodes[0].as_deref() {
                        Some(HeightMap::Text(_)) => true,
                        Some(gap @ HeightMap::Gap { .. }) => gap.flags() & FLAG_SINGLE_LINE > 0,
                        _ => false,
                    }
                    && n.length.abs_diff(nodes[0].as_ref().unwrap().length()) < 10;
                if fits {
                    let mut node = nodes.pop().unwrap().unwrap();
                    if let HeightMap::Gap { length, .. } = *node {
                        node = HeightMap::new_text(length, n.height, n.space_above);
                    } else {
                        *node.height_mut() = n.height;
                    }
                    if n.flags & FLAG_OUTDATED == 0 {
                        node.set_outdated(false);
                    }
                    node
                } else {
                    HeightMap::of(nodes)
                }
            }
            HeightMap::Block(_) => HeightMap::of(nodes),
            HeightMap::Gap { length, .. } => {
                let after = length - to;
                if after > 0 {
                    match nodes.last() {
                        Some(Some(last)) if matches!(**last, HeightMap::Gap { .. }) => {
                            let len = last.length();
                            *nodes.last_mut().unwrap() = Some(HeightMap::new_gap(len + after));
                        }
                        _ => {
                            nodes.push(None);
                            nodes.push(Some(HeightMap::new_gap(after - 1)));
                        }
                    }
                }
                if from > 0 {
                    match nodes.first() {
                        Some(Some(first)) if matches!(**first, HeightMap::Gap { .. }) => {
                            let len = first.length();
                            nodes[0] = Some(HeightMap::new_gap(from + len));
                        }
                        _ => {
                            nodes.insert(0, None);
                            nodes.insert(0, Some(HeightMap::new_gap(from - 1)));
                        }
                    }
                }
                HeightMap::of(nodes)
            }
            HeightMap::Branch(b) => {
                let brk = usize::from(b.flags & FLAG_BREAK > 0);
                let right_start = b.left.length() + brk;
                let (left_kind, right_kind) = (b.left.kind(), b.right.kind());
                if to < right_start {
                    let left = b.left.replace(from, to, nodes);
                    return balanced(brk > 0, left, left_kind, b.right, right_kind);
                }
                if from > b.left.length() {
                    let right = b.right.replace(from - right_start, to - right_start, nodes);
                    return balanced(brk > 0, b.left, left_kind, right, right_kind);
                }
                let mut result: Nodes = Vec::new();
                if from > 0 {
                    b.left.decompose_left(from, &mut result);
                }
                let left = result.len();
                result.extend(nodes);
                if from > 0 {
                    merge_gaps(&mut result, left as isize - 1);
                }
                if to < b.length {
                    let right = result.len();
                    b.right.decompose_right(to - right_start, &mut result);
                    merge_gaps(&mut result, right as isize);
                }
                HeightMap::of(result)
            }
        }
    }

    #[allow(clippy::boxed_local)]
    fn decompose_left(self: Box<Self>, to: usize, result: &mut Nodes) {
        match *self {
            HeightMap::Gap { .. } => {
                result.push(Some(HeightMap::new_gap(to - 1)));
                result.push(None);
            }
            HeightMap::Branch(b) => {
                let mut left = b.left.length();
                if to <= left {
                    return b.left.decompose_left(to, result);
                }
                result.push(Some(b.left));
                if b.flags & FLAG_BREAK > 0 {
                    left += 1;
                    if to >= left {
                        result.push(None);
                    }
                }
                if to > left {
                    b.right.decompose_left(to - left, result);
                }
            }
            leaf => result.push(Some(Box::new(leaf))),
        }
    }

    #[allow(clippy::boxed_local)]
    fn decompose_right(self: Box<Self>, from: usize, result: &mut Nodes) {
        match *self {
            HeightMap::Gap { length, .. } => {
                result.push(None);
                result.push(Some(HeightMap::new_gap(length - from - 1)));
            }
            HeightMap::Branch(b) => {
                let left = b.left.length();
                let brk = usize::from(b.flags & FLAG_BREAK > 0);
                let right = left + brk;
                if from >= right {
                    return b.right.decompose_right(from - right, result);
                }
                if from < left {
                    b.left.decompose_right(from, result);
                }
                if brk > 0 && from < right {
                    result.push(None);
                }
                result.push(Some(b.right));
            }
            leaf => result.push(Some(Box::new(leaf))),
        }
    }

    /// Update the map for document changes. `oracle` must hold the new
    /// document.
    pub(crate) fn apply_changes(
        self: Box<Self>,
        decorations: &[DecorationSet],
        old_doc: &Text,
        oracle: &mut HeightOracle,
        changes: &[ChangedRange],
    ) -> Box<HeightMap> {
        let mut me = self;
        let doc = oracle.doc.clone();
        let mut i = changes.len();
        while i > 0 {
            i -= 1;
            let ChangedRange { from_a, to_a, from_b, to_b } = changes[i];
            let (mut from_a, mut to_a, mut from_b, mut to_b) = (from_a as i64, to_a as i64, from_b as i64, to_b as i64);
            oracle.set_doc(old_doc);
            let mut start = me.line_at(from_a as f64, QueryType::ByPosNoHeight, oracle, 0.0, 0);
            let end = if start.to() as i64 >= to_a {
                start.clone()
            } else {
                me.line_at(to_a as f64, QueryType::ByPosNoHeight, oracle, 0.0, 0)
            };
            to_b += end.to() as i64 - to_a;
            to_a = end.to() as i64;
            while i > 0 && start.from as i64 <= changes[i - 1].to_a as i64 {
                from_a = changes[i - 1].from_a as i64;
                from_b = changes[i - 1].from_b as i64;
                i -= 1;
                if from_a < start.from as i64 {
                    start = me.line_at(from_a as f64, QueryType::ByPosNoHeight, oracle, 0.0, 0);
                }
            }
            from_b += start.from as i64 - from_a;
            from_a = start.from as i64;
            oracle.set_doc(&doc);
            let nodes = NodeBuilder::build(oracle, decorations, from_b as usize, to_b as usize);
            let kind = me.kind();
            me = replace_node(kind, me.replace(from_a as usize, to_a as usize, nodes));
        }
        me.update_height(oracle, 0, false, None)
    }

    // `nodes` uses `None` to indicate the position of line breaks. There are
    // never line breaks at the start or end of the array, or two line
    // breaks next to each other, and the array isn't allowed to be empty.
    fn of(mut nodes: Nodes) -> Box<HeightMap> {
        if nodes.len() == 1 {
            return nodes.pop().unwrap().expect("a lone line break in a height map");
        }
        let size = |n: &Option<Box<HeightMap>>| n.as_ref().map_or(0, |n| n.size());
        let (mut i, mut j) = (0usize, nodes.len());
        let (mut before, mut after) = (0usize, 0usize);
        loop {
            if i == j {
                if before > after * 2 {
                    let split = nodes.remove(i - 1).unwrap();
                    let HeightMap::Branch(b) = *split else { panic!("expected a branch") };
                    let brk = b.flags & FLAG_BREAK > 0;
                    let (bl, br) = (b.left, b.right);
                    i -= 1;
                    if brk {
                        nodes.splice(i..i, [Some(bl), None, Some(br)]);
                    } else {
                        nodes.splice(i..i, [Some(bl), Some(br)]);
                    }
                    j += 1 + usize::from(brk);
                    before -= b.size;
                } else if after > before * 2 {
                    let split = nodes.remove(j).unwrap();
                    let HeightMap::Branch(b) = *split else { panic!("expected a branch") };
                    let brk = b.flags & FLAG_BREAK > 0;
                    let (bl, br) = (b.left, b.right);
                    if brk {
                        nodes.splice(j..j, [Some(bl), None, Some(br)]);
                    } else {
                        nodes.splice(j..j, [Some(bl), Some(br)]);
                    }
                    j += 2 + usize::from(brk);
                    after -= b.size;
                } else {
                    break;
                }
            } else if before < after {
                before += size(&nodes[i]);
                i += 1;
            } else {
                j -= 1;
                after += size(&nodes[j]);
            }
        }

        let mut brk = false;
        if nodes[i - 1].is_none() {
            brk = true;
            i -= 1;
        } else if nodes[i].is_none() {
            brk = true;
            j += 1;
        }
        let right = nodes.split_off(j);
        nodes.truncate(i);
        HeightMap::new_branch(HeightMap::of(nodes), brk, HeightMap::of(right))
    }
}

impl std::fmt::Display for HeightMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeightMap::Text(n) => {
                write!(f, "line({}", n.length)?;
                if n.collapsed > 0 {
                    write!(f, "-{}", n.collapsed)?;
                }
                if n.widget_height != 0.0 {
                    write!(f, ":{}", n.widget_height)?;
                }
                write!(f, ")")
            }
            HeightMap::Block(n) => write!(f, "block({})", n.length),
            HeightMap::Gap { length, .. } => write!(f, "gap({length})"),
            HeightMap::Branch(b) => {
                write!(f, "{}{}{}", b.left, if b.flags & FLAG_BREAK > 0 { " " } else { "-" }, b.right)
            }
        }
    }
}

fn space_block(offset: usize, top: f64, space_above: f64) -> BlockInfo {
    let deco = Decoration::replace(ReplaceSpec::default());
    BlockInfo::new(offset, 0, top, space_above, BlockContent::Deco(Arc::new(deco)))
}

fn set_measured_height(n: &mut LineNode, measured: &mut MeasuredHeights) {
    let mut next = measured.next();
    if next < 0.0 {
        n.space_above = -next;
        next = measured.next();
    } else {
        n.space_above = 0.0;
    }
    if n.height != next {
        if (n.height - next).abs() > EPSILON {
            flag_height_change();
        }
        n.height = next;
    }
}

fn replace_node(old: NodeKind, val: Box<HeightMap>) -> Box<HeightMap> {
    if old != val.kind() {
        flag_height_change();
    }
    val
}

fn balanced(
    brk: bool,
    left: Box<HeightMap>,
    old_left: NodeKind,
    right: Box<HeightMap>,
    old_right: NodeKind,
) -> Box<HeightMap> {
    if left.size() > 2 * right.size() || right.size() > 2 * left.size() {
        return HeightMap::of(if brk { vec![Some(left), None, Some(right)] } else { vec![Some(left), Some(right)] });
    }
    let left = replace_node(old_left, left);
    let right = replace_node(old_right, right);
    let outdated = left.outdated() || right.outdated();
    let height = left.height() + right.height();
    let size = left.size() + right.size();
    let length = left.length() + usize::from(brk) + right.length();
    // CodeMirror updates the branch in place, which counts a height change
    // through `setHeight`; the branch height is the sum of its children,
    // whose changes were already flagged.
    Box::new(HeightMap::Branch(BranchNode {
        left,
        right,
        length,
        height,
        flags: (if brk { FLAG_BREAK } else { 0 }) | (if outdated { FLAG_OUTDATED } else { 0 }),
        size,
    }))
}

fn merge_gaps(nodes: &mut Nodes, around: isize) {
    if around < 1 {
        return;
    }
    let around = around as usize;
    if around + 1 >= nodes.len() || nodes[around].is_some() {
        return;
    }
    let (Some(before), Some(after)) = (&nodes[around - 1], &nodes[around + 1]) else { return };
    if let (HeightMap::Gap { length: a, .. }, HeightMap::Gap { length: b, .. }) = (&**before, &**after) {
        let merged = HeightMap::new_gap(a + 1 + b);
        nodes.splice(around - 1..around + 2, [Some(merged)]);
    }
}

const RELEVANT_WIDGET_HEIGHT: f64 = 5.0;

struct NodeBuilder<'a> {
    nodes: Nodes,
    written_to: usize,
    line_start: Option<usize>,
    line_end: Option<usize>,
    covering: Option<usize>,
    pos: usize,
    oracle: &'a HeightOracle,
}

impl<'a> NodeBuilder<'a> {
    fn is_covered(&self) -> bool {
        self.covering.is_some_and(|c| c + 1 == self.nodes.len())
    }

    fn last_text(&mut self) -> Option<&mut LineNode> {
        match self.nodes.last_mut() {
            Some(Some(node)) => match &mut **node {
                HeightMap::Text(n) => Some(n),
                _ => None,
            },
            _ => None,
        }
    }

    fn enter_line(&mut self) {
        if self.line_start.is_some() {
            return;
        }
        let line = self.oracle.doc.line_at(self.pos);
        self.line_start = Some(line.from);
        self.line_end = Some(line.to);
        if self.written_to < line.from {
            // In CodeMirror an empty list also counts (`undefined == null`).
            if self.written_to + 1 < line.from || matches!(self.nodes.last(), Some(None) | None) {
                let blank = self.blank_content(self.written_to, line.from - 1);
                self.nodes.push(Some(blank));
            }
            self.nodes.push(None);
        }
        if self.pos > line.from {
            self.nodes.push(Some(HeightMap::new_text(self.pos - line.from, -1.0, 0.0)));
        }
        self.written_to = self.pos;
    }

    fn blank_content(&self, from: usize, to: usize) -> Box<HeightMap> {
        let mut gap = HeightMap::new_gap(to - from);
        if self.oracle.doc.line_at(from).to == to {
            *gap.flags_mut() |= FLAG_SINGLE_LINE;
        }
        gap
    }

    fn ensure_line(&mut self) -> &mut LineNode {
        self.enter_line();
        if self.last_text().is_none() {
            self.nodes.push(Some(HeightMap::new_text(0, -1.0, 0.0)));
        }
        self.last_text().unwrap()
    }

    fn add_block(&mut self, block: Box<HeightMap>, deco: &Decoration) {
        self.enter_line();
        if deco.start_side > 0 && !self.is_covered() {
            self.ensure_line();
        }
        let len = block.length();
        self.nodes.push(Some(block));
        self.pos += len;
        self.written_to = self.pos;
        if deco.end_side > 0 {
            self.covering = Some(self.nodes.len() - 1);
        }
    }

    fn add_line_deco(&mut self, height: f64, breaks: usize, length: usize) {
        let line = self.ensure_line();
        line.length += length;
        line.collapsed += length;
        line.widget_height = line.widget_height.max(height);
        line.breaks += breaks;
        self.pos += length;
        self.written_to = self.pos;
    }

    fn finish(mut self, from: usize) -> Nodes {
        let last_is_text = self.last_text().is_some();
        let last_is_break = matches!(self.nodes.last(), Some(None)) || self.nodes.is_empty();
        if self.line_start.is_some() && !last_is_text && !self.is_covered() {
            self.nodes.push(Some(HeightMap::new_text(0, -1.0, 0.0)));
        } else if self.written_to < self.pos || last_is_break {
            let blank = self.blank_content(self.written_to, self.pos);
            self.nodes.push(Some(blank));
        }
        let mut pos = from;
        let oracle = self.oracle;
        let nodes = std::mem::take(&mut self.nodes);
        let mut result = Vec::with_capacity(nodes.len());
        for node in nodes {
            let len = node.as_ref().map_or(1, |n| n.length());
            let node =
                node.map(
                    |n| if matches!(*n, HeightMap::Text(_)) { n.update_height(oracle, pos, false, None) } else { n },
                );
            result.push(node);
            pos += len;
        }
        result
    }

    // Always called with a region that on both sides either stretches to a
    // line break or the end of the document.
    fn build(oracle: &HeightOracle, decorations: &[DecorationSet], from: usize, to: usize) -> Nodes {
        let mut builder = NodeBuilder {
            nodes: Vec::new(),
            written_to: from,
            line_start: None,
            line_end: None,
            covering: None,
            pos: from,
            oracle,
        };
        RangeSet::spans(decorations, from, to, &mut builder, 0);
        builder.finish(from)
    }
}

impl SpanIterator<Decoration> for NodeBuilder<'_> {
    fn span(&mut self, _from: usize, to: usize, _active: &[Arc<Decoration>], _open_start: usize) {
        if self.line_start.is_some() {
            let end = to.min(self.line_end.unwrap());
            let pos = self.pos;
            if let Some(last) = self.last_text() {
                last.length += end - pos;
            } else if end > pos || !self.is_covered() {
                self.nodes.push(Some(HeightMap::new_text(end - pos, -1.0, 0.0)));
            }
            self.written_to = end;
            if to > end {
                self.nodes.push(None);
                self.written_to += 1;
                self.line_start = None;
            }
        }
        self.pos = to;
    }

    fn point(
        &mut self,
        from: usize,
        to: usize,
        deco: &Arc<Decoration>,
        _active: &[Arc<Decoration>],
        _open_start: usize,
        _index: usize,
    ) {
        if from < to || deco.height_relevant() {
            let mut height = deco.widget.as_ref().map_or(0.0, |w| w.estimated_height());
            let breaks = deco.widget.as_ref().map_or(0, |w| w.line_breaks());
            if height < 0.0 {
                height = self.oracle.line_height;
            }
            let len = to - from;
            if deco.is_block() {
                self.add_block(HeightMap::new_block(len, height, Some(deco.clone())), deco);
            } else if len > 0 || breaks > 0 || height >= RELEVANT_WIDGET_HEIGHT {
                self.add_line_deco(height, breaks, len);
            }
        } else if to > from {
            self.span(from, to, &[], 0);
        }
        if let Some(line_end) = self.line_end {
            if line_end < self.pos {
                self.line_end = Some(self.oracle.doc.line_at(self.pos).to);
            }
        }
    }
}

/// The ranges (as flat from/to pairs) where height-relevant decorations
/// differ between `a` and `b`.
pub(crate) fn height_relevant_deco_changes(a: &[DecorationSet], b: &[DecorationSet], diff: &ChangeDesc) -> Vec<usize> {
    struct Comparator {
        changes: Vec<usize>,
    }
    impl RangeComparator<Decoration> for Comparator {
        fn compare_range(&mut self, _: usize, _: usize, _: &[Arc<Decoration>], _: &[Arc<Decoration>]) {}
        fn compare_point(&mut self, from: usize, to: usize, a: Option<&Arc<Decoration>>, b: Option<&Arc<Decoration>>) {
            if from < to || a.is_some_and(|a| a.height_relevant()) || b.is_some_and(|b| b.height_relevant()) {
                add_range(from, to, &mut self.changes, 5);
            }
        }
    }
    let mut comp = Comparator { changes: Vec::new() };
    RangeSet::compare(a, b, diff, &mut comp, 0);
    comp.changes
}

#[cfg(test)]
mod tests;
