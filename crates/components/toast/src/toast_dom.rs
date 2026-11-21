//! Port of `components/toast/toast-dom.ts`: the window side of a toast
//! region: where it sits by default, the stack's measurements, swipe
//! directions, where focus goes when a focused toast closes, and where the
//! user's attention is. Decisions are pure; the region calls them as it binds
//! and draws.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    Context, Div, Edges, FocusHandle, IntoElement, ParentElement, Pixels, SharedString, Size, Styled,
    Subscription, Window, div, px,
};
use ui_sdk_kernel::BoundPart;
use ui_sdk_kernel::focus_events::{on_focus_in_from, on_focus_out};

use crate::swipe::SwipeDirection;
use crate::toast::{ToastAlign, ToastSide};

/// Lays a region out where the original's default styles put it: at its side
/// of the window and its place along that edge, above the on-screen keyboard
/// and inside the safe area, no wider than the window (`addToastStyles`). The
/// app's own styles on the region come after and win, as the original's
/// defaults sit in the lowest cascade layer. `start` and `end` follow the
/// writing direction; the start is kept clear of the left safe area and the
/// end of the right one, as the original's `inset-inline-*` are.
pub fn place_region(
    region: impl IntoElement,
    side: ToastSide,
    align: ToastAlign,
    rtl: bool,
    keyboard_inset: Pixels,
    safe_area: Edges<Pixels>,
) -> Div {
    let frame = div().size_full().flex().flex_col();
    let frame = match side {
        ToastSide::Bottom => frame.justify_end().pb(keyboard_inset + safe_area.bottom),
        ToastSide::Top => frame.justify_start().pt(safe_area.top),
    };
    // Flex start is the left in this column, whichever way text runs.
    let frame = match (align, rtl) {
        (ToastAlign::Center, _) => frame.items_center(),
        (ToastAlign::Start, false) => frame.items_start().pl(safe_area.left),
        (ToastAlign::Start, true) => frame.items_end().pr(safe_area.left),
        (ToastAlign::End, false) => frame.items_end().pr(safe_area.right),
        (ToastAlign::End, true) => frame.items_start().pl(safe_area.right),
    };
    frame.child(region)
}

/// The region's own defaults: a column, no wider than the window, as the
/// original's `display: flex; flex-direction: column; max-width: 100vw`.
/// Only what the app hasn't set itself.
pub(crate) fn region_defaults<E: Styled>(mut el: E, viewport: Size<Pixels>) -> E {
    let style = el.style();
    let display = style.display.is_none();
    let direction = style.flex_direction.is_none();
    let max_width = style.max_size.width.is_none();
    if display {
        el = el.flex();
    }
    if direction {
        el = el.flex_col();
    }
    if max_width {
        el = el.max_w(viewport.width);
    }
    el
}

/// Where a toast can be swiped to dismiss it: toward the edge its region sits
/// on, and toward the side it's aligned to. `start` and `end` follow the
/// writing direction.
pub fn swipe_directions(side: ToastSide, align: ToastAlign, rtl: bool) -> Vec<SwipeDirection> {
    let mut directions = vec![match side {
        ToastSide::Top => SwipeDirection::Up,
        ToastSide::Bottom => SwipeDirection::Down,
    }];
    match align {
        ToastAlign::End => directions.push(if rtl {
            SwipeDirection::Left
        } else {
            SwipeDirection::Right
        }),
        ToastAlign::Start => directions.push(if rtl {
            SwipeDirection::Right
        } else {
            SwipeDirection::Left
        }),
        ToastAlign::Center => {}
    }
    directions
}

/// A shown toast, as [`stack_layout`] places it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackToast {
    pub id: SharedString,
    pub index: usize,
    pub leaving: bool,
}

/// One toast's place in the stack: its position, 0 for the newest, and how
/// far the toasts in front of it reach (`--toast-index`, `--toast-offset`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToastPlace {
    pub index: usize,
    pub offset: Pixels,
}

/// The stack's measurements, from [`stack_layout`].
#[derive(Clone, Debug, PartialEq)]
pub struct StackLayout {
    /// Each toast that isn't leaving, by id.
    pub layout: HashMap<SharedString, ToastPlace>,
    /// Every one's height added up (`--toast-stack-height`).
    pub total: Pixels,
    /// The newest one's height (`--toast-front-height`): `None` while it
    /// isn't measured, 0 with no toast in front.
    pub front: Option<Pixels>,
}

