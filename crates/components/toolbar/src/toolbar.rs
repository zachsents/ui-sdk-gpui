//! Port of `components/toolbar/toolbar.ts`: one tab stop for the controls of
//! an editor, a canvas or a table, with arrow keys between them.
//!
//! Tab lands on the first enabled item, then on the one focused last, as the
//! APG toolbar suggests. Buttons, links and text fields are the toolbar's
//! parts, a component's part renders through a toolbar button with
//! [`Toolbar::join`], like a menu trigger, and toggles and toggle groups
//! join through the toolbar's [`RovingScope`] ([`Toolbar::scope`]). A
//! disabled item keeps focus and moves between items, but presses and keys
//! never reach it or a component rendered through it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, Entity, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div,
};
use ui_sdk_focus::{
    Orientation, RovingFocus, RovingFocusOptions, RovingMember, RovingOrientation, RovingScope,
    Step, orientation_of, step_for,
};
use ui_sdk_forms::{Node, watch_form_disabled};
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::prelude::AriaRole;
use ui_sdk_kernel::roles::HandledKey;
use ui_sdk_kernel::{Agent, AgentState, Aria, Instance, Key, Need, Part};
use ui_sdk_platform::{ButtonElement, button_role, is_rtl};
use ui_sdk_text::TextField;

use crate::toolbar_fields::{field_keeps, place_caret};

/// Which way the toolbar runs, which picks the arrow keys that move between
/// its items.
pub type ToolbarOrientation = Orientation;

/// The options `Toolbar` takes.
#[derive(Clone, Debug, Default)]
pub struct ToolbarOptions {
    /// Which arrow keys move between items: Left and Right, or Up and Down.
    /// Horizontal when unset.
    pub orientation: Option<ToolbarOrientation>,
    /// Whether arrow keys wrap from the last item to the first. True when
    /// unset.
    pub r#loop: Option<bool>,
    /// Stops every item from being used. Items stay focusable and report
    /// themselves disabled, so the toolbar keeps its tab stop and stays
    /// discoverable.
    pub disabled: bool,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl ToolbarOptions {
    pub fn orientation(mut self, orientation: ToolbarOrientation) -> Self {
        self.orientation = Some(orientation);
        self
    }

    pub fn r#loop(mut self, r#loop: bool) -> Self {
        self.r#loop = Some(r#loop);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// The props the `button` part takes, and a part rendered through one with
/// [`Toolbar::join`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolbarButtonProps {
    /// Stops the item from being used. It stays focusable and reports itself
    /// disabled, so arrow keys still reach it and a tooltip can say why.
    pub disabled: bool,
    /// Whether a disabled item stays reachable with arrow keys. Turn it off
    /// to skip it, as a natively disabled button is. True by default.
    pub focusable_when_disabled: bool,
    /// The `group` part the item sits in, by its key, whose `disabled`
    /// disables it too.
    pub group: Option<SharedString>,
    /// What the button renders as: a native button, the default, a link, or
    /// another element that acts as a button. The original reads it off the
    /// element it renders.
    pub element: ButtonElement,
}

impl Default for ToolbarButtonProps {
    fn default() -> Self {
        Self {
            disabled: false,
            focusable_when_disabled: true,
            group: None,
            element: ButtonElement::Button,
        }
    }
}

impl ToolbarButtonProps {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn focusable_when_disabled(mut self, focusable: bool) -> Self {
        self.focusable_when_disabled = focusable;
        self
    }

    pub fn group(mut self, group: impl Into<SharedString>) -> Self {
        self.group = Some(group.into());
        self
    }

    pub fn element(mut self, element: ButtonElement) -> Self {
        self.element = element;
        self
    }
}

/// The props the `input` part takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolbarInputProps {
    /// Stops the field from being used. It stays focusable and reports
    /// itself disabled, so arrow keys still reach it.
    pub disabled: bool,
    /// Whether a disabled field stays reachable with arrow keys. Turn it off
    /// to skip it, as a natively disabled input is. True by default.
    pub focusable_when_disabled: bool,
    /// The `group` part the field sits in, by its key.
    pub group: Option<SharedString>,
}

