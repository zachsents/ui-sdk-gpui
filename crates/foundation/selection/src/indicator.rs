//! Port of `foundation/selection/indicator.ts`: an indicator over a list's
//! active item, like the selected tab, and which way it moved.
//!
//! The original measures the item against the list whenever it's told to,
//! and when either changes size, and hands the box to `write` without a
//! render. Here the list and its items record their bounds as they lay out,
//! and the component reads the box as it renders the indicator part. When a
//! layout moves the box, the component renders again with it, and `write`
//! hears it, the frame after, as a `ResizeObserver` callback comes after
//! layout.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hash;
use std::rc::{Rc, Weak};

use gpui::{
    App, Bounds, Context, EntityId, ParentElement, Pixels, ScrollHandle, Styled, Window, canvas,
};
use ui_sdk_focus::{Direction, Orientation};
use ui_sdk_platform::{LayoutBox, layout_box, layout_origin};

/// What `write` runs: with the box the active item takes in the list, or
/// `None` when there's no active item.
pub type WriteFn = Rc<dyn Fn(Option<LayoutBox>, &mut Window, &mut App)>;

/// How [`bind_indicator`] measures.
#[derive(Clone, Default)]
pub struct IndicatorOptions {
    /// The list's scroll handle, when it scrolls, so the box is inside its
    /// scrollable area, where an indicator inside the list scrolls with the
    /// items.
    pub scroll: Option<ScrollHandle>,
    /// Hears the box each time a layout moves it, after the frame.
    pub write: Option<WriteFn>,
}

impl IndicatorOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn scroll(mut self, scroll: &ScrollHandle) -> Self {
        self.scroll = Some(scroll.clone());
        self
    }

    pub fn write(mut self, write: impl Fn(Option<LayoutBox>, &mut Window, &mut App) + 'static) -> Self {
        self.write = Some(Rc::new(write));
        self
    }
}

struct Inner<K> {
    owner: EntityId,
    options: IndicatorOptions,
    list: Option<Bounds<Pixels>>,
    items: HashMap<K, Bounds<Pixels>>,
    // The item the indicator covers, as the component last asked.
    active: Option<K>,
    // The box the component last rendered, and whether a check is queued.
    written: Option<LayoutBox>,
    queued: bool,
}

impl<K: Eq + Hash> Inner<K> {
    fn current(&self) -> Option<LayoutBox> {
        let list = self.list?;
        let item = self.items.get(self.active.as_ref()?)?;
        let scroll = self.options.scroll.as_ref();
        // The list is measured from inside, so its measure scrolls with the
        // items: the list itself is where it measured with no scroll.
        let list = Bounds {
            origin: layout_origin(list, scroll),
            size: list.size,
        };
        Some(layout_box(*item, list, scroll))
    }
}

/// Keeps an indicator over a list's active item (`bindIndicator`'s
/// binding). Dropping it is the original's cleanup. Cloning it gives another
/// handle to the same indicator.
pub struct Indicator<K> {
    inner: Rc<RefCell<Inner<K>>>,
}

impl<K> Clone for Indicator<K> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Keeps an indicator over a list's active item, like the selected tab
/// (`bindIndicator`): the component measures the list with
/// [`list`](Indicator::list) and each item with [`item`](Indicator::item),
/// and reads the active item's box in the list with
/// [`layout_box`](Indicator::layout_box) as it renders the indicator. `cx`
/// is the component's, which renders again when a layout moves the box.
pub fn bind_indicator<T: 'static, K: Clone + Eq + Hash + 'static>(
    options: IndicatorOptions,
    cx: &Context<T>,
) -> Indicator<K> {
    Indicator {
        inner: Rc::new(RefCell::new(Inner {
            owner: cx.entity_id(),
            options,
            list: None,
            items: HashMap::new(),
            active: None,
            written: None,
            queued: false,
        })),
    }
}

impl<K: Clone + Eq + Hash + 'static> Indicator<K> {
    /// Hands it new options.
    pub fn set_options(&self, options: IndicatorOptions) {
        self.inner.borrow_mut().options = options;
    }

    /// The box the active item takes in the list as of the last layout, for
    /// the indicator part, or `None` without an active item or before the
    /// two have laid out. A later layout that moves it renders the
    /// component again.
    pub fn layout_box(&self, active: Option<&K>) -> Option<LayoutBox> {
        let mut inner = self.inner.borrow_mut();
        inner.active = active.cloned();
        let current = inner.current();
        inner.written = current;
        current
    }

    /// Measures the list, which the box is relative to.
    pub fn list<E: ParentElement + Styled>(&self, el: E) -> E {
        let inner = Rc::downgrade(&self.inner);
        el.relative().child(
            canvas(
                move |bounds, window, cx| Self::laid_out(&inner, None, bounds, window, cx),
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }

    /// Measures an item, which the box covers while it's active.
    pub fn item<E: ParentElement + Styled>(&self, key: K, el: E) -> E {
        let inner = Rc::downgrade(&self.inner);
        el.relative().child(
            canvas(
                move |bounds, window, cx| Self::laid_out(&inner, Some(key), bounds, window, cx),
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }

    // Records a layout, and once the frame is drawn, renders the component
    // again if the box it rendered moved.
    fn laid_out(
        inner: &Weak<RefCell<Inner<K>>>,
        key: Option<K>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(inner) = inner.upgrade() else {
            return;
        };
        let check = {
            let Ok(mut state) = inner.try_borrow_mut() else {
                return;
            };
            let previous = match key {
                Some(key) => state.items.insert(key, bounds),
                None => state.list.replace(bounds),
            };
            let moved = previous != Some(bounds);
            let check = moved && !state.queued;
            if check {
                state.queued = true;
            }
            check
        };
        if !check {
            return;
        }
        let weak = Rc::downgrade(&inner);
        window.defer(cx, move |window, cx| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let (moved, owner, write, current) = {
                let mut state = inner.borrow_mut();
                state.queued = false;
                let current = state.current();
                let moved = current != state.written;
                if moved {
                    state.written = current;
                }
                (moved, state.owner, state.options.write.clone(), current)
            };
            if !moved {
                return;
            }
            cx.notify(owner);
            if let Some(write) = write {
                write(current, window, cx);
            }
        });
    }
}

/// Which way `to` lies from `from` on screen (`directionBetween`), along
/// the list's orientation, or `Direction::None` when either is missing or
/// they're level.
pub fn direction_between(
    from: Option<&LayoutBox>,
    to: Option<&LayoutBox>,
    orientation: Orientation,
) -> Direction {
    let (Some(from), Some(to)) = (from, to) else {
        return Direction::None;
    };
    let vertical = orientation == Orientation::Vertical;
    let (a, b) = if vertical {
        (from.top, to.top)
    } else {
        (from.left, to.left)
    };
    if b == a {
        return Direction::None;
    }
    match (vertical, b > a) {
        (true, true) => Direction::Down,
        (true, false) => Direction::Up,
        (false, true) => Direction::Right,
        (false, false) => Direction::Left,
    }
}
