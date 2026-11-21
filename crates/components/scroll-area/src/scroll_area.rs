//! Port of `components/scroll-area/scroll-area.ts`: a scrolling region with
//! scrollbars that match the app's design, edge indicators, and following
//! the end as content grows.
//!
//! The viewport is a gpui scroll container, so the wheel, trackpads and touch
//! scroll it themselves. gpui draws no scrollbars of its own: the scrollbar
//! and thumb parts are the only ones, sized and placed from the scroll
//! position as they render.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    A11ySubtreeBuilder, AbsoluteLength, AnyElement, AnyWindowHandle, App, Bounds, Context,
    DefiniteLength, Div, DispatchPhase, Element, ElementId, Entity, FocusHandle, Focusable,
    GlobalElementId, HitboxBehavior, InspectorElementId, InteractiveElement, Interactivity,
    IntoElement, LayoutId, MouseButton, MouseExitEvent, MouseMoveEvent, MouseUpEvent, Orientation,
    ParentElement, Pixels, Role, ScrollHandle, Stateful, StatefulInteractiveElement,
    StyleRefinement, Styled, Window, canvas, point, px,
};
use ui_sdk_drag_and_drop::on_pointer_down;
use ui_sdk_kernel::part::{FocusMode, PartElement, with_role};
use ui_sdk_kernel::roles::Role as AriaRole;
use ui_sdk_kernel::{Agent, AgentState, Aria, Instance, Need, Part};
use ui_sdk_platform::is_rtl;

use crate::scroll_area_model::{Axis, ScrollAreaType, edges_of, thumb_geometry};
use crate::scroll_area_pointer::ScrollbarInput;
use crate::scroll_area_view::{
    ScrollAreaModel, ScrollAreaScrollOptions, ScrollView, Tracks, TrackBox, metrics_of,
};

/// The axis a scrollbar scrolls, vertical by default.
pub type ScrollAreaOrientation = Orientation;

/// The options `ScrollArea` takes.
#[derive(Clone, Debug, Default)]
pub struct ScrollAreaOptions {
    /// When the scrollbars show: `Auto` whenever the content overflows,
    /// `Always` even when it doesn't, `Scroll` while scrolling, and `Hover`
    /// while scrolling or while the pointer is over the scroll area. Each
    /// scrollbar's state has `visible` while it should show; style that.
    /// `Hover` by default.
    pub r#type: Option<ScrollAreaType>,
    /// How long scrollbars stay after they stop being needed, with `Scroll`
    /// and `Hover`. 600ms by default.
    pub scroll_hide_delay: Option<Duration>,
    /// Keeps the end in view while the content grows, like a chat or streamed
    /// output: it starts at the bottom and stays there. Scrolling toward the
    /// top lets go, and scrolling back to the bottom sticks again.
    pub stick_to_end: bool,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl ScrollAreaOptions {
    pub fn r#type(mut self, r#type: ScrollAreaType) -> Self {
        self.r#type = Some(r#type);
        self
    }

    pub fn scroll_hide_delay(mut self, delay: Duration) -> Self {
        self.scroll_hide_delay = Some(delay);
        self
    }

