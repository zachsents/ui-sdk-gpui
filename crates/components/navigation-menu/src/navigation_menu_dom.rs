//! Port of `components/navigation-menu/navigation-menu-dom.ts`: what a
//! navigation menu measures: the open content's size for the viewport, and
//! the active item's box for the indicator.
//!
//! The original measures without forcing layout: a `ResizeObserver` reports
//! sizes after layout, and the indicator reads offsets once something moved.
//! Here each measured element carries a probe that records its bounds as it
//! lays out, and when what the menu rendered with moved, renders the menu
//! again once the frame is drawn, as those observers call back after layout.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    App, Bounds, EntityId, ParentElement, Pixels, SharedString, Size, Styled, Window, canvas,
};
use ui_sdk_platform::{LayoutBox, Region, layout_box};

/// The sizes of the contents in the viewport, which the viewport takes from
/// the open one (`createViewportSize`): its `--viewport-width` and
/// `--viewport-height`, which a size transition can animate.
pub(crate) struct ViewportSize {
    owner: EntityId,
    sizes: RefCell<HashMap<SharedString, Size<Pixels>>>,
    // The size the viewport last showed, kept while it closes.
    written: Cell<Option<Size<Pixels>>>,
    queued: Cell<bool>,
}

impl ViewportSize {
    pub(crate) fn new(owner: EntityId) -> Rc<Self> {
        Rc::new(Self {
            owner,
            sizes: RefCell::new(HashMap::new()),
            written: Cell::new(None),
            queued: Cell::new(false),
        })
    }

    /// Starts watching a content in the viewport: a probe that records its
    /// border box each layout, and renders the menu again when it changed.
    pub(crate) fn observe<E: ParentElement + Styled>(self: &Rc<Self>, key: SharedString, el: E) -> E {
        let this = Rc::downgrade(self);
        el.child(
            canvas(
                move |bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App| {
                    let Some(this) = this.upgrade() else {
                        return;
                    };
                    let moved = this.sizes.borrow_mut().insert(key, bounds.size) != Some(bounds.size);
                    if moved && !this.queued.replace(true) {
                        let owner = this.owner;
                        let weak = Rc::downgrade(&this);
                        window.defer(cx, move |_window, cx| {
                            if let Some(this) = weak.upgrade() {
                                this.queued.set(false);
                            }
                            cx.notify(owner);
                        });
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }

    /// Stops watching a content that left the viewport.
    pub(crate) fn unobserve(&self, key: &SharedString) {
        self.sizes.borrow_mut().remove(key);
    }

    /// The size the viewport takes: the open content's, once it laid out,
    /// or the last one while nothing is open, so a closing viewport keeps it
    /// (`write`).
    pub(crate) fn write(&self, open: Option<&SharedString>) -> Option<Size<Pixels>> {
        if let Some(size) = open.and_then(|key| self.sizes.borrow().get(key).copied()) {
            self.written.set(Some(size));
        }
        self.written.get()
    }
}

/// Where the indicator goes: over its target, relative to the root it sits
/// in, as of the last layout, and a probe that renders the menu again when a
/// layout moves it (`bindIndicator`). Built from layout, like Tabs', so
/// nothing an ancestor animates moves it.
pub(crate) struct IndicatorProbe {
    owner: EntityId,
    queued: Cell<bool>,
}

impl IndicatorProbe {
    pub(crate) fn new(owner: EntityId) -> Rc<Self> {
        Rc::new(Self {
            owner,
            queued: Cell::new(false),
        })
    }

    /// The target's box inside the root, or `None` without a target or
    /// before the two have laid out.
    pub(crate) fn layout_box(target: Option<&Region>, root: &Region) -> Option<LayoutBox> {
        let target = target?.bounds()?;
        let root = root.bounds()?;
        Some(layout_box(target, root, None))
    }

    /// Adds the probe to the indicator, which checks the box it rendered
    /// against the layout its target and the root have this frame.
    pub(crate) fn watch<E: ParentElement + Styled>(
        self: &Rc<Self>,
        el: E,
        target: Option<Region>,
        root: Region,
        rendered: Option<LayoutBox>,
    ) -> E {
        let this = Rc::downgrade(self);
        el.child(
            canvas(
                move |_bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App| {
                    let Some(this) = this.upgrade() else {
                        return;
                    };
                    let now = Self::layout_box(target.as_ref(), &root);
                    if now != rendered && !this.queued.replace(true) {
                        let owner = this.owner;
                        let weak = Rc::downgrade(&this);
                        window.defer(cx, move |_window, cx| {
                            if let Some(this) = weak.upgrade() {
                                this.queued.set(false);
                            }
                            cx.notify(owner);
                        });
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }
}
