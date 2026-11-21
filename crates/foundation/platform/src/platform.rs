//! Port of `foundation/platform/platform.ts`: what the platform says about
//! focus, containment, motion and keyboard shortcuts.
//!
//! The original asks the DOM which element holds which node. gpui has no
//! nodes, so containment asks about a [`Target`]: a point of the window, like
//! a press, or an element by its focus handle. A [`Region`] is an element as
//! containment and positioning see it: where it last laid out, its hitbox,
//! and the focus handle it tracks.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::rc::{Rc, Weak};

use gpui::{
    App, Bounds, ElementId, FocusHandle, Hitbox, HitboxBehavior, Keystroke, OperatingSystem,
    ParentElement, Pixels, Point, Styled, Window, WindowId,
};
use ui_sdk_kernel::{Key, ensure_positioned};

use crate::layout::placed_canvas;

/// The top layer keeps popups above everything without portals. gpui's
/// `deferred` draws paint above the tree that declares them, so every window
/// has one.
pub fn supports_top_layer() -> bool {
    true
}

/// The focused element, by its focus handle.
pub fn active_element(window: &Window, cx: &App) -> Option<FocusHandle> {
    window.focused(cx)
}

/// What a containment check asks about, where the original passes a node.
#[derive(Clone, Debug)]
pub enum Target {
    /// Whatever is at a point of the window, like the element a press landed on.
    Point(Point<Pixels>),
    /// An element that takes focus, like the focused one or a trigger.
    Focus(FocusHandle),
}

impl From<Point<Pixels>> for Target {
    fn from(point: Point<Pixels>) -> Self {
        Target::Point(point)
    }
}

impl From<FocusHandle> for Target {
    fn from(handle: FocusHandle) -> Self {
        Target::Focus(handle)
    }
}

impl From<&FocusHandle> for Target {
    fn from(handle: &FocusHandle) -> Self {
        Target::Focus(handle.clone())
    }
}

#[derive(Default)]
struct RegionState {
    bounds: Option<Bounds<Pixels>>,
    hitbox: Option<Hitbox>,
    focus: Option<FocusHandle>,
}

/// An element as containment and positioning see it: where it last laid
/// out, its hitbox, and the focus handle it tracks. [`Region::measure`] adds
/// it to the element; clones share it.
///
/// A press is inside when the region's hitbox is hit at its point, so an
/// element covered by another, like a trigger under a dialog's backdrop, holds
/// no press there, as a DOM target wouldn't be it. Focus is inside when the
/// region's focus handle contains the focused element in gpui's dispatch tree,
/// which follows where elements are declared, `deferred` ones included. Bounds
/// are the padding box, as the kernel's `Measured` reports them.
#[derive(Clone, Default)]
pub struct Region(Rc<RefCell<RegionState>>);

impl Region {
    pub fn new() -> Self {
        Self::default()
    }

    /// A region whose element tracks `handle` for focus.
    pub fn with_focus_handle(handle: FocusHandle) -> Self {
        let region = Self::new();
        region.set_focus_handle(Some(handle));
        region
    }

    /// The focus handle the element tracks, which focus containment asks.
    pub fn set_focus_handle(&self, handle: Option<FocusHandle>) {
        self.0.borrow_mut().focus = handle;
    }

    pub fn focus_handle(&self) -> Option<FocusHandle> {
        self.0.borrow().focus.clone()
    }

    /// The element's bounds as of its last layout.
    pub fn bounds(&self) -> Option<Bounds<Pixels>> {
        self.0.borrow().bounds
    }