    pub fn stick_to_end(mut self, stick_to_end: bool) -> Self {
        self.stick_to_end = stick_to_end;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

// The settings options sync, with their defaults.
fn settings(options: &ScrollAreaOptions) -> (ScrollAreaType, Duration, bool) {
    (
        options.r#type.unwrap_or_default(),
        options.scroll_hide_delay.unwrap_or(Duration::from_millis(600)),
        options.stick_to_end,
    )
}

/// What the root, viewport and content parts' `with` read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollAreaState {
    /// Whether the content is wider than the viewport (`data-overflow-x`).
    pub overflow_x: bool,
    /// Whether the content is taller than the viewport (`data-overflow-y`).
    pub overflow_y: bool,
    /// Whether the view is at the start of the scroll area: the top, or the
    /// inline start when only the horizontal axis overflows.
    pub at_start: bool,
    /// Whether the view is at the end of the scroll area: the bottom, or the
    /// inline end when only the horizontal axis overflows.
    pub at_end: bool,
    /// Whether the user is scrolling now.
    pub scrolling: bool,
    /// Whether a mouse is over the scroll area.
    pub hovering: bool,
    /// Whether the view follows the end as the content grows, with `stick_to_end`.
    pub sticking: bool,
    /// Whether the view is at each edge of each axis (`data-at-x-start` and the rest).
    pub at_x_start: bool,
    pub at_x_end: bool,
    pub at_y_start: bool,
    pub at_y_end: bool,
    /// How far the content reaches past each edge, the original's
    /// `--scroll-area-overflow-x-start` and the rest: a fade as tall as
    /// `overflow_y_start.min(px(32.))` is gone at the top and full once
    /// there's more above.
    pub overflow_x_start: Pixels,
    pub overflow_x_end: Pixels,
    pub overflow_y_start: Pixels,
    pub overflow_y_end: Pixels,
    /// How far along the view is on each axis, from 0 to 1
    /// (`--scroll-area-progress-x` and `-y`).
    pub progress_x: f32,
    pub progress_y: f32,
    /// The corner's size while both scrollbars show, and 0 otherwise, which
    /// each scrollbar stops short of (`--scroll-area-corner-width` and
    /// `-height`).
    pub corner_width: Pixels,
    pub corner_height: Pixels,
}

/// What a scrollbar's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollAreaScrollbarState {
    /// The axis it scrolls (`data-orientation`).
    pub orientation: ScrollAreaOrientation,
    /// Whether it should show now, by `type` (`data-visible`). While it
    /// shouldn't, it ignores the pointer.
    pub visible: bool,
    /// Whether its axis is scrolling now.
    pub scrolling: bool,
    /// Whether a mouse is over the scroll area.
    pub hovering: bool,
    /// Whether its thumb is being dragged.
    pub dragging: bool,
    /// Whether the content overflows each axis (`data-overflow-x` and `-y`).
    pub overflow_x: bool,
    pub overflow_y: bool,
    /// The thumb's length, the original's `--scroll-area-thumb-height` or
    /// `-width`.
    pub thumb_size: Pixels,
    /// How far the thumb is along the track (`--scroll-area-thumb-offset`).
    /// Right to left, a horizontal one's is negative: it starts at the right
    /// and moves left.
    pub thumb_offset: Pixels,
}

/// What a thumb's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollAreaThumbState {
    /// The axis it scrolls (`data-orientation`).
    pub orientation: ScrollAreaOrientation,
    /// Whether it's being dragged (`data-dragging`).
    pub dragging: bool,
}

/// What the scroll-to-end button's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollAreaScrollToEndState {
    /// Whether it should show: the view is away from the end and isn't
    /// heading there (`data-visible`). While it shouldn't, it's inert.
    pub visible: bool,
}

pub(crate) fn axis_of(orientation: ScrollAreaOrientation) -> Axis {
    match orientation {
        Orientation::Horizontal => Axis::X,
        Orientation::Vertical => Axis::Y,
    }
}

fn pixels(value: f64) -> Pixels {
    px(((value * 100.0).round() / 100.0) as f32)
}

/// A scroll area. Its entity is its controller: `scroll_to_end`,
/// `scroll_to_start`, `is_at_end`, `is_at_start` and `is_sticking` drive and
/// read it from anywhere.
pub struct ScrollArea {
    pub(crate) instance: Instance,
    pub(crate) model: ScrollAreaModel,
    pub(crate) view: ScrollView,
    pub(crate) input: ScrollbarInput,
    window: AnyWindowHandle,
    scroll_to_end_focus: FocusHandle,
}

