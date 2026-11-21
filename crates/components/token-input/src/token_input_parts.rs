//! Port of `components/token-input/token-input-parts.ts`: TokenInput's parts
//! and what they carry.
//!
//! The editor is a [`TokenInputEditor`], a part with the element that lays
//! the value out inside it, since the input owns what's inside the editor:
//! the app styles it like any part and says how each token's host renders.
//! Tokens and their remove parts are parts the app renders inside a host,
//! from [`TokenInputEditor::render_token`]. The input takes part in forms
//! through its internals, which the editor's element carries, with the codec
//! text under its name.

use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    AccessibleAction, AnyElement, App, Div, Entity, Interactivity, KeyContext, MouseButton, MouseDownEvent,
    MouseUpEvent, ScrollEvent, SharedString, Stateful, StyleRefinement, Window,
};
use ui_sdk_focus::press_keeps_focus;
use ui_sdk_kernel::part::{FocusMode, PartElement};
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{Aria, Part, Reason};
use ui_sdk_native::{Copy, Cut, KEY_CONTEXT, Paste, Redo, SelectAll, Undo};
use ui_sdk_text::Token;

use crate::token_input::{
    HostStyle, RenderToken, TokenInput, TokenInputEditorState, TokenInputTokenRemoveState,
    TokenInputTokenState,
};
use crate::token_input_api::Edit;
use crate::token_input_model::{Segment, tokens_of, value_length};
use crate::token_input_view::{TokenEditorElement, build};

// The text a value reads as: each token as its label and description.
fn read_text(input: &TokenInput) -> SharedString {
    let mut text = String::new();
    for segment in &input.committed {
        match segment {
            Segment::Text(part) => text.push_str(part),
            Segment::Token(token) => {
                text.push_str(&token.label);
                text.push(' ');
                text.push_str(&input.describe(token));
            }
        }
    }
    text.into()
}

// The key context the editor sets, a multi-line text field's, so key
// bindings and shortcuts can tell typing goes on.
fn key_context() -> KeyContext {
    let mut context = KeyContext::default();
    context.add(KEY_CONTEXT);
    context.add("multiline");
    context
}

impl TokenInput {
    pub(crate) fn editor_state(&self) -> TokenInputEditorState {
        TokenInputEditorState {
            empty: value_length(&self.committed) == 0 && self.composition.is_none(),
            disabled: self.is_disabled(),
            read_only: self.settings.read_only,
        }
    }

