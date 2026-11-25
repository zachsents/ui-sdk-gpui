//! Port of `foundation/kernel`: what every component shares, its change
//! details, controlled state, parts, roles and ARIA, identity, agent surface
//! and watchers.
//!
//! A component is a struct in an entity: its fields are the state, its
//! methods the api, its part methods the slots, each returning a [`Part`]
//! with the slot's role, focus, press, keys and ARIA on it. It owns an
//! [`Instance`], handed its entity with [`Instance::with_agent`], and
//! implements [`Agent`], which is what `observe` and the agent bridge read.
//!
//! # TypeScript file → Rust module
//!
//! `action` → [`action`], `attrs` → [`attrs`], `bound-keys` → [`bound_keys`],
//! `change` → [`change`], `controller` → [`controller`], `derive` →
//! [`derive`](mod@derive), `details` → [`details`], `dev` → [`dev`], `draft` → [`draft`],
//! `elements` → [`elements`], `events` → [`events`], `input-source` →
//! [`input_source`] (its `createChange` and `createCommit` are
//! [`Controllable`] in [`change`]), `keep` → [`keep`], `modality` →
//! [`modality`], `option-names` → [`option_names`], `order` → [`order`],
//! `part` → [`part`], `primitive` → [`primitive`], `primitive-types` →
//! [`primitive_types`], `roles` → [`roles`], `undo-stack` → [`undo_stack`],
//! `watchers` → [`watchers`], `index` → these re-exports.
//!
//! [`page`] has no TypeScript file: it's what the kernel records of the page
//! in place of the DOM that `observe` and the agent bridge read, each part's
//! and marked element's place, role, accessible node and [`AgentMarks`].
//! Parts also keep their press ([`PartPress`]), whether gpui placed them
//! ([`BoundPart::shown`]) and the native `type` and `autocomplete` of a
//! control ([`Part::input_attrs`]) for the bridge, and, for a component that
//! reads them back as the original reads its elements, what names them
//! ([`Part::keep_name`], [`name_of`]); a component clicks its own part as
//! `el.click()` does with [`press_of`] and [`PartPress::click`].
//!
//! # Dropped, and why
//!
//! - `after-bind.ts`: writes a microtask after Vue binds mid-hydration. There
//!   is no hydration.
//! - `controlled-lists.ts`: copies of Vue, Svelte and Solid reactive arrays
//!   changed in place. Rust options are owned values compared by value.
//! - `dev-stub.ts`: production stand-ins for `dev.ts`. Its checks are behind
//!   `cfg!(debug_assertions)`.
//! - `early-text.ts`: text typed before a page's script ran. No server
//!   rendering.
//! - `inline-style.ts` (`css`): a `style` attribute for server markup. gpui
//!   styles elements directly, and what CSS variables carried is part state.
//! - `live.ts` (`live`, `direct`, `applyAttrs`, `releaseAttrs`,
//!   `markAuthored`): attributes written to a DOM element without a render.
//!   gpui builds parts on every render, and an author's own label wins since
//!   the app's `aria_label` and `Part::aria` come after the component's
//!   ([`Aria::merge`]).
//! - `selects.ts`: narrowing which parts hear a change. An entity's parts
//!   render again when it notifies.
//! - `store.ts`: the state container. The entity is the store: `sameValue` is
//!   `PartialEq`, `subscribe` and `subscribeSkipping` are `cx.observe`, and
//!   `heldState` has no state a framework delivers ahead of it to set apart.
//! - `trusted.ts`: whether a person caused an event, swapped by the testing
//!   kit. Every gpui event comes from the platform; assistive technology's
//!   clicks come as accesskit actions, which [`part::on_press`] reports as
//!   `ChangeSource::Virtual`, the original's untrusted-looking virtual click.
//! - `events.ts`'s `on`: listeners are element methods and subscriptions.
//! - `keep.ts`'s `keep`, `release`, `unwatched`, `whenKept` and the
//!   lifecycle records: a framework keeping or releasing an instance, like
//!   React's StrictMode and `<Activity>`. An entity is alive or dropped; work
//!   outside it is a `Subscription` or `Task` field. `isDestroying` stays.
//! - `primitive.ts`'s `asleep`, `view`, `preview`, `follow`, `startState`,
//!   `use` (features), `schemaOf`, `renderVersion` and the relay checks: how
//!   frameworks render a primitive. `definePrimitive` is the component struct
//!   with its [`Instance`].
//! - `primitive-types.ts`'s `PropSpec`, `PropSchema`, `OptionsOf`,
//!   `SlotDefinition`, `SlotFactory`, `Feature`, `WithFeatures`,
//!   `WithGeneric`, `PrimitiveGeneric`, `Named`, `BaseSlotProps`,
//!   `PrimitiveContext`, `PrimitiveInstance` and `ReasonFor`: types renderers
//!   and TypeScript read. Options are typed structs, slots are part methods,
//!   generics are type parameters, an author's `id` is
//!   [`Part::with_element_id`], an agent's arguments parse by kind in
//!   [`AgentArgs`], and each `on_*_change` option documents its reasons.
//!   `PrimitiveOptions`' `debug` is [`Instance::debug`].
//! - `part.ts`'s `attrs`, `bind`, `ref`, `connect`, `commit`, `press`,
//!   `handlers`, `present`, `conditional`, `liveAttrs`, `directNames`,
//!   `selectedKeys`, `followed`, `touched`, `takeOwnId` and `yieldToAuthor`:
//!   how adapters render and bind DOM elements. A part's presence is whether
//!   the app renders it, or what it decides as it lays out
//!   ([`Part::on_layout`]), its id following is the part registry, and
//!   `subscribe` is `cx.observe`. `checkShared` (two elements binding one
//!   key) leaves with them; gpui's debug builds catch duplicate ids.
//! - `roles.ts`'s `nativeStates`: states a native input exposes by its type.
//!   gpui has no native inputs.
//! - `dev.ts`'s devtools hook and `__UI_SDK__` history (`emit`, `install`,
//!   `exposeShortcuts`, `measured`, `isMeasuring`), strict mode, `warnMissing`
//!   (a missing slot is a missing method), `checkMarks` (agent marks are
//!   data attributes, the bridge's), `checkPrevented` (a callback's event has
//!   no `preventDefault`), and `checkControlled`'s warning for both `x` and
//!   `defaultX` (a `default_x` option isn't optional, so passing both can't
//!   be told).
//! - `order.ts`'s document walks and `MutationObserver`s, `controller.ts`'s
//!   `Proxy`, `bound-keys.ts`'s `later`, and `modality.ts`'s document
//!   listeners, `isSyntheticPointer` and `clickSource`: what reads the DOM.
//!   Page order is render order, a controller is typed, a part in the page is
//!   one gpui draws, and gpui tracks the window's input modality.
//! - `action.ts`'s `isPromise`, `popupResult` and `canceledAction`: reading
//!   what a handler returned. A Rust handler returns an [`ActionOutcome`].

