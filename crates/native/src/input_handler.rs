//! The field's side of the platform's text input: typed text, input
//! methods composing (the marked text, its range, committing it or giving it
//! up), and the geometry they place their candidate windows with. The
//! platform counts in UTF-16; the field counts UTF-8 bytes.

use std::ops::Range;

use gpui::{
    Bounds, ClipboardItem, Context, EntityInputHandler, Pixels, Point, SharedString,
    TextInputConfiguration, UTF16Selection, Window, point,
};
use ui_sdk_kernel::ChangeSource;

use crate::boundaries::{
    offset_to_utf16, offsets_to_utf16, text_diff, utf16_range_to_offsets, utf16_to_offset,
};
use crate::editing::Change;
use crate::events::{BeforeEdit, Edit, InputType, SelectionDirection};
use crate::history::{Group, Selection};
use crate::style::TextFieldKind;
use crate::text_field::TextField;

/// An input method composing: the marked text, and what the field held
/// before it began, which committing records as one step and refusing puts back.
pub(crate) struct Composition {
    pub range: Range<usize>,
    before: SharedString,
    before_selection: Selection,
}

impl TextField {
    // Typed text, or an input method's commit.
    fn platform_insert(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_editable() {
            return;
        }
        let text = without_lone_control(text);
        if let Some(composition) = self.composition.take() {
            let range = range_utf16
                .map(|range| self.clamp_range(utf16_range_to_offsets(&self.value, &range)))
                .unwrap_or_else(|| composition.range.clone());
            self.finish_composition(composition, range, text.to_string(), window, cx);
            return;
        }
        // A key a listener refused still sends its text.
        if let Some(refused) = self.refused_text.take()
            && refused == text
        {
            return;
        }
        let selection = self.start..self.end;
        let (range, input_type) = match range_utf16 {
            Some(range) => {
                let range = self.clamp_range(utf16_range_to_offsets(&self.value, &range));
                let input_type = if range == selection {
                    InputType::InsertText
                } else {
                    InputType::InsertReplacementText
                };
                (range, input_type)
            }
            None => (selection, InputType::InsertText),
        };
        let group = if input_type == InputType::InsertText {
            Group::Typing
        } else {
            Group::Single
        };
        self.apply(
            Change {
                input_type,
                range,
                text: text.to_string(),
                source: ChangeSource::Keyboard,
                user: true,
                group,
                selection: None,
            },
            window,
            cx,
        );
    }

    // An input method's text while it composes, which replaces the marked
    // text, or the selection when it starts.
    fn platform_compose(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        selected_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_editable() {
            return;
        }
        if self.composition.is_none() {
            self.composition = Some(Composition {
                range: self.start..self.end,
                before: self.value.clone(),
                before_selection: self.selection_tuple(),
            });
            self.history.close();
            ui_sdk_kernel::set_composing(window, true, cx);
            self.fire_composition_start(window, cx);
            if self.composition.is_none() {
                return;
            }
        }
        let marked = self
            .composition
            .as_ref()
            .map(|composition| composition.range.clone())
            .unwrap_or(self.start..self.end);
        let range = range_utf16
            .map(|range| self.clamp_range(utf16_range_to_offsets(&self.value, &range)))
            .unwrap_or(marked);
        let range = self.clamp_range(range);
        // An input method's text can't be refused while it composes.
        let before = BeforeEdit::new(
            InputType::InsertCompositionText,
            range.clone(),
            text.to_string(),
            true,
            false,
            ChangeSource::Keyboard,
        );
        let revision = self.revision;
        self.before_input(&before, window, cx);
        if self.revision != revision || self.composition.is_none() {
            return;
        }
        let mut next = String::with_capacity(self.value.len() + text.len());
        next.push_str(&self.value[..range.start]);
        next.push_str(text);
        next.push_str(&self.value[range.end..]);
        self.replace_value(next.into());
        self.dirty = true;
        self.user_edited = true;
        let inserted = range.start..range.start + text.len();
        let (start, end) = match selected_utf16 {
            Some(selected) => {
                let start = inserted.start + utf16_to_offset(text, selected.start);
                let end = inserted.start + utf16_to_offset(text, selected.end);
                (start.min(inserted.end), end.min(inserted.end))
            }
            None => (inserted.end, inserted.end),
        };
        self.set_selection_quiet(start, end, SelectionDirection::None);
        self.upstream = false;
        self.goal_x = None;
        if text.is_empty() {
            // The input method gave up, leaving what it composed over gone.
            let composition = self.composition.take();
            ui_sdk_kernel::set_composing(window, false, cx);
            self.fire_input(self.edit(InputType::InsertCompositionText, range, String::new()), window, cx);
            if let Some(composition) = composition
                && composition.before != self.value
            {
                self.record_since(&composition);
            }
            self.fire_composition_end(String::new(), false, window, cx);
        } else {
            if let Some(composition) = self.composition.as_mut() {
                composition.range = inserted;
            }
            let mut edit = self.edit(InputType::InsertCompositionText, range, text.to_string());
            edit.composing = true;
            self.fire_input(edit, window, cx);
        }
        self.changed(window, cx);
    }

