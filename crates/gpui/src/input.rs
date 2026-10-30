//! Text input from the platform: typed characters, IME composition and the
//! candidate window position. GPUI speaks UTF-16 offsets here; the
//! document uses UTF-8 bytes, converted through the rope.

use std::ops::Range;

use gpui::{point, px, Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window};

use crate::view::EditorView;

impl EditorView {
    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.utf16_to_byte(r.start)..self.utf16_to_byte(r.end)
    }

    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.byte_to_utf16(r.start)..self.byte_to_utf16(r.end)
    }
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.state().slice_doc(range.start, range.end))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let main = self.state().selection().main();
        Some(UTF16Selection {
            range: self.range_to_utf16(&(main.from..main.to)),
            reversed: main.head() < main.anchor(),
        })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state().read_only() {
            return;
        }
        let range = range_utf16.map(|r| self.range_from_utf16(&r));
        self.replace_text(range, text, false, None, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state().read_only() {
            return;
        }
        let range = range_utf16.map(|r| self.range_from_utf16(&r));
        // The new selection is in UTF-16 units within the new text.
        let to_byte = |u: usize| {
            let mut units = 0;
            for (i, c) in new_text.char_indices() {
                if units >= u {
                    return i;
                }
                units += c.len_utf16();
            }
            new_text.len()
        };
        let sel = new_selected_range_utf16.map(|r| to_byte(r.start)..to_byte(r.end));
        self.replace_text(range, new_text, true, sel, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let pos = self.utf16_to_byte(range_utf16.start);
        let (x, top, bottom) = self.doc_coords(window, pos, 1);
        let o = self.origin();
        Some(Bounds::from_corners(point(o.x + px(x), o.y + px(top)), point(o.x + px(x + 1.0), o.y + px(bottom))))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let (pos, _) = self.pos_at_point(window, point);
        Some(self.byte_to_utf16(pos))
    }
}
