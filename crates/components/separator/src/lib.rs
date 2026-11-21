//! Port of `components/separator`: a line between groups of content.
//!
//! # TypeScript file → Rust module
//!
//! `separator.ts` → [`separator`].
//!
//! # Dropped, and why
//!
//! - `settings`' reading of `orientation` as written, where anything but
//!   `"vertical"` is the default: a custom element passes its attributes as
//!   text. Here the orientation is [`SeparatorOrientation`], so nothing else
//!   arrives.
//! - `roles: ["none", "separator"]`, the roles the slot may take, which
//!   renderers and type checks read: the root declares the one it takes with
//!   `aria_role`.
//! - `SeparatorPrimitive`, the definition's type: the component is the
//!   [`Separator`] struct.

pub mod separator;

pub use separator::{Separator, SeparatorOptions, SeparatorOrientation, SeparatorState};
