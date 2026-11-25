//! Selecting with the mouse: a press puts the caret, Shift extends, a
//! double press selects a word and a triple press a line, and dragging
//! extends by the same unit, as does a Shift press after them while the
//! selection is still theirs. The wheel scrolls text that overflows.
//!
//! The field's handlers are the first on its part, so ones the app adds
//! there run before them, as a `mousedown` listener runs before the
//! browser's default. `window.prevent_default()` or `cx.stop_propagation()`
//! in one keeps the field from placing the caret.

use std::ops::Range;

use gpui::{Context, MouseDownEvent, Pixels, Point, ScrollEvent, Window, px};

use crate::boundaries::{floor_char, paragraph_end, paragraph_start, word_at};
use crate::events::SelectionDirection;
use crate::style::TextFieldKind;
use crate::text_field::TextField;

/// The unit a drag selects by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Granularity {
    Character,
    Word,
    Line,
}

/// A drag selecting text.
#[derive(Clone, Debug)]
pub(crate) struct Drag {
    granularity: Granularity,
    // What the press selected, which the drag keeps selected.
    origin: Range<usize>,
}

impl TextField {
    pub(crate) fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.default_prevented() {
            return;
        }
        // A disabled field takes no focus, and its text no selection.
        window.prevent_default();
        if self.disabled {
            return;
        }
        cx.stop_propagation();
        window.request_virtual_keyboard();
        self.commit_composition(window, cx);
        let focusing = !self.focus_handle.is_focused(window);
        if focusing {
            self.pointer_focus = true;
            window.focus(&self.focus_handle, cx);
        }
        self.goal_x = None;
        self.history.close();
        if focusing && self.select_on_focus {
            let len = self.value.len();
            self.set_selection_quiet(0, len, SelectionDirection::None);
            self.drag = None;
            self.changed(window, cx);
            return;
        }
        let Some((offset, upstream)) = self.hit(event.position) else {
            return;
        };
        match event.click_count {
            0 | 1 => {
                // A Shift press after a double or triple press extends by its
                // unit, while what it selected is still the selection.
                let unit = self
                    .unit
                    .clone()
                    .filter(|(_, selected)| *selected == self.selection())
                    .map(|(drag, _)| drag);
                if let Some(drag) = unit.filter(|_| event.modifiers.shift) {
                    self.extend_by_unit(&drag, event.position);
                    self.drag = Some(drag);
                    self.changed(window, cx);
                    return;
                }
                self.unit = None;
                if event.modifiers.shift {
                    let anchor = self.anchor();
                    self.select_to(anchor, offset, upstream);
                } else {
                    self.place(offset, upstream);
                }
                self.drag = Some(Drag {
                    granularity: Granularity::Character,
                    origin: self.anchor()..self.anchor(),
                });
            }
            2 => {
                let word = self.word_under(event.position);
                self.set_selection_quiet(word.start, word.end, SelectionDirection::None);
                self.upstream = false;
                let drag = Drag {
                    granularity: Granularity::Word,
                    origin: word,
                };
                self.unit = Some((drag.clone(), self.selection()));
                self.drag = Some(drag);
            }
            _ => {
                let line = self.line_under(event.position);
                self.set_selection_quiet(line.start, line.end, SelectionDirection::None);
                self.upstream = false;
                let drag = Drag {
                    granularity: Granularity::Line,
                    origin: line,
                };
                self.unit = Some((drag.clone(), self.selection()));
                self.drag = Some(drag);
            }
        }
        self.changed(window, cx);
    }

    /// Extends the selection a drag makes to a window position.
    pub(crate) fn drag_to(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.drag.clone() else {
            return;
        };
        let Some((offset, upstream)) = self.hit(position) else {
            return;
        };
        match drag.granularity {
            Granularity::Character => self.select_to(drag.origin.start, offset, upstream),
            Granularity::Word | Granularity::Line => {
                self.extend_by_unit(&drag, position);
                self.unit = Some((drag, self.selection()));
            }
        }
        self.changed(window, cx);
    }

    // Extends what a double or triple press selected to the word or line at
    // a position, keeping it selected whichever way the pointer goes.
    fn extend_by_unit(&mut self, drag: &Drag, position: Point<Pixels>) {
        let unit = if drag.granularity == Granularity::Word {
            self.word_under(position)
        } else {
            self.line_under(position)
        };
        if unit.start < drag.origin.start {
            self.select_to(drag.origin.end, unit.start, false);
        } else {
            self.select_to(drag.origin.start, unit.end.max(drag.origin.end), false);
        }
    }

    pub(crate) fn end_drag(&mut self) {
        self.drag = None;
    }

    pub(crate) fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    // The word, spaces or punctuation under a position, as value offsets.
    fn word_under(&self, position: Point<Pixels>) -> Range<usize> {
        let Some(geometry) = &self.geometry else {
            return 0..0;
        };
        if self.kind == TextFieldKind::Password && !self.multi_line {
            // A password is one word to a double press.
            return 0..self.value.len();
        }
        let shown = geometry.layout.char_at(geometry.local(position));
        let offset = floor_char(&self.value, geometry.display.to_value(shown));
        word_at(&self.value, offset)
    }

    // The paragraph under a position, or all of a single-line field.
    fn line_under(&self, position: Point<Pixels>) -> Range<usize> {
        if !self.multi_line {
            return 0..self.value.len();
        }
        let Some((offset, _)) = self.hit(position) else {
            return 0..self.value.len();
        };
        paragraph_start(&self.value, offset)..paragraph_end(&self.value, offset)
    }

    pub(crate) fn scroll_wheel(
        &mut self,
        event: &ScrollEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(geometry) = &self.geometry else {
            return;
        };
        let layout = &geometry.layout;
        let delta = event.delta.pixel_delta(layout.line_height);
        let area = geometry.bounds.size;
        let mut scroll = self.scroll;
        if self.multi_line {
            let max = (layout.content_height() - area.height).max(px(0.));
            scroll.y = (scroll.y - delta.y).clamp(px(0.), max);
        } else {
            let max = (layout.content_width() + self.style.caret_width - area.width).max(px(0.));
            scroll.x = (scroll.x - delta.x).clamp(px(0.), max);
        }
        if scroll != self.scroll {
            self.scroll = scroll;
            cx.stop_propagation();
            cx.notify();
        }
    }

    /// A middle press on Linux pastes the primary selection where it lands.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub(crate) fn paste_primary(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.default_prevented() || !self.is_editable() {
            return;
        }
        cx.stop_propagation();
        let Some(text) = cx.read_from_primary().and_then(|item| item.text()) else {
            return;
        };
        if !self.focus_handle.is_focused(window) {
            self.pointer_focus = true;
            window.focus(&self.focus_handle, cx);
        }
        if let Some((offset, upstream)) = self.hit(event.position) {
            self.place(offset, upstream);
        }
        self.paste_text(&text, ui_sdk_kernel::ChangeSource::Mouse, window, cx);
    }
}
