//! Port of `foundation/shortcuts/shortcut-slot.ts`: the Shortcut part
//! Dialog, Drawer, Popover and Tooltip share: the keys of a shortcut, for
//! people to see. Screen readers hear `aria-keyshortcuts` on the trigger
//! instead, so the part is hidden from them.

use std::cell::RefCell;
use std::panic::Location;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Div, Element, ElementId, GlobalElementId, InspectorElementId,
    InteractiveElement, Interactivity, IntoElement, KeyDownEvent, LayoutId, ParentElement, Pixels,
    SharedString, Stateful, StatefulInteractiveElement, Style, StyleRefinement, Styled, Window,
};
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{Aria, Instance, InstanceHandle, Part, PartElement};
use ui_sdk_platform::{format_shortcut, is_apple};

use crate::create_shortcuts::{RegisteredShortcut, ShortcutHandler, ShortcutKeys, ShortcutOptions};
use crate::shortcuts::shortcuts;

/// What a Shortcut part shows: "⌘K" on Apple platforms, "Ctrl+K" elsewhere,
/// and nothing without keys.
pub fn shortcut_text(shortcut: Option<&str>, apple: bool) -> SharedString {
    match shortcut {
        Some(keys) if !keys.is_empty() => format_shortcut(keys, apple).into(),
        _ => SharedString::default(),
    }
}

/// Switches shortcut names to this platform once a part renders
/// (`adoptPlatform`), and says whether `apple` changed, so the component
/// notifies. The original renders neutral names until the client binds;
/// gpui knows the platform from the first frame (`initial_apple`), so this
/// only follows a `set_platform` made since.
pub fn adopt_platform(apple: &mut bool, cx: &App) -> bool {
    let now = is_apple(cx);
    if *apple == now {
        return false;
    }
    *apple = now;
    true
}

/// Registers the shortcut a part's props name while they name one, and again
/// whenever they change it (`bindPartShortcut`). A window of triggers without
/// shortcuts then costs a key press nothing, where reading every trigger's
/// props on every key press grew with the triggers.
///
/// The original registers from the part's `bind` and each `onProps`, and
/// unregisters in the bind's cleanup. Here the registration lives in an
/// element added to `el`, which takes no room: it registers as the part
/// renders with keys, again when they change, and unregisters once a frame
/// doesn't render the part. The handler that runs is the one the part
/// rendered with last; `options` are those the keys first rendered with.
/// A part takes one.
pub fn bind_part_shortcut<E: ParentElement>(
    el: E,
    keys: Option<SharedString>,
    handler: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
    options: ShortcutOptions,
) -> E {
    el.child(PartShortcut {
        keys: keys.filter(|keys| !keys.is_empty()),
        handler: Rc::new(handler),
        options,
    })
}

// The element `bind_part_shortcut` adds: the registration is its element
// state, which gpui drops once a frame doesn't render it.
struct PartShortcut {
    keys: Option<SharedString>,
    handler: ShortcutHandler,
    options: ShortcutOptions,
}

struct BoundShortcut {
    keys: Option<SharedString>,
    handler: Rc<RefCell<ShortcutHandler>>,
    _registered: Option<RegisteredShortcut>,
}

impl Element for PartShortcut {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name("ui-sdk-part-shortcut".into()))
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        if let Some(id) = id {
            let keys = self.keys.clone();
            let handler = self.handler.clone();
            let options = self.options.clone();
            window.with_element_state::<BoundShortcut, _>(id, |bound, _window| {
                let bound = match bound {
                    Some(bound) if bound.keys == keys => {
                        *bound.handler.borrow_mut() = handler;
                        bound
                    }
                    stale => {
                        // The keys changed: the old ones go first.
                        drop(stale);
                        let current = Rc::new(RefCell::new(handler));
                        let registered = keys.clone().map(|keys| {
                            let current = current.clone();
                            shortcuts().register_handler(
                                ShortcutKeys::Keys(keys),
                                Rc::new(move |event: &KeyDownEvent, window: &mut Window, cx: &mut App| {
                                    let handler = current.borrow().clone();
                                    handler(event, window, cx)
                                }),
                                options,
                                cx,
                            )
                        });
                        BoundShortcut {
                            keys,
                            handler: current,
                            _registered: registered,
                        }
                    }
                };
                ((), bound)
            });
        }
        let style = Style {
            position: gpui::Position::Absolute,
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}

