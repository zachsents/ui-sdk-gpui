//! Port of `foundation/lists/scroll-picker.ts`: makes a scrolling list of
//! options pick by scrolling, like a wheel of hours or a thumbwheel of
//! pages: the option at the center is the value. Scrolling or a drag with
//! the mouse selects the option at the center as it passes, and settles on
//! the nearest enabled one when it stops. When the list selects an option
//! itself, from a click, an arrow key or code, it glides there without
//! selecting the ones it passes. Each option has an offset, its distance
//! from the center in options ([`ScrollPicker::offset`]), to draw a wheel
//! with.
//!
//! The original binds a listbox's element and reads its options, their
//! `aria-selected` and `aria-disabled`, and their boxes from the page. Here
//! the component that renders the list tells the picker its options each
//! render ([`ScrollPicker::sync`]), measures each option's element
//! ([`ScrollPicker::option`]), and puts [`ScrollPicker::track`] on the list's
//! element, the one that scrolls. gpui's scrollers don't snap, so the
//! picker's own settling is the snap, and they don't scroll smoothly, so the
//! picker glides itself, on animation frames.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gpui::{
    AnyWindowHandle, App, Bounds, Context, Element, ElementId, FocusHandle, GlobalElementId, InspectorElementId,
    InteractiveElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, Orientation, ParentElement, Pixels,
    Refineable, ScrollEvent, ScrollHandle, SharedString, Style, StyleRefinement, Styled, Task, WeakEntity, Window,
    point, px,
};
use ui_sdk_drag_and_drop::{
    BindDragOptions, DRAG_THRESHOLD, DragSession, PointerEvent, create_drag_session, swallow_next_click,
};
use ui_sdk_kernel::{InputEvent, Key};
use ui_sdk_motion::{VelocityTracker, VelocityTrackerOptions, project_distance, timestamp};
use ui_sdk_number::clamp;
use ui_sdk_platform::prefers_reduced_motion;

use crate::listbox_keyboard::set_paged_elsewhere;

// No scroll for this long counts as rest: gpui has no `scrollend`.
const REST_TIMEOUT: Duration = Duration::from_millis(120);

// UIScrollView's normal deceleration, for where a flick coasts to.
const DECELERATION_RATE: f64 = 0.998;

// How long the picker's own glide to an option takes.
const GLIDE: Duration = Duration::from_millis(250);

/// What `on_select` hears with the option it brought to the center
/// (`ScrollPickerSelectDetails`).
#[derive(Clone, Debug)]
pub struct ScrollPickerSelectDetails {
    /// The option at the center, by its key.
    pub key: SharedString,
    /// What moved it there: a scroll, a drag's pointer, or a Page Up or
    /// Page Down key.
    pub event: Option<InputEvent>,
}

/// Hears an option brought to the center, with its index among all the
/// options, inside the component's update.
pub type ScrollPickerSelectFn<C> =
    Rc<dyn Fn(&mut C, usize, &ScrollPickerSelectDetails, &mut Window, &mut Context<C>)>;

/// How [`bind_scroll_picker`] scrolls and what it reports
/// (`ScrollPickerOptions`).
pub struct ScrollPickerOptions<C: 'static> {
    /// Which way the list scrolls. Defaults to vertical, the list's own
    /// orientation in the original.
    pub orientation: Option<Orientation>,
    /// Runs when someone's scroll, drag, or Page Up or Page Down brings
    /// another enabled option to the center, with its index among all the
    /// options. Select it here. Clicks and arrow keys select through the list
    /// itself.
    pub on_select: Option<ScrollPickerSelectFn<C>>,
}

impl<C: 'static> Default for ScrollPickerOptions<C> {
    fn default() -> Self {
        Self {
            orientation: None,
            on_select: None,
        }
    }
}

impl<C: 'static> ScrollPickerOptions<C> {
    pub fn orientation(mut self, orientation: Orientation) -> Self {
        self.orientation = Some(orientation);
        self
    }

    pub fn on_select(
        mut self,
        on_select: impl Fn(&mut C, usize, &ScrollPickerSelectDetails, &mut Window, &mut Context<C>) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(on_select));
        self
    }
}

