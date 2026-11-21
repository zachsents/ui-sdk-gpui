//! Port of `foundation/platform/bind-layer.ts`: any element as a layer in the
//! stack our popups share, for a popup of a shape ours don't take, like a
//! ring of actions centered on the pointer.

use std::rc::Rc;

use gpui::{App, ParentElement, Styled, Window};
use ui_sdk_kernel::modality::window_source;
use ui_sdk_kernel::{DismissReason, InputEvent, Reason};

use crate::layers::{Layer, LayerOptions, PushedLayer, listener, push_layer};
use crate::platform::{Region, Target};

/// Why a layer of your own is asked to close.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayerDismissReason {
    EscapeKey,
    OutsidePress,
    WindowBlur,
}

impl LayerDismissReason {
    /// The original's name for it, like `"escape-key"`.
    pub fn as_str(self) -> &'static str {
        match self {
            LayerDismissReason::EscapeKey => "escape-key",
            LayerDismissReason::OutsidePress => "outside-press",
            LayerDismissReason::WindowBlur => "window-blur",
        }
    }

    /// The same reason in the shared vocabulary, for change details.
    pub fn reason(self) -> Reason {
        self.dismiss_reason().reason()
    }

    /// The same reason as the kernel's dismissals name it.
    pub fn dismiss_reason(self) -> DismissReason {
        match self {
            LayerDismissReason::EscapeKey => DismissReason::EscapeKey,
            LayerDismissReason::OutsidePress => DismissReason::OutsidePress,
            LayerDismissReason::WindowBlur => DismissReason::WindowBlur,
        }
    }
}

impl From<LayerDismissReason> for DismissReason {
    fn from(reason: LayerDismissReason) -> Self {
        reason.dismiss_reason()
    }
}

/// Hears a layer of your own asked to close, with what asked.
pub type DismissCallback = Rc<dyn Fn(LayerDismissReason, &InputEvent, &mut Window, &mut App)>;

/// Whether a target counts as inside a layer of your own, beside its
/// element.
pub type BindLayerContains = Rc<dyn Fn(&Target, &Window, &App) -> bool>;

/// What [`bind_layer`] takes.
#[derive(Clone, Default)]
pub struct BindLayerOptions {
    /// Runs when Escape, a press outside or the window losing focus asks the
    /// layer to close.
    pub on_dismiss: Option<DismissCallback>,
    /// Other targets that count as inside, like the trigger that opened it.
    pub contains: Option<BindLayerContains>,
    /// Whether a press outside only closes the layer, without reaching the
    /// window beneath. Defaults to `false`.
    pub modal: bool,
    /// Whether the window losing focus closes it, like a click into another
    /// window. Defaults to `false`.
    pub close_on_blur: bool,
}

impl BindLayerOptions {
    pub fn on_dismiss(
        mut self,
        on_dismiss: impl Fn(LayerDismissReason, &InputEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }

    pub fn contains(mut self, contains: impl Fn(&Target, &Window, &App) -> bool + 'static) -> Self {
        self.contains = Some(Rc::new(contains));
        self
    }

    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    pub fn close_on_blur(mut self, close_on_blur: bool) -> Self {
        self.close_on_blur = close_on_blur;
        self
    }
}

/// An element in the layer stack, until unbound or dropped.
#[must_use = "dropping it takes the layer off the stack"]
pub struct BoundLayer {
    region: Region,
    pushed: PushedLayer,
}

impl BoundLayer {
    /// The layer it pushed.
    pub fn layer(&self) -> &Layer {
        self.pushed.layer()
    }

    /// Adds what the stack needs to the bound element: its region's probe,
    /// and the listener presses reach the stack through. Call it on the
    /// element every frame it renders.
    pub fn measure<E: ParentElement + Styled>(&self, el: E) -> E {
        self.region.measure(el).child(listener())
    }

    /// Takes the layer off the stack, the original's returned cleanup.
    pub fn unbind(self) {}
}

impl Drop for BoundLayer {
    fn drop(&mut self) {
        self.pushed.layer().set_active(false);
        self.pushed.pop();
    }
}

/// Makes an element a layer in the stack our popups share, until the
/// returned guard drops. Escape goes to the topmost layer, so a menu opened
/// from inside yours closes first; a press outside every layer in a tree
/// closes them from the top; and modal ones keep presses from reaching the
/// window. `region` is the element's, which [`BoundLayer::measure`] measures.
///
/// The original can move the element into the browser's top layer, over
/// dialogs and clipping. In gpui, render it in `deferred(..)` at
/// [`LAYER_PRIORITY`](crate::LAYER_PRIORITY) and position it yourself.
pub fn bind_layer(
    region: Region,
    options: BindLayerOptions,
    window: &mut Window,
    cx: &mut App,
) -> BoundLayer {
    let contains = {
        let region = region.clone();
        let others = options.contains.clone();
        move |target: &Target, _event: Option<&InputEvent>, window: &Window, cx: &App| {
            region.contains(target, window)
                || others.as_ref().is_some_and(|others| others(target, window, cx))
        }
    };
    let on_dismiss = options.on_dismiss.clone();
    let escape = on_dismiss.clone();
    let outside = on_dismiss.clone();
    let close_on_blur = options.close_on_blur;
    let layer = Layer::new(
        LayerOptions::default()
            .contains(contains)
            .modal(options.modal)
            .component("layer")
            .on_escape(move |event, window, cx| {
                window.prevent_default();
                if let Some(escape) = &escape {
                    escape(
                        LayerDismissReason::EscapeKey,
                        &InputEvent::KeyDown(event.clone()),
                        window,
                        cx,
                    );
                }
            })
            .on_outside_press(move |press, window, cx| {
                if let Some(outside) = &outside {
                    outside(LayerDismissReason::OutsidePress, &press.event, window, cx);
                }
            })
            .on_blur(move |window, cx| {
                if !close_on_blur {
                    return;
                }
                if let Some(on_dismiss) = &on_dismiss {
                    let event = InputEvent::Focus(window_source(window));
                    on_dismiss(LayerDismissReason::WindowBlur, &event, window, cx);
                }
            }),
    );
    let pushed = push_layer(layer, window, cx);
    BoundLayer { region, pushed }
}
