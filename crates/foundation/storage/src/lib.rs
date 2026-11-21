//! Port of `foundation/storage`: a value kept under an ID, in local storage,
//! which lasts between runs, or session storage, which lasts while the app
//! runs, and heard from anywhere in the app when it changes.
//!
//! `localStorage` and `sessionStorage` themselves are [`Kind`]'s areas, which
//! `area.rs` keeps for the whole app in a gpui global. Local storage persists
//! to a file the app names with [`init`] as it starts.
//!
//! Dropped from the original:
//! - `createCookieStorage`, `CookieStorageOptions` and `readsOnServer`: a
//!   cookie is the storage a server reads from a request, and a gpui app has
//!   neither.
//! - `script`, `StorageScriptOptions`, `readSaved` and `literal`: they write
//!   the inline script that puts a saved value on a `data-*` attribute before
//!   hydration. Nothing here renders on a server or hydrates, and a read is
//!   synchronous, so the first render already has the saved value; parts
//!   style from state rather than `data-*` attributes.
//! - The `isBrowser` checks: there's always an app.
//! - Hearing other tabs through `storage` events: every window shares the
//!   app's storage and hears its writes directly. Another process of the app
//!   writing the same file isn't heard, and the last one to write wins.
//! - `canWrite`'s probe: local storage takes writes once `init` made its
//!   directory. Its file is written in the background, so a write that fails
//!   there is logged rather than returned.

pub mod area;
pub mod storage;

pub use area::{Kind, init};
pub use storage::{ValueStorage, create_local_storage, create_session_storage};