impl ScrollArea {
    pub fn new(options: ScrollAreaOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (r#type, scroll_hide_delay, stick_to_end) = settings(&options);
        let mut instance = Instance::new("scroll-area").with_agent(cx);
        instance.debug = options.debug;
        Self {
            instance,
            model: ScrollAreaModel {
                r#type,
                scroll_hide_delay,
                stick_to_end,
                scrollbars_x: 0,
                scrollbars_y: 0,
                overflow_x: false,
                overflow_y: false,
                at_x_start: true,
                at_x_end: true,
                at_y_start: true,
                at_y_end: true,
                scrolling_x: false,
                scrolling_y: false,
                hovering: false,
                visible_x: false,
                visible_y: false,
                dragging: None,
                sticking: stick_to_end,
            },
            view: ScrollView::new(cx.focus_handle()),
            input: ScrollbarInput::new(cx),
            window: window.window_handle(),
            scroll_to_end_focus: cx.focus_handle(),
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). `stick_to_end` turned on later sticks at once, as it does
    /// from the start, and turned off stops.
    pub fn set_options(&mut self, options: ScrollAreaOptions, cx: &mut Context<Self>) {
        let (r#type, scroll_hide_delay, stick_to_end) = settings(&options);
        self.instance.debug = options.debug;
        let changed = self.model.r#type != r#type
            || self.model.scroll_hide_delay != scroll_hide_delay
            || self.model.stick_to_end != stick_to_end;
        if !changed {
            return;
        }
        if self.model.stick_to_end != stick_to_end {
            self.model.sticking = stick_to_end;
        }
        self.model.r#type = r#type;
        self.model.scroll_hide_delay = scroll_hide_delay;
        self.model.stick_to_end = stick_to_end;
        cx.notify();
        // Once the options are handed over, so what it measures isn't written beside them.
        let window = self.window;
        cx.spawn(async move |this, cx| {
            window
                .update(cx, |_, window, cx| {
                    this.update(cx, |this, cx| this.refresh(window, cx)).ok();
                })
                .ok();
        })
        .detach();
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The viewport's scroll state, for scrolling it to a child
    /// (`scroll_to_item`) or reading where it is.
    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.view.handle
    }

    /// What it holds now.
    pub fn model(&self) -> &ScrollAreaModel {
        &self.model
    }

    /// Scrolls to the end: the bottom, or the inline end when only the
    /// horizontal axis overflows. With `stick_to_end`, it sticks to the end
    /// again.
    pub fn scroll_to_end(&mut self, options: ScrollAreaScrollOptions, window: &mut Window, cx: &mut Context<Self>) {
        self.scroll_to_edge(true, options, window, cx);
    }

    /// Scrolls to the start: the top, or the inline start when only the
    /// horizontal axis overflows.
    pub fn scroll_to_start(&mut self, options: ScrollAreaScrollOptions, window: &mut Window, cx: &mut Context<Self>) {
        self.scroll_to_edge(false, options, window, cx);
    }

    /// Whether the view is at the end now, even in a scroll handler, before
    /// the state catches up.
    pub fn is_at_end(&self, cx: &App) -> bool {
        self.is_at(true, cx).unwrap_or(self.model.at_end())
    }

    /// Whether the view is at the start now, even in a scroll handler,
    /// before the state catches up.
    pub fn is_at_start(&self, cx: &App) -> bool {
        self.is_at(false, cx).unwrap_or(self.model.at_start())
    }

    /// Whether the view follows the end as the content grows, with `stick_to_end`.
    pub fn is_sticking(&self) -> bool {
        self.model.stick_to_end && self.model.sticking
    }

    // The scroll area's state, with the per-frame values read from the
    // viewport as it is now.
    fn area_state(&self, cx: &App) -> ScrollAreaState {
        let model = &self.model;
        let metrics = metrics_of(&self.view.handle, is_rtl(cx));
        let (x, y) = (edges_of(&metrics.x), edges_of(&metrics.y));
        let tracks = self.view.tracks.borrow();
        // Scrollbars stop short of the corner only while both show.
        let both = model.overflow_x && model.overflow_y && model.scrollbars_x > 0 && model.scrollbars_y > 0;
        ScrollAreaState {
            overflow_x: model.overflow_x,
            overflow_y: model.overflow_y,
            at_start: model.at_start(),
            at_end: model.at_end(),
            scrolling: model.scrolling_x || model.scrolling_y,
            hovering: model.hovering,
            sticking: model.stick_to_end && model.sticking,
            at_x_start: model.at_x_start,
            at_x_end: model.at_x_end,
            at_y_start: model.at_y_start,
            at_y_end: model.at_y_end,
            overflow_x_start: pixels(x.before),
            overflow_x_end: pixels(x.after),
            overflow_y_start: pixels(y.before),
            overflow_y_end: pixels(y.after),
            progress_x: x.progress as f32,
            progress_y: y.progress as f32,
            corner_width: pixels(if both { tracks.thickness(Axis::Y) } else { 0.0 }),
            corner_height: pixels(if both { tracks.thickness(Axis::X) } else { 0.0 }),
        }
    }

    /// The `root` part, which positions the scrollbars over the viewport's
    /// edges. Give it a size.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<ScrollAreaState> {
        let area = this.read(cx);
        let state = area.area_state(cx);
        let entity = this.downgrade();
        let hitbox = area.view.hitbox.clone();
        let probe = {
            let entity = entity.clone();
            canvas(
                move |bounds, window, _cx| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                move |_bounds, own, window, _cx| {
                    *hitbox.borrow_mut() = Some(own.clone());
                    let hover = own.clone();
                    let over = entity.clone();
                    window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture || window.last_input_was_touch() {
                            return;
                        }
                        let hovering = hover.is_hovered_at(window.mouse_position(), window);
                        over.update(cx, |this, cx| this.set_hovering(hovering, window, cx))
                            .ok();
                    });
                    let out = entity.clone();
                    window.on_mouse_event(move |_: &MouseExitEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture {
                            out.update(cx, |this, cx| this.set_hovering(false, window, cx)).ok();
                        }
                    });
                    let lifted = entity;
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                            lifted.update(cx, |this, cx| this.lift(window, cx)).ok();
                        }
                    });
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        };
        Part::new(&area.instance, "root", None, state)
            .relative()
            .child(probe)
            .child(area.input.drags.listener())
            .on_props(move |window, cx| {
                entity.update(cx, |this, cx| this.after_frame(window, cx)).ok();
            })
    }

    /// The `viewport` part: the scroller. Give it the root's full width and
    /// height. It's a tab stop while it scrolls, so keyboard users reach it,
    /// and Tab passes over it while something inside takes focus. Make it a
    /// landmark with an `aria_role` of region and an `aria_label` when it's
    /// worth finding, or give a chat a log role.
    pub fn viewport(this: &Entity<Self>, cx: &App) -> Part<ScrollAreaState> {
        let area = this.read(cx);
        let state = area.area_state(cx);
        let model = &area.model;
        let scrollable = model.overflow_x || model.overflow_y;
        // Sticking to the end, growth keeps the end in view before the frame
        // paints: gpui puts the view at the bottom as it lays the viewport out.
        if area.following() && area.unmoved() {
            area.view.handle.scroll_to_bottom();
            area.view.pinned.set(true);
        }
        let tab_stop = area.view.tab_stop.clone();
        let handle = area.view.handle.clone();
        let wide = model.scrollbars_x > 0;
        let scrolled = this.downgrade();
        let pressed = this.downgrade();
        let keyed = this.downgrade();
        let part = Part::new(&area.instance, "viewport", None, state)
            .map_el(|el| tab_stop.bind(el, scrollable))
            .overflow_scroll()
            .track_scroll(&handle)
            .on_scroll(move |event, window, cx| {
                scrolled
                    .update(cx, |this, cx| this.viewport_scroll(event, window, cx))
                    .ok();
            })
            .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                pressed.update(cx, |this, _cx| this.viewport_press()).ok();
            })
            .on_keys(VIEWPORT_KEYS, move |event, window, cx| {
                keyed
                    .update(cx, |this, cx| this.viewport_key_down(event, window, cx))
                    .ok();
            });
        // The content grows wider than the viewport only when there's a
        // horizontal scrollbar, so text truncates as usual otherwise.
        if wide {
            part.flex().flex_col().items_start()
        } else {
            part
        }
    }

    /// The `content` part, which wraps the content inside the viewport. It
    /// grows wider than the viewport only when there's a horizontal
    /// scrollbar, so text truncates as usual otherwise.
    pub fn content(this: &Entity<Self>, cx: &App) -> Part<ScrollAreaState> {
        let area = this.read(cx);
        let wide = area.model.scrollbars_x > 0;
        let part = Part::new(&area.instance, "content", None, area.area_state(cx));
        if wide {
            part.flex_shrink_0().min_w_full()
        } else {
            part
        }
    }

    /// A `scrollbar` part for an axis, vertical by default. It sits over the
    /// viewport's edge, at the inline end for a vertical one, and stops short
    /// of the corner while both show. Give it a width (vertical) or a height
    /// (horizontal), and show it while its state is `visible`. Hidden from
    /// assistive technology: the viewport scrolls, which screen readers
    /// already use. Pressing it never moves focus.
    pub fn scrollbar(this: &Entity<Self>, orientation: ScrollAreaOrientation, cx: &App) -> ScrollAreaScrollbar {
        let area = this.read(cx);
        let axis = axis_of(orientation);
        let model = &area.model;
        let rtl = is_rtl(cx);
        let root = area.area_state(cx);
        let metrics = metrics_of(&area.view.handle, rtl);
        let geometry = thumb_geometry(&metrics.get(axis), area.track(axis));
        let visible = model.visible(axis);
        let state = ScrollAreaScrollbarState {
            orientation,
            visible,
            scrolling: model.scrolling(axis),
            hovering: model.hovering,
            dragging: model.dragging == Some(axis),
            overflow_x: model.overflow_x,
            overflow_y: model.overflow_y,
            thumb_size: pixels(geometry.size),
            thumb_offset: pixels(if axis == Axis::X && rtl {
                -geometry.offset
            } else {
                geometry.offset
            }),
        };
        let shown = model.overflow(axis) || model.r#type == ScrollAreaType::Always;
        let pressed = this.downgrade();
        let wheeled = this.downgrade();
        let part = Part::new(&area.instance, "scrollbar", Some(axis.as_str()), state)
            .map_el(|el| with_role(el, Role::Group))
            .aria(Aria::new().hidden(true));
        let part = on_pointer_down(part, move |event, window, cx| {
            pressed
                .update(cx, |this, cx| {
                    // A hidden scrollbar must not catch presses meant for the
                    // content under it.
                    if !this.model.visible(axis) {
                        return;
                    }
                    this.press_track(axis, event, window, cx);
                    // Pressing a scrollbar never moves focus, like a native
                    // one, and the content under it never hears the press.
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .ok();
        })
        .on_scroll(move |event, window, cx| {
            wheeled
                .update(cx, |this, cx| this.forward_wheel(axis, event, window, cx))
                .ok();
        });
        let part = match axis {
            Axis::Y => {
                let part = part.top_0().bottom(root.corner_height);
                if rtl { part.left_0() } else { part.right_0() }
            }
            Axis::X => {
                let part = part.bottom_0();
                if rtl {
                    part.right_0().left(root.corner_width)
                } else {
                    part.left_0().right(root.corner_width)
                }
            }
        };
        let part = part.absolute();
        ScrollAreaScrollbar {
            part: if shown { part } else { part.hidden() },
            axis,
            tracks: area.view.tracks.clone(),
        }
    }

    /// The `thumb` part, inside the scrollbar of the same `orientation`. It
    /// sizes and moves itself along the track, so leave its length to it and
    /// give it the rest of its looks; it shows while its axis overflows.
    pub fn thumb(this: &Entity<Self>, orientation: ScrollAreaOrientation, cx: &App) -> Part<ScrollAreaThumbState> {
        let area = this.read(cx);
        let axis = axis_of(orientation);
        let model = &area.model;
        let rtl = is_rtl(cx);
        let track = area.track(axis);
        let metrics = metrics_of(&area.view.handle, rtl);
        let geometry = thumb_geometry(&metrics.get(axis), track);
        let state = ScrollAreaThumbState {
            orientation,
            dragging: model.dragging == Some(axis),
        };
        let entity = this.downgrade();
        let part = Part::new(&area.instance, "thumb", Some(axis.as_str()), state);
        let part = on_pointer_down(part, move |event, window, cx| {
            entity
                .update(cx, |this, cx| this.press_thumb(axis, event, window, cx))
                .ok();
        })
        .flex_none();
        let part = match axis {
            Axis::Y => part.h(pixels(geometry.size)).top(pixels(geometry.offset)),
            // Right to left, the horizontal thumb starts at the right and moves left.
            Axis::X if rtl => part
                .w(pixels(geometry.size))
                .left(pixels(track - geometry.size - geometry.offset)),
            Axis::X => part.w(pixels(geometry.size)).left(pixels(geometry.offset)),
        };
        if model.overflow(axis) { part } else { part.hidden() }
    }

    /// The `corner` part, where the two scrollbars meet. Only there while both show.
    pub fn corner(this: &Entity<Self>, cx: &App) -> Part {
        let area = this.read(cx);
        let model = &area.model;
        let state = area.area_state(cx);
        let both = model.overflow_x && model.overflow_y && model.scrollbars_x > 0 && model.scrollbars_y > 0;
        let part = Part::new(&area.instance, "corner", None, ())
            .map_el(|el| with_role(el, Role::Group))
            .aria(Aria::new().hidden(true))
            .absolute()
            .bottom_0()
            .w(state.corner_width)
            .h(state.corner_height);
        let part = if is_rtl(cx) { part.left_0() } else { part.right_0() };
        if both { part } else { part.hidden() }
    }

    /// The `scroll-to-end` part: a button that scrolls to the end, like "Jump
    /// to latest" in a chat. While it shouldn't show, it's inert, so it can
    /// fade out without taking focus or presses.
    pub fn scroll_to_end_part(this: &Entity<Self>, cx: &App) -> Part<ScrollAreaScrollToEndState> {
        let area = this.read(cx);
        let visible = area.model.end_button_shown();
        let handle = area.scroll_to_end_focus.clone();
        let entity = this.downgrade();
        let part = Part::new(
            &area.instance,
            "scroll-to-end",
            None,
            ScrollAreaScrollToEndState { visible },
        )
        .aria_role(AriaRole::Button);
        let part = if visible {
            part.focus_mode(FocusMode::Native, Some(&area.scroll_to_end_focus), false)
        } else {
            part.unneeded(Need::Focus, "It's inert while it shouldn't show.")
        };
        part.aria(Aria::new().hidden(!visible)).on_press(move |_event, window, cx| {
            entity
                .update(cx, |this, cx| {
                    if !this.model.end_button_shown() {
                        return;
                    }
                    // The button goes inert at the end, which would drop focus on the page.
                    if handle.is_focused(window) {
                        this.focus_viewport(window, cx);
                    }
                    this.scroll_to_end(ScrollAreaScrollOptions::default(), window, cx);
                })
                .ok();
        })
    }
}

// The keys a focused scroller handles: the ones that scroll it, and those
// that let go of the end.
const VIEWPORT_KEYS: &[&str] = &[
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "PageUp",
    "PageDown",
    "Home",
    "End",
    " ",
];


impl Focusable for ScrollArea {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.view.focus_handle().clone()
    }
}

