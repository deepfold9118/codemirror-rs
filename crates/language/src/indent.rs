//! Indentation. Port of `indent.ts`, without the syntax-tree strategies
//! (`indentNodeProp`, `TreeIndentContext`, `delimitedIndent`, ...), which
//! wait for the parser.

use std::sync::{Arc, LazyLock};

use codemirror_state::{count_column, ChangeSet, ChangeSpec, EditorState, Facet, FacetSpec};

use crate::{is_space, leading_space};

/// A function that computes the indentation column for the line starting at
/// the given position. `None` defers to the next service. `Some(None)`
/// means no indentation can be determined and the line should inherit the
/// indentation of the one above it (CodeMirror's `undefined` and `null`).
pub type IndentService = Arc<dyn Fn(&IndentContext, usize) -> Option<Option<usize>> + Send + Sync>;

/// Facet that defines a way to provide a function that computes the
/// appropriate indentation depth, as a column number (see
/// [`indent_string`]), at the start of a given line.
pub static INDENT_SERVICE: LazyLock<Facet<IndentService>> = LazyLock::new(Facet::list);

/// Facet for overriding the unit by which indentation happens. Should be a
/// string consisting entirely of the same whitespace character. When not
/// set, this defaults to 2 spaces.
pub static INDENT_UNIT: LazyLock<Facet<String, String>> = LazyLock::new(|| {
    Facet::define(
        FacetSpec::combine(|values: &[String]| {
            let Some(unit) = values.first() else { return "  ".to_string() };
            let first = unit.chars().next();
            if first.is_none() || unit.chars().any(|c| !is_space(c) || Some(c) != first) {
                panic!("Invalid indent unit: {unit:?}");
            }
            unit.clone()
        })
        .compare_eq(),
    )
});

/// Return the _column width_ of an indent unit in the state. Determined by
/// the [`INDENT_UNIT`] facet, and the tab size when that contains tabs.
pub fn get_indent_unit(state: &EditorState) -> usize {
    let unit = state.facet(&INDENT_UNIT);
    let len = unit.chars().count();
    if unit.starts_with('\t') {
        state.tab_size() * len
    } else {
        len
    }
}

/// Create an indentation string that covers columns 0 to `cols`. Will use
/// tabs for as much of the columns as possible when the [`INDENT_UNIT`]
/// facet contains tabs.
pub fn indent_string(state: &EditorState, mut cols: usize) -> String {
    let mut result = String::new();
    let ts = state.tab_size();
    let mut ch = state.facet(&INDENT_UNIT).chars().next().unwrap_or(' ');
    if ch == '\t' {
        while cols >= ts {
            result.push('\t');
            cols -= ts;
        }
        ch = ' ';
    }
    for _ in 0..cols {
        result.push(ch);
    }
    result
}

/// Get the indentation, as a column number, at the given position. Will
/// first consult any [indent services](INDENT_SERVICE) that are registered,
/// and if none of those return an indentation, falls back to the syntax
/// tree. Returns `None` when no indentation could be determined.
///
/// There is no syntax tree yet. This port answers the way CodeMirror does
/// when no language is loaded: its empty tree only covers position 0, where
/// the top-level indentation is 0.
pub fn get_indentation(context: &IndentContext, pos: usize) -> Option<usize> {
    for service in context.state.facet(&INDENT_SERVICE) {
        if let Some(result) = service(context, pos) {
            return result;
        }
    }
    if pos == 0 {
        Some(0)
    } else {
        None
    }
}

/// Create a change set that auto-indents all lines touched by the given
/// document range.
pub fn indent_range(state: &EditorState, from: usize, to: usize) -> ChangeSet {
    let updated = std::cell::RefCell::new(std::collections::HashMap::<usize, usize>::new());
    let context = IndentContext::with_options(
        state,
        IndentOptions {
            override_indentation: Some(Box::new(|start| updated.borrow().get(&start).copied())),
            ..IndentOptions::default()
        },
    );
    let mut changes = Vec::new();
    let mut pos = from;
    while pos <= to {
        let line = state.doc().line_at(pos);
        pos = line.to + 1;
        let Some(mut indent) = get_indentation(&context, line.from) else { continue };
        if line.text.chars().all(is_space) {
            indent = 0;
        }
        let cur = leading_space(&line.text);
        let norm = indent_string(state, indent);
        if line.text[..cur] != norm {
            updated.borrow_mut().insert(line.from, indent);
            changes.push(ChangeSpec::replace(line.from, line.from + cur, norm));
        }
    }
    state.changes(changes)
}