pub mod action;
pub mod attrs;
pub mod bound_keys;
pub mod change;
pub mod controller;
pub mod derive;
pub mod details;
pub mod dev;
pub mod draft;
pub mod elements;
pub mod events;
pub mod focus_events;
pub mod input_source;
pub mod keep;
pub mod modality;
pub mod option_names;
pub mod order;
pub mod page;
pub mod part;
pub mod primitive;
pub mod primitive_types;
pub mod roles;
pub mod undo_stack;
pub mod watchers;

pub use accesskit;

pub use action::{
    ActionDetails, ActionError, ActionHandler, ActionHandlers, ActionOutcome, AnnouncePending,
    run_action,
};
pub use attrs::{AgentMarks, Aria, AutoComplete, Checked, Current, HasPopup, Invalid, Live, Relation, Sort};
pub use bound_keys::{flag_while_bound, key_while_bound};
pub use change::{ChangeCallback, CommitCallback, Controllable, after_update, log};
pub use controller::{ConnectedController, Controller};
pub use derive::{Derived, derive_from_options};
pub use details::{
    ChangeDetails, ChangeOptions, ChangeReason, ChangeSource, CommitDetails, DismissAttemptDetails,
    DismissAttemptReason, DismissReason, InputEvent, Reason, create_details, is_dismissal,
    resolve_source, with_optional_source, with_source,
};
pub use draft::Draft;
pub use elements::{Measured, PartInfo, Parts, ensure_positioned};
pub use events::{Key, SelectEvent, is_composing, key_held, select_event, set_composing};
pub use input_source::{change_source, dispatch, dispatch_then};
pub use keep::{is_destroying, while_destroying};
pub use modality::{
    Modality, ModalityTracker, is_focus_from_keyboard, modality_of, move_focus, restore_focus,
    source_of, track_input, track_modality,
};
pub use page::{PageElement, page_ancestors, page_element, page_element_by_id, page_elements, page_elements_in};
pub use focus_events::FocusChange;
pub use part::{
    BoundPart, FocusMode, Part, PartElement, PartLayout, PartName, PartPress, held, made_label, name_of,
    press_of, with_agent_marks, with_aria,
};
pub use primitive::{AgentActionInfo, AgentHandle, Instance, InstanceHandle, id_part};
pub use primitive_types::{
    Agent, AgentAction, AgentArgs, AgentError, AgentItem, AgentResult, AgentState, AgentSurface,
    AgentValue, PENDING, json,
};
pub use roles::{Need, Unneeded};
pub use undo_stack::{UndoStack, UndoStackOptions, create_undo_stack};

pub mod prelude {
    pub use crate::roles::Role as AriaRole;
    pub use crate::{
        Agent, AgentAction, AgentState, Aria, ChangeDetails, ChangeOptions, ChangeSource,
        Controllable, FocusMode, InputEvent, Instance, Need, Part, Reason,
    };
}