    /// Adds a layer to `el` that records its bounds and hitbox each frame it
    /// lays out, and where it is, which a layer's [`listener`](crate::listener)
    /// finds its element by.
    pub fn measure<E: ParentElement + Styled>(&self, el: E) -> E {
        let state = self.0.clone();
        ensure_positioned(el).child(
            placed_canvas(
                "region",
                move |bounds, place, window, _cx| {
                    let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                    {
                        let mut region = state.borrow_mut();
                        region.bounds = Some(bounds);
                        region.hitbox = Some(hitbox);
                    }
                    record_place(window.window_handle().window_id(), place, &state);
                },
                |_, _, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }

    /// Whether the element holds `target`.
    pub fn contains(&self, target: &Target, window: &Window) -> bool {
        match target {
            Target::Point(point) => self.contains_point(*point, window),
            Target::Focus(handle) => self.contains_focus(handle, window),
        }
    }

    /// Whether a press at `point` lands on the element: its hitbox, from the
    /// last frame, is hit there.
    pub fn contains_point(&self, point: Point<Pixels>, window: &Window) -> bool {
        self.0
            .borrow()
            .hitbox
            .as_ref()
            .is_some_and(|hitbox| hitbox.is_hovered_at(point, window))
    }

    /// Whether `handle` is the element's focus handle or inside it.
    pub fn contains_focus(&self, handle: &FocusHandle, window: &Window) -> bool {
        self.0
            .borrow()
            .focus
            .as_ref()
            .is_some_and(|focus| focus.contains(handle, window))
    }

    /// Whether focus is on the element or inside it.
    pub fn contains_focused(&self, window: &Window, cx: &App) -> bool {
        self.0
            .borrow()
            .focus
            .as_ref()
            .is_some_and(|focus| focus.contains_focused(window, cx))
    }

    // Whether `other` sits inside this region: in its focus handle when both
    // track one, else in its bounds.
    fn holds(&self, other: &Region, window: &Window) -> bool {
        let (ours, theirs) = (self.0.borrow(), other.0.borrow());
        if let (Some(ours), Some(theirs)) = (&ours.focus, &theirs.focus) {
            return ours.contains(theirs, window);
        }
        match (ours.bounds, theirs.bounds) {
            (Some(ours), Some(theirs)) => {
                theirs.left() >= ours.left()
                    && theirs.right() <= ours.right()
                    && theirs.top() >= ours.top()
                    && theirs.bottom() <= ours.bottom()
            }
            _ => false,
        }
    }
}

impl PartialEq for Region {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.0.borrow();
        f.debug_struct("Region")
            .field("bounds", &state.bounds)
            .field("focus", &state.focus)
            .finish()
    }
}

// The regions each window measured, by the place of their element: its
// path of element ids, as the kernel records parts' places.
#[derive(Default)]
struct Places {
    regions: HashMap<Vec<ElementId>, Weak<RefCell<RegionState>>>,
    // How many it held after it last let go of regions that went away.
    kept: usize,
}

thread_local! {
    static PLACES: RefCell<HashMap<WindowId, Places>> = RefCell::new(HashMap::new());
}

fn record_place(window: WindowId, place: &[ElementId], region: &Rc<RefCell<RegionState>>) {
    PLACES.with(|places| {
        let mut places = places.borrow_mut();
        let places = places.entry(window).or_default();
        if places
            .regions
            .get(place)
            .is_some_and(|known| std::ptr::eq(known.as_ptr(), Rc::as_ptr(region)))
        {
            return;
        }
        places.regions.insert(place.to_vec(), Rc::downgrade(region));
        if places.regions.len() > places.kept * 2 + 64 {
            places.regions.retain(|_, region| region.strong_count() > 0);
            places.kept = places.regions.len();
        }
    });
}

/// The region measured last at `place` in `window`, if it's still kept.
pub(crate) fn region_at(window: WindowId, place: &[ElementId]) -> Option<Region> {
    PLACES.with(|places| {
        places
            .borrow()
            .get(&window)
            .and_then(|places| places.regions.get(place))
            .and_then(Weak::upgrade)
            .map(Region)
    })
}

/// Whether `container` holds `node`, as gpui's dispatch tree has it. The
/// original crosses shadow roots, which gpui doesn't have.
pub fn composed_contains(container: &FocusHandle, node: &FocusHandle, window: &Window) -> bool {
    container.contains(node, window)
}

/// The nearest of `candidates` around `target`, like the item a pointer was
/// released on. The original walks up from a node to an element matching a
/// selector; gpui has neither, so the component names the candidates.
pub fn composed_closest<K>(
    target: &Target,
    candidates: impl IntoIterator<Item = (K, Region)>,
    window: &Window,
) -> Option<K> {
    let mut nearest: Option<(K, Region)> = None;
    for (key, region) in candidates {
        if !region.contains(target, window) {
            continue;
        }
        let closer = nearest
            .as_ref()
            .is_none_or(|(_, current)| current.holds(&region, window));
        if closer {
            nearest = Some((key, region));
        }
    }
    nearest.map(|(key, _)| key)
}

/// Whether the user asked for less motion. Animations jump instead. It's
/// gpui's `reduce_motion` flag, which the motion crate reads too; gpui
/// doesn't read the system setting, so the app sets the flag
/// (`cx.set_reduce_motion`).
pub fn prefers_reduced_motion(cx: &App) -> bool {
    cx.reduce_motion()
}

/// The platform shortcuts and their labels follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    Apple,
    Other,
}