    // Commits what an input method composed, with `text` over `range`.
    fn finish_composition(
        &mut self,
        composition: Composition,
        range: Range<usize>,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        ui_sdk_kernel::set_composing(window, false, cx);
        let data = self.clean(&text, true);
        let text = self.fit(&data, &range);
        let before = BeforeEdit::new(
            InputType::InsertFromComposition,
            range.clone(),
            data,
            false,
            true,
            ChangeSource::Keyboard,
        )
        .cut(text.clone());
        let revision = self.revision;
        let allowed = self.before_input(&before, window, cx);
        if self.revision != revision {
            self.fire_composition_end(String::new(), false, window, cx);
            return;
        }
        if !allowed {
            // Refused: the text from before composing comes back.
            let (changed, inserted) = text_diff(&self.value, &composition.before);
            self.replace_value(composition.before.clone());
            let (start, end, direction) = composition.before_selection;
            self.set_selection_quiet(start, end, direction);
            self.fire_composition_end(String::new(), false, window, cx);
            if !changed.is_empty() || !inserted.is_empty() {
                self.fire_input(self.edit(InputType::InsertFromComposition, changed, inserted), window, cx);
            }
            self.changed(window, cx);
            return;
        }
        let mut next = String::with_capacity(self.value.len() + text.len());
        next.push_str(&self.value[..range.start]);
        next.push_str(&text);
        next.push_str(&self.value[range.end..]);
        self.replace_value(next.into());
        let caret = range.start + text.len();
        self.set_selection_quiet(caret, caret, SelectionDirection::None);
        self.upstream = false;
        self.goal_x = None;
        self.dirty = true;
        self.user_edited = true;
        self.record_since(&composition);
        self.fire_composition_end(text.clone(), false, window, cx);
        self.fire_input(self.edit(InputType::InsertFromComposition, range, text), window, cx);
        self.changed(window, cx);
    }

    // Records everything a composition changed as one undo step.
    fn record_since(&mut self, composition: &Composition) {
        let (range, inserted) = text_diff(&composition.before, &self.value);
        let removed = composition.before[range.clone()].to_string();
        self.history.record(
            range.start,
            removed,
            inserted,
            composition.before_selection,
            self.selection_tuple(),
            Group::Single,
        );
    }

    fn edit(&self, input_type: InputType, range: Range<usize>, inserted: String) -> Edit {
        Edit {
            input_type,
            range,
            inserted,
            value: self.value.clone(),
            composing: false,
            user: true,
            source: ChangeSource::Keyboard,
        }
    }

    /// Commits what an input method is composing, as it stands: what a press
    /// elsewhere or losing focus does.
    pub(crate) fn commit_composition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(composition) = self.composition.take() {
            let range = self.clamp_range(composition.range.clone());
            let text = self.value[range.clone()].to_string();
            self.finish_composition(composition, range, text, window, cx);
        }
    }

    // Ends a composition the value is about to be written over. Code made
    // the write, so the report comes once its update is over.
    pub(crate) fn cancel_composition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.composition.take().is_some() {
            ui_sdk_kernel::set_composing(window, false, cx);
            cx.defer_in(window, |field, window, cx| {
                field.fire_composition_end(String::new(), true, window, cx)
            });
        }
    }

    // The window rectangle of a range's start, as wide as the range on its row.
    fn platform_bounds(&self, range: Range<usize>) -> Option<Bounds<Pixels>> {
        let geometry = self.geometry.as_ref()?;
        let start = geometry.display.to_display(range.start);
        let end = geometry.display.to_display(range.end);
        let layout = &geometry.layout;
        let row = layout.row_for(start, false);
        let x0 = layout.x_for(row, start);
        let x1 = if layout.row_for(end, true) == row {
            layout.x_for(row, end)
        } else {
            x0
        };
        let top = layout.line_height * row as f32;
        Some(Bounds::from_corners(
            geometry.window(point(x0, top)),
            geometry.window(point(x1.max(x0), top + layout.line_height)),
        ))
    }
}

// Enter, Tab and Backspace arrive as keys. A platform that also sends their
// control character as text has it dropped here.
fn without_lone_control(text: &str) -> &str {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_control() => "",
        _ => text,
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        // A password's text stays out of the platform's text services.
        if self.kind == TextFieldKind::Password && !self.multi_line {
            return None;
        }
        let range = self.clamp_range(utf16_range_to_offsets(&self.value, &range_utf16));
        adjusted_range.replace(offsets_to_utf16(&self.value, &range));
        Some(self.value[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: offsets_to_utf16(&self.value, &(self.start..self.end)),
            reversed: self.direction == SelectionDirection::Backward,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.composition
            .as_ref()
            .map(|composition| offsets_to_utf16(&self.value, &composition.range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_composition(window, cx);
    }

    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = item.text() {
            self.paste_text(&text, ChangeSource::Keyboard, window, cx);
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.platform_insert(range_utf16, text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.platform_compose(range_utf16, new_text, new_selected_range, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.clamp_range(utf16_range_to_offsets(&self.value, &range_utf16));
        self.platform_bounds(range)
    }

    fn selection_bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Vec<Bounds<Pixels>> {
        let range = self.clamp_range(utf16_range_to_offsets(&self.value, &range_utf16));
        self.range_bounds(range)
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let (offset, _) = self.hit(point)?;
        Some(offset_to_utf16(&self.value, offset))
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = utf16_range_to_offsets(&self.value, &range_utf16);
        self.set_selection_quiet(range.start, range.end, SelectionDirection::None);
        self.upstream = false;
        self.goal_x = None;
        self.history.close();
        self.changed(window, cx);
    }

    fn text_length_utf16(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        Some(offset_to_utf16(&self.value, self.value.len()))
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        self.is_editable()
    }

    fn text_input_configuration(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> TextInputConfiguration {
        if self.kind == TextFieldKind::Password && !self.multi_line {
            return TextInputConfiguration::default();
        }
        self.configuration.clone()
    }
}
