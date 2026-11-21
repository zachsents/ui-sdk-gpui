//! Port of `components/tooltip`: names a control or adds a short hint when
//! the pointer rests on it or keyboard focus reaches it.
//!
//! `Tooltip` is built on `ui_sdk_popups`' `create_popup` with
//! `tooltip_preset`: create it with `cx.new(|cx| Tooltip::new(options,
//! window, cx))`, render `Tooltip::trigger` and `Tooltip::content`, which is
//! always in the window, hidden while closed, and inside it `Tooltip::arrow`
//! and `Tooltip::shortcut`. Tooltips group without a provider: while one is
//! open, or was a moment ago, the next opens at once.
//!
//! # TypeScript file → Rust module
//!
//! `tooltip` → [`tooltip`](mod@tooltip).
//!
//! # Deviations and drops
//!
//! - The `text` option is new. A closed tooltip's content may be missing
//!   from the accessibility tree, and gpui can't read the text an element
//!   renders back, so the trigger is named (`aria_label`) or described
//!   (`aria_description`) by this text, beside the relation to the content,
//!   and the content carries it as its label (`labelContent`). A press that
//!   opens a toggleable tooltip announces it (`textOf(content)`).
//! - Whether the trigger has a name of its own (`hasOwnName`, the state's
//!   `unnamed`) is decided as gpui builds its accessibility node, once its
//!   children are in: an `aria_label`, a labelled-by relation, or anything
//!   inside it in the tree, like `text!(..)`. The original reads the
//!   element's attributes, text and images. That makes the trigger a
//!   [`TooltipTrigger`] around its part.
//! - The trigger is a button (`type="button"`); the original's render prop
//!   that makes another component's trigger the tooltip's too has no
//!   counterpart, since an element is one part.
//! - `asleep`, which waits for the trigger before building the api: a
//!   framework's rendering. The trigger and content bind as they render.
//! - The trigger's development checks: `checkName` and `nameFrom` (another
//!   component's trigger on the same element), and the warning for a
//!   natively disabled button, which the tooltip's own part never is.
//! - `instant` is read from the warm group as the content renders, rather
//!   than kept in the state as it opens.
//! - `container` (`PortalProps`), and the trigger's `controller` prop: the
//!   content renders above the window through `deferred`, and the app holds
//!   the entity, so it renders `Tooltip::trigger` anywhere. The tooltip's
//!   own `controller` option stays.
//! - `type` is the option `kind`, a [`TooltipType`].
//! - Payloads are typed: `Tooltip<P>`, `P` being the controller's `Payload`.

pub mod tooltip;

pub use tooltip::{
    Tooltip, TooltipArrowState, TooltipContentProps, TooltipContentState, TooltipController,
    TooltipOpenChangeReason, TooltipOptions, TooltipPayloadChangeReason, TooltipShortcutProps,
    TooltipShortcutState, TooltipTrigger, TooltipTriggerElement, TooltipTriggerProps, TooltipTriggerState,
    TooltipType, create_tooltip_controller,
};
pub use ui_sdk_popups::tooltip_preset;