/// A `scrollbar` part. It renders as its [`Part`], and measures its track,
/// its content box, as it lays out, for the thumb to travel along.
pub struct ScrollAreaScrollbar {
    part: Part<ScrollAreaScrollbarState>,
    axis: Axis,
    tracks: Rc<RefCell<Tracks>>,
}

impl ScrollAreaScrollbar {
    pub fn state(&self) -> &ScrollAreaScrollbarState {
        self.part.state()
    }

    /// Styles the part from its state.
    pub fn with(
        mut self,
        style: impl FnOnce(Stateful<Div>, &ScrollAreaScrollbarState) -> Stateful<Div>,
    ) -> Self {
        self.part = self.part.with(style);
        self
    }

    /// Adds ARIA beyond gpui's setters.
    pub fn aria(mut self, aria: Aria) -> Self {
        self.part = self.part.aria(aria);
        self
    }

    /// Changes the part with any of [`Part`]'s methods.
    pub fn map_part(
        mut self,
        map: impl FnOnce(Part<ScrollAreaScrollbarState>) -> Part<ScrollAreaScrollbarState>,
    ) -> Self {
        self.part = map(self.part);
        self
    }
}

impl Styled for ScrollAreaScrollbar {
    fn style(&mut self) -> &mut StyleRefinement {
        self.part.style()
    }
}

