//! Port of `foundation/kernel/details.ts`: why and how a change happened, the
//! shared reason vocabulary, the details every `on_*_change` receives, and the
//! source of a change no event explains. `input_source.rs` reads the source of
//! the ones events do.

use std::cell::{Cell, RefCell};

use gpui::{
    BackGestureEvent, ClickEvent, FileDropEvent, KeyDownEvent, KeyUpEvent, LongPressEvent,
    Modifiers, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PinchEvent, ScrollEvent,
    TouchClickEvent, TouchDragEvent, TouchEvent, Window,
};

/// How the user caused a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChangeSource {
    Mouse,
    Touch,
    Pen,
    Keyboard,
    /// A click with no pointer or key behind it, like assistive technology's.
    Virtual,
    Programmatic,
    /// An AI agent's call through the agent bridge.
    Agent,
}

impl ChangeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mouse => "mouse",
            Self::Touch => "touch",
            Self::Pen => "pen",
            Self::Keyboard => "keyboard",
            Self::Virtual => "virtual",
            Self::Programmatic => "programmatic",
            Self::Agent => "agent",
        }
    }
}

/// Every reason a change can report, and what it means (`ReasonVocabulary` in
/// the original). Components report a subset, documented on each
/// `on_*_change` option. Controllers and cascades have no event, so
/// `ChangeDetails::event` is always optional.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reason {
    /// Click, tap, Enter or Space on the part that opens, closes or toggles.
    /// Arrow keys that open a menu or submenu count too. A tap on a
    /// combobox's input that opens a sheet reports its touch.
    TriggerPress,
    /// The pointer rested on a trigger, or left it, with hover opening.
    TriggerHover,
    /// Keyboard focus reached a trigger that opens on focus, like a tooltip's.
    TriggerFocus,
    /// A member of a collection was chosen: a menu item, a tab, a radio or a checkbox item.
    ItemPress,
    /// A Close part was pressed, or a form that closes its dialog was submitted.
    ClosePress,
    /// A single-element control was pressed: a checkbox, a switch or a toggle.
    Press,
    /// Arrow, Home or End keys moved the selection, typing picked an item, or
    /// Ctrl or ⌘ with A selected every item of a listbox.
    ListNavigation,
    /// A link was pressed, which closes the navigation menu it is in, like after a route change.
    LinkPress,
    /// The pointer moved onto an item, which highlighted it, like in a command list.
    ItemHover,
    /// A press outside the layer.
    OutsidePress,
    /// Escape, or a submenu's close key (ArrowLeft in left-to-right layouts).
    EscapeKey,
    /// Focus left the layer, or Tab moved it out.
    FocusOut,
    /// Keyboard focus moved into the component, which stops a rotating
    /// carousel, as the carousel pattern asks, or onto a canvas item, which
    /// brought it into view, or onto a calendar's day another way, like a
    /// screen reader's.
    FocusIn,
    /// The window lost focus.
    WindowBlur,
    /// Another submenu in the same menu, or another menu in the same menubar, took over.
    SubmenuSwitch,
    /// The parent layer closed.
    ParentClose,
    /// A press-and-hold open was released outside the popup.
    TriggerRelease,
    /// The browser's find in page matched hidden content. Kept for the
    /// vocabulary; gpui has no find in page.
    FindInPage,
    /// The selected item left the page, and another was picked.
    ItemRemoved,
    /// A form reset.
    Reset,
    /// State was restored, like a saved value read back from storage.
    Restore,
    /// Another window, or another storage with the same id, saved a new value.
    StorageSync,
    /// A swipe gesture: a finger, mouse or pen dragged it.
    Swipe,
    /// The user scrolled a carousel some other way: a wheel, a trackpad, a
    /// scrollbar, or focus brought into view.
    Scroll,
    /// A drawer's handle was pressed, which moved it to its next snap point.
    HandlePress,
    /// Arrow keys, Home or End on a drawer's handle moved it between snap points.
    HandleKey,
    /// The content or the window changed size, and the drawer's snap point or
    /// the carousel's page went away, so it moved to the nearest one.
    ContentResize,
    /// Going back: a back gesture, or Backspace in an empty command input,
    /// which returns to the previous page.
    Back,
    /// A controller or api call.
    Imperative,
    /// A keyboard shortcut, like a trigger's `shortcut`.
    Shortcut,
    /// A duration ran out, like a toast's, or a rotating carousel's interval.
    Timeout,
    /// A Close part's `action` finished.
    Action,
    /// The component left the page while open. Only a controller's `open()` reports it.
    Unmount,
    /// A Clear part was pressed, like on an input, a select or a combobox.
    ClearPress,
    /// Typing, pasting or dropping changed a text field's value; in a combobox
    /// it can also open or close it, or Enter keeps the typed text as the value.
    InputChange,
    /// ArrowUp or ArrowDown brought back an earlier value with an input
    /// history, or what was being typed.
    HistoryKey,
    /// A chip's remove part was pressed, or Backspace or Delete removed a chip or a span.
    RemovePress,
    /// Enter, or leaving a number input, committed what was typed: a number
    /// input parses, clamps and snaps it to the step, and an autocomplete
    /// closes its suggestions on it.
    InputCommit,
    /// Arrow, Page Up, Page Down, Home or End stepped a slider or a number input.
    StepKey,
    /// A press on a slider's track or an area's surface moved the nearest thumb there.
    TrackPress,
    /// A slider thumb or a resize handle was dragged, or an item was dragged
    /// to a new place in a list.
    Drag,
    /// Space or Enter dropped a list item that arrow keys, Home or End moved
    /// to a new place, or an arrow moved or resized a span.
    MoveKey,
    /// Enter or a double-click on a resize handle collapsed the panel beside it, or restored it.
    Collapse,
    /// The group changed size, so panels with sizes in pixels moved to stay within their bounds.
    Resize,
    /// A number input's increment button was pressed, or held.
    IncrementPress,
    /// A number input's decrement button was pressed, or held.
    DecrementPress,
    /// The mouse wheel stepped a focused number input.
    Wheel,
    /// A drag on a number input's scrub area.
    Scrub,
    /// A screen reader's adjust gesture, like a swipe up or down on a slider
    /// (accesskit's `Increment`, `Decrement` and `SetValue` actions).
    AccessibilityAdjust,
    /// Files were dropped on a drop zone, or text and tokens on a token input.
    Drop,
    /// Files were pasted, like a screenshot, or text and tokens into a token input.
    Paste,
    /// Files were chosen in the system's file picker.
    Picker,
    /// The caret left a trigger menu's match: an arrow key, Home or End, a
    /// click, or a selection change.
    CaretMove,
    /// A selection was cut from a token input.
    Cut,
    /// Undo, from the keyboard, the Edit menu or a controller.
    Undo,
    /// Redo, from the keyboard, the Edit menu or a controller.
    Redo,
    /// An image loaded. No event when it was already cached.
    ImageLoad,
    /// An image failed to load, or has no source. No event when that was known at once.
    ImageError,
    /// An image got a new source, or left the page, so its status starts over.
    SrcChange,
    /// A toggle was pressed: a password input's, which shows or hides the
    /// password, or a carousel's, which starts or stops rotation.
    TogglePress,
    /// The form was submitted, which hides a shown password so it can be saved.
    FormSubmit,
    /// The window was hidden, which hides a shown password.
    PageHide,
    /// A pagination's or a carousel's Previous part was pressed.
    PreviousPress,
    /// A pagination's or a carousel's Next part was pressed.
    NextPress,
    /// A pagination's First part was pressed.
    FirstPress,
    /// A pagination's Last part was pressed.
    LastPress,
    AspectRatioChange,
    ConnectKey,
    DoubleClick,
    EditKey,
    EnterKey,
    EyeDropperPick,
    FlipPress,
    Lasso,
    Pinch,
    PresetPress,
    ResizeKey,
    RotateKey,
    RotatePress,
    SelectChange,
    TabKey,
    Type,
    ZoomInput,
    ZoomKey,
}