impl Default for ToolbarInputProps {
    fn default() -> Self {
        Self {
            disabled: false,
            focusable_when_disabled: true,
            group: None,
        }
    }
}

impl ToolbarInputProps {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn focusable_when_disabled(mut self, focusable: bool) -> Self {
        self.focusable_when_disabled = focusable;
        self
    }

    pub fn group(mut self, group: impl Into<SharedString>) -> Self {
        self.group = Some(group.into());
        self
    }
}

/// The props the `link` part takes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolbarLinkProps {
    /// The `group` part the link sits in, by its key.
    pub group: Option<SharedString>,
}

impl ToolbarLinkProps {
    pub fn group(mut self, group: impl Into<SharedString>) -> Self {
        self.group = Some(group.into());
        self
    }
}

/// The props the `group` part takes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolbarGroupProps {
    /// Disables every item in the group. They stay focusable and report
    /// themselves disabled.
    pub disabled: bool,
}

impl ToolbarGroupProps {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// What the root part's `with` and the app read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolbarState {
    /// The toolbar's `orientation` option.
    pub orientation: ToolbarOrientation,
    /// Whether the whole toolbar is disabled, by its `disabled` option or a
    /// disabled fieldset around it.
    pub disabled: bool,
}

/// What a button's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolbarButtonState {
    /// Whether the item is disabled, by itself, its group or the toolbar.
    pub disabled: bool,
    /// The toolbar's `orientation` option.
    pub orientation: ToolbarOrientation,
}

/// What a link's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolbarLinkState {
    /// Whether the link is disabled, by its group or the toolbar.
    pub disabled: bool,
    /// The toolbar's `orientation` option.
    pub orientation: ToolbarOrientation,
}

/// What a text field in the toolbar reads, from [`Toolbar::input_state`].
pub type ToolbarInputState = ToolbarButtonState;

/// What a group's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolbarGroupState {
    /// Whether the group is disabled, by itself or the toolbar.
    pub disabled: bool,
    /// The toolbar's `orientation` option.
    pub orientation: ToolbarOrientation,
}

/// What a separator's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolbarSeparatorState {
    /// Which way the line runs: across the toolbar, so vertical in a
    /// horizontal toolbar.
    pub orientation: ToolbarOrientation,
}

fn across(orientation: ToolbarOrientation) -> ToolbarOrientation {
    match orientation {
        Orientation::Horizontal => Orientation::Vertical,
        Orientation::Vertical => Orientation::Horizontal,
    }
}

// The keys the root handles, by orientation.
const HORIZONTAL_KEYS: &[HandledKey] = &["ArrowLeft", "ArrowRight", "Home", "End"];
const VERTICAL_KEYS: &[HandledKey] = &["ArrowUp", "ArrowDown", "Home", "End"];

// Keys that would use a disabled item, or a component rendered through it,
// like Down opening a menu. Arrows that move between items still move.
fn is_using_key(key: &Key) -> bool {
    matches!(
        key,
        Key::Enter | Key::Space | Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight
    )
}

// What the toolbar shares with the scope its toggles join and the handlers
// on its parts, which run outside its update.
struct Shared {
    roving: RovingFocus<SharedString>,
    orientation: Cell<ToolbarOrientation>,
    // Whether the toolbar is disabled, as of its last render.
    disabled: Cell<bool>,
    // Each group's own `disabled`, by key, as of its last render.
    groups: RefCell<HashMap<SharedString, bool>>,
    // Text fields, which keep the arrows that move their caret.
    fields: RefCell<HashMap<SharedString, Rc<dyn TextField>>>,
}

impl Shared {
    fn group_disabled(&self, group: Option<&SharedString>) -> bool {
        group.is_some_and(|group| self.groups.borrow().get(group).copied().unwrap_or(false))
    }