/// The shortcut of the part of `slot` an element sits in, like the trigger
/// or command a Shortcut part renders inside (`shortcutOf`). With `beside`,
/// a part in the element's parent counts too, like a trigger next to its
/// Shortcut part.
///
/// The original asks the DOM which element holds which. gpui has no DOM: an
/// element's place is its `GlobalElementId`, its id and its ancestors', and a
/// part's is the path it bound with ([`BoundPart::path`]), so one holds
/// another whose path starts with its own. The element's parent is its
/// nearest ancestor with an id. `shortcut` gives the shortcut the part with
/// a key rendered with, the original's `props.shortcut`.
///
/// [`BoundPart::path`]: ui_sdk_kernel::BoundPart::path
pub fn shortcut_of(
    instance: &InstanceHandle,
    slot: &str,
    el: &GlobalElementId,
    beside: bool,
    shortcut: impl Fn(Option<&SharedString>) -> Option<SharedString>,
) -> Option<SharedString> {
    let parent = el.len().checked_sub(1).map(|end| &el[..end]);
    let owner = instance
        .bound_parts()
        .into_iter()
        .filter(|part| part.slot == slot)
        .find(|part| {
            el.starts_with(&part.path)
                || (beside && parent.is_some_and(|parent| part.path.starts_with(parent)))
        })?;
    shortcut(owner.key.as_ref())
}

/// The props a Shortcut part takes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShortcutProps {
    /// The keys to show, like `"mod+s"`. By default, the shortcut of its
    /// trigger or component.
    pub shortcut: Option<SharedString>,
}

impl ShortcutProps {
    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }
}

/// What a Shortcut part's `with` and the app read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShortcutState {
    /// The shortcut formatted for this platform: `⌘S` on Apple platforms and
    /// `Ctrl+S` elsewhere. Empty without one.
    pub text: SharedString,
}

type Inherit = Rc<dyn Fn(&GlobalElementId, &App) -> Option<SharedString>>;

/// A Shortcut part of `instance`'s `shortcut` slot (`shortcutSlot`), with
/// `key` for a component that renders several. It's hidden from assistive
/// technology, which hears the trigger's `aria-keyshortcuts`.
///
/// `inherited` names the keys a part without its own `shortcut` shows, as
/// renders know them, like a tooltip's `shortcut`. [`ShortcutPart::inherit_from`]
/// looks them up where the part renders instead, like the shortcut of the
/// trigger it sits in, which renders can't know, so the state's `text`
/// leaves them out, as the original's does. A part without children shows
/// the keys itself, like the original's empty element.
pub fn shortcut_slot(
    instance: &Instance,
    key: Option<&str>,
    props: ShortcutProps,
    inherited: Option<SharedString>,
    apple: bool,
) -> ShortcutPart {
    let shortcut = props.shortcut;
    let state = ShortcutState {
        text: shortcut_text(
            shortcut.as_ref().or(inherited.as_ref()).map(|keys| keys.as_ref()),
            apple,
        ),
    };
    let part = Part::new(instance, "shortcut", key, state)
        .aria_role(Role::Group)
        .aria(Aria::new().hidden(true));
    ShortcutPart {
        part,
        shortcut,
        inherited,
        inherit: None,
        apple,
        filled: false,
    }
}

/// A Shortcut part, as the app styles it: a [`Part`] that shows its keys
/// itself unless the app gives it children.
pub struct ShortcutPart {
    part: Part<ShortcutState>,
    shortcut: Option<SharedString>,
    inherited: Option<SharedString>,
    inherit: Option<Inherit>,
    apple: bool,
    filled: bool,
}

impl ShortcutPart {
    pub fn state(&self) -> &ShortcutState {
        self.part.state()
    }

    /// Styles the part from its state.
    pub fn with(mut self, style: impl FnOnce(Stateful<Div>, &ShortcutState) -> Stateful<Div>) -> Self {
        self.part = self.part.with(style);
        self
    }