/// Options for an [`IndentContext`].
#[derive(Default)]
pub struct IndentOptions<'a> {
    /// Override line indentations provided to the indentation helper
    /// function, which is useful when implementing region indentation,
    /// where indentation for later lines needs to refer to previous lines,
    /// which may have been reindented compared to the original start state.
    /// If given, this function should return `None` for lines (given by
    /// start position) that didn't change, and an updated indentation
    /// otherwise.
    pub override_indentation: Option<Box<dyn Fn(usize) -> Option<usize> + 'a>>,
    /// Make it look, to the indent logic, like a line break was added at
    /// the given position (which is mostly just useful for implementing
    /// something like `insert_newline_and_indent`).
    pub simulate_break: Option<usize>,
    /// When `simulate_break` is given, this can be used to make the
    /// simulated break behave like a double line break.
    pub simulate_double_break: bool,
}

/// Indentation contexts are used when calling [indentation
/// services](INDENT_SERVICE). They provide helper utilities useful in
/// indentation logic, and can selectively override the indentation
/// reported for some lines.
pub struct IndentContext<'a> {
    /// The editor state.
    pub state: &'a EditorState,
    /// The indent unit (number of columns per indentation level).
    pub unit: usize,
    options: IndentOptions<'a>,
}

impl<'a> IndentContext<'a> {
    /// Create an indent context.
    pub fn new(state: &'a EditorState) -> Self {
        IndentContext::with_options(state, IndentOptions::default())
    }

    /// Create an indent context with options.
    pub fn with_options(state: &'a EditorState, options: IndentOptions<'a>) -> Self {
        IndentContext { state, unit: get_indent_unit(state), options }
    }

    /// Get the text and start position of the line at the given position,
    /// taking simulated line breaks into account. If there is such a break
    /// at `pos`, the `bias` argument determines whether the part of the
    /// line before or after the break is used.
    pub fn line_at(&self, pos: usize, bias: i32) -> (String, usize) {
        let line = self.state.doc().line_at(pos);
        if let Some(sim) = self.options.simulate_break {
            if sim >= line.from && sim <= line.to {
                if self.options.simulate_double_break && sim == pos {
                    return (String::new(), pos);
                } else if if bias < 0 { sim < pos } else { sim <= pos } {
                    return (line.text[sim - line.from..].to_string(), sim);
                } else {
                    return (line.text[..sim - line.from].to_string(), line.from);
                }
            }
        }
        (line.text, line.from)
    }

    /// Get the text directly after `pos`, either the entire line or the
    /// next 100 bytes (rounded down to a character boundary), whichever is
    /// shorter.
    pub fn text_after_pos(&self, pos: usize, bias: i32) -> String {
        if self.options.simulate_double_break && Some(pos) == self.options.simulate_break {
            return String::new();
        }
        let (text, from) = self.line_at(pos, bias);
        let start = pos - from;
        let mut end = text.len().min(start + 100);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text[start..end].to_string()
    }

    /// Find the column for the given position.
    pub fn column(&self, pos: usize, bias: i32) -> usize {
        let (text, from) = self.line_at(pos, bias);
        let result = self.count_column(&text, pos - from);
        match self.override_at(from) {
            Some(over) => (result + over).saturating_sub(self.count_column(&text, leading_space(&text))),
            None => result,
        }
    }

    /// Find the column position (taking tabs into account) of the given
    /// byte offset in the given string.
    pub fn count_column(&self, line: &str, pos: usize) -> usize {
        count_column(line, self.state.tab_size(), Some(pos))
    }

    /// Find the indentation column of the line at the given point.
    pub fn line_indent(&self, pos: usize, bias: i32) -> usize {
        let (text, from) = self.line_at(pos, bias);
        if let Some(over) = self.override_at(from) {
            return over;
        }
        self.count_column(&text, leading_space(&text))
    }

    /// Returns the simulated line break for this context, if any.
    pub fn simulated_break(&self) -> Option<usize> {
        // CodeMirror uses `simulateBreak || null`, so a break at 0 counts as none.
        self.options.simulate_break.filter(|&b| b != 0)
    }

    fn override_at(&self, from: usize) -> Option<usize> {
        self.options.override_indentation.as_ref().and_then(|f| f(from))
    }
}
