//! The field as `ui-sdk-text`'s editor helpers take it, what an `<input>`
//! or a `<textarea>` is to the original's `textFieldAdapter`, input history
//! and trigger menu: [`TextFieldHandle`], which implements
//! [`ui_sdk_text::TextField`].
//!
//! Its listeners hear what happens as the DOM's listeners do: `beforeinput`
//! before an edit applies, `input` after it, and composition as it starts
//! and ends. They run inside the field's update, so the handle reads a copy
//! of what they ask about, which the field keeps current, and an edit one of
//! them asks for waits until the update is over.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{App, Context, Entity, FocusHandle, SharedString, Subscription, WeakEntity, Window};
use ui_sdk_text as text;

use crate::events::SelectionDirection;
use crate::style::TextFieldKind;
use crate::text_field::{Geometry, TextField};

/// What the handle reads, as the field last left it.
#[derive(Default)]
pub(crate) struct Mirror {
    pub value: SharedString,
    pub selection: Range<usize>,
    pub kind: text::TextFieldKind,
    pub read_only: bool,
    pub expanded: bool,
    pub geometry: Option<Geometry>,
    listeners: Vec<(usize, text::TextFieldListener)>,
    next_listener: usize,
    // While the field tells its listeners something, edits they ask for wait
    // for its update to end.
    telling: usize,
}

// The kind the text crate reads, which decides what arrows do.
fn kind_of(field: &TextField) -> text::TextFieldKind {
    if field.multi_line {
        return text::TextFieldKind::TextArea;
    }
    match field.kind {
        TextFieldKind::Text => text::TextFieldKind::Text,
        TextFieldKind::Search => text::TextFieldKind::Search,
        TextFieldKind::Email => text::TextFieldKind::Email,
        TextFieldKind::Tel => text::TextFieldKind::Tel,
        TextFieldKind::Url => text::TextFieldKind::Url,
        TextFieldKind::Number => text::TextFieldKind::Number,
        TextFieldKind::Password => text::TextFieldKind::Password,
    }
}

/// A text field as `ui-sdk-text` takes one. Make it with
/// [`TextField::handle`]; it doesn't keep the field alive. A field's own
/// hooks already hold the field, and edit it through that instead.
#[derive(Clone)]
pub struct TextFieldHandle {
    field: WeakEntity<TextField>,
    focus_handle: FocusHandle,
    mirror: Rc<RefCell<Mirror>>,
}

impl TextFieldHandle {
    pub(crate) fn new(
        field: WeakEntity<TextField>,
        focus_handle: FocusHandle,
        mirror: Rc<RefCell<Mirror>>,
    ) -> Self {
        Self {
            field,
            focus_handle,
            mirror,
        }
    }

    /// The field, while it's alive.
    pub fn entity(&self) -> Option<Entity<TextField>> {
        self.field.upgrade()
    }

    // Runs an edit on the field now, or once its update is over when one of
    // its listeners asked for it.
    fn edit(
        &self,
        window: &mut Window,
        cx: &mut App,
        edit: impl FnOnce(&mut TextField, &mut Window, &mut Context<TextField>) + 'static,
    ) {
        let field = self.field.clone();
        if self.mirror.borrow().telling > 0 {
            window.defer(cx, move |window, cx| {
                field.update(cx, |field, cx| edit(field, window, cx)).ok();
            });
        } else {
            field.update(cx, |field, cx| edit(field, window, cx)).ok();
        }
    }
}

impl text::TextField for TextFieldHandle {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn value(&self, _cx: &App) -> SharedString {
        self.mirror.borrow().value.clone()
    }

    fn selection_range(&self, _cx: &App) -> text::TextRange {
        let selection = self.mirror.borrow().selection.clone();
        text::TextRange::new(selection.start, selection.end)
    }

    fn set_selection_range(&self, range: text::TextRange, window: &mut Window, cx: &mut App) {
        self.edit(window, cx, move |field, window, cx| {
            field.set_selection_range(range.start, range.end, SelectionDirection::None, window, cx);
        });
    }

    fn replace_range(&self, range: text::TextRange, text: &str, window: &mut Window, cx: &mut App) {
        let text = text.to_string();
        self.edit(window, cx, move |field, window, cx| {
            field.replace_range(range.range(), &text, window, cx);
        });
    }

    fn caret_rect(&self, offset: usize, _window: &Window, _cx: &App) -> Option<text::TextRect> {
        let mirror = self.mirror.borrow();
        let offset = offset.min(mirror.value.len());
        mirror.geometry.as_ref().map(|geometry| geometry.caret_bounds(offset))
    }

    fn kind(&self, _cx: &App) -> text::TextFieldKind {
        self.mirror.borrow().kind
    }

    fn read_only(&self, _cx: &App) -> bool {
        self.mirror.borrow().read_only
    }

    fn expanded(&self, _cx: &App) -> bool {
        self.mirror.borrow().expanded
    }

    fn submit(&self, window: &mut Window, cx: &mut App) {
        self.edit(window, cx, |field, window, cx| field.submit(window, cx));
    }

    fn subscribe(&self, listener: text::TextFieldListener, _cx: &mut App) -> Subscription {
        let id = {
            let mut mirror = self.mirror.borrow_mut();
            let id = mirror.next_listener;
            mirror.next_listener += 1;
            mirror.listeners.push((id, listener));
            id
        };
        let mirror = Rc::downgrade(&self.mirror);
        Subscription::new(move || {
            if let Some(mirror) = mirror.upgrade() {
                mirror.borrow_mut().listeners.retain(|(listener, _)| *listener != id);
            }
        })
    }
}

impl TextField {
    /// The field as `ui-sdk-text`'s editor helpers take it, like
    /// `text_field_adapter` and input history.
    pub fn handle(&self) -> TextFieldHandle {
        TextFieldHandle::new(
            self.this.clone(),
            self.focus_handle.clone(),
            self.mirror.clone(),
        )
    }

    /// Keeps what the handle reads current.
    pub(crate) fn sync_mirror(&self) {
        let mut mirror = self.mirror.borrow_mut();
        mirror.value = self.value.clone();
        mirror.selection = self.start..self.end;
        mirror.kind = kind_of(self);
        mirror.read_only = self.read_only;
        mirror.expanded = self.expanded;
        mirror.geometry = self.geometry.clone();
    }

    /// Tells the handle's listeners what happened.
    pub(crate) fn tell(
        &mut self,
        event: text::TextFieldEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let listeners: Vec<text::TextFieldListener> = {
            let mut mirror = self.mirror.borrow_mut();
            if mirror.listeners.is_empty() {
                return;
            }
            mirror.telling += 1;
            mirror.listeners.iter().map(|(_, listener)| listener.clone()).collect()
        };
        self.sync_mirror();
        for listener in listeners {
            listener(&event, window, cx);
        }
        self.mirror.borrow_mut().telling -= 1;
    }
}
