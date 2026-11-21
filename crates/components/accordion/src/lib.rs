//! Port of `components/accordion`: a stack of items that each open a section
//! of content, one at a time unless `multiple` is set.
//!
//! `accordion.ts` → [`accordion`](mod@accordion), `index.ts` → these
//! re-exports.
//!
//! An app makes an [`Accordion`] with `cx.new(|cx| Accordion::new(options,
//! window, cx))` and renders its parts: [`Accordion::root`], then for each
//! item [`Accordion::item`] around [`Accordion::header`] with
//! [`Accordion::trigger`] inside it, and [`Accordion::panel`], which is
//! `None` while the panel is out of the page. Each panel is
//! `ui-sdk-disclosure`'s `Panel`: its part's state carries its presence
//! phases and the height it animates through, which the app gives it,
//! clipping its overflow while it moves. `panel_timing` is the transition the
//! original's CSS declares.
//!
//! Tab moves between triggers, and Enter or Space opens and closes their
//! panels. Arrow keys, Home and End leave focus where it is, as in the
//! original: the APG took them out of the pattern, and a screen reader's
//! heading navigation reaches each trigger.
//!
//! # Dropped, and why
//!
//! - `accordion-generic.ts`: TypeScript's inference of the item value type
//!   for renderers. Values are `SharedString`.
//! - `createAccordionController`, `AccordionController` and the `controller`
//!   option: the app holds the entity, whose methods are the controller's.
//! - `AccordionHeaderProps`, which is empty: the header takes no props.
//! - An item without a `value` getting a generated id: a part has no identity
//!   of its own across renders to make one from, so every item passes its
//!   value.
//! - `triggerKeydown`: gpui clicks a focused part on Enter and Space itself.
//!   `bindTriggerElement`'s role and tab stop are the trigger part's
//!   `aria_role` and `focus_mode`.
//! - Find in page and `hidden="until-found"`: gpui has neither. A panel with
//!   `hidden_until_found` stays in the page, hidden, as `keep_mounted` keeps
//!   it, and only a call that passes `Reason::FindInPage` opens an item at
//!   once.
//! - The root's `MutationObserver`, `syncItems`, `queueSync` and the item and
//!   panel registries: items register as they render, which is page order,
//!   and the kernel knows which panels are in the page. The trigger's
//!   `aria-controls` always names its panel, which the kernel leaves out
//!   while it isn't rendered.
//! - `selects` and `live`: a change renders the accordion's parts again.
//! - `data-*` attributes and `--accordion-panel-height` and `-width`: part
//!   state.
//!
//! # Deviations
//!
//! - The header is a heading, level 3, as the ready-made accordion's `h3`:
//!   in gpui every part is a div, so the role comes from the part.
//! - `set_options` and `set_controlled_value` take the window: a closing
//!   `value` starts its panels' exits in the same pass, which runs on
//!   animation frames.
//! - A hidden panel is `display: none`, as the `hidden` attribute makes it,
//!   so what's inside it isn't drawn: its controls leave the form's page
//!   order until it shows, where a hidden native input still submits.

pub mod accordion;

pub use accordion::{
    Accordion, AccordionChangeOptions, AccordionHeaderState, AccordionItemProps,
    AccordionItemState, AccordionOptions, AccordionPanelProps, AccordionPanelState,
    AccordionState, AccordionTriggerProps, AccordionTriggerState, AccordionValueChangeReason,
    OpenChangeCompleteCallback,
};
