//! Port of `foundation/kernel/input-source.ts`: the source of a change an
//! event explains, whether a key, a pointer or a screen reader caused it.
//!
//! `createChange` and `createCommit` are `Controllable::change` and
//! `Controllable::commit` (`change.rs`). What's here is the source on its
//! own, and `dispatch` for callbacks that aren't state.

use gpui::{App, Window};

use super::details::{ChangeDetails, ChangeOptions, ChangeSource, InputEvent, Reason, resolve_source, with_source};
use super::modality;

/// How a change came about, from its reason, its event and the change that
/// caused it (`changeSource`).
pub fn change_source(reason: Reason, event: Option<&InputEvent>) -> ChangeSource {
    resolve_source(reason, event)
}

/// `createChange`'s source of a change, from its reason and event. Input
/// tracking starts here, as the component is made, so its first change
/// already knows how the window was used before it (`inputSource`).
pub fn input_source(window: &mut Window, cx: &mut App) -> fn(Reason, Option<&InputEvent>) -> ChangeSource {
    modality::track_input(window, cx);
    change_source
}

/// An event option that isn't state, like `on_action(value, details)`.
pub type EventCallback<V> = std::rc::Rc<dyn Fn(&V, &ChangeDetails, &mut Window, &mut App)>;

/// Calls an event option that isn't state, like `on_action(value, details)`
/// when a menu runs an action, with the details a change passes: the reason,
/// the event and who caused it. Returns whether the callback canceled it, so
/// a component can skip what the event would have done (`dispatch`).
///
/// ```ignore
/// if dispatch(self.on_action.as_ref(), &value, ChangeOptions::event(Reason::ItemPress, event), window, cx) {
///     return;
/// }
/// ```
pub fn dispatch<V: ?Sized>(
    callback: Option<&EventCallback<V>>,
    value: &V,
    options: ChangeOptions,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let reason = options.reason.unwrap_or(Reason::Imperative);
    let source = options
        .source
        .unwrap_or_else(|| change_source(reason, options.event.as_ref()));
    let details = ChangeDetails::new(reason, source, options.event);
    if let Some(callback) = callback {
        with_source(details.source, || callback(value, &details, window, cx));
    }
    details.canceled()
}