/// A reason from the shared vocabulary.
pub type ChangeReason = Reason;

impl Reason {
    /// The kebab-case name the original reports, like `"trigger-press"`.
    pub fn as_str(self) -> &'static str {
        use Reason::*;
        match self {
            TriggerPress => "trigger-press",
            TriggerHover => "trigger-hover",
            TriggerFocus => "trigger-focus",
            ItemPress => "item-press",
            ClosePress => "close-press",
            Press => "press",
            ListNavigation => "list-navigation",
            LinkPress => "link-press",
            ItemHover => "item-hover",
            OutsidePress => "outside-press",
            EscapeKey => "escape-key",
            FocusOut => "focus-out",
            FocusIn => "focus-in",
            WindowBlur => "window-blur",
            SubmenuSwitch => "submenu-switch",
            ParentClose => "parent-close",
            TriggerRelease => "trigger-release",
            FindInPage => "find-in-page",
            ItemRemoved => "item-removed",
            Reset => "reset",
            Restore => "restore",
            StorageSync => "storage-sync",
            Swipe => "swipe",
            Scroll => "scroll",
            HandlePress => "handle-press",
            HandleKey => "handle-key",
            ContentResize => "content-resize",
            Back => "back",
            Imperative => "imperative",
            Shortcut => "shortcut",
            Timeout => "timeout",
            Action => "action",
            Unmount => "unmount",
            ClearPress => "clear-press",
            InputChange => "input-change",
            HistoryKey => "history-key",
            RemovePress => "remove-press",
            InputCommit => "input-commit",
            StepKey => "step-key",
            TrackPress => "track-press",
            Drag => "drag",
            MoveKey => "move-key",
            Collapse => "collapse",
            Resize => "resize",
            IncrementPress => "increment-press",
            DecrementPress => "decrement-press",
            Wheel => "wheel",
            Scrub => "scrub",
            AccessibilityAdjust => "accessibility-adjust",
            Drop => "drop",
            Paste => "paste",
            Picker => "picker",
            CaretMove => "caret-move",
            Cut => "cut",
            Undo => "undo",
            Redo => "redo",
            ImageLoad => "image-load",
            ImageError => "image-error",
            SrcChange => "src-change",
            TogglePress => "toggle-press",
            FormSubmit => "form-submit",
            PageHide => "page-hide",
            PreviousPress => "previous-press",
            NextPress => "next-press",
            FirstPress => "first-press",
            LastPress => "last-press",
            AspectRatioChange => "aspect-ratio-change",
            ConnectKey => "connect-key",
            DoubleClick => "double-click",
            EditKey => "edit-key",
            EnterKey => "enter-key",
            EyeDropperPick => "eye-dropper-pick",
            FlipPress => "flip-press",
            Lasso => "lasso",
            Pinch => "pinch",
            PresetPress => "preset-press",
            ResizeKey => "resize-key",
            RotateKey => "rotate-key",
            RotatePress => "rotate-press",
            SelectChange => "select-change",
            TabKey => "tab-key",
            Type => "type",
            ZoomInput => "zoom-input",
            ZoomKey => "zoom-key",
        }
    }

    /// Reasons no input causes, which always report `Programmatic` unless an
    /// agent caused them (`programmaticReasons`).
    pub fn is_programmatic(self) -> bool {
        use Reason::*;
        matches!(
            self,
            ImageLoad
                | ImageError
                | Imperative
                | ItemRemoved
                | Unmount
                | Reset
                | Restore
                | Timeout
                | Resize
                | PageHide
        )
    }

    /// Whether the reason dismissed a layer (`isDismissal`).
    pub fn is_dismissal(self) -> bool {
        DismissReason::of(self).is_some()
    }
}