    // Moves focus for a key pressed on an item. A text field focus moves
    // into gets its caret at the end the move pointed to.
    fn r#move(&self, event: &KeyDownEvent, key: &SharedString, window: &mut Window, cx: &mut App) {
        if event.keystroke.modifiers.shift {
            return;
        }
        let field = self.fields.borrow().get(key).cloned();
        if field.is_some_and(|field| field_keeps(event, field.as_ref(), cx)) {
            return;
        }
        let Some(moved) = self.roving.keydown(event, key, window, cx) else {
            return;
        };
        if moved.key == moved.from {
            return;
        }
        let Some(next) = self.fields.borrow().get(&moved.key).cloned() else {
            return;
        };
        let forward = match step_for(&Key::of(&event.keystroke), self.orientation.get(), is_rtl(cx)) {
            Some(Step::Last) => true,
            Some(Step::By(by)) => by > 0,
            _ => false,
        };
        place_caret(next.as_ref(), forward, window, cx);
    }
}

/// What toggles and toggle groups inside a toolbar see of it: its tab stop,
/// its arrow keys and whether it's disabled. In a toolbar group, the group's
/// `disabled` reaches them as the toolbar's, since the scope is all they
/// know of it.
struct ToolbarScope {
    shared: Rc<Shared>,
    group: Option<SharedString>,
}

impl RovingScope for ToolbarScope {
    fn roving(&self) -> &RovingFocus<SharedString> {
        &self.shared.roving
    }

    fn orientation(&self, _cx: &App) -> RovingOrientation {
        self.shared.orientation.get()
    }

    fn disabled(&self, _cx: &App) -> bool {
        self.shared.disabled.get() || self.shared.group_disabled(self.group.as_ref())
    }

    fn keydown(&self, event: &KeyDownEvent, key: &SharedString, window: &mut Window, cx: &mut App) {
        self.shared.r#move(event, key, window, cx);
    }
}

// Whether an item can be used and whether it takes focus.
#[derive(Clone, Copy)]
struct Item {
    disabled: bool,
    focusable: bool,
}

/// A toolbar. Its entity is its controller.
pub struct Toolbar {
    instance: Instance,
    orientation: ToolbarOrientation,
    r#loop: bool,
    disabled: bool,
    // The root's place in the page, so a disabled fieldset around it
    // disables it.
    node: Node,
    shared: Rc<Shared>,
}