/// Where `at` falls among the option `centers`, ascending, as a fractional
/// index: 2 at the third option's center, 2.5 halfway to the fourth. Past
/// either end it carries on at the spacing of the last two
/// (`pickerPosition`).
pub fn picker_position(centers: &[f64], at: f64) -> f64 {
    let Some(last) = centers.len().checked_sub(1) else {
        return 0.0;
    };
    if last < 1 {
        return 0.0;
    }
    let mut low = 0;
    let mut high = last - 1;
    while low < high {
        let middle = (low + high).div_ceil(2);
        if centers[middle] <= at {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    let span = centers[low + 1] - centers[low];
    if span > 0.0 {
        low as f64 + (at - centers[low]) / span
    } else {
        low as f64
    }
}

/// The enabled index nearest `position`, a fractional index from
/// [`picker_position`], or `None` when every option is disabled
/// (`nearestEnabled`).
pub fn nearest_enabled(position: f64, count: usize, is_disabled: impl Fn(usize) -> bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let start = clamp(position.round(), 0.0, (count - 1) as f64) as isize;
    let lean: isize = if position >= start as f64 { 1 } else { -1 };
    for distance in 0..count as isize {
        let near = start + distance * lean;
        if near >= 0 && (near as usize) < count && !is_disabled(near as usize) {
            return Some(near as usize);
        }
        let far = start - distance * lean;
        if far >= 0 && (far as usize) < count && !is_disabled(far as usize) {
            return Some(far as usize);
        }
    }
    None
}

/// An option as the picker sees it: its key, whether it's disabled
/// (`aria-disabled`), whether the list has it selected (`aria-selected`),
/// and the focus handle it tracks in a roving list.
#[derive(Clone, Debug, PartialEq)]
pub struct PickerOption {
    pub key: SharedString,
    pub disabled: bool,
    pub selected: bool,
    pub focus: Option<FocusHandle>,
}

impl PickerOption {
    pub fn new(key: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            disabled: false,
            selected: false,
            focus: None,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn focus(mut self, focus: FocusHandle) -> Self {
        self.focus = Some(focus);
        self
    }
}

struct Drag {
    from: f64,
    scroll: f64,
    moved: bool,
}

struct Glide {
    from: f64,
    to: f64,
    started: Instant,
}

struct PickerState {
    options: Vec<PickerOption>,
    // Options changed since the picker last looked.
    changed: bool,
    bounds: Vec<Option<Bounds<Pixels>>>,
    bounds_next: Vec<(usize, Bounds<Pixels>)>,
    centers: Vec<f64>,
    sized: bool,
    // The option selected last, by the picker or the list.
    selected: Option<usize>,
    // The scroll position the picker is moving to itself, so its own scroll
    // doesn't select what it passes.
    target: Option<f64>,
    scrolling: bool,
    last_event: Option<InputEvent>,
    rest: Option<Task<()>>,
    drag: Option<Drag>,
    tracker: VelocityTracker,
    glide: Option<Glide>,
    last_scroll: f64,
    last_size: (Pixels, Pixels),
    checking: bool,
    bound: bool,
}

struct PickerInner<C: 'static> {
    me: Weak<PickerInner<C>>,
    options: ScrollPickerOptions<C>,
    list: ElementId,
    scroll: ScrollHandle,
    entity: WeakEntity<C>,
    window: AnyWindowHandle,
    session: DragSession<C>,
    state: RefCell<PickerState>,
}

/// A list of options that picks by scrolling (`bindScrollPicker`'s
/// binding). Cloning it gives another handle to the same picker.
pub struct ScrollPicker<C: 'static> {
    inner: Rc<PickerInner<C>>,
}

impl<C: 'static> Clone for ScrollPicker<C> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Makes a scrolling list of options pick by scrolling (`bindScrollPicker`),
/// for the component of `cx`, whose update `on_select` runs in. `list` is
/// the list part's element id, whose Page Up and Page Down the picker takes
/// over ([`set_paged_elsewhere`]), and `scroll` the handle its scroller
/// tracks. Set the listbox's `keep_in_view` off, so it doesn't jump to an
/// option the wheel is about to glide to. [`ScrollPicker::unbind`] undoes it.
pub fn bind_scroll_picker<C: 'static>(
    list: ElementId,
    scroll: ScrollHandle,
    options: ScrollPickerOptions<C>,
    window: &Window,
    cx: &Context<C>,
) -> ScrollPicker<C> {
    set_paged_elsewhere(&list, true);
    ScrollPicker {
        inner: Rc::new_cyclic(|me| PickerInner {
            me: me.clone(),
            options,
            list,
            scroll,
            entity: cx.weak_entity(),
            window: window.window_handle(),
            session: create_drag_session(cx),
            state: RefCell::new(PickerState {
                options: Vec::new(),
                changed: false,
                bounds: Vec::new(),
                bounds_next: Vec::new(),
                centers: Vec::new(),
                sized: false,
                selected: None,
                target: None,
                scrolling: false,
                last_event: None,
                rest: None,
                drag: None,
                tracker: VelocityTracker::new(VelocityTrackerOptions::default()),
                glide: None,
                last_scroll: 0.0,
                last_size: (px(0.), px(0.)),
                checking: false,
                bound: true,
            }),
        }),
    }
}

