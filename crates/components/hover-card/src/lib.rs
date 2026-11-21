//! Port of `components/hover-card`: a preview of what a link points to,
//! which opens when the pointer or keyboard focus rests on the link. Tab
//! moves from the link into it, and focus leaving closes it.
//!
//! `HoverCard` is built on `ui_sdk_popups`' `create_popup` with
//! `hover_card_preset`: create it with `cx.new(|cx| HoverCard::new(options,
//! window, cx))`, render `HoverCard::trigger` and `HoverCard::content`, and
//! `HoverCard::arrow` inside it. The trigger keeps the role the app gives it,
//! usually a link.
//!
//! # TypeScript file → Rust module
//!
//! `hover-card` → [`hover_card`](mod@hover_card).
//!
//! # Deviations and drops
//!
//! - `container` (`PortalProps`), and the trigger's `controller` prop: the
//!   content renders above the window through `deferred`, and the app holds
//!   the entity, so it renders `HoverCard::trigger` anywhere. The card's own
//!   `controller` option stays.
//! - The content's state is the popups crate's, whose `modal` and
//!   `presentation` stay `False` and `Popover` for a hover card.
//! - Payloads are typed: `HoverCard<P>`, `P` being the controller's
//!   `Payload`.

pub mod hover_card;

pub use hover_card::{
    HoverCard, HoverCardArrowState, HoverCardContentProps, HoverCardContentState, HoverCardController,
    HoverCardOpenChangeReason, HoverCardOptions, HoverCardPayloadChangeReason, HoverCardTriggerProps,
    HoverCardTriggerState, create_hover_card_controller,
};
pub use ui_sdk_popups::hover_card_preset;