    /// Adds ARIA beyond gpui's setters, as on any part.
    pub fn aria(mut self, aria: Aria) -> Self {
        self.part = self.part.aria(aria);
        self
    }

    /// Changes the part with any of [`Part`]'s methods.
    pub fn map_part(mut self, map: impl FnOnce(Part<ShortcutState>) -> Part<ShortcutState>) -> Self {
        self.part = map(self.part);
        self
    }

    /// The keys a part without its own shows, looked up from where it
    /// renders, by its element's place: the original's `inherited` with an
    /// element, like [`shortcut_of`] the trigger it sits in.
    pub fn inherit_from(
        mut self,
        inherit: impl Fn(&GlobalElementId, &App) -> Option<SharedString> + 'static,
    ) -> Self {
        self.inherit = Some(Rc::new(inherit));
        self
    }
}

impl Styled for ShortcutPart {
    fn style(&mut self) -> &mut StyleRefinement {
        self.part.style()
    }
}

impl InteractiveElement for ShortcutPart {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.part.interactivity()
    }
}

impl StatefulInteractiveElement for ShortcutPart {}

impl ParentElement for ShortcutPart {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        let mut elements = elements.into_iter().peekable();
        if elements.peek().is_some() {
            self.filled = true;
        }
        self.part.extend(elements)
    }
}

impl IntoElement for ShortcutPart {
    type Element = ShortcutPartElement;

    fn into_element(self) -> Self::Element {
        let (mut id, mut source_location) = (None, None);
        let part = self.part.map_el(|el| {
            id = Element::id(&el);
            source_location = Element::source_location(&el);
            el
        });
        ShortcutPartElement {
            part: Some(part),
            element: None,
            id,
            source_location,
            shortcut: self.shortcut,
            inherited: self.inherited,
            inherit: self.inherit,
            apple: self.apple,
            filled: self.filled,
        }
    }
}

/// What a [`ShortcutPart`] renders as: its part, which writes its keys as it
/// lays out when it's empty, as the original's `bind` writes `textContent`.
pub struct ShortcutPartElement {
    part: Option<Part<ShortcutState>>,
    element: Option<PartElement>,
    id: Option<ElementId>,
    source_location: Option<&'static Location<'static>>,
    shortcut: Option<SharedString>,
    inherited: Option<SharedString>,
    inherit: Option<Inherit>,
    apple: bool,
    filled: bool,
}

impl Element for ShortcutPartElement {
    type RequestLayoutState = <PartElement as Element>::RequestLayoutState;
    type PrepaintState = <PartElement as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.id.clone()
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        self.source_location
    }

    fn a11y_role(&self) -> Option<gpui::Role> {
        self.element.as_ref().and_then(Element::a11y_role)
    }

    fn write_a11y_info(&self, node: &mut ui_sdk_kernel::accesskit::Node) {
        if let Some(element) = &self.element {
            Element::write_a11y_info(element, node);
        }
    }

    fn a11y_synthetic_children(
        &mut self,
        prepaint: &mut Self::PrepaintState,
        builder: &mut gpui::A11ySubtreeBuilder,
    ) {
        if let Some(element) = &mut self.element {
            Element::a11y_synthetic_children(element, prepaint, builder);
        }
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut part = self.part.take().expect("an element lays out once a frame");
        // An empty element shows the keys itself, like a web component
        // without children.
        if !self.filled {
            let keys = match (&self.shortcut, &self.inherit) {
                (Some(own), _) => Some(own.clone()),
                (None, Some(inherit)) => id.and_then(|id| inherit(id, cx)),
                (None, None) => self.inherited.clone(),
            };
            let text = shortcut_text(keys.as_deref(), self.apple);
            if !text.is_empty() {
                part = part.child(text);
            }
        }
        let element = self.element.insert(part.into_element());
        Element::request_layout(element, id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let element = self.element.as_mut().expect("laid out before prepaint");
        Element::prepaint(element, id, inspector_id, bounds, request_layout, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let element = self.element.as_mut().expect("laid out before paint");
        Element::paint(
            element,
            id,
            inspector_id,
            bounds,
            request_layout,
            prepaint,
            window,
            cx,
        )
    }
}