impl<C: 'static> ScrollPicker<C> {
    fn vertical(&self) -> bool {
        self.inner.options.orientation != Some(Orientation::Horizontal)
    }

    // gpui's scroll positions run from the start edge whatever the direction.
    fn scroll(&self) -> f64 {
        let offset = self.inner.scroll.offset();
        let along = if self.vertical() { offset.y } else { offset.x };
        -f64::from(along)
    }

    fn view_size(&self) -> f64 {
        let bounds = self.inner.scroll.bounds();
        f64::from(if self.vertical() {
            bounds.size.height
        } else {
            bounds.size.width
        })
    }

    fn max_scroll(&self) -> f64 {
        let max = self.inner.scroll.max_offset();
        f64::from(if self.vertical() { max.y } else { max.x })
    }

    fn write_scroll(&self, to: f64) {
        let to = px(to.max(0.0) as f32);
        let offset = self.inner.scroll.offset();
        if self.vertical() {
            self.inner.scroll.set_offset(point(offset.x, -to));
        } else {
            self.inner.scroll.set_offset(point(-to, offset.y));
        }
    }

    fn is_busy(state: &PickerState) -> bool {
        state.scrolling || state.drag.as_ref().is_some_and(|drag| drag.moved)
    }

    /// The list's options as it renders, in order, every option, disabled
    /// ones included (the original reads them off the list, and hears them
    /// change). Call it each render.
    pub fn sync(&self, options: Vec<PickerOption>) {
        let mut state = self.inner.state.borrow_mut();
        if state.options != options {
            state.options = options;
            state.changed = true;
        }
    }

    /// Measures an option's element, by its index among all the options.
    pub fn option<E: ParentElement + Styled>(&self, index: usize, el: E) -> E {
        el.relative().child(
            OptionProbe {
                index,
                picker: Rc::downgrade(&self.inner) as Weak<dyn PickerProbe>,
                style: StyleRefinement::default(),
            }
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }

    /// An option's distance from the center, in options (`--picker-offset`):
    /// 0 at the center, 1 for the next option, -2 two before it. It's
    /// fractional while the list moves between options.
    pub fn offset(&self, index: usize) -> f32 {
        let position = self.position();
        ((index as f64 - position) * 1000.0).round() as f32 / 1000.0
    }

    // Where the center is among the options, as a fractional index.
    fn position(&self) -> f64 {
        let state = self.inner.state.borrow();
        let at = self.scroll() + self.view_size() / 2.0;
        let mut position = picker_position(&state.centers, at);
        // Scroll positions round to whole pixels, so an option within one
        // counts as centered.
        let nearest = position.round();
        if nearest >= 0.0
            && let Some(center) = state.centers.get(nearest as usize)
            && (center - at).abs() < 1.0
        {
            position = nearest;
        }
        position
    }

    // The options' centers along the list, from where they last laid out.
    fn measure(&self, state: &mut PickerState) {
        let viewport = self.inner.scroll.bounds();
        let vertical = self.vertical();
        let size = if vertical {
            viewport.size.height
        } else {
            viewport.size.width
        };
        state.sized = size > px(0.);
        let at = self.scroll();
        state.centers = state
            .bounds
            .iter()
            .map(|bounds| {
                bounds.map_or(0.0, |rect| {
                    let offset = if vertical {
                        rect.top() + rect.size.height / 2. - viewport.top()
                    } else {
                        rect.left() + rect.size.width / 2. - viewport.left()
                    };
                    f64::from(offset) + at
                })
            })
            .collect();
    }

    // A roving list moves its focus along, so the next key starts here.
    fn focus_along(&self, index: usize, window: &mut Window, cx: &mut App) {
        let (target, holds) = {
            let state = self.inner.state.borrow();
            let Some(target) = state.options.get(index).and_then(|option| option.focus.clone()) else {
                return;
            };
            let holds = state
                .options
                .iter()
                .any(|option| option.focus.as_ref().is_some_and(|focus| focus.is_focused(window)));
            (target, holds)
        };
        if holds && !target.is_focused(window) {
            window.focus(&target, cx);
        }
    }

    fn select(&self, this: &mut C, index: usize, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<C>) {
        let key = {
            let mut state = self.inner.state.borrow_mut();
            state.selected = Some(index);
            state.options.get(index).map(|option| option.key.clone()).unwrap_or_default()
        };
        if let Some(on_select) = self.inner.options.on_select.clone() {
            on_select(this, index, &ScrollPickerSelectDetails { key, event }, window, cx);
        }
        self.focus_along(index, window, cx);
    }

    fn arrive(&self) {
        let mut state = self.inner.state.borrow_mut();
        state.target = None;
        state.glide = None;
    }

    fn wait_for_rest(&self, cx: &App) {
        let picker = self.clone();
        let window = self.inner.window;
        let entity = self.inner.entity.clone();
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(REST_TIMEOUT).await;
            window
                .update(cx, |_, window, cx| {
                    entity.update(cx, |this, cx| picker.on_rest(this, window, cx)).ok();
                })
                .ok();
        });
        self.inner.state.borrow_mut().rest = Some(task);
    }

    fn center_on(&self, index: usize, animate: bool, window: &mut Window, cx: &mut App) {
        let center = self.inner.state.borrow().centers.get(index).copied();
        let Some(center) = center else {
            return;
        };
        let to = clamp(center - self.view_size() / 2.0, 0.0, self.max_scroll());
        if (to - self.scroll()).abs() < 1.0 {
            return self.arrive();
        }
        let glides = animate && !prefers_reduced_motion(cx);
        {
            let mut state = self.inner.state.borrow_mut();
            state.target = Some(to);
            state.glide = glides.then(|| Glide {
                from: self.scroll(),
                to,
                started: Instant::now(),
            });
        }
        if glides {
            self.glide(window);
        } else {
            self.write_scroll(to);
            window.refresh();
        }
        self.wait_for_rest(cx);
    }

    // The picker's own smooth scroll, one step a frame.
    fn glide(&self, window: &mut Window) {
        let picker = self.clone();
        window.on_next_frame(move |window, _cx| {
            let step = {
                let state = picker.inner.state.borrow();
                state.glide.as_ref().map(|glide| {
                    let progress = (glide.started.elapsed().as_secs_f64() / GLIDE.as_secs_f64()).min(1.0);
                    let eased = 1.0 - (1.0 - progress).powi(3);
                    (glide.from + (glide.to - glide.from) * eased, progress >= 1.0)
                })
            };
            let Some((at, done)) = step else {
                return;
            };
            picker.write_scroll(at);
            window.refresh();
            if done {
                picker.inner.state.borrow_mut().glide = None;
            } else {
                picker.glide(window);
            }
        });
    }

    fn settle_at(&self, this: &mut C, at: f64, event: Option<InputEvent>, window: &mut Window, cx: &mut Context<C>) {
        let index = {
            let state = self.inner.state.borrow();
            nearest_enabled(at, state.options.len(), |each| state.options[each].disabled)
        };
        let Some(index) = index else {
            return self.arrive();
        };
        if self.inner.state.borrow().selected != Some(index) {
            self.select(this, index, event, window, cx);
        }
        self.center_on(index, true, window, cx);
    }

    // The list scrolled, by someone or by the picker.
    fn on_scroll(&self, this: &mut C, window: &mut Window, cx: &mut Context<C>) {
        // Bound while hidden, it has measured nothing, and the list can
        // scroll as it shows.
        if !self.inner.state.borrow().sized {
            return self.on_resize(window, cx);
        }
        let target = self.inner.state.borrow().target;
        if let Some(target) = target {
            if (self.scroll() - target).abs() < 1.0 {
                self.arrive();
            } else {
                self.wait_for_rest(cx);
            }
        } else if !self.inner.state.borrow().drag.as_ref().is_some_and(|drag| drag.moved) {
            self.inner.state.borrow_mut().scrolling = true;
            self.wait_for_rest(cx);
        }
        self.follow(this, window, cx);
    }

    fn follow(&self, this: &mut C, window: &mut Window, cx: &mut Context<C>) {
        let position = self.position();
        let pick = {
            let state = self.inner.state.borrow();
            if state.target.is_some() || !state.scrolling || state.options.is_empty() {
                return;
            }
            let index = clamp(position.round(), 0.0, (state.options.len() - 1) as f64) as usize;
            (Some(index) != state.selected && !state.options[index].disabled).then(|| (index, state.last_event.clone()))
        };
        if let Some((index, event)) = pick {
            self.select(this, index, event, window, cx);
        }
    }

    fn on_rest(&self, this: &mut C, window: &mut Window, cx: &mut Context<C>) {
        let (target, scrolling, dragging) = {
            let mut state = self.inner.state.borrow_mut();
            state.rest = None;
            (
                state.target,
                state.scrolling,
                state.drag.as_ref().is_some_and(|drag| drag.moved),
            )
        };
        if target.is_some() {
            return self.arrive();
        }
        if !scrolling || dragging {
            return;
        }
        let event = {
            let mut state = self.inner.state.borrow_mut();
            state.scrolling = false;
            state.last_event.clone()
        };
        let position = self.position();
        self.settle_at(this, position, event, window, cx);
    }

    // Someone took over from the picker's own scroll.
    fn interrupt(&self) {
        let mut state = self.inner.state.borrow_mut();
        state.target = None;
        state.glide = None;
    }

    fn on_resize(&self, window: &mut Window, cx: &mut App) {
        let center = {
            let mut state = self.inner.state.borrow_mut();
            self.measure(&mut state);
            state
                .selected
                .filter(|_| !Self::is_busy(&state) && state.target.is_none())
        };
        if let Some(selected) = center {
            self.center_on(selected, false, window, cx);
        }
    }

    // The options or which is selected changed.
    fn on_changed(&self, window: &mut Window, cx: &mut App) {
        let (index, animate) = {
            let mut state = self.inner.state.borrow_mut();
            self.measure(&mut state);
            let index = state.options.iter().position(|option| option.selected);
            match index {
                Some(index) if Some(index) != state.selected && !Self::is_busy(&state) => {
                    // A list whose options render after it starts there at once.
                    let animate = state.selected.is_some();
                    state.selected = Some(index);
                    (Some(index), animate)
                }
                _ => (None, false),
            }
        };
        if let Some(index) = index {
            self.focus_along(index, window, cx);
            self.center_on(index, animate, window, cx);
        }
    }

    // Once a frame that drew the list is done: what scrolled, what resized,
    // and what the list changed.
    fn check(&self, window: &mut Window, cx: &mut App) {
        let (scrolled, resized, changed) = {
            let mut state = self.inner.state.borrow_mut();
            state.checking = false;
            if !state.bound {
                return;
            }
            let count = state.options.len();
            let mut bounds = vec![None; count];
            for (index, rect) in std::mem::take(&mut state.bounds_next) {
                if index < count {
                    bounds[index] = Some(rect);
                }
            }
            let resized_items = bounds != state.bounds;
            state.bounds = bounds;
            let scroll = self.scroll();
            let viewport = self.inner.scroll.bounds();
            let size = (viewport.size.width, viewport.size.height);
            let scrolled = (scroll - state.last_scroll).abs() > 0.5;
            let resized = size != state.last_size || resized_items;
            state.last_scroll = scroll;
            state.last_size = size;
            let changed = std::mem::take(&mut state.changed);
            (scrolled, resized, changed)
        };
        if changed {
            self.on_changed(window, cx);
        } else if resized {
            self.on_resize(window, cx);
        }
        if scrolled && let Some(entity) = self.inner.entity.upgrade() {
            entity.update(cx, |this, cx| self.on_scroll(this, window, cx));
        }
    }

    /// The list's element, the one that scrolls (`bind`'s listeners): a
    /// drag with the mouse scrolls it, the wheel takes over from the
    /// picker's own glide, Page Up and Page Down move a screenful of
    /// options, and the picker looks at what changed once each frame is
    /// drawn.
    pub fn track<E: ParentElement + Styled + InteractiveElement>(&self, el: E) -> E {
        let wheel = self.clone();
        let press = self.clone();
        let keys = self.clone();
        let el = el
            .on_scroll(move |_event: &ScrollEvent, _window, _cx| wheel.interrupt())
            .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, window, cx| {
                let picker = press.clone();
                let entity = press.inner.entity.clone();
                let down = PointerEvent::mouse_down(event, window, cx);
                entity
                    .update(cx, |this, cx| picker.press(this, &down, window, cx))
                    .ok();
            })
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                let picker = keys.clone();
                let entity = keys.inner.entity.clone();
                entity
                    .update(cx, |this, cx| picker.keydown(this, event, window, cx))
                    .ok();
            })
            .child(
                FrameProbe {
                    picker: Rc::downgrade(&self.inner) as Weak<dyn PickerProbe>,
                    style: StyleRefinement::default(),
                }
                .absolute()
                .size_0(),
            );
        self.inner.session.track(el)
    }

    fn along(&self, x: Pixels, y: Pixels) -> f64 {
        f64::from(if self.vertical() { y } else { x })
    }

    fn press(&self, this: &mut C, down: &PointerEvent, window: &mut Window, cx: &mut Context<C>) {
        let session = self.inner.session.clone();
        if !session.accepts(this, down, window, cx) {
            return;
        }
        self.interrupt();
        let from = self.along(down.x(), down.y());
        {
            let mut state = self.inner.state.borrow_mut();
            state.drag = Some(Drag {
                from,
                scroll: self.scroll(),
                moved: false,
            });
            state.tracker.reset();
            state.tracker.add(timestamp(), from);
        }
        let moving = self.clone();
        let ending = self.clone();
        session.start(
            down,
            BindDragOptions::new()
                .on_move(move |_this: &mut C, sample, window, _cx| {
                    let at = moving.along(sample.x, sample.y);
                    let to = {
                        let mut state = moving.inner.state.borrow_mut();
                        let Some(drag) = state.drag.as_mut() else {
                            return;
                        };
                        if !drag.moved {
                            if (at - drag.from).abs() < f64::from(DRAG_THRESHOLD) {
                                return;
                            }
                            drag.moved = true;
                            state.scrolling = true;
                        }
                        let to = state.drag.as_ref().map_or(0.0, |drag| drag.scroll - (at - drag.from));
                        state.tracker.add(timestamp(), at);
                        state.last_event = Some(sample.event.event.clone());
                        to
                    };
                    moving.write_scroll(to);
                    window.refresh();
                })
                .on_end(move |this: &mut C, event, canceled, window, cx| {
                    ending.release(this, event, canceled, window, cx);
                }),
            window,
            cx,
        );
    }

    fn release(&self, this: &mut C, event: &PointerEvent, canceled: bool, window: &mut Window, cx: &mut Context<C>) {
        let moved = {
            let mut state = self.inner.state.borrow_mut();
            let moved = state.drag.take().is_some_and(|drag| drag.moved);
            if moved {
                state.scrolling = false;
            }
            moved
        };
        if !moved {
            return;
        }
        // The click a drag ends with isn't a press on an option.
        swallow_next_click(false, window, cx);
        let velocity = if canceled {
            0.0
        } else {
            self.inner.state.borrow_mut().tracker.velocity(timestamp())
        };
        let coast = clamp(
            self.scroll() - project_distance(velocity, DECELERATION_RATE),
            0.0,
            self.max_scroll(),
        );
        let at = {
            let state = self.inner.state.borrow();
            picker_position(&state.centers, coast + self.view_size() / 2.0)
        };
        self.settle_at(this, at, Some(event.event.clone()), window, cx);
    }

    fn keydown(&self, this: &mut C, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<C>) {
        let direction: isize = match Key::of(&event.keystroke) {
            Key::PageDown => 1,
            Key::PageUp => -1,
            _ => 0,
        };
        let (count, spacing, from) = {
            let state = self.inner.state.borrow();
            let count = state.options.len();
            if direction == 0 || window.default_prevented() || count < 2 {
                return;
            }
            let spacing = (state.centers[count - 1] - state.centers[0]) / (count - 1) as f64;
            (count, spacing, state.selected)
        };
        window.prevent_default();
        let page = if spacing > 0.0 {
            ((self.view_size() / spacing).round() as isize - 1).max(1)
        } else {
            1
        };
        let from = from.map_or_else(|| self.position().round() as isize, |selected| selected as isize);
        let to = (from + direction * page).clamp(0, count as isize - 1);
        self.settle_at(this, to as f64, Some(InputEvent::from(event)), window, cx);
    }

    /// Undoes the binding: the list's Page Up and Page Down are its own
    /// again, and the picker stops gliding and waiting.
    pub fn unbind(&self) {
        set_paged_elsewhere(&self.inner.list, false);
        let mut state = self.inner.state.borrow_mut();
        state.bound = false;
        state.rest = None;
        state.glide = None;
        state.target = None;
    }
}