    /// The `editor` part: the editable text, a multi-line textbox. The input
    /// owns what's inside it: its text, the selection, the caret and each
    /// token's host, which [`TokenInputEditor::render_token`] fills. Style
    /// it like any part; its text draws with the part's text style and
    /// `field_style`. Name it with `aria_label`, a forms `Label` or a Field.
    pub fn editor(this: &Entity<Self>, cx: &App) -> TokenInputEditor {
        let input = this.read(cx);
        input.sync_form();
        let state = input.editor_state();
        let weak = this.downgrade();
        let snapshot = input.a11y.clone();
        let internals = input.internals.clone();
        let tracked = internals.clone();
        let part = Part::new(&input.instance, "editor", None, state.clone())
            .map_el(move |el| tracked.track(el))
            .aria_role(Role::Textbox)
            .focus_mode(FocusMode::Native, Some(&input.focus_handle), state.disabled)
            .aria(
                Aria::new()
                    .disabled(state.disabled)
                    .read_only(state.read_only)
                    .required(input.settings.required),
            )
            .aria(internals.aria())
            .key_context(key_context())
            .aria_value(read_text(input))
            .a11y_synthetic_children(move |builder| build(&snapshot, builder))
            .when(!state.disabled, |part| part.cursor_text())
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, {
                let weak = weak.clone();
                move |event: &MouseDownEvent, window, cx| {
                    weak.update(cx, |input, cx| input.mouse_down(event, window, cx)).ok();
                }
            })
            .on_mouse_up(MouseButton::Left, {
                let weak = weak.clone();
                move |_: &MouseUpEvent, _, cx| {
                    weak.update(cx, |input, _| input.end_drag()).ok();
                }
            })
            .on_mouse_up_out(MouseButton::Left, {
                let weak = weak.clone();
                move |_: &MouseUpEvent, _, cx| {
                    weak.update(cx, |input, _| input.end_drag()).ok();
                }
            })
            .on_scroll({
                let weak = weak.clone();
                move |event: &ScrollEvent, window, cx| {
                    weak.update(cx, |input, cx| input.scroll_wheel(event, window, cx)).ok();
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &Copy, _, cx| {
                    weak.update(cx, |input, cx| input.copy(cx)).ok();
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &Cut, window, cx| {
                    weak.update(cx, |input, cx| input.cut(None, window, cx)).ok();
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &Paste, window, cx| {
                    weak.update(cx, |input, cx| input.paste(None, window, cx)).ok();
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &Undo, window, cx| {
                    weak.update(cx, |input, cx| {
                        if input.is_editable() {
                            input.undo_with(None, window, cx);
                        }
                    })
                    .ok();
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &Redo, window, cx| {
                    weak.update(cx, |input, cx| {
                        if input.is_editable() {
                            input.redo_with(None, window, cx);
                        }
                    })
                    .ok();
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &SelectAll, window, cx| {
                    weak.update(cx, |input, cx| input.select_all(window, cx)).ok();
                }
            })
            .on_a11y_action(AccessibleAction::SetValue, {
                let weak = weak.clone();
                move |data, window, cx| {
                    if let Some(gpui::accesskit::ActionData::Value(value)) = data {
                        let value = value.to_string();
                        weak.update(cx, |input, cx| input.a11y_set_value(value, window, cx)).ok();
                    }
                }
            })
            .on_a11y_action(AccessibleAction::ReplaceSelectedText, {
                let weak = weak.clone();
                move |data, window, cx| {
                    if let Some(gpui::accesskit::ActionData::Value(value)) = data {
                        let value = value.to_string();
                        weak.update(cx, |input, cx| input.a11y_replace(value, window, cx)).ok();
                    }
                }
            })
            .on_a11y_action(AccessibleAction::SetTextSelection, {
                let weak = weak.clone();
                move |data, window, cx| {
                    weak.update(cx, |input, cx| input.a11y_select(data, window, cx)).ok();
                }
            });
        TokenInputEditor {
            part,
            input: this.clone(),
            placeholder: None,
            render_token: None,
            style_host: None,
        }
    }

    /// The `token` part: a token's content, which the app renders inside its
    /// host from [`TokenInputEditor::render_token`], with the token's state.
    pub fn token(this: &Entity<Self>, id: &str, cx: &App) -> Part<TokenInputTokenState> {
        let input = this.read(cx);
        let token = tokens_of(&input.committed)
            .into_iter()
            .find(|token| token.id.as_deref() == Some(id))
            .cloned()
            .unwrap_or_else(|| Token::new("", "", ""));
        let state = input.token_state(&token);
        Part::new(&input.instance, "token", Some(id), state)
    }

    /// The `token-remove` part: removes its token when pressed, and keeps
    /// focus and the caret in the editor. For pointers only: the keyboard's
    /// way is Backspace, so it's hidden from assistive technology. Disabled
    /// and read-only inputs mark it, so it can hide or dim.
    pub fn token_remove(this: &Entity<Self>, id: &str, cx: &App) -> Part<TokenInputTokenRemoveState> {
        let input = this.read(cx);
        let state = TokenInputTokenRemoveState {
            disabled: input.is_disabled(),
            read_only: input.settings.read_only,
        };
        let weak = this.downgrade();
        let token = SharedString::from(id.to_string());
        let part = Part::new(&input.instance, "token-remove", Some(id), state)
            .aria_role(Role::Group)
            .aria(Aria::new().hidden(true))
            .on_press(move |event, window, cx| {
                weak.update(cx, |input, cx| {
                    if input.is_editable() {
                        let edit = Edit::new(Reason::RemovePress).event(Some(event));
                        input.remove_token_with(&token, edit, window, cx);
                    }
                })
                .ok();
            });
        press_keeps_focus(part)
    }
}

/// The `editor` part, with the element that shows the value inside it. Style
/// it like any part. Its state is [`TokenInputEditorState`].
pub struct TokenInputEditor {
    part: Part<TokenInputEditorState>,
    input: Entity<TokenInput>,
    placeholder: Option<SharedString>,
    render_token: Option<RenderToken>,
    style_host: Option<HostStyle>,
}

impl TokenInputEditor {
    pub fn state(&self) -> &TokenInputEditorState {
        self.part.state()
    }

    /// Text shown while the input is empty, also its `aria-placeholder`. It
    /// draws in the field style's placeholder color.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        let placeholder = placeholder.into();
        self.part = self.part.aria_placeholder(placeholder.clone());
        self.placeholder = Some(placeholder);
        self
    }

    /// Renders each token's content inside its host, usually a
    /// [`TokenInput::token`] part with a [`TokenInput::token_remove`]
    /// inside. Without it, each token shows its label.
    pub fn render_token(
        mut self,
        render: impl Fn(&Entity<TokenInput>, &Token, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.render_token = Some(Rc::new(render));
        self
    }

    /// Styles each token's host from its token's state, like a chip around
    /// its label when nothing renders its content (the original's
    /// `token-input-host`, with its `data-*` attributes).
    pub fn style_host(
        mut self,
        style: impl Fn(Stateful<Div>, &TokenInputTokenState) -> Stateful<Div> + 'static,
    ) -> Self {
        self.style_host = Some(Rc::new(style));
        self
    }

    /// Styles the part from its state.
    pub fn with(mut self, style: impl FnOnce(Stateful<Div>, &TokenInputEditorState) -> Stateful<Div>) -> Self {
        self.part = self.part.with(style);
        self
    }

    /// Adds ARIA beyond gpui's setters.
    pub fn aria(mut self, aria: Aria) -> Self {
        self.part = self.part.aria(aria);
        self
    }
}

impl Styled for TokenInputEditor {
    fn style(&mut self) -> &mut StyleRefinement {
        self.part.style()
    }
}

impl InteractiveElement for TokenInputEditor {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.part.interactivity()
    }
}

impl StatefulInteractiveElement for TokenInputEditor {}

impl IntoElement for TokenInputEditor {
    type Element = PartElement;

    fn into_element(self) -> Self::Element {
        let element = TokenEditorElement {
            input: self.input,
            placeholder: self.placeholder,
            render_token: self.render_token,
            style_host: self.style_host,
        };
        self.part.child(element).into_element()
    }
}
