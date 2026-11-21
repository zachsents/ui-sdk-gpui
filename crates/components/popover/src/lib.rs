//! Port of `components/popover`: rich content opened from a button, beside
//! it. The content is a dialog, non-modal by default, that its title names
//! and its description describes. It can open on hover, present as a sheet,
//! and open from a trigger's keyboard shortcut.
//!
//! `Popover` is built on `ui_sdk_popups`' `create_popup` with
//! `popover_preset`: create it with `cx.new(|cx| Popover::new(options,
//! window, cx))`, render `Popover::trigger`, `Popover::content` (inside
//! `Popover::viewport` while it presents as a sheet), and inside it
//! `Popover::title`, `Popover::description`, `Popover::close_part` and
//! `Popover::arrow`; `Popover::shortcut` shows a trigger's shortcut.
//!
//! # TypeScript file → Rust module
//!
//! `popover` → [`popover`](mod@popover).
//!
//! # Deviations and drops
//!
//! - `container` (`PortalProps`) on the popover and its content: the content
//!   renders above the window through `deferred`, which replaces portals.
//! - The trigger's `controller` prop, which connects a trigger outside the
//!   popover: the app holds the entity, so it renders `Popover::trigger`
//!   anywhere. The popover's own `controller` option stays, for a handle made
//!   before the popover.
//! - The content's development warning for a form inside a popover rendered
//!   inside another form: nesting forms is an HTML parsing rule, and the
//!   forms crate finds a control's nearest form.
//! - The `close` slot is `Popover::close_part`, since `close` is the api's.
//! - `modal` is [`PopoverModal`], the subset of `Modal` a popover takes.
//! - Payloads are typed: `Popover<P>`, `P` being the controller's `Payload`.
//!   `close`'s return values are text, the original's default `Result`.

pub mod popover;

pub use popover::{
    Popover, PopoverArrowState, PopoverContentProps, PopoverContentState, PopoverController,
    PopoverDismissReason, PopoverFinalFocus, PopoverInitialFocus, PopoverModal, PopoverOpenChangeReason,
    PopoverOptions, PopoverPayloadChangeReason, PopoverShortcutProps, PopoverShortcutState,
    PopoverTriggerProps, PopoverTriggerState, create_popover_controller,
};
pub use ui_sdk_popups::popover_preset;
