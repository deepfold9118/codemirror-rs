//! The document data structure. Port of `text.ts`.
//!
//! CodeMirror's `Text` is its own balanced tree of line arrays. This port
//! wraps a [`crop::Rope`] instead, which gives the same properties that
//! matter to the rest of the library: edits are O(log n), and cloning is
//! cheap because unchanged nodes are shared, so every editor state can keep
//! its own immutable document.
//!
//! Positions are **byte offsets** into the UTF-8 text (CodeMirror uses UTF-16
//! code units). Lines are separated by `"\n"`, which counts as one position,
//! as in CodeMirror. Other line separators are normalized away when text is
//! created from a string.

use std::fmt;

use crop::Rope;

/// The document's line separator inside a [`Text`].
pub(crate) const NEWLINE: &str = "\n";

/// An immutable text document.
#[derive(Clone, Default)]
pub struct Text {
    rope: Rope,
}

/// Describes a line in the document. Created on demand by [`Text::line`]
/// and [`Text::line_at`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The position of the start of the line.
    pub from: usize,
    /// The position at the end of the line (before the line break, or at
    /// the end of the document for the last line).
    pub to: usize,
    /// This line's line number (1-based).
    pub number: usize,
    /// The line's content.
    pub text: String,
}

impl Line {
    /// The length of the line (not including any line break after it).
    pub fn length(&self) -> usize {
        self.to - self.from
    }
}

impl Text {
    /// The empty document.
    pub fn empty() -> Text {
        Text { rope: Rope::new() }
    }

    /// Create a document from an array of lines.
    ///
    /// # Panics
    ///
    /// Panics when `lines` is empty (a document has at least one line) or
    /// when a line contains a `"\n"`.
    pub fn of<S: AsRef<str>>(lines: &[S]) -> Text {
        assert!(!lines.is_empty(), "A document must have at least one line");
        let mut s = String::new();
        for (i, line) in lines.iter().enumerate() {
            let line = line.as_ref();
            assert!(!line.contains('\n'), "Lines passed to Text::of may not contain line breaks");
            if i > 0 {
                s.push('\n');
            }
            s.push_str(line);
        }
        Text { rope: Rope::from(s.as_str()) }
    }

    /// Create a document from a string, splitting it into lines on `sep`
    /// (or on `"\r\n"`, `"\r"` and `"\n"` when `sep` is `None`).
    pub fn from_str_with_separator(s: &str, sep: Option<&str>) -> Text {
        Text { rope: Rope::from(normalize_line_breaks(s, sep).as_str()) }
    }

    /// The underlying rope.
    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    /// The length of the document, in bytes.
    pub fn len(&self) -> usize {
        self.rope.byte_len()
    }

    /// True when the document is empty.
    pub fn is_empty(&self) -> bool {
        self.rope.is_empty()
    }

    /// The number of lines in the document (always at least 1).
    pub fn lines(&self) -> usize {
        self.rope.line_of_byte(self.rope.byte_len()) + 1
    }

    fn line_start(&self, index: usize) -> usize {
        self.rope.byte_of_line(index)
    }

    fn make_line(&self, index: usize) -> Line {
        let from = self.line_start(index);
        let to = if index + 1 < self.lines() { self.line_start(index + 1) - 1 } else { self.len() };
        Line { from, to, number: index + 1, text: self.rope.byte_slice(from..to).to_string() }
    }

    /// Get the description for the line at the given position. A position
    /// inside a character, such as an estimate from the height map, gives
    /// the line of that character.
    ///
    /// # Panics
    ///
    /// Panics when `pos` is outside the document.
    pub fn line_at(&self, pos: usize) -> Line {
        assert!(pos <= self.len(), "Invalid position {pos} in document of length {}", self.len());
        let mut pos = pos;
        while !self.rope.is_char_boundary(pos) {
            pos -= 1;
        }
        self.make_line(self.rope.line_of_byte(pos))
    }

    /// Get the description for the given (1-based) line number.
    ///
    /// # Panics
    ///
    /// Panics when there is no such line.
    pub fn line(&self, n: usize) -> Line {
        assert!(n >= 1 && n <= self.lines(), "Invalid line number {n} in {}-line document", self.lines());
        self.make_line(n - 1)
    }

    /// Replace a range of the text with the given content. Out-of-range
    /// boundaries are clipped to the document.
    pub fn replace(&self, from: usize, to: usize, text: &Text) -> Text {
        let (from, to) = clip(self, from, to);
        let mut rope = self.rope.clone();
        if from == to {
            if !text.is_empty() {
                rope.insert(from, text.to_string());
            }
        } else {
            rope.replace(from..to, text.to_string());
        }
        Text { rope }
    }