/// The gpui event behind a change, standing in for the DOM event.
#[derive(Clone, Debug)]
pub enum InputEvent {
    Click(ClickEvent),
    KeyDown(KeyDownEvent),
    MouseDown(MouseDownEvent),
    MouseMove(MouseMoveEvent),
    Scroll(ScrollEvent),
    /// Focus moved. gpui's focus events carry no payload, so this keeps the
    /// source the window's input modality gave when it happened.
    Focus(ChangeSource),
    /// An accessibility action from assistive technology, like a screen
    /// reader activating a part: the original's virtual click.
    Accessibility,
    /// A release that ends an interaction, like a drag's `pointerup`.
    MouseUp(MouseUpEvent),
    /// A key released, like Space activating a button on release.
    KeyUp(KeyUpEvent),
    /// A raw touch, like a swipe's.
    Touch(TouchEvent),
    /// A touch drag an element claimed, like a drawer's swipe.
    TouchDrag(TouchDragEvent),
    /// A back gesture, which reports `Reason::Back`.
    BackGesture(BackGestureEvent),
    /// Files dropped on the window.
    FileDrop(FileDropEvent),
    /// A trackpad pinch, like zooming a canvas.
    Pinch(PinchEvent),
    /// A touch held in place, like a press that opens a menu.
    LongPress(LongPressEvent),
    /// A finger's press, which gpui hands over as a mouse press as a tap
    /// ends ([`InputEvent::mouse_down`]).
    TapDown(MouseDownEvent),
    /// A finger's release, which gpui hands over as a mouse release
    /// ([`InputEvent::mouse_up`]).
    TapUp(MouseUpEvent),
}