impl InteractiveElement for ScrollAreaScrollbar {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.part.interactivity()
    }
}

impl StatefulInteractiveElement for ScrollAreaScrollbar {}

impl ParentElement for ScrollAreaScrollbar {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.part.extend(elements)
    }
}

impl IntoElement for ScrollAreaScrollbar {
    type Element = ScrollAreaScrollbarElement;

    fn into_element(mut self) -> Self::Element {
        let style = self.part.style();
        let padding = [
            style.padding.top,
            style.padding.right,
            style.padding.bottom,
            style.padding.left,
        ];
        let borders = [
            style.border_widths.top,
            style.border_widths.right,
            style.border_widths.bottom,
            style.border_widths.left,
        ];
        ScrollAreaScrollbarElement {
            element: self.part.into_element(),
            axis: self.axis,
            tracks: self.tracks,
            padding,
            borders,
        }
    }
}

/// What a [`ScrollAreaScrollbar`] renders as: its part, which records its
/// track as it lays out.
pub struct ScrollAreaScrollbarElement {
    element: PartElement,
    axis: Axis,
    tracks: Rc<RefCell<Tracks>>,
    // Top, right, bottom, left, as the app styled them.
    padding: [Option<DefiniteLength>; 4],
    borders: [Option<AbsoluteLength>; 4],
}