    /// Append another document to this one.
    pub fn append(&self, other: &Text) -> Text {
        self.replace(self.len(), self.len(), other)
    }

    /// Retrieve the text between the given points. Out-of-range boundaries
    /// are clipped to the document.
    pub fn slice(&self, from: usize, to: usize) -> Text {
        let (from, to) = clip(self, from, to);
        Text { rope: Rope::from(self.rope.byte_slice(from..to)) }
    }

    /// Whether `pos` is at a char boundary (or at or past the end).
    pub fn is_char_boundary(&self, pos: usize) -> bool {
        pos >= self.len() || self.rope.is_char_boundary(pos)
    }

    /// The largest char boundary at or before `pos`, clipped to the
    /// document.
    pub fn floor_char_boundary(&self, pos: usize) -> usize {
        let mut pos = pos.min(self.len());
        while !self.is_char_boundary(pos) {
            pos -= 1;
        }
        pos
    }

    /// The smallest char boundary at or after `pos`, clipped to the
    /// document.
    pub fn ceil_char_boundary(&self, pos: usize) -> usize {
        let mut pos = pos.min(self.len());
        while !self.is_char_boundary(pos) {
            pos += 1;
        }
        pos
    }

    /// Retrieve a part of the document as a string, joining lines with
    /// `line_sep`. Out-of-range boundaries are clipped to the document.
    pub fn slice_string(&self, from: usize, to: usize, line_sep: &str) -> String {
        let (from, to) = clip(self, from, to);
        let mut out = String::with_capacity(to - from);
        for chunk in self.rope.byte_slice(from..to).chunks() {
            out.push_str(chunk);
        }
        if line_sep != NEWLINE {
            out = out.replace(NEWLINE, line_sep);
        }
        out
    }

    /// Iterate over the content in chunks. Chunk boundaries are arbitrary;
    /// line breaks are part of the chunks.
    pub fn chunks(&self) -> impl Iterator<Item = &str> {
        self.rope.chunks()
    }

    /// Iterate over the chunks of a range of the content. Out-of-range
    /// boundaries are clipped to the document.
    pub fn iter_range(&self, from: usize, to: usize) -> impl Iterator<Item = &str> {
        let (from, to) = clip(self, from, to);
        self.rope.byte_slice(from..to).chunks()
    }

    /// Iterate over the lines between the given (1-based) line numbers,
    /// `from` inclusive, `to` exclusive (defaults to all lines).
    pub fn iter_lines(&self, from: Option<usize>, to: Option<usize>) -> impl Iterator<Item = String> + '_ {
        let from = from.unwrap_or(1).max(1);
        let to = to.unwrap_or(self.lines() + 1).min(self.lines() + 1);
        (from..to.max(from)).map(move |n| self.line(n).text)
    }

    /// Return the document's lines as strings.
    pub fn to_lines(&self) -> Vec<String> {
        self.iter_lines(None, None).collect()
    }
}

impl PartialEq for Text {
    fn eq(&self, other: &Text) -> bool {
        self.len() == other.len() && self.rope == other.rope
    }
}

impl Eq for Text {}

impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for chunk in self.rope.chunks() {
            f.write_str(chunk)?;
        }
        Ok(())
    }
}

impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Text({:?})", self.to_string())
    }
}

impl From<&str> for Text {
    /// Splits on `"\r\n"`, `"\r"` and `"\n"`.
    fn from(s: &str) -> Text {
        Text::from_str_with_separator(s, None)
    }
}

impl From<String> for Text {
    fn from(s: String) -> Text {
        Text::from(s.as_str())
    }
}

fn clip(text: &Text, from: usize, to: usize) -> (usize, usize) {
    let from = from.min(text.len());
    (from, to.min(text.len()).max(from))
}

/// Convert `sep` (or any of `"\r\n"`, `"\r"`, `"\n"` when `None`) to `"\n"`.
pub(crate) fn normalize_line_breaks(s: &str, sep: Option<&str>) -> String {
    match sep {
        None => {
            if !s.contains('\r') {
                return s.to_string();
            }
            s.replace("\r\n", "\n").replace('\r', "\n")
        }
        Some(sep) if sep == NEWLINE || sep.is_empty() => s.to_string(),
        Some(sep) => s.replace(sep, NEWLINE),
    }
}