impl Toolbar {
    pub fn new(options: ToolbarOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("toolbar").with_agent(cx);
        instance.debug = options.debug;
        let orientation = orientation_of(options.orientation, Orientation::Horizontal);
        let r#loop = options.r#loop.unwrap_or(true);
        let node = watch_form_disabled(cx);
        let shared = Rc::new_cyclic(|this: &Weak<Shared>| {
            let this = this.clone();
            // Text fields that left the toolbar keep no arrows.
            let roving = RovingFocus::new(
                RovingFocusOptions::default()
                    .orientation(orientation)
                    .r#loop(r#loop)
                    .on_items_change(move |_window, _cx| {
                        if let Some(shared) = this.upgrade() {
                            let items: Vec<SharedString> =
                                shared.roving.items().into_iter().map(|item| item.key).collect();
                            shared.fields.borrow_mut().retain(|key, _| items.contains(key));
                        }
                    }),
                window,
                cx,
            );
            Shared {
                roving,
                orientation: Cell::new(orientation),
                disabled: Cell::new(options.disabled),
                groups: RefCell::new(HashMap::new()),
                fields: RefCell::new(HashMap::new()),
            }
        });
        Self {
            instance,
            orientation,
            r#loop,
            disabled: options.disabled,
            node,
            shared,
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). Every option is controlled.
    pub fn set_options(&mut self, options: ToolbarOptions, cx: &mut Context<Self>) {
        let orientation = orientation_of(options.orientation, Orientation::Horizontal);
        let r#loop = options.r#loop.unwrap_or(true);
        let changed = self.orientation != orientation || self.r#loop != r#loop || self.disabled != options.disabled;
        self.orientation = orientation;
        self.r#loop = r#loop;
        self.disabled = options.disabled;
        self.instance.debug = options.debug;
        self.shared.roving.set_orientation(orientation);
        self.shared.roving.set_loop(r#loop);
        self.shared.orientation.set(orientation);
        self.shared.disabled.set(self.is_disabled());
        if changed {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    pub fn state(&self) -> ToolbarState {
        ToolbarState {
            orientation: self.orientation,
            disabled: self.is_disabled(),
        }
    }

    /// Whether the whole toolbar is disabled, by its `disabled` option or a
    /// disabled fieldset around it.
    pub fn is_disabled(&self) -> bool {
        self.disabled || self.node.form_disabled()
    }

    /// Moves focus to the item that holds the toolbar's tab stop: the one
    /// focused last, or the first, like the Alt+F10 shortcut editors give
    /// their toolbars.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.shared.roving.focus(None, window, cx);
    }

    /// The scope toggles and toggle groups inside the toolbar join: its tab
    /// stop, its arrow keys and whether it's disabled.
    pub fn scope(this: &Entity<Self>, cx: &App) -> Rc<dyn RovingScope> {
        Rc::new(ToolbarScope {
            shared: this.read(cx).shared.clone(),
            group: None,
        })
    }

    /// The scope for toggles and toggle groups in a `group` part, by its
    /// key, whose `disabled` they see as the toolbar's (`scopeIn`).
    pub fn scope_in(this: &Entity<Self>, group: impl Into<SharedString>, cx: &App) -> Rc<dyn RovingScope> {
        Rc::new(ToolbarScope {
            shared: this.read(cx).shared.clone(),
            group: Some(group.into()),
        })
    }

    fn item(&self, disabled: bool, focusable_when_disabled: bool, group: Option<&SharedString>) -> Item {
        let disabled = disabled || self.shared.group_disabled(group) || self.is_disabled();
        Item {
            disabled,
            focusable: !disabled || focusable_when_disabled,
        }
    }

    /// What a text field in the toolbar renders with. One that's disabled
    /// can't be edited either: give the field `read_only` while it's
    /// disabled and focusable, and `disabled` while it isn't, as the
    /// original writes `readonly` and `disabled` on its input.
    pub fn input_state(&self, props: &ToolbarInputProps) -> ToolbarInputState {
        ToolbarInputState {
            disabled: self.item(props.disabled, props.focusable_when_disabled, props.group.as_ref()).disabled,
            orientation: self.orientation,
        }
    }

    // Joins an item, with the focus handle its element tracks. A disabled
    // one keeps focus and moves between items, but presses and keys never
    // reach it or a component rendered through it: its key handler runs
    // before theirs and stops what it refuses, and a press lands on a layer
    // over it, which focuses the item and goes no further.
    fn join_item<S: 'static>(
        &self,
        part: Part<S>,
        key: SharedString,
        handle: &FocusHandle,
        item: Item,
        field: Option<Rc<dyn TextField>>,
        cx: &App,
    ) -> Part<S> {
        let member = RovingMember::new()
            .disabled(item.disabled)
            .focusable(item.focusable);
        let tab_stop = self.shared.roving.bind(key.clone(), handle, member, cx);
        let part = part.focus_mode(
            FocusMode::Roving {
                tab_stop: item.focusable && tab_stop,
            },
            Some(handle),
            false,
        );
        if !item.disabled {
            return part;
        }
        let shared = self.shared.clone();
        let focus = item.focusable.then(|| handle.clone());
        part.capture_key_down(move |event, window, cx| {
            if !is_using_key(&Key::of(&event.keystroke)) {
                return;
            }
            // A disabled field still moves its caret for the arrows it keeps,
            // so they reach the end an arrow leaves from, as in a read-only
            // field.
            if field
                .as_ref()
                .is_some_and(|field| field_keeps(event, field.as_ref(), cx))
            {
                return;
            }
            shared.r#move(event, &key, window, cx);
            window.prevent_default();
            cx.stop_propagation();
        })
        .child(refusal(focus))
    }

    /// The `root` part: the toolbar, which Left and Right move through, or Up
    /// and Down in a vertical one, with Home and End to the first and last
    /// item. The arrows reach it from any item after the item's own
    /// handlers, so a component rendered through one, like a number field,
    /// keeps the keys it uses. Name it with `aria_label`.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<ToolbarState> {
        let toolbar = this.read(cx);
        let state = toolbar.state();
        toolbar.shared.roving.begin(cx);
        toolbar.shared.disabled.set(state.disabled);
        let shared = toolbar.shared.clone();
        let keys = match state.orientation {
            Orientation::Horizontal => HORIZONTAL_KEYS,
            Orientation::Vertical => VERTICAL_KEYS,
        };
        Part::new(&toolbar.instance, "root", None, state)
            .aria_role(AriaRole::Toolbar)
            .aria_orientation(state.orientation)
            .map_el(|el| toolbar.node.track(el))
            .on_keys(keys, move |event, window, cx| {
                if let Some(key) = shared.roving.focused(window) {
                    shared.r#move(event, &key, window, cx);
                }
            })
    }

    /// A `button` part, by a key of the app's own: a button in the toolbar's
    /// tab stop. The app's click does what it does, and gpui turns Enter and
    /// Space into a click. Disabled, by itself, its group or the toolbar, it
    /// stays focusable unless `focusable_when_disabled` is off, and presses
    /// and keys don't reach it.
    pub fn button(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: ToolbarButtonProps,
        cx: &App,
    ) -> Part<ToolbarButtonState> {
        let toolbar = this.read(cx);
        let key: SharedString = key.into();
        let item = toolbar.item(props.disabled, props.focusable_when_disabled, props.group.as_ref());
        let handle = toolbar.shared.roving.focus_handle(&key, cx);
        let state = ToolbarButtonState {
            disabled: item.disabled,
            orientation: toolbar.orientation,
        };
        let part = Part::new(&toolbar.instance, "button", Some(&key), state)
            .aria_role(button_role(props.element))
            .unneeded(
                Need::Press,
                "The app's click does the work, and gpui turns Enter and Space into one.",
            )
            .aria(Aria::new().disabled(item.disabled))
            .when(props.element == ButtonElement::Link, links_ignore_space);
        toolbar.join_item(part, key, &handle, item, None, cx)
    }

    /// Renders another component's part through a toolbar button, like a
    /// menu or select trigger, which keeps its own behavior while it takes
    /// part in the toolbar's tab stop and arrow keys (`render` on a
    /// `ToolbarButton`). `handle` is the focus handle its element tracks.
    /// While it's disabled, presses and keys don't reach it.
    pub fn join<S: 'static>(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: ToolbarButtonProps,
        part: Part<S>,
        handle: &FocusHandle,
        cx: &App,
    ) -> Part<S> {
        let toolbar = this.read(cx);
        let item = toolbar.item(props.disabled, props.focusable_when_disabled, props.group.as_ref());
        let part = part
            .aria(Aria::new().disabled(item.disabled))
            .when(props.element == ButtonElement::Link, links_ignore_space);
        toolbar.join_item(part, key.into(), handle, item, None, cx)
    }

