//! Port of `page`: what works across a whole page rather than one
//! component: the agent bridge, `observe`, history and undo, shortcuts and
//! overlays.
//!
//! # TypeScript file → Rust module
//!
//! `observe/marks` → [`marks`], `observe/observe` → [`observe`](mod@observe),
//! `observe/registry` → [`registry`], `agent/agent` → [`agent`],
//! `agent/guard` → [`guard`], `history/history` → [`history`],
//! `history/stage` → [`stage`], `shortcuts/shortcuts` → [`shortcuts`],
//! `overlays/overlays` → [`overlays`](mod@overlays): the registry `toast()`
//! and `confirm()` open into, which a host the app renders shows.
//!
//! # The bridge in gpui
//!
//! [`create_agent`] gives an [`AgentBridge`]: `snapshot`, `act` and `query`,
//! the three tools [`agent_tools`] describes as JSON Schema, and
//! `subscribe`. [`AgentBridge::call`] takes a model's tool call by name, JSON
//! in and JSON out, so any tool-calling or MCP layer can route to it; every
//! result also has `to_value`. `act` and `query` return tasks, which keep
//! one call at a time in the order they came.
//!
//! The registry hears every instance through the kernel's watchers. A
//! component is an instance whose entity implements the kernel's `Agent`;
//! its parts are its elements. What the original reads from the DOM comes
//! from what the kernel records of each part, and of each element given
//! ARIA or marks (`ui_sdk_kernel::page`):
//!
//! - **Names and text** are accessible names: a part's `aria_label`, a
//!   label's text, or the text of the parts that label it. gpui keeps no
//!   text an element draws where it can be read back, so a title, a button
//!   or a field the agent should read needs `aria_label`; text in `text!`
//!   alone doesn't reach it, and the original's walk of text nodes
//!   (`textOf`) reads names.
//! - **Rows** are the nearest element around a component with a row or list
//!   item role, a part or an element given ARIA with `with_aria`. Its text is
//!   its own name and the names of the elements in it.
//! - **Containment** is by paths of element ids: an element holds one whose
//!   path starts with its own, through deferred layers too. `observe`'s and
//!   history's `within` is an ancestor's element id ([`Within`]).
//! - **Marks** are [`AgentMarks`], which an app puts on a part with
//!   `Part::agent_marks` and on any other element with an id with
//!   [`with_agent_marks`]. A confirm mark's text is the nearest one around an
//!   element; hidden and secret cover everything inside.
//! - **Secret fields** are the original's: a component whose `AgentSurface`
//!   says so, one inside a secret mark, or one with a part whose native
//!   `type` or `autocomplete` (`Part::input_attrs`, like an Input's kind and
//!   hints) names a password, a card or a one-time code. A field the native
//!   crate reports as a password (`Role::PasswordInput`) is one too.
//! - **Buttons** are parts with a button or menu item role and a press, of
//!   any component, inside the component's parts. `press` runs the part's
//!   press as a click with no pointer behind it, as assistive technology
//!   clicks, and the original's `el.click()` does.
//! - **Shown** is placed by gpui (a part inside an element that doesn't
//!   display isn't), not hidden from assistive technology, and not in a
//!   popup animating out (`[data-ending-style]`): inside the element of a
//!   layer that says it's exiting, the one that paints its listener
//!   (`ui_sdk_platform::in_exiting_layer`). **Inert** is in such a popup, or
//!   behind a modal dialog, the window layer that pauses the window's
//!   shortcuts, outside every layer above it and every persistent layer.
//! - **Off the page**: a component's entity can outlive its parts, like a
//!   closed tab's field the app keeps. One that was on the page and has no
//!   parts in it now is listed off the page with its state as it is now; one
//!   whose entity dropped is listed with what it last showed.
//! - **Values** a watcher hears are the kernel's data where it reads the
//!   type, and otherwise read back from their debug text
//!   (`registry::decoded`): `Some(x)` and a variant around one value are
//!   the value, so a select's value reads as its key. A value of a type of
//!   its own registers how it reads as the original's value
//!   (`ui_sdk_kernel::watchers::register_data`), so it reads back exactly
//!   and history puts it back as it was: a calendar's dates (`null` for
//!   none), and a token input's tokens and an image cropper's crop (objects
//!   of their fields). A color picker's is a CSS string, which
//!   the kernel reads as data.
//!
//! # Dropped, and why
//!
//! - `registerTools` and WebMCP (`document.modelContext.registerTool`, tool
//!   annotations): a browser's agent has no gpui meaning. Wire
//!   [`AgentBridge::call`] and [`agent_tools`] to a model or MCP server.
//! - `source` (`data-review-loc` and Svelte's locations): gpui elements carry
//!   no attributes, and a part's location is the kernel's, not the app's.
//! - `checkMarks`, the warning for a misspelled mark: marks are typed.
//! - A hidden input's off-limits: no part here is a hidden input; forms
//!   carry their values without elements.
//! - `internal.elementOf`, `keyOf` and `options().emptyItems`: components
//!   report items by key, and the items a list shows while its query is
//!   empty are among its `agent_items`, so a suggestion a query hides isn't
//!   told apart.
//! - `valueFor`'s schema check of a `set<Key>` argument: each component's
//!   action reads its arguments by kind (`AgentArgs`), and a refusal reads as
//!   the original's, "setChecked on Notify (c2) takes true or false, not
//!   \"yes\"."
//! - A promise an action returns that rejects before the step ends: actions
//!   return at once, and work that goes on reports through `pending` and
//!   `error`.
//!
//! # Deviations
//!
//! - Every call that waits is a `Task`; dropping it stops the call where it
//!   waits. A step waiting on `allow` aborts its signal then, where the
//!   original's abandoned promise goes on waiting for the answer.
//!   `snapshot` takes the window the call runs in, which reads which
//!   components a modal hides; `call("snapshot")` reads it in a task.
//! - `allow` answers with [`Allowed`], now or later, and runs in the
//!   component's window with the app, so it can show its own confirm.
//!   [`AbortSignal`] and [`AbortController`] stand in for the web's.
//! - Listeners (`observe`, `subscribe`) hear a change as it goes through,
//!   inside the component's update, without the app. History's listeners
//!   run a moment later, with it.
//! - A list that opens and reports no items yet, like a select whose
//!   options render only while it's open, has no data: the bridge opens it
//!   to read them.
//! - [`ObservedChange`] and [`HistoryEntry`] carry the kernel's `Reason` and
//!   `ChangeSource`; `to_value` writes their names.

pub mod agent;
pub mod guard;
pub mod history;
pub mod marks;
pub mod observe;
pub mod overlays;
pub mod registry;
pub mod shortcuts;
pub mod stage;

pub use agent::{
    AbortController, AbortSignal, ActError, ActOptions, ActResult, AgentAction, AgentBridge, AgentChange,
    AgentComponent, AgentStep, AgentTool, Allow, AllowContext, Allowed, CreateAgentOptions, QueryInput,
    QueryResult, StepLog, agent_tools, components_value, create_agent, query_result_value,
};
pub use history::{History, HistoryEntry, HistoryOptions, RestoreStep, create_history, describe_entry};
pub use marks::{AgentMarks, with_agent_marks};
pub use observe::{ObserveOptions, ObservedChange, Within, element_of, observe, with_source};
pub use overlays::{OverlayEntry, Overlays, OverlaysListener, RenderedFn, create_overlays, overlays};
pub use stage::{AppliedChange, StagedChange, StagedChanges, stage_changes};
