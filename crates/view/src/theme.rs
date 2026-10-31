//! The visual theme. CodeMirror styles the editor with CSS (`theme.ts`);
//! here a [`Theme`] value, provided through the [`THEME`](crate::THEME)
//! facet, holds the font and colors.

use gpui::{font, rgb, rgba, Font, FontFeatures, Hsla, SharedString};

/// Fonts, colors and spacing for an editor view.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// The editor font family.
    pub font_family: SharedString,
    /// OpenType features of the editor font (CSS `font-feature-settings`),
    /// such as [`FontFeatures::disable_ligatures`].
    pub font_features: FontFeatures,
    /// Font size in pixels.
    pub font_size: f32,
    /// Line height as a multiple of the font size (CSS `line-height`).
    pub line_height: f32,
    /// Text color.
    pub text: Hsla,
    /// Editor background.
    pub background: Hsla,
    /// Selection background while the editor has focus.
    pub selection: Hsla,
    /// Selection background without focus.
    pub selection_unfocused: Hsla,
    /// Caret color.
    pub caret: Hsla,
    /// Caret width in pixels.
    pub caret_width: f32,
    /// Background of lines with a cursor, when the active line highlight
    /// is enabled.
    pub active_line: Hsla,
    /// Gutter background.
    pub gutter_background: Hsla,
    /// Line number color.
    pub gutter_text: Hsla,
    /// Line number color on lines with a cursor.
    pub gutter_active_text: Hsla,
    /// Scrollbar thumb.
    pub scrollbar: Hsla,
    /// Space between the gutter (or the left edge) and the text.
    pub padding_left: f32,
    /// Space above the first line and below the last.
    pub padding_y: f32,
}

impl Theme {
    /// The default dark theme.
    pub fn dark() -> Theme {
        Theme {
            font_family: "Consolas".into(),
            font_features: FontFeatures::default(),
            font_size: 14.0,
            line_height: 1.4,
            text: rgb(0xd4d4d4).into(),
            background: rgb(0x1e1e1e).into(),
            selection: rgb(0x264f78).into(),
            selection_unfocused: rgb(0x3a3d41).into(),
            caret: rgb(0xaeafad).into(),
            caret_width: 1.5,
            active_line: rgba(0xffffff0a).into(),
            gutter_background: rgb(0x1e1e1e).into(),
            gutter_text: rgb(0x858585).into(),
            gutter_active_text: rgb(0xc6c6c6).into(),
            scrollbar: rgba(0x79797966).into(),
            padding_left: 6.0,
            padding_y: 4.0,
        }
    }

    /// The default light theme.
    pub fn light() -> Theme {
        Theme {
            text: rgb(0x1f1f1f).into(),
            background: rgb(0xffffff).into(),
            selection: rgb(0xadd6ff).into(),
            selection_unfocused: rgb(0xe5ebf1).into(),
            caret: rgb(0x000000).into(),
            active_line: rgba(0x0000000a).into(),
            gutter_background: rgb(0xffffff).into(),
            gutter_text: rgb(0x6e7681).into(),
            gutter_active_text: rgb(0x171184).into(),
            scrollbar: rgba(0x64646466).into(),
            ..Theme::dark()
        }
    }

    /// The editor font.
    pub fn font(&self) -> Font {
        Font { features: self.font_features.clone(), ..font(self.font_family.clone()) }
    }

    /// The height of a line of text.
    pub fn row_height(&self) -> f32 {
        (self.font_size * self.line_height).round()
    }
}
