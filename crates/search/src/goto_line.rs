//! The position logic of `goto-line.ts`. CodeMirror's `gotoLine` command
//! asks for the line in a dialog; here the host asks, and passes the
//! answer to [`goto_line_selection`].

use codemirror_state::{EditorSelection, EditorState};

/// The cursor position for a "go to line" input, or `None` when the input
/// isn't valid.
///
/// Accepts line numbers, relative line offsets prefixed with `+` or `-`,
/// document percentages suffixed with `%`, and an optional column after a
/// `:` (counted in bytes, as all positions are).
pub fn goto_line_selection(state: &EditorState, input: &str) -> Option<EditorSelection> {
    // /^([+-])?(\d+)?(:\d+)?(%)?$/
    let mut rest = input;
    let sign = rest.chars().next().filter(|c| *c == '+' || *c == '-');
    if sign.is_some() {
        rest = &rest[1..];
    }
    let digits = |s: &str| s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let n = digits(rest);
    let ln = (n > 0).then(|| &rest[..n]);
    rest = &rest[n..];
    let mut cl = None;
    if let Some(after) = rest.strip_prefix(':') {
        let n = digits(after);
        if n == 0 {
            return None;
        }
        cl = Some(&after[..n]);
        rest = &after[n..];
    }
    let percent = rest == "%";
    if !rest.is_empty() && !percent {
        return None;
    }

    let doc = state.doc();
    let start_line = doc.line_at(state.selection().main().head());
    let col: usize = cl.and_then(|c| c.parse().ok()).unwrap_or(0);
    let mut line: f64 = match ln {
        Some(ln) => ln.parse().ok()?,
        None => start_line.number as f64,
    };
    if ln.is_some() && percent {
        let mut pc = line / 100.0;
        if let Some(sign) = sign {
            pc = pc * if sign == '-' { -1.0 } else { 1.0 } + start_line.number as f64 / doc.lines() as f64;
        }
        line = (doc.lines() as f64 * pc).round();
    } else if ln.is_some() {
        if let Some(sign) = sign {
            line = line * if sign == '-' { -1.0 } else { 1.0 } + start_line.number as f64;
        }
    }
    let target = doc.line((line.max(1.0) as usize).min(doc.lines()));
    let pos = doc.floor_char_boundary(target.from + col.min(target.length()));
    Some(EditorSelection::single(pos, pos))
}