thread_local! {
    static PLATFORM_OVERRIDE: Cell<Option<Platform>> = const { Cell::new(None) };
    static PARSED: RefCell<[HashMap<String, Vec<String>>; 2]> =
        RefCell::new([HashMap::new(), HashMap::new()]);
}

/// Overrides platform detection for shortcuts and their labels, like for
/// screenshots of another platform's labels.
pub fn set_platform(platform: Platform) {
    PLATFORM_OVERRIDE.with(|value| value.set(Some(platform)));
}

/// Whether shortcuts follow Apple's conventions: macOS and iOS, including an
/// app running in a browser on them.
pub fn is_apple(cx: &App) -> bool {
    if let Some(platform) = PLATFORM_OVERRIDE.with(Cell::get) {
        return platform == Platform::Apple;
    }
    matches!(
        cx.platform_info().os,
        OperatingSystem::Mac | OperatingSystem::Ios
    )
}

/// What a component's first render assumes. The original renders the
/// override's answer until hydration; gpui knows the platform from the first
/// frame, so it's [`is_apple`].
pub fn initial_apple(cx: &App) -> bool {
    is_apple(cx)
}

fn key_name(lower: &str) -> Option<&'static str> {
    Some(match lower {
        "alt" | "option" | "opt" => "Alt",
        "shift" => "Shift",
        "ctrl" | "control" => "Control",
        "cmd" | "command" | "meta" => "Meta",
        "backspace" => "Backspace",
        "delete" | "del" => "Delete",
        "enter" | "return" => "Enter",
        "escape" | "esc" => "Escape",
        "tab" => "Tab",
        "space" => "Space",
        "up" | "arrowup" => "ArrowUp",
        "down" | "arrowdown" => "ArrowDown",
        "left" | "arrowleft" => "ArrowLeft",
        "right" | "arrowright" => "ArrowRight",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        _ => return None,
    })
}