impl InputEvent {
    /// The source this event implies (`sourceOf` in `kernel/modality.ts`).
    pub fn source(&self) -> ChangeSource {
        match self {
            InputEvent::Click(ClickEvent::Mouse(_)) => ChangeSource::Mouse,
            InputEvent::Click(ClickEvent::Touch(_)) => ChangeSource::Touch,
            InputEvent::Click(ClickEvent::Keyboard(_)) => ChangeSource::Keyboard,
            InputEvent::KeyDown(_) | InputEvent::KeyUp(_) => ChangeSource::Keyboard,
            InputEvent::MouseDown(_)
            | InputEvent::MouseMove(_)
            | InputEvent::MouseUp(_)
            | InputEvent::Scroll(_)
            | InputEvent::FileDrop(_)
            | InputEvent::Pinch(_) => ChangeSource::Mouse,
            InputEvent::Touch(_)
            | InputEvent::TouchDrag(_)
            | InputEvent::BackGesture(_)
            | InputEvent::LongPress(_)
            | InputEvent::TapDown(_)
            | InputEvent::TapUp(_) => ChangeSource::Touch,
            InputEvent::Focus(source) => *source,
            InputEvent::Accessibility => ChangeSource::Virtual,
        }
    }

    /// The modifier keys held with the event; none for touches, focus and
    /// assistive technology.
    pub fn modifiers(&self) -> Modifiers {
        match self {
            InputEvent::Click(event) => event.modifiers(),
            InputEvent::KeyDown(event) => event.keystroke.modifiers,
            InputEvent::KeyUp(event) => event.keystroke.modifiers,
            InputEvent::MouseDown(event) => event.modifiers,
            InputEvent::MouseMove(event) => event.modifiers,
            InputEvent::MouseUp(event) => event.modifiers,
            InputEvent::Scroll(event) => event.modifiers,
            InputEvent::Pinch(event) => event.modifiers,
            _ => Modifiers::default(),
        }
    }

    pub fn key(&self) -> Option<&str> {
        match self {
            InputEvent::KeyDown(event) => Some(event.keystroke.key.as_str()),
            InputEvent::KeyUp(event) => Some(event.keystroke.key.as_str()),
            _ => None,
        }
    }

