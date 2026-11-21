//! Port of `components/button`: a button that runs an action, shows that
//! it's working, and stays reachable when disabled.
//!
//! # TypeScript file → Rust module
//!
//! `button.ts` → [`button`].
//!
//! # In gpui
//!
//! - The press's action is the `action` option, which the original takes
//!   from adapters (`ButtonInternalOptions.action`) and the React button as
//!   `onClick`. It gets the event and how the button was pressed, and says
//!   with its [`ActionOutcome`](ui_sdk_kernel::ActionOutcome) whether its
//!   work goes on, which makes the button pending.
//! - What the part renders as is the `element` option, which the original
//!   reads off the element once it binds: gpui renders every part as a div.
//!   A native button, the default, takes part in its form: a submit button
//!   submits it through `activate_button` after a press nothing canceled.
//! - A disabled fieldset around the button disables it through its
//!   `ElementInternals`. A field never takes a native button for its
//!   control, as the original's control selector leaves buttons out, so
//!   it doesn't wire or disable one.
//!
//! # Dropped, and why
//!
//! - `buttonKeydown` and `buttonKeyup`: gpui turns Enter and Space on a
//!   focused part into a keyboard click itself (Enter on release too), and
//!   the page has nothing for Space to scroll. A link ignores the click
//!   Space makes, since links answer Enter alone.
//! - The `"unknown"` element and the attributes both a native button and
//!   anything else get until the part binds: there's no server render.
//! - `event.preventDefault()` on a refused press: a refused press simply
//!   doesn't go on to the form, which is what it kept from happening.

pub mod button;

pub use button::{Button, ButtonAction, ButtonActionDetails, ButtonOptions, ButtonState};
pub use ui_sdk_forms::ButtonType;
pub use ui_sdk_platform::ButtonElement;