impl ScrollAreaScrollbarElement {
    // The content box inside a border box: less the borders and the padding.
    fn content_box(&self, bounds: Bounds<Pixels>, window: &Window) -> Bounds<Pixels> {
        let rem = window.rem_size();
        let border = |index: usize| self.borders[index].map_or(px(0.), |width| width.to_pixels(rem));
        let pad = |index: usize, base: Pixels| {
            self.padding[index].map_or(px(0.), |length| length.to_pixels(base.into(), rem))
        };
        let width = bounds.size.width;
        let height = bounds.size.height;
        let top = border(0) + pad(0, height);
        let right = border(1) + pad(1, width);
        let bottom = border(2) + pad(2, height);
        let left = border(3) + pad(3, width);
        Bounds::from_corners(
            point(bounds.left() + left, bounds.top() + top),
            point(
                (bounds.right() - right).max(bounds.left() + left),
                (bounds.bottom() - bottom).max(bounds.top() + top),
            ),
        )
    }
}

impl Element for ScrollAreaScrollbarElement {
    type RequestLayoutState = <PartElement as Element>::RequestLayoutState;
    type PrepaintState = <PartElement as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Element::id(&self.element)
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        Element::source_location(&self.element)
    }

    fn a11y_role(&self) -> Option<gpui::Role> {
        Element::a11y_role(&self.element)
    }

    fn write_a11y_info(&self, node: &mut ui_sdk_kernel::accesskit::Node) {
        Element::write_a11y_info(&self.element, node);
    }

    fn a11y_synthetic_children(&mut self, prepaint: &mut Self::PrepaintState, builder: &mut A11ySubtreeBuilder) {
        Element::a11y_synthetic_children(&mut self.element, prepaint, builder);
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        Element::request_layout(&mut self.element, id, inspector_id, window, cx)
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
        let track = TrackBox {
            content: self.content_box(bounds, window),
            border: bounds,
        };
        self.tracks.borrow_mut().set(self.axis, track);
        Element::prepaint(&mut self.element, id, inspector_id, bounds, request_layout, window, cx)
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
        Element::paint(&mut self.element, id, inspector_id, bounds, request_layout, prepaint, window, cx)
    }
}

/// `controlled: []`, and no agent actions: nothing here is a value a user
/// sets and gets back through a callback, and the api's methods aren't named
/// like changes. An agent scrolls the content it reads.
impl Agent for ScrollArea {
    const CONTROLLED: Option<&'static [&'static str]> = Some(&[]);

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
    }
}