    /// The kind of event, as development warnings name it.
    pub fn kind(&self) -> &'static str {
        match self {
            InputEvent::Click(ClickEvent::Mouse(_)) => "mouse click",
            InputEvent::Click(ClickEvent::Touch(_)) => "tap",
            InputEvent::Click(ClickEvent::Keyboard(_)) => "keyboard click",
            InputEvent::KeyDown(_) => "key down",
            InputEvent::KeyUp(_) => "key up",
            InputEvent::MouseDown(_) => "mouse down",
            InputEvent::MouseMove(_) => "mouse move",
            InputEvent::MouseUp(_) => "mouse up",
            InputEvent::Scroll(_) => "scroll",
            InputEvent::Focus(_) => "focus",
            InputEvent::Accessibility => "accessibility action",
            InputEvent::Touch(_) => "touch",
            InputEvent::TouchDrag(_) => "touch drag",
            InputEvent::BackGesture(_) => "back gesture",
            InputEvent::FileDrop(_) => "file drop",
            InputEvent::Pinch(_) => "pinch",
            InputEvent::LongPress(_) => "long press",
            InputEvent::TapDown(_) => "tap down",
            InputEvent::TapUp(_) => "tap up",
        }
    }

    /// A click as the window had it. gpui hands a tap over as a mouse click,
    /// which the window's last input tells apart, so a tap is the touch
    /// click it was, as the original's `pointerType` says.
    pub fn click(event: &ClickEvent, window: &Window) -> Self {
        match event {
            ClickEvent::Mouse(click) if window.last_input_was_touch() => {
                InputEvent::Click(ClickEvent::Touch(TouchClickEvent {
                    position: click.up.position,
                    tap_count: click.up.click_count,
                    long_press: false,
                }))
            }
            event => InputEvent::Click(event.clone()),
        }
    }

    /// A mouse press as the window had it: a tap's press, which gpui hands
    /// over as a mouse press, is a finger's (`TapDown`).
    pub fn mouse_down(event: &MouseDownEvent, window: &Window) -> Self {
        if window.last_input_was_touch() {
            InputEvent::TapDown(event.clone())
        } else {
            InputEvent::MouseDown(event.clone())
        }
    }

    /// A mouse release as the window had it: a tap's release is a finger's
    /// (`TapUp`).
    pub fn mouse_up(event: &MouseUpEvent, window: &Window) -> Self {
        if window.last_input_was_touch() {
            InputEvent::TapUp(event.clone())
        } else {
            InputEvent::MouseUp(event.clone())
        }
    }
}

impl From<&ClickEvent> for InputEvent {
    fn from(event: &ClickEvent) -> Self {
        InputEvent::Click(event.clone())
    }
}

impl From<&KeyDownEvent> for InputEvent {
    fn from(event: &KeyDownEvent) -> Self {
        InputEvent::KeyDown(event.clone())
    }
}

impl From<&MouseDownEvent> for InputEvent {
    fn from(event: &MouseDownEvent) -> Self {
        InputEvent::MouseDown(event.clone())
    }
}

impl From<&MouseMoveEvent> for InputEvent {
    fn from(event: &MouseMoveEvent) -> Self {
        InputEvent::MouseMove(event.clone())
    }
}

impl From<&MouseUpEvent> for InputEvent {
    fn from(event: &MouseUpEvent) -> Self {
        InputEvent::MouseUp(event.clone())
    }
}

impl From<&KeyUpEvent> for InputEvent {
    fn from(event: &KeyUpEvent) -> Self {
        InputEvent::KeyUp(event.clone())
    }
}

impl From<&ScrollEvent> for InputEvent {
    fn from(event: &ScrollEvent) -> Self {
        InputEvent::Scroll(event.clone())
    }
}

impl From<&TouchEvent> for InputEvent {
    fn from(event: &TouchEvent) -> Self {
        InputEvent::Touch(event.clone())
    }
}

impl From<&TouchDragEvent> for InputEvent {
    fn from(event: &TouchDragEvent) -> Self {
        InputEvent::TouchDrag(event.clone())
    }
}

impl From<&BackGestureEvent> for InputEvent {
    fn from(event: &BackGestureEvent) -> Self {
        InputEvent::BackGesture(event.clone())
    }
}

impl From<&FileDropEvent> for InputEvent {
    fn from(event: &FileDropEvent) -> Self {
        InputEvent::FileDrop(event.clone())
    }
}

impl From<&PinchEvent> for InputEvent {
    fn from(event: &PinchEvent) -> Self {
        InputEvent::Pinch(event.clone())
    }
}

impl From<&LongPressEvent> for InputEvent {
    fn from(event: &LongPressEvent) -> Self {
        InputEvent::LongPress(event.clone())
    }
}

/// What every `on_*_change` callback receives after the new value.
#[derive(Clone, Debug)]
pub struct ChangeDetails {
    /// Why it changed, like `EscapeKey`.
    pub reason: Reason,
    /// How the user caused it: `Mouse`, `Touch`, `Pen`, `Keyboard`, `Virtual`
    /// (a click with no pointer or key behind it, which may be assistive
    /// technology), `Agent` (an AI agent's call through the agent bridge) or
    /// `Programmatic`. For analytics and focus choices, never for behavior.
    pub source: ChangeSource,
    /// The event behind it. `None` for controllers and cascades.
    pub event: Option<InputEvent>,
    canceled: Cell<bool>,
}

