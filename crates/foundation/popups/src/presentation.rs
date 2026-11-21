//! Port of `foundation/popups/presentation.ts`: how a popup presents: its
//! own way, or as a sheet from the bottom of the screen with the Drawer's
//! motion (`presentation: Sheet`, or `Auto` on phones).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{App, Context, FocusHandle, Window, px};
use ui_sdk_kernel::InputEvent;
use ui_sdk_motion::SnapPoint;
use ui_sdk_platform::{Region, is_ios};

use crate::dismissal::SheetDismissal;
use crate::popup::{AttachConfig, PopupLayer, PopupOpener};
use crate::sheet::Sheet;
use crate::sheet_presentation::present_sheet;

/// How a popup can present (`PresentationSetting`). `Auto` picks the sheet
/// on small touch screens; `Default` is the component's own way, like a
/// popover's or a dialog's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PresentationSetting {
    Auto,
    #[default]
    Default,
    Sheet,
}

impl PresentationSetting {
    /// The original's name for it, with the component's own way as
    /// `default`, like `"popover"`.
    pub fn as_str(self, default: &'static str) -> &'static str {
        match self {
            PresentationSetting::Auto => "auto",
            PresentationSetting::Default => default,
            PresentationSetting::Sheet => "sheet",
        }
    }
}

/// How a popup presents while open, decided as it opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Presentation {
    #[default]
    Popover,
    Dialog,
    Sheet,
}

impl Presentation {
    /// The original's name for it, its `data-presentation`.
    pub fn as_str(self) -> &'static str {
        match self {
            Presentation::Popover => "popover",
            Presentation::Dialog => "dialog",
            Presentation::Sheet => "sheet",
        }
    }
}

/// Hears the sheet a popup presents as once it's attached, and `None` once
/// it's gone, for a popup that moves it, like a dialog that nudges it.
pub type OnSheet<T> = Rc<dyn Fn(Option<Sheet<T>>)>;

/// For a popup inside a sheet, like a submenu: what it shows in place of,
/// the content of the popup it's in, and the trigger that stays above it.
pub type DrillIn = Rc<dyn Fn() -> (Option<Region>, Option<Region>)>;

/// What a popup tells the sheet it presents as, beside its popup config
/// (`SheetConfig`).
pub struct SheetConfig<T: 'static> {
    pub attach: AttachConfig<T>,
    /// Where the sheet can rest. Defaults to its own size.
    pub snap_points: Option<Vec<SnapPoint>>,
    /// Whether a swipe can dismiss it. Defaults to true.
    pub dismissible: Option<Rc<dyn Fn() -> bool>>,
    /// A swipe tried to dismiss it while `dismissible` is false.
    pub on_dismiss_attempt: Option<Rc<dyn Fn(Option<&InputEvent>, &mut Window, &mut App)>>,
    /// Hears the sheet once it's attached, and `None` once it's gone.
    pub on_sheet: Option<OnSheet<T>>,
    /// Shows it in place of the popup it's in, which keeps only the way to
    /// it.
    pub drill_in: Option<DrillIn>,
}

impl<T: 'static> Default for SheetConfig<T> {
    fn default() -> Self {
        Self {
            attach: AttachConfig::default(),
            snap_points: None,
            dismissible: None,
            on_dismiss_attempt: None,
            on_sheet: None,
            drill_in: None,
        }
    }
}

impl<T: 'static> Clone for SheetConfig<T> {
    fn clone(&self) -> Self {
        Self {
            attach: self.attach.clone(),
            snap_points: self.snap_points.clone(),
            dismissible: self.dismissible.clone(),
            on_dismiss_attempt: self.on_dismiss_attempt.clone(),
            on_sheet: self.on_sheet.clone(),
            drill_in: self.drill_in.clone(),
        }
    }
}