/// `"mod+shift+n"` becomes `"Meta+Shift+N"` on Apple platforms and
/// `"Control+Shift+N"` elsewhere.
pub fn aria_keyshortcuts(shortcut: &str, apple: bool) -> String {
    shortcut
        .split('+')
        .map(|key| {
            let lower = key.to_lowercase();
            if lower == "mod" {
                return if apple { "Meta" } else { "Control" }.to_string();
            }
            if let Some(name) = key_name(&lower) {
                return name.to_string();
            }
            let mut chars = key.chars();
            match (chars.next(), chars.as_str()) {
                (Some(first), rest) if !rest.is_empty() => {
                    first.to_uppercase().collect::<String>() + rest
                }
                _ => key.to_uppercase(),
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

fn apple_glyph(key: &str) -> Option<&'static str> {
    Some(match key {
        "Meta" => "⌘",
        "Control" => "⌃",
        "Alt" => "⌥",
        "Shift" => "⇧",
        "Backspace" => "⌫",
        "Delete" => "⌦",
        "Enter" => "↩",
        "Escape" => "⎋",
        "Tab" => "⇥",
        "Space" => "␣",
        "ArrowUp" => "↑",
        "ArrowDown" => "↓",
        "ArrowLeft" => "←",
        "ArrowRight" => "→",
        "PageUp" => "⇞",
        "PageDown" => "⇟",
        "Home" => "↖",
        "End" => "↘",
        _ => return None,
    })
}

fn short_name(key: &str) -> Option<&'static str> {
    Some(match key {
        "Control" => "Ctrl",
        "Delete" => "Del",
        "Escape" => "Esc",
        "ArrowUp" => "Up",
        "ArrowDown" => "Down",
        "ArrowLeft" => "Left",
        "ArrowRight" => "Right",
        _ => return None,
    })
}

/// What to show next to an item: `"⌘N"` on Apple platforms, `"Ctrl+N"`
/// elsewhere. The text is wrapped in a left-to-right isolate, so a
/// right-to-left layout never reorders it into `"N⌘"`.
pub fn format_shortcut(shortcut: &str, apple: bool) -> String {
    let aria = aria_keyshortcuts(shortcut, apple);
    if aria.is_empty() {
        return String::new();
    }
    let keys = aria.split('+');
    let text = if apple {
        keys.map(|key| apple_glyph(key).unwrap_or(key))
            .collect::<String>()
    } else {
        keys.map(|key| short_name(key).unwrap_or(key))
            .collect::<Vec<_>>()
            .join("+")
    };
    format!("\u{2066}{text}\u{2069}")
}

fn is_single(key: &str, valid: impl Fn(char) -> bool) -> bool {
    let mut chars = key.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if valid(c))
}

// Letters and digits match by physical key, gpui's `key`, when the typed
// character isn't the key itself. Option+N on a Mac types "˜", and other
// alphabets type their own letters. Every other key matches by what it types,
// the original's `event.key`.
fn matches_key(keystroke: &Keystroke, key: &str) -> bool {
    let name = Key::of(keystroke).name().to_string();
    let typed = if name.chars().count() == 1 {
        name.to_lowercase()
    } else {
        name
    };
    if key == "Space" {
        return typed == " ";
    }
    if !is_single(key, |c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
        return typed.to_lowercase() == key.to_lowercase();
    }
    let lower = key.to_lowercase();
    if typed == lower {
        return true;
    }
    // A Latin letter or digit that isn't this key is another key on this layout.
    if is_single(&typed, |c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
        return false;
    }
    keystroke.key.to_lowercase() == lower
}

/// Whether a keystroke matches a shortcut like `"mod+k"`, from a key event's
/// `event.keystroke`. `mod` is Command on Apple platforms and Control
/// elsewhere, and letters match by physical key when the layout types another
/// character, like Option+N on a Mac. `Meta` is gpui's `platform` modifier;
/// the Fn key, which the DOM never reports, doesn't count.
pub fn matches_shortcut(keystroke: &Keystroke, shortcut: &str, apple: bool) -> bool {
    let keys = PARSED.with(|parsed| {
        let mut parsed = parsed.borrow_mut();
        let cache = &mut parsed[usize::from(apple)];
        if let Some(keys) = cache.get(shortcut) {
            return keys.clone();
        }
        let keys: Vec<String> = aria_keyshortcuts(shortcut, apple)
            .split('+')
            .map(str::to_string)
            .collect();
        cache.insert(shortcut.to_string(), keys.clone());
        keys
    });
    let has = |name: &str| keys.iter().any(|key| key == name);
    let modifiers = keystroke.modifiers;
    keys.last().is_some_and(|key| matches_key(keystroke, key))
        && modifiers.platform == has("Meta")
        && modifiers.control == has("Control")
        && modifiers.alt == has("Alt")
        && modifiers.shift == has("Shift")
}

/// Whether a shortcut holds Command, Control or Option, so it can't be typing.
pub fn has_command_modifier(keys: &str) -> bool {
    aria_keyshortcuts(keys, true)
        .split('+')
        .any(|key| matches!(key, "Meta" | "Control" | "Alt"))
}
