//! Port of `components/token-input`: TokenInput, a text field that holds
//! atomic tokens alongside its text, like the people mentioned in a message
//! or the files attached to a prompt. Each token moves, deletes and copies as
//! one piece, and one codec turns the value into text at every boundary: the
//! clipboard, forms and drafts.
//!
//! `token-input.ts` → [`token_input`](mod@token_input), `token-input-model.ts`
//! → [`token_input_model`], `token-input-view.ts` → [`token_input_view`],
//! `token-input-editing.ts` → [`token_input_editing`], `token-input-api.ts`
//! → [`token_input_api`], `token-input-parts.ts` → [`token_input_parts`],
//! `index.ts` → these re-exports.
//!
//! An app makes a [`TokenInput`] with `cx.new(|cx| TokenInput::new(options,
//! window, cx))` and renders [`TokenInput::editor`], styled like any part,
//! with `render_token` giving each token its content, usually a
//! [`TokenInput::token`] part with a [`TokenInput::token_remove`] inside. A
//! value is a list of [`Segment`]s, text and [`Token`]s. Backspace and
//! Delete treat a token as `token_deletion` says, the arrows step over it in
//! one press, the caret never stops inside one, and a selection that touches
//! one covers all of it. Enter submits (`on_submit`, or the form the input
//! is in) as `submit_shortcut` says; otherwise it starts a new line. Typing
//! is grouped into undo steps; pastes, cuts and token changes are steps of
//! their own. A trigger menu reads and writes the input through
//! [`TokenInput::text_editor`], a `ui_sdk_text::TextEditorAdapter`.
//!
//! # In gpui
//!
//! - The editor is an element of this crate, since gpui has no
//!   contenteditable: it lays the value out (text that wraps as `pre-wrap`
//!   and `overflow-wrap: anywhere` do, at UAX #14 break opportunities, with
//!   each token's host an atomic box in its row), paints the text, the
//!   selection, the caret and an input method's marked text, scrolls to keep
//!   the caret in view past the part's `max_h`, and handles the platform's
//!   text input as gpui's `EntityInputHandler`. Its keys, moves and mouse
//!   selection are the native crate's text field's, with a token one
//!   character wide and a word of its own; a press on a token selects it, as
//!   `user-select: all` does.
//! - Offsets are UTF-8 byte offsets into the value's text with each token as
//!   U+FFFC, as the text crate's editor offsets are, so a token spans
//!   [`TOKEN_LEN`] offsets rather than one, and an edit that covers part of a
//!   character covers all of it. Deleting a character deletes one code point,
//!   as the original does.
//! - Typing applies to the value as it comes from the platform, where the
//!   original lets the browser type and reads it back: the steps, reasons and
//!   input types are the same. An input method's composition shows in the
//!   editor and joins the value when it commits, as one undo step, as the
//!   original reads it back at `compositionend`.
//! - The clipboard carries the codec text in the item's metadata, where the
//!   original puts it in `text/html` as `data-ui-tokens`, so a paste into a
//!   token input gets the tokens back, whatever `copy` writes as text.
//! - Each token is one character to assistive technology, which reads its
//!   label and description, and its host's content is hidden from it, as the
//!   original's host carries its visually hidden description. The editor is
//!   a multi-line text input with text runs, like the native field.
//! - A host shows its token's label until `render_token` renders its
//!   content; `style_host` styles hosts from their token's state, what the
//!   original's `token-input-host` attributes are for.
//! - The editor reports the codec text to its form through its internals,
//!   under `name`, with a missing value while required and empty. A Field or
//!   a `Label` names it through them.
//! - `revert` puts back the trigger a token's data carries, `data.trigger`:
//!   the text crate's `TokenTrigger`, which a trigger menu's tokens carry, or
//!   data of the app's own as a JSON object (`ui_sdk_kernel::AgentValue`)
//!   with a `trigger` key, or the trigger itself (a `SharedString` or
//!   `String`), since a token's data is `Rc<dyn Any>` here.
//! - The value and the edit's details reach `on_value_change` as a
//!   [`TokenInputValueChangeDetails`], which derefs to the kernel's
//!   `ChangeDetails`. A controlled value passed back equal keeps its
//!   normalized form, where the original keys the normalized form by array.
//!   A value reads as the original's segments in `observe` and history
//!   ([`Segment::to_data`]), which puts it back exactly.
//! - `set_options` and `set_controlled_value` take the window, since a new
//!   value resets the selection the editor shows.
//! - `isDisabled`, which the original imports through the lists crate's
//!   parts, is the forms crate's, which the lists crate re-exports, so the
//!   crate doesn't depend on lists.
//!
//! # Dropped, and why
//!
//! - `createTokenInputController`, `TokenInputController` and the
//!   `controller` option: the app holds the entity, whose methods are the
//!   controller's; its `editor` is [`TokenInput::text_editor`].
//! - The `root` slot (web components' form participation) and the
//!   `form-input` slot (a hidden textarea for React and Vue): the editor's
//!   internals report to forms.
//! - `restore` and the `Restore` reason's form state: gpui restores no forms
//!   after a reload.
//! - `renderStaticHtml`, `typedBeforeBind`, the server hosts and
//!   `data-token-index`: server rendering and text typed before hydration.
//! - The DOM's shape: zero-width anchors around hosts, the trailing `<br>`,
//!   reading back what the browser typed (`read`, `reconcile`), `offsetAt` of
//!   DOM points, `getTargetRanges` and `crossToken`, which exists because of
//!   the anchors; a token is one character to the caret here.
//! - `hosts`, `hostElement`, `setCustom`, the `hosts` and `custom` state and
//!   `measured`: how frameworks render into hosts the core owns.
//! - `editorStyles` and `writeStyle`: CSS for `pre-wrap` and `user-select`,
//!   which the editor's layout does.
//! - `live`: attributes written without a render. Parts render from the
//!   entity's state.
//! - Dragging text and tokens within, into or out of the editor, and the
//!   `Drop` reason: gpui's text has no drag sources, and the native field
//!   drops them too.
//! - Formatting shortcuts and `historyUndo` from the browser: there is no
//!   formatting to refuse; the Edit menu reaches the editor as the native
//!   crate's `Undo`, `Redo`, `Copy`, `Cut`, `Paste` and `SelectAll` actions.
//! - Right-to-left caret moves: gpui lays text out left to right, as the
//!   native field does.