impl<T: 'static> SheetConfig<T> {
    /// `patch` over this: its fields where it sets them.
    pub fn merge(mut self, patch: SheetConfig<T>) -> Self {
        let attach = patch.attach;
        if attach.position.is_some() {
            self.attach.position = attach.position;
        }
        if attach.anchor.is_some() {
            self.attach.anchor = attach.anchor;
        }
        if attach.initial_focus.is_some() {
            self.attach.initial_focus = attach.initial_focus;
        }
        if attach.backdrop.is_some() {
            self.attach.backdrop = attach.backdrop;
        }
        if attach.host.is_some() {
            self.attach.host = attach.host;
        }
        self.attach.inline |= attach.inline;
        if attach.on_show.is_some() {
            self.attach.on_show = attach.on_show;
        }
        if attach.on_close.is_some() {
            self.attach.on_close = attach.on_close;
        }
        if patch.snap_points.is_some() {
            self.snap_points = patch.snap_points;
        }
        if patch.dismissible.is_some() {
            self.dismissible = patch.dismissible;
        }
        if patch.on_dismiss_attempt.is_some() {
            self.on_dismiss_attempt = patch.on_dismiss_attempt;
        }
        if patch.on_sheet.is_some() {
            self.on_sheet = patch.on_sheet;
        }
        if patch.drill_in.is_some() {
            self.drill_in = patch.drill_in;
        }
        self
    }
}

/// Whether the screen is a phone's: small and touched (`(max-width: 640px)
/// and (pointer: coarse)`).
pub fn is_phone(window: &Window) -> bool {
    window.viewport_size().width <= px(640.) && coarse_pointer(window)
}

/// Whether the main pointer is a finger (`(pointer: coarse)`): an iPhone or
/// iPad, or a window last used by touch.
pub fn coarse_pointer(window: &Window) -> bool {
    is_ios() || window.last_input_was_touch()
}

/// A popup's presentation, kept as its state's `presentation`
/// (`createPresentation`). Call [`open`](Self::open) as it opens, and
/// [`bind`](Self::bind) to bind its content in place of the layer's own.
/// Clones share it.
pub struct PresentationFeature<T: 'static> {
    value: Rc<Cell<Presentation>>,
    setting: Rc<dyn Fn(&T) -> Option<PresentationSetting>>,
    fallback: Presentation,
    dismissal: Option<SheetDismissal>,
    sheet: Rc<RefCell<Option<Sheet<T>>>>,
}

impl<T: 'static> Clone for PresentationFeature<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            setting: self.setting.clone(),
            fallback: self.fallback,
            dismissal: self.dismissal.clone(),
            sheet: self.sheet.clone(),
        }
    }
}

/// A popup's presentation over the `presentation` its state keeps
/// (`createPresentation`). `setting` reads how it asks to present;
/// `fallback` is its own way.
pub fn create_presentation<T: 'static>(
    value: Rc<Cell<Presentation>>,
    setting: impl Fn(&T) -> Option<PresentationSetting> + 'static,
    initial: Option<PresentationSetting>,
    fallback: Presentation,
    dismissal: Option<SheetDismissal>,
) -> PresentationFeature<T> {
    if initial == Some(PresentationSetting::Sheet) {
        value.set(Presentation::Sheet);
    }
    PresentationFeature {
        value,
        setting: Rc::new(setting),
        fallback,
        dismissal,
        sheet: Rc::new(RefCell::new(None)),
    }
}