    /// A `link` part, by a key of the app's own, in the toolbar's tab stop.
    /// It keeps its link behavior: the app's click follows it, on Enter too,
    /// and Space doesn't press it. Its group or the toolbar disables it.
    pub fn link(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: ToolbarLinkProps,
        cx: &App,
    ) -> Part<ToolbarLinkState> {
        let toolbar = this.read(cx);
        let key: SharedString = key.into();
        let item = toolbar.item(false, true, props.group.as_ref());
        let handle = toolbar.shared.roving.focus_handle(&key, cx);
        let state = ToolbarLinkState {
            disabled: item.disabled,
            orientation: toolbar.orientation,
        };
        let part = Part::new(&toolbar.instance, "link", Some(&key), state)
            .aria_role(AriaRole::Link)
            .unneeded(Need::Press, "The app's click follows the link.")
            .aria(Aria::new().disabled(item.disabled))
            .map_el(links_ignore_space);
        toolbar.join_item(part, key, &handle, item, None, cx)
    }

    /// An `input` part, by a key of the app's own: a text field rendered
    /// through the toolbar, like a native text field's root or a number
    /// input's control. It keeps Home, End and the arrows while they move
    /// its caret, and at the matching end, with nothing selected, an arrow
    /// moves to the next item. Entering it with an arrow puts its caret at
    /// the far end in that direction. `field` is the field itself; see
    /// [`Toolbar::input_state`] for what a disabled one needs.
    pub fn input<S: 'static>(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: ToolbarInputProps,
        field: Rc<dyn TextField>,
        part: Part<S>,
        cx: &App,
    ) -> Part<S> {
        let toolbar = this.read(cx);
        let key: SharedString = key.into();
        let item = toolbar.item(props.disabled, props.focusable_when_disabled, props.group.as_ref());
        let handle = field.focus_handle(cx);
        toolbar
            .shared
            .fields
            .borrow_mut()
            .insert(key.clone(), field.clone());
        let part = part.aria(Aria::new().disabled(item.disabled));
        toolbar.join_item(part, key, &handle, item, Some(field), cx)
    }

    /// A `group` part, by a key of the app's own, which names a set of
    /// related items. Its `disabled` disables the items that name it as
    /// their group, which stay focusable. Name it with `aria_label`.
    pub fn group(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: ToolbarGroupProps,
        cx: &App,
    ) -> Part<ToolbarGroupState> {
        let toolbar = this.read(cx);
        let key: SharedString = key.into();
        toolbar
            .shared
            .groups
            .borrow_mut()
            .insert(key.clone(), props.disabled);
        let state = ToolbarGroupState {
            disabled: props.disabled || toolbar.is_disabled(),
            orientation: toolbar.orientation,
        };
        Part::new(&toolbar.instance, "group", Some(&key), state).aria_role(AriaRole::Group)
    }

    /// A `separator` part, by a key of the app's own: a line between sets of
    /// items, which runs across the toolbar.
    pub fn separator(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        cx: &App,
    ) -> Part<ToolbarSeparatorState> {
        let toolbar = this.read(cx);
        let key: SharedString = key.into();
        let orientation = across(toolbar.orientation);
        Part::new(
            &toolbar.instance,
            "separator",
            Some(&key),
            ToolbarSeparatorState { orientation },
        )
        .aria_role(AriaRole::Separator)
        .aria_orientation(orientation)
    }
}

// A link answers Enter alone, as a native one does: Space makes no press of
// it.
fn links_ignore_space<E: InteractiveElement>(el: E) -> E {
    el.capture_key_down(|event, window, _cx| {
        if Key::of(&event.keystroke) == Key::Space && !event.keystroke.modifiers.modified() {
            window.prevent_default();
        }
    })
}

// What a press on a disabled item lands on: a layer over it, which focuses
// the item, like any focusable disabled control, and goes no further, so
// neither the item nor a component rendered through it hears the press.
// It's under the item's own children, and the window's listeners for
// presses outside a popup still hear it.
fn refusal(focus: Option<FocusHandle>) -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .on_any_mouse_down(move |_, window, cx| {
            if let Some(handle) = &focus {
                window.focus(handle, cx);
            }
            cx.stop_propagation();
        })
}

/// No `agent` key and no api method named like a change: an agent uses the
/// controls inside it.
impl Agent for Toolbar {
    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new().with("disabled", self.is_disabled())
    }
}