pub mod token_input;
pub mod token_input_api;
pub mod token_input_editing;
pub mod token_input_model;
pub mod token_input_parts;
pub mod token_input_view;

pub use token_input::{
    FocusAt, HostStyle, RenderToken, TokenDeletion, TokenDescription, TokenInput,
    TokenInputChangeOptions, TokenInputCopy, TokenInputEditorState, TokenInputOptions,
    TokenInputSubmit, TokenInputSubmitDetails, TokenInputSubmitShortcut, TokenInputTokenRemoveState,
    TokenInputTokenState, TokenInputValueChange, TokenInputValueChangeDetails,
    TokenInputValueChangeReason,
};
pub use token_input_api::{HistoryEntry, InsertAt, InsertOptions, TokenInputContent, TokenInputEditorAdapter};
pub use token_input_model::{
    Segment, TOKEN_CHAR, TOKEN_LEN, TokenCodec, TokenEscape, TokenFormat, TokenInputValue, TokenParse,
    default_codec, from_text, normalize_value, offset_text, to_label, to_text, value_length,
};
pub use token_input_parts::TokenInputEditor;
pub use ui_sdk_kernel::UndoStackOptions;
pub use ui_sdk_text::{TextEditorAdapter, TextRange, Token, TokenInsert, TokenStatus, TokenTrigger, TokenUpdate};
