//! The visual theme. CodeMirror styles the editor with CSS (`theme.ts`);
//! here a [`Theme`] value, provided through the [`THEME`](crate::THEME)
//! facet, holds the font and colors.

use egui::{Color32, FontId, Visuals};

/// Fonts, colors and spacing for an editor view.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// The editor font.
    pub font: FontId,
    /// Line height as a multiple of the font size (CSS `line-height`).
    pub line_height: f32,
    /// Text color.
    pub text: Color32,
    /// Editor background.
    pub background: Color32,
    /// Selection background while the editor has focus.
    pub selection: Color32,
    /// Selection background without focus.
    pub selection_unfocused: Color32,
    /// Caret color.
    pub caret: Color32,
    /// Caret width in points.
    pub caret_width: f32,
    /// Background of lines with a cursor, when the active line highlight
    /// is enabled.
    pub active_line: Color32,
    /// Gutter background.
    pub gutter_background: Color32,
    /// Line number color.
    pub gutter_text: Color32,
    /// Line number color on lines with a cursor.
    pub gutter_active_text: Color32,
    /// Space between the gutter (or the left edge) and the text.
    pub padding_left: f32,
    /// Space to the right of the text.
    pub padding_right: f32,
    /// Space above the first line and below the last.
    pub padding_y: f32,
    /// Underline color for IME composition text.
    pub composition: Color32,
}

impl Theme {
    /// A theme that follows the given egui visuals.
    pub fn from_visuals(visuals: &Visuals) -> Theme {
        let text = visuals.text_color();
        let selection = visuals.selection.bg_fill;
        Theme {
            font: FontId::monospace(14.0),
            line_height: 1.4,
            text,
            background: visuals.extreme_bg_color,
            selection,
            selection_unfocused: selection.gamma_multiply(0.55),
            caret: visuals.text_cursor.stroke.color,
            caret_width: 1.5,
            active_line: if visuals.dark_mode {
                Color32::from_rgba_unmultiplied(0x99, 0xee, 0xff, 0x0c)
            } else {
                Color32::from_rgba_unmultiplied(0xcc, 0xee, 0xff, 0x44)
            },
            gutter_background: visuals.faint_bg_color,
            gutter_text: visuals.weak_text_color(),
            gutter_active_text: text,
            padding_left: 6.0,
            padding_right: 2.0,
            padding_y: 4.0,
            composition: text,
        }
    }

    /// The default dark theme.
    pub fn dark() -> Theme {
        Theme::from_visuals(&Visuals::dark())
    }

    /// The default light theme.
    pub fn light() -> Theme {
        Theme::from_visuals(&Visuals::light())
    }

    /// The height of a line of text.
    pub fn row_height(&self) -> f32 {
        (self.font.size * self.line_height).round()
    }
}
