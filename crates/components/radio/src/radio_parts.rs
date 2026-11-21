//! Port of `components/radio/radio-parts.ts`: the parts Radio, Segmented
//! Control and Rating share. The group takes part in forms and holds the
//! radios; each radio takes focus and keys, and its press checks it.

use gpui::{App, Entity, InteractiveElement, MouseButton, SharedString, StatefulInteractiveElement};
use ui_sdk_focus::RovingMember;
use ui_sdk_forms::submit_implicitly;
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{Aria, ChangeOptions, Key, Need, Part, Reason};

use crate::radio::{RadioProps, RadioState};
use crate::radio_api::{RadioGroupApi, RadioHost};

/// The keys a radio handles.
const RADIO_KEYS: &[&str] = &["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Enter", " "];

/// The group's form side, and the labels pointing at it, on its part, for
/// Radio, Segmented Control and Rating (`bindRadioGroup`): it reports the
/// checked value to its form and puts the group's part in forms in its
/// element. A label pointing at the group focuses its tab stop, and a submit
/// that finds it invalid does too, through its internals.
pub fn bind_radio_group<S>(radio: &RadioGroupApi, part: Part<S>) -> Part<S> {
    radio.sync_form();
    let aria = radio.internals().aria();
    part.map_el(|el| radio.track(el)).aria(aria)
}

/// The group's ARIA (`radioGroupAttrs`): which way its radios run, for a
/// segmented control, and whether it's required, read-only or disabled.
pub fn radio_group_attrs<S>(radio: &RadioGroupApi, part: Part<S>) -> Part<S> {
    let part = match radio.orientation() {
        Some(orientation) => part.aria_orientation(orientation),
        None => part,
    };
    part.aria(
        Aria::new()
            .required(radio.is_required())
            .read_only(radio.is_read_only())
            .disabled(radio.is_disabled()),
    )
}

/// The group part (`radioSlots`' `group`): a radio group whose radios hold
/// focus and handle the keys, with its form side. `state` is what the part
/// hands `with`, which each component makes its own (`groupState`). Build it
/// before its radios: it starts their render, so the order they render in is
/// page order.
pub fn radio_group<T: RadioHost, S>(this: &Entity<T>, state: S, cx: &App) -> Part<S> {
    let radio = this.read(cx).radio();
    let part = Part::new(radio.instance(), radio.slots().group, None, state)
        .aria_role(Role::RadioGroup)
        .unneeded(Need::Keys, "Focus stays on a radio, whose keydown handles them.");
    let part = bind_radio_group(radio, radio_group_attrs(radio, part));
    radio.begin(cx);
    part
}

/// One radio (`radioSlots`' `item`), keyed by the value the group takes when
/// it's checked. It holds the group's one tab stop while it's checked, or
/// with nothing checked while it's the first enabled radio. Arrow keys move
/// to the next enabled radio and check it, a press or Space checks it, and
/// Enter submits its form. A disabled radio is skipped, like a native one:
/// disabled itself, or by a field of its own, it can't take focus at all;
/// disabled by its group, it leaves the tab order and a press doesn't focus
/// it. A label around it or pointing at it names it, and pressing the label
/// checks it.
///
/// A component adds what its radios need beyond these on the part, like a
/// rating's names and hit areas (`radioAttrs`).
pub fn radio_item<T: RadioHost>(
    this: &Entity<T>,
    value: impl Into<SharedString>,
    props: RadioProps,
    cx: &App,
) -> Part<RadioState> {
    let value = value.into();
    let radio = this.read(cx).radio();
    let internals = radio.radio_internals(this, &value, &props);
    // A radio in a field of its own is that field's control, and takes its
    // `disabled` as the original's adapters pass it in. In the group's field,
    // the group is the control.
    let props = RadioProps {
        disabled: props.disabled || internals.control_attrs().disabled,
    };
    radio.register(value.clone(), props);
    let state = radio.item_state_with(&value, &props);
    let enabled = !state.disabled;
    let handle = radio.roving().focus_handle(&value, cx);
    // Until the group has a stop, the checked radio holds it, or with
    // nothing checked, the first enabled one claims it.
    let member = RovingMember::new()
        .disabled(!enabled)
        .focusable(enabled)
        .preferred(state.checked)
        .claims(enabled && (radio.get_value().is_none() || state.checked));
    let holds_stop = radio.roving().bind(value.clone(), &handle, member, cx);
    let focus = (!props.disabled).then_some(&handle);
    let mut part = Part::new(radio.instance(), radio.slots().item, Some(&value), state)
        .map_el(|el| internals.track(el))
        .aria_role(Role::Radio)
        .focus_mode(FocusMode::Roving { tab_stop: enabled && holds_stop }, focus, false)
        .aria(internals.aria())
        .aria(Aria::new().checked(state.checked).disabled(state.disabled));
    if state.disabled {
        // A group's disabled leaves the radio focusable to code, so a press
        // mustn't focus it either, like a disabled native radio.
        part = part.on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default());
    }
    let submits = this.downgrade();
    let keys = this.downgrade();
    let presses = this.downgrade();
    let key = value.clone();
    part.capture_key_down(move |event, window, cx| {
        let modifiers = event.keystroke.modifiers;
        if Key::of(&event.keystroke) != Key::Enter || modifiers.alt || modifiers.control || modifiers.platform {
            return;
        }
        // Enter submits the form, like on a native radio, and never checks
        // it: canceled here, gpui makes no click of it.
        window.prevent_default();
        if event.is_held {
            return;
        }
        let form = submits.upgrade().and_then(|host| host.read(cx).radio().form());
        submit_implicitly(form.as_ref(), window, cx);
    })
    .on_keys(RADIO_KEYS, move |event, window, cx| {
        keys.update(cx, |host, cx| host.radio_key_down(event, &key, window, cx))
            .ok();
    })
    .on_press(move |event, window, cx| {
        // A disabled radio takes no press, like a disabled native one.
        if props.disabled {
            return;
        }
        presses
            .update(cx, |host, cx| {
                let options = ChangeOptions::event_of(Reason::ItemPress, event);
                host.check(value.clone(), options, window, cx);
            })
            .ok();
    })
}