impl ChangeDetails {
    pub fn new(reason: Reason, source: ChangeSource, event: Option<InputEvent>) -> Self {
        Self {
            reason,
            source,
            event,
            canceled: Cell::new(false),
        }
    }

    /// Stops the change. A controlled component ignores it, since the next
    /// value comes from the app.
    pub fn cancel(&self) {
        self.canceled.set(true);
    }

    /// Whether `cancel()` was called.
    pub fn canceled(&self) -> bool {
        self.canceled.get()
    }
}

/// What an `on_*_commit` callback receives after the value: why and how the
/// interaction that ended changed it. The original's type has no `cancel`;
/// here it's the same struct, and commits ignore `cancel()`. `event` is the
/// event that ended the interaction, or the one behind the last change when
/// the end was another kind, like a blur after a wheel.
pub type CommitDetails = ChangeDetails;

/// How a call through a controller or the api reports itself, like
/// `{ reason, event }` in the original's `ChangeOptions`. The reason defaults
/// to `Imperative`.
#[derive(Clone, Debug, Default)]
pub struct ChangeOptions {
    pub reason: Option<Reason>,
    pub source: Option<ChangeSource>,
    pub event: Option<InputEvent>,
}

impl ChangeOptions {
    pub fn reason(reason: Reason) -> Self {
        Self {
            reason: Some(reason),
            ..Default::default()
        }
    }

    pub fn event(reason: Reason, event: impl Into<InputEvent>) -> Self {
        Self::event_of(reason, event.into())
    }

    pub fn event_of(reason: Reason, event: InputEvent) -> Self {
        Self {
            reason: Some(reason),
            event: Some(event),
            source: None,
        }
    }

    pub fn source(mut self, source: ChangeSource) -> Self {
        self.source = Some(source);
        self
    }
}

/// The reasons that mean the user dismissed a layer, rather than chose
/// something in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DismissReason {
    EscapeKey,
    OutsidePress,
    FocusOut,
    WindowBlur,
    Back,
    Swipe,
}

impl DismissReason {
    /// The dismissal `reason` is, if it's one.
    pub fn of(reason: Reason) -> Option<Self> {
        Some(match reason {
            Reason::EscapeKey => Self::EscapeKey,
            Reason::OutsidePress => Self::OutsidePress,
            Reason::FocusOut => Self::FocusOut,
            Reason::WindowBlur => Self::WindowBlur,
            Reason::Back => Self::Back,
            Reason::Swipe => Self::Swipe,
            _ => return None,
        })
    }

    pub fn reason(self) -> Reason {
        match self {
            Self::EscapeKey => Reason::EscapeKey,
            Self::OutsidePress => Reason::OutsidePress,
            Self::FocusOut => Reason::FocusOut,
            Self::WindowBlur => Reason::WindowBlur,
            Self::Back => Reason::Back,
            Self::Swipe => Reason::Swipe,
        }
    }

    pub fn as_str(self) -> &'static str {
        self.reason().as_str()
    }
}

impl From<DismissReason> for Reason {
    fn from(reason: DismissReason) -> Self {
        reason.reason()
    }
}

impl TryFrom<Reason> for DismissReason {
    type Error = Reason;

    fn try_from(reason: Reason) -> Result<Self, Reason> {
        Self::of(reason).ok_or(reason)
    }
}

/// The dismissals a person makes on purpose, which `on_dismiss_attempt` hears
/// about when `dismissible` refuses them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DismissAttemptReason {
    EscapeKey,
    OutsidePress,
    Back,
    Swipe,
}

impl DismissAttemptReason {
    /// The attempt `reason` is, if it's one.
    pub fn of(reason: Reason) -> Option<Self> {
        Some(match reason {
            Reason::EscapeKey => Self::EscapeKey,
            Reason::OutsidePress => Self::OutsidePress,
            Reason::Back => Self::Back,
            Reason::Swipe => Self::Swipe,
            _ => return None,
        })
    }

    pub fn reason(self) -> Reason {
        match self {
            Self::EscapeKey => Reason::EscapeKey,
            Self::OutsidePress => Reason::OutsidePress,
            Self::Back => Reason::Back,
            Self::Swipe => Reason::Swipe,
        }
    }