impl<T: 'static> PresentationFeature<T> {
    /// How it presents now.
    pub fn get(&self) -> Presentation {
        self.value.get()
    }

    /// How it would present if it opened now.
    pub fn resolve(&self, this: &T, window: &Window) -> Presentation {
        match (self.setting)(this) {
            Some(PresentationSetting::Sheet) => Presentation::Sheet,
            Some(PresentationSetting::Auto) if is_phone(window) => Presentation::Sheet,
            _ => self.fallback,
        }
    }

    /// The sheet it presents as, while it does.
    pub fn sheet(&self) -> Option<Sheet<T>> {
        self.sheet.borrow().clone()
    }

    /// Decides how it presents, as it opens.
    pub fn open(&self, this: &T, window: &Window) {
        self.value.set(self.resolve(this, window));
    }

    /// Binds the content in place of the layer's own `bind`: as a sheet
    /// while it presents as one.
    pub fn bind(
        &self,
        this: &mut T,
        layer: &PopupLayer<T>,
        el: Region,
        config: SheetConfig<T>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        // Open from the start, it rendered its own way first. Once it's
        // bound, `Auto` decides.
        if (self.setting)(this) == Some(PresentationSetting::Auto)
            && layer.opener() == PopupOpener::Initial
            && layer.is_open()
        {
            self.open(this, window);
        }
        if self.value.get() != Presentation::Sheet {
            return layer.bind(this, el, config.attach, window, cx);
        }
        let mut config = config;
        if let Some(dismissal) = &self.dismissal {
            if config.dismissible.is_none() {
                config.dismissible = Some(dismissal.dismissible.clone());
            }
            if config.on_dismiss_attempt.is_none() {
                config.on_dismiss_attempt = Some(dismissal.on_dismiss_attempt.clone());
            }
        }
        let slot = self.sheet.clone();
        let heard = config.on_sheet.clone();
        config.on_sheet = Some(Rc::new(move |sheet: Option<Sheet<T>>| {
            *slot.borrow_mut() = sheet.clone();
            if let Some(heard) = &heard {
                heard(sheet);
            }
        }));
        present_sheet(this, layer, el, config, window, cx);
    }
}

/// The `modal` setting a popup uses: the app's, else modal while it
/// presents as a sheet, like iOS's sheets, else the component's default
/// (`sheetModal`).
pub fn sheet_modal<S>(presentation: Presentation, setting: Option<S>, fallback: S, sheet: S) -> S {
    setting.unwrap_or(if presentation == Presentation::Sheet {
        sheet
    } else {
        fallback
    })
}

/// Where a list with an input rests as a sheet: at most half the screen, and
/// its full size once someone types (`sheetSnapPoints`).
pub fn sheet_snap_points() -> Vec<SnapPoint> {
    vec![SnapPoint::Number(0.5), SnapPoint::Number(1.0)]
}

/// A text input that opens a popup that may present as a sheet, like a
/// combobox's, which moves into the sheet's header while it's open
/// (`bindSheetInput`). A tap on it that opens a sheet doesn't focus it, so
/// no keyboard opens and focus goes to the sheet. Moving into the header and
/// back renders the input again, so it remembers it held focus until focus
/// went somewhere else, and takes it back where it lands. A finger's
/// keyboard stays closed. Clones share it.
#[derive(Clone, Default)]
pub struct SheetInput {
    held: Rc<Cell<bool>>,
}

/// The input's side of a sheet: make it once and keep it in a field.
pub fn bind_sheet_input() -> SheetInput {
    SheetInput::default()
}

impl SheetInput {
    /// A tap on the input. While the popup is closed and would present as a
    /// sheet, it opens it instead of focusing the input: returns true, and
    /// the caller opens the popup and keeps the tap from focusing the input
    /// with `window.prevent_default()`.
    pub fn tap(&self, open: bool, resolves_sheet: bool) -> bool {
        !open && resolves_sheet
    }

    /// The input took focus.
    pub fn focused(&self) {
        self.held.set(true);
    }

    /// The input lost focus, to `next` when something else took it.
    pub fn blurred(&self, next: Option<&FocusHandle>) {
        if next.is_some() {
            self.held.set(false);
        }
    }

    /// The input rendered again, like moved into the sheet's header: it
    /// takes focus back if it held it, unless the last input was a touch.
    /// Call it from an event handler or a deferred callback, with the input's
    /// focus handle.
    pub fn rendered(&self, handle: &FocusHandle, presentation: Presentation, window: &mut Window, cx: &mut App) {
        let held = self.held.replace(false);
        if held && presentation == Presentation::Sheet && !window.last_input_was_touch() && handle.is_rendered(window) {
            window.focus(handle, cx);
        }
        if handle.is_focused(window) {
            self.held.set(true);
        }
    }
}