// What the probes tell the picker while gpui draws.
trait PickerProbe {
    fn laid_out(&self, index: usize, bounds: Bounds<Pixels>);
    fn frame(&self, window: &mut Window, cx: &mut App);
}

impl<C: 'static> PickerProbe for PickerInner<C> {
    fn laid_out(&self, index: usize, bounds: Bounds<Pixels>) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.bounds_next.push((index, bounds));
        }
    }

    fn frame(&self, window: &mut Window, cx: &mut App) {
        let Ok(mut state) = self.state.try_borrow_mut() else {
            return;
        };
        if std::mem::replace(&mut state.checking, true) {
            return;
        }
        drop(state);
        let Some(inner) = self.me.upgrade() else {
            return;
        };
        let picker = ScrollPicker { inner };
        window.defer(cx, move |window, cx| picker.check(window, cx));
    }
}

// What `track` puts in the list's element: it takes no space and draws
// nothing, and asks the picker to look once the frame is drawn.
struct FrameProbe {
    picker: Weak<dyn PickerProbe>,
    style: StyleRefinement,
}

// What `option` puts in an option's element: it reports where the option
// laid out.
struct OptionProbe {
    index: usize,
    picker: Weak<dyn PickerProbe>,
    style: StyleRefinement,
}

impl Styled for FrameProbe {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Styled for OptionProbe {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

fn probe_layout(style: &StyleRefinement, window: &mut Window, cx: &mut App) -> LayoutId {
    let mut resolved = Style::default();
    resolved.refine(style);
    window.request_layout(resolved, [], cx)
}

impl Element for FrameProbe {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (probe_layout(&self.style, window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        if let Some(picker) = self.picker.upgrade() {
            picker.frame(window, cx);
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}

impl Element for OptionProbe {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (probe_layout(&self.style, window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        if let Some(picker) = self.picker.upgrade() {
            picker.laid_out(self.index, bounds);
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}