    pub fn as_str(self) -> &'static str {
        self.reason().as_str()
    }
}

impl From<DismissAttemptReason> for Reason {
    fn from(reason: DismissAttemptReason) -> Self {
        reason.reason()
    }
}

impl From<DismissAttemptReason> for DismissReason {
    fn from(reason: DismissAttemptReason) -> Self {
        match reason {
            DismissAttemptReason::EscapeKey => Self::EscapeKey,
            DismissAttemptReason::OutsidePress => Self::OutsidePress,
            DismissAttemptReason::Back => Self::Back,
            DismissAttemptReason::Swipe => Self::Swipe,
        }
    }
}

impl TryFrom<Reason> for DismissAttemptReason {
    type Error = Reason;

    fn try_from(reason: Reason) -> Result<Self, Reason> {
        Self::of(reason).ok_or(reason)
    }
}

/// What `on_dismiss_attempt` receives when `dismissible` refused a dismissal.
#[derive(Clone, Debug)]
pub struct DismissAttemptDetails {
    /// How the user tried to dismiss it, like `EscapeKey`.
    pub reason: DismissAttemptReason,
    /// The event behind it.
    pub event: Option<InputEvent>,
}

/// Whether a reason dismissed a layer: Escape, an outside press, focus or the
/// window leaving, a back gesture or a swipe. For "Discard changes?":
/// `if !open && is_dismissal(details.reason) && dirty { details.cancel() }`.
pub fn is_dismissal(reason: Reason) -> bool {
    reason.is_dismissal()
}

thread_local! {
    // The source of the change running now, so changes it causes in turn,
    // like a submenu closing because its parent did, report the same source.
    static CURRENT_SOURCE: RefCell<Vec<Option<ChangeSource>>> = const { RefCell::new(Vec::new()) };
}

/// Runs `run` as part of an interaction made with `source`, and returns what
/// it returns. The changes it makes report what they would have during the
/// interaction itself, so an agent's stay the agent's for `observe` and undo.
/// An action that awaits runs on after its press ended, so wrap what it
/// changes after the wait with the source the action received.
pub fn with_source<R>(source: ChangeSource, run: impl FnOnce() -> R) -> R {
    with_optional_source(Some(source), run)
}

/// `with_source` for a source that may be unknown, which clears the cause
/// while `run` runs (`withSource(undefined, run)`).
pub fn with_optional_source<R>(source: Option<ChangeSource>, run: impl FnOnce() -> R) -> R {
    CURRENT_SOURCE.with(|stack| stack.borrow_mut().push(source));
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            CURRENT_SOURCE.with(|stack| {
                stack.borrow_mut().pop();
            });
        }
    }
    let _pop = Pop;
    run()
}

/// The source of the change running now, if any.
pub fn current_source() -> Option<ChangeSource> {
    CURRENT_SOURCE.with(|stack| stack.borrow().last().copied().flatten())
}

/// How a change came about, from its reason, the source of its event if it
/// has one, and the change that caused it (`resolveSource`).
pub fn resolve_source(reason: Reason, event: Option<&InputEvent>) -> ChangeSource {
    let cause = current_source();
    // An agent's call, and every change it causes, reports the agent.
    if cause == Some(ChangeSource::Agent) {
        return ChangeSource::Agent;
    }
    if reason.is_programmatic() {
        return ChangeSource::Programmatic;
    }
    if let Some(event) = event {
        let source = event.source();
        // A click a component makes for a key keeps the key's source.
        return match (source, cause) {
            (ChangeSource::Programmatic, Some(cause)) => cause,
            _ => source,
        };
    }
    cause.unwrap_or(ChangeSource::Programmatic)
}

/// The details a callback receives: the reason, the event and how the user
/// caused it (`createDetails`). Its `cancel()` cancels a change; a commit's
/// is ignored. Extra fields the original passes, like which item changed,
/// are arguments of the component's own callback here.
pub fn create_details(reason: Reason, event: Option<InputEvent>, source: ChangeSource) -> ChangeDetails {
    ChangeDetails::new(reason, source, event)
}