/// The stacking measurements for each shown toast, newest first: its position
/// (`index`), and how far the toasts in front of it reach when the stack is
/// expanded (`offset`, the sum of their heights). Closing toasts keep the
/// place they had, so the rest close up around them.
pub fn stack_layout(toasts: &[StackToast], heights: &HashMap<SharedString, Pixels>) -> StackLayout {
    let mut offset = px(0.);
    let mut total = px(0.);
    let mut layout = HashMap::new();
    for toast in toasts {
        if toast.leaving {
            continue;
        }
        layout.insert(
            toast.id.clone(),
            ToastPlace {
                index: toast.index,
                offset,
            },
        );
        let height = heights.get(&toast.id).copied().unwrap_or(px(0.));
        offset += height;
        total += height;
    }
    let front = match toasts.iter().find(|toast| !toast.leaving) {
        Some(front) => heights.get(&front.id).copied(),
        None => Some(px(0.)),
    };
    StackLayout {
        layout,
        total,
        front,
    }
}

/// A toast's border-box height as it last laid out (`naturalHeight`), from
/// the bounds the kernel records for its root part, among an instance's
/// `bound_parts`. The original lays the toast out again with `height: auto`
/// to read its content's height whatever height the stack gives it; gpui
/// can't lay an element out twice, so the region measures a toast only while
/// the stack leaves its height to its content: in front, or with the stack
/// expanded.
pub fn natural_height(parts: &[BoundPart], id: &str) -> Option<Pixels> {
    parts
        .iter()
        .find(|part| part.slot == "root" && part.key.as_deref() == Some(id))
        .and_then(|part| part.bounds)
        .map(|bounds| bounds.size.height)
}

/// Where focus goes when the focused toast closes: the next toast, else the
/// one before it, else back where it was before it entered the region. Never
/// nowhere, while there's somewhere to go.
pub fn focus_after_close<T: Clone + PartialEq>(
    toasts: &[T],
    closing: &T,
    fallback: Option<T>,
) -> Option<T> {
    let index = toasts.iter().position(|toast| toast == closing);
    let rest: Vec<&T> = toasts.iter().filter(|toast| *toast != closing).collect();
    if rest.is_empty() {
        return fallback;
    }
    let at = index.unwrap_or(0).min(rest.len() - 1);
    Some(rest[at].clone())
}

/// The ways the window or the page goes away (`"window-blur"`,
/// `"page-hide"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Away {
    /// The window lost focus, like a click into another window.
    WindowBlur,
    /// The window was hidden: minimized, fully covered, or on a sleeping
    /// display.
    PageHide,
}

/// Focus came into the region or left it. `from` is where it came from,
/// outside the region, when it entered.
pub type FocusReport<T> =
    Rc<dyn Fn(&mut T, bool, Option<FocusHandle>, &mut Window, &mut Context<T>)>;

/// The window or the page went away, or came back.
pub type AwayReport<T> = Rc<dyn Fn(&mut T, Away, bool, &mut Window, &mut Context<T>)>;

/// What [`watch_attention`] reports, inside the owner's update. Hover is the
/// region's own, from the pointer over its parts as they paint.
pub struct AttentionReport<T: 'static> {
    pub focus: FocusReport<T>,
    pub away: AwayReport<T>,
}

/// Follows where the user's attention is for a region, until dropped: focus
/// in it, with where focus came from, and the window or page going away
/// (`watchAttention`). A window that's hidden from the start reports it at
/// once.
pub struct Attention {
    _subscriptions: Vec<Subscription>,
}

/// Starts following focus in the element `region` tracks, and the window.
pub fn watch_attention<T: 'static>(
    this: &mut T,
    region: &FocusHandle,
    report: AttentionReport<T>,
    window: &mut Window,
    cx: &mut Context<T>,
) -> Attention {
    let mut subscriptions = Vec::new();
    {
        let focus = report.focus.clone();
        let inside = region.clone();
        subscriptions.push(on_focus_in_from(region, window, cx, move |this, change, window, cx| {
            // Where focus came from (`relatedTarget`), when that's outside.
            let came_from = change
                .previous
                .clone()
                .filter(|from| !inside.contains(from, window));
            focus(this, true, came_from, window, cx);
        }));
    }
    {
        let focus = report.focus.clone();
        subscriptions.push(on_focus_out(
            region,
            window,
            cx,
            move |this, _event, window, cx| {
                focus(this, false, None, window, cx);
            },
        ));
    }
    {
        let away = report.away.clone();
        subscriptions.push(
            cx.observe_window_activation(window, move |this, window, cx| {
                let blurred = !window.is_window_active();
                away(this, Away::WindowBlur, blurred, window, cx);
            }),
        );
    }
    {
        let away = report.away.clone();
        subscriptions.push(cx.observe_window_visibility(
            window,
            move |this, visibility, window, cx| {
                away(this, Away::PageHide, !visibility.is_visible(), window, cx);
            },
        ));
    }
    if !window.is_visible() {
        (report.away)(this, Away::PageHide, true, window, cx);
    }
    Attention {
        _subscriptions: subscriptions,
    }
}
