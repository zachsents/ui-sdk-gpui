//! The text field the original builds every text-entry component on:
//! `<input>` and `<textarea>`, headless. It edits as the platform does,
//! composes with input methods, keeps an undo history, and reports what
//! happened the way the browser's events do.
//!
//! Keys reach the field the way `keydown` reaches an element before the
//! browser's default action: listeners on its part, or above it, run first,
//! then every keystroke observer, like the shortcuts' document listener, and
//! the field moves the caret or edits only once they're done. A listener
//! keeps the field from acting on a key with `window.prevent_default()`, as
//! `preventDefault()` does, or with `cx.stop_propagation()`, which in gpui
//! also means the key typed nothing. An app's own key bindings come first of
//! all; bind them with `!TextField` in their context to leave fields alone.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AccessibleAction, App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable,
    KeyContext, KeystrokeEvent, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Point,
    ScrollEvent, SharedString, Subscription, Task, TextInputConfiguration, WeakEntity,
    WeakFocusHandle, Window, actions, prelude::*, px, size,
};
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::Role as AriaRole;
use ui_sdk_kernel::{Aria, ChangeSource, Instance, Part, focus_events, modality};

use crate::accessibility::{self, A11ySnapshot};
use crate::adapter::Mirror;
use crate::boundaries;
use crate::display::Display;
use crate::element::TextFieldElement;
use crate::events::{
    BeforeEdit, Edit, FieldHook, FieldNotice, SelectionDirection, TextFieldEvent, TextFilter,
};
use crate::history::{History, Selection};
use crate::input_handler::Composition;
use crate::keys;
use crate::layout::TextLayout;
use crate::mouse::Drag;
use crate::style::{TextFieldKind, TextFieldStyle};

actions!(
    text_field,
    [
        /// Copies the selection, for an app's Edit menu.
        Copy,
        /// Cuts the selection.
        Cut,
        /// Pastes over the selection.
        Paste,
        Undo,
        Redo,
        SelectAll,
    ]
);

/// The key context a text field's part sets, so key bindings can tell a
/// field is focused: `TextField`, with `multiline` or `password` too.
pub const KEY_CONTEXT: &str = "TextField";

const BLINK_INTERVAL: Duration = Duration::from_millis(500);

/// The options `TextField` takes, what an `<input>` or `<textarea>` has as
/// attributes.
#[derive(Clone, Default)]
pub struct TextFieldOptions {
    /// The value it starts with, `defaultValue`. Until the value is edited
    /// or written, the field follows it, and `reset` returns to it.
    pub default_value: SharedString,
    pub placeholder: Option<SharedString>,
    /// A `<textarea>`: Enter adds a line, and the text wraps.
    pub multi_line: bool,
    /// How many rows tall a multi-line field is when nothing sets its height,
    /// `rows`. Defaults to 2.
    pub rows: Option<usize>,
    /// How many characters wide the field is when nothing sets its width,
    /// `size` or `cols`: `0`s of its font, CSS's `ch`. Defaults to 20. The
    /// style's `width_ch` takes over from it.
    pub cols: Option<usize>,
    /// Grows and shrinks a multi-line field with its text, `field-sizing:
    /// content`. Bound it with `min_h` and `max_h` on the part; past the
    /// maximum, it scrolls.
    pub auto_resize: bool,
    /// What a single-line field holds, `type`.
    pub kind: TextFieldKind,
    /// Can't be focused or edited.
    pub disabled: bool,
    /// Can be focused, selected and copied, but not edited.
    pub read_only: bool,
    /// Reported to assistive technology. Validation is the forms crate's.
    pub required: bool,
    /// The most characters typing, pasting and input methods may leave,
    /// `maxlength`. It cuts what an edit puts in after `on_before_input`,
    /// which hears all of it. Writes from code aren't limited.
    pub max_length: Option<usize>,
    /// Which characters typing, pasting and input methods may put in.
    /// Others are dropped, and an edit left with nothing doesn't happen.
    pub filter: Option<TextFilter>,
    /// A number field's lowest value, `min`.
    pub min: Option<f64>,
    /// A number field's highest value, `max`.
    pub max: Option<f64>,
    /// How far a number field's arrows step, `step`. Defaults to 1.
    pub step: Option<f64>,
    /// Selects all the text whenever it gains focus, even from a press.
    /// Without it, focus from Tab selects a single-line field's text, and
    /// other focus keeps the selection it had.
    pub select_on_focus: bool,
    /// What a software keyboard and the platform's text services offer:
    /// corrections, capitals, suggestions, and the Return key's label, what
    /// the original's `inputHints` set as attributes. A password gets none.
    pub configuration: TextInputConfiguration,
    pub style: TextFieldStyle,
    /// Runs before a person's edit applies, `beforeinput`: typing, an input
    /// method, pasting, cutting, deleting, undoing, assistive technology.
    /// `prevent_default` refuses it, unless an input method is composing.
    /// Its `data` is all of what's put in, before the maximum length cuts
    /// it, so it runs even when there's no room for any.
    pub on_before_input: Option<FieldHook<BeforeEdit>>,
    /// Runs after an edit applied, `input`. A program's edit through
    /// `replace_range` comes with `user` false; a hook that updates whoever
    /// made the call leaves those alone. Writing the value fires none.
    pub on_input: Option<FieldHook<Edit>>,
    /// Runs when what the user typed is committed, `change`: on blur, or on
    /// Enter in a single-line field.
    pub on_change: Option<FieldNotice>,
    /// Runs when the selection or the caret moved, once the update that
    /// moved it is over, as `selectionchange` comes after the call.
    pub on_selection_change: Option<FieldNotice>,
    pub on_focus: Option<FieldNotice>,
    pub on_blur: Option<FieldNotice>,
    /// Runs when an input method starts composing.
    pub on_composition_start: Option<FieldNotice>,
    /// Runs when it ends, with the text it committed, empty when it gave up.
    pub on_composition_end: Option<FieldHook<String>>,
    /// Runs when something asks to submit the form the field is in,
    /// `form.requestSubmit()`, like a suggestion that sends what's typed.
    /// The field knows no form; this does.
    pub on_submit: Option<FieldNotice>,
}

impl TextFieldOptions {
    pub fn default_value(mut self, value: impl Into<SharedString>) -> Self {
        self.default_value = value.into();
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn multi_line(mut self, multi_line: bool) -> Self {
        self.multi_line = multi_line;
        self
    }

    pub fn rows(mut self, rows: usize) -> Self {
        self.rows = Some(rows);
        self
    }

    pub fn cols(mut self, cols: usize) -> Self {
        self.cols = Some(cols);
        self
    }

    pub fn auto_resize(mut self, auto_resize: bool) -> Self {
        self.auto_resize = auto_resize;
        self
    }

    pub fn kind(mut self, kind: TextFieldKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub fn max_length(mut self, max_length: usize) -> Self {
        self.max_length = Some(max_length);
        self
    }

    pub fn filter(mut self, filter: impl Fn(char) -> bool + 'static) -> Self {
        self.filter = Some(Rc::new(filter));
        self
    }

    pub fn min(mut self, min: f64) -> Self {
        self.min = Some(min);
        self
    }

    pub fn max(mut self, max: f64) -> Self {
        self.max = Some(max);
        self
    }

    pub fn step(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }

    pub fn select_on_focus(mut self, select_on_focus: bool) -> Self {
        self.select_on_focus = select_on_focus;
        self
    }

    pub fn configuration(mut self, configuration: TextInputConfiguration) -> Self {
        self.configuration = configuration;
        self
    }

    pub fn style(mut self, style: TextFieldStyle) -> Self {
        self.style = style;
        self
    }

    pub fn on_before_input(
        mut self,
        hook: impl Fn(&mut TextField, &BeforeEdit, &mut Window, &mut Context<TextField>) + 'static,
    ) -> Self {
        self.on_before_input = Some(Rc::new(hook));
        self
    }

    pub fn on_input(
        mut self,
        hook: impl Fn(&mut TextField, &Edit, &mut Window, &mut Context<TextField>) + 'static,
    ) -> Self {
        self.on_input = Some(Rc::new(hook));
        self
    }

    pub fn on_change(
        mut self,
        hook: impl Fn(&mut TextField, &mut Window, &mut Context<TextField>) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(hook));
        self
    }

    pub fn on_selection_change(
        mut self,
        hook: impl Fn(&mut TextField, &mut Window, &mut Context<TextField>) + 'static,
    ) -> Self {
        self.on_selection_change = Some(Rc::new(hook));
        self
    }

    pub fn on_focus(
        mut self,
        hook: impl Fn(&mut TextField, &mut Window, &mut Context<TextField>) + 'static,
    ) -> Self {
        self.on_focus = Some(Rc::new(hook));
        self
    }

    pub fn on_blur(
        mut self,
        hook: impl Fn(&mut TextField, &mut Window, &mut Context<TextField>) + 'static,
    ) -> Self {
        self.on_blur = Some(Rc::new(hook));
        self
    }

    pub fn on_composition_start(
        mut self,
        hook: impl Fn(&mut TextField, &mut Window, &mut Context<TextField>) + 'static,
    ) -> Self {
        self.on_composition_start = Some(Rc::new(hook));
        self
    }

    pub fn on_composition_end(
        mut self,
        hook: impl Fn(&mut TextField, &String, &mut Window, &mut Context<TextField>) + 'static,
    ) -> Self {
        self.on_composition_end = Some(Rc::new(hook));
        self
    }

    pub fn on_submit(
        mut self,
        hook: impl Fn(&mut TextField, &mut Window, &mut Context<TextField>) + 'static,
    ) -> Self {
        self.on_submit = Some(Rc::new(hook));
        self
    }
}

/// What the root part's `with` and the app read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextFieldState {
    pub focused: bool,
    /// Focused from the keyboard, `:focus-visible`.
    pub focus_visible: bool,
    /// Holds any text.
    pub filled: bool,
    pub disabled: bool,
    pub read_only: bool,
    pub required: bool,
    pub multi_line: bool,
    /// An input method is composing.
    pub composing: bool,
    /// Shows its placeholder, `:placeholder-shown`.
    pub placeholder_shown: bool,
    /// A popup it controls is open, `data-open`.
    pub expanded: bool,
}

/// Where the last frame laid the field out, which clicks, vertical moves
/// and the platform's text input read.
#[derive(Clone)]
pub(crate) struct Geometry {
    pub layout: Rc<TextLayout>,
    pub display: Rc<Display>,
    /// The text area, in window coordinates.
    pub bounds: Bounds<Pixels>,
    pub scroll: Point<Pixels>,
}

impl Geometry {
    /// A window position, from the text area's top left with scrolling.
    pub fn local(&self, position: Point<Pixels>) -> Point<Pixels> {
        position - self.bounds.origin + self.scroll
    }

    /// A position from the text area's top left with scrolling, in the window.
    pub fn window(&self, local: Point<Pixels>) -> Point<Pixels> {
        local + self.bounds.origin - self.scroll
    }

    /// Where the caret at a value offset is drawn, with no width.
    pub fn caret_bounds(&self, offset: usize) -> Bounds<Pixels> {
        let shown = self.display.to_display(offset);
        let caret = self.layout.caret(shown, false);
        Bounds::new(self.window(caret), size(px(0.), self.layout.line_height))
    }
}

#[derive(Clone)]
pub(crate) struct Hooks {
    pub before_input: Option<FieldHook<BeforeEdit>>,
    pub input: Option<FieldHook<Edit>>,
    pub change: Option<FieldNotice>,
    pub selection_change: Option<FieldNotice>,
    pub focus: Option<FieldNotice>,
    pub blur: Option<FieldNotice>,
    pub composition_start: Option<FieldNotice>,
    pub composition_end: Option<FieldHook<String>>,
    pub submit: Option<FieldNotice>,
}

impl Hooks {
    fn of(options: &TextFieldOptions) -> Self {
        Self {
            before_input: options.on_before_input.clone(),
            input: options.on_input.clone(),
            change: options.on_change.clone(),
            selection_change: options.on_selection_change.clone(),
            focus: options.on_focus.clone(),
            blur: options.on_blur.clone(),
            composition_start: options.on_composition_start.clone(),
            composition_end: options.on_composition_end.clone(),
            submit: options.on_submit.clone(),
        }
    }
}

thread_local! {
    // Every live field's focus handle, so code above it can find the one with focus.
    static FIELDS: RefCell<Vec<(WeakFocusHandle, WeakEntity<TextField>)>> =
        const { RefCell::new(Vec::new()) };
    // Whether the last key was Tab, so focus it brings selects as Tab does.
    static TAB_PRESSED: Cell<bool> = const { Cell::new(false) };
}

/// A text field. Its entity is its element: the app holds it and calls what
/// an `HTMLInputElement` offers.
pub struct TextField {
    pub(crate) instance: Instance,
    pub(crate) this: WeakEntity<Self>,
    pub(crate) focus_handle: FocusHandle,

    pub(crate) default_value: SharedString,
    pub(crate) placeholder: Option<SharedString>,
    pub(crate) multi_line: bool,
    pub(crate) rows: usize,
    pub(crate) cols: usize,
    pub(crate) auto_resize: bool,
    pub(crate) kind: TextFieldKind,
    pub(crate) disabled: bool,
    pub(crate) read_only: bool,
    pub(crate) required: bool,
    pub(crate) max_length: Option<usize>,
    pub(crate) filter: Option<TextFilter>,
    pub(crate) min: Option<f64>,
    pub(crate) max: Option<f64>,
    pub(crate) step: Option<f64>,
    pub(crate) select_on_focus: bool,
    pub(crate) configuration: TextInputConfiguration,
    pub(crate) style: TextFieldStyle,
    pub(crate) hooks: Hooks,

    pub(crate) value: SharedString,
    // Counts every change to the value, so a listener's own edit shows.
    pub(crate) revision: usize,
    // Edited or written since it was made or reset, HTML's dirty value flag.
    pub(crate) dirty: bool,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) direction: SelectionDirection,
    // The caret sits at the end of a wrapped row rather than the next's start.
    pub(crate) upstream: bool,
    // Where vertical moves aim, kept across them.
    pub(crate) goal_x: Option<Pixels>,
    pub(crate) composition: Option<Composition>,
    pub(crate) history: History,
    // The value the last `change` reported, and whether the user edited since.
    pub(crate) committed: SharedString,
    pub(crate) user_edited: bool,
    // The selection last reported, and whether a report is on its way.
    pub(crate) reported: Selection,
    selection_pending: bool,

    pub(crate) focused: bool,
    pub(crate) focus_visible: bool,
    pub(crate) pointer_focus: bool,
    pub(crate) window_blurred: bool,
    pub(crate) caret_visible: bool,
    blink_epoch: usize,
    blink_task: Option<Task<()>>,
    pub(crate) scroll: Point<Pixels>,
    pub(crate) autoscroll: bool,
    pub(crate) geometry: Option<Geometry>,
    pub(crate) drag: Option<Drag>,
    // The last double or triple press's unit, and what it had selected, which
    // a Shift press extends by.
    pub(crate) unit: Option<(Drag, Range<usize>)>,
    // Text a refused key would have typed, which the platform still sends.
    pub(crate) refused_text: Option<String>,
    pub(crate) expanded: bool,
    pub(crate) a11y: Rc<RefCell<A11ySnapshot>>,
    pub(crate) mirror: Rc<RefCell<Mirror>>,
    _subscriptions: Vec<Subscription>,
}

impl TextField {
    pub fn new(options: TextFieldOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        modality::track_input(window, cx);
        let subscriptions = vec![
            focus_events::on_focus(&focus_handle, window, cx, Self::focused_in),
            focus_events::on_blur(&focus_handle, window, cx, Self::blurred),
            cx.observe_keystrokes(Self::keystroke),
            // A field that leaves mid-composition leaves no composition behind.
            cx.on_release_in(window, |field, window, cx| {
                if field.composition.is_some() {
                    ui_sdk_kernel::set_composing(window, false, cx);
                }
            }),
        ];
        let weak = cx.weak_entity();
        let handle = focus_handle.downgrade();
        FIELDS.with(|fields| {
            let mut fields = fields.borrow_mut();
            fields.retain(|(handle, field)| handle.upgrade().is_some() && field.upgrade().is_some());
            fields.push((handle, weak));
        });
        let value = sanitize_line_breaks(&options.default_value, options.multi_line);
        let end = value.len();
        let field = Self {
            // Its own entity, so the views its text draws in redraw as it
            // changes.
            instance: Instance::new("text-field").with_entity(cx),
            this: cx.weak_entity(),
            focus_handle,
            default_value: options.default_value.clone(),
            placeholder: options.placeholder.clone(),
            multi_line: options.multi_line,
            rows: options.rows.unwrap_or(2).max(1),
            cols: options.cols.unwrap_or(20).max(1),
            auto_resize: options.auto_resize,
            kind: options.kind,
            disabled: options.disabled,
            read_only: options.read_only,
            required: options.required,
            max_length: options.max_length,
            filter: options.filter.clone(),
            min: options.min,
            max: options.max,
            step: options.step,
            select_on_focus: options.select_on_focus,
            configuration: options.configuration.clone(),
            style: options.style,
            hooks: Hooks::of(&options),
            committed: value.clone(),
            value,
            revision: 0,
            dirty: false,
            start: end,
            end,
            direction: SelectionDirection::None,
            upstream: false,
            goal_x: None,
            composition: None,
            history: History::default(),
            user_edited: false,
            reported: (end, end, SelectionDirection::None),
            selection_pending: false,
            focused: false,
            focus_visible: false,
            pointer_focus: false,
            window_blurred: false,
            caret_visible: false,
            blink_epoch: 0,
            blink_task: None,
            scroll: Point::default(),
            autoscroll: true,
            geometry: None,
            drag: None,
            unit: None,
            refused_text: None,
            expanded: false,
            a11y: Rc::new(RefCell::new(A11ySnapshot::default())),
            mirror: Rc::default(),
            _subscriptions: subscriptions,
        };
        field.sync_mirror();
        field
    }

    /// Hands it new options, as a framework re-renders with new attributes.
    /// A new default value shows while the value is the untouched default.
    pub fn set_options(&mut self, options: TextFieldOptions, cx: &mut Context<Self>) {
        let mut changed = false;
        if options.default_value != self.default_value {
            self.default_value = options.default_value.clone();
            if !self.dirty {
                let value = sanitize_line_breaks(&self.default_value, options.multi_line);
                if value != self.value {
                    self.replace_value(value);
                    let end = self.value.len();
                    self.set_selection_quiet(end, end, SelectionDirection::None);
                    cx.emit(TextFieldEvent::Write(self.value.clone()));
                }
                changed = true;
            }
        }
        let rows = options.rows.unwrap_or(2).max(1);
        let cols = options.cols.unwrap_or(20).max(1);
        changed |= self.placeholder != options.placeholder
            || self.multi_line != options.multi_line
            || self.rows != rows
            || self.cols != cols
            || self.auto_resize != options.auto_resize
            || self.kind != options.kind
            || self.disabled != options.disabled
            || self.read_only != options.read_only
            || self.required != options.required
            || self.style != options.style;
        if self.multi_line && !options.multi_line && self.value.contains('\n') {
            let value = sanitize_line_breaks(&self.value, false);
            self.replace_value(value);
            let end = self.value.len();
            self.set_selection_quiet(end, end, SelectionDirection::None);
        }
        self.placeholder = options.placeholder.clone();
        self.multi_line = options.multi_line;
        self.rows = rows;
        self.cols = cols;
        self.auto_resize = options.auto_resize;
        self.kind = options.kind;
        self.disabled = options.disabled;
        self.read_only = options.read_only;
        self.required = options.required;
        self.max_length = options.max_length;
        self.filter = options.filter.clone();
        self.min = options.min;
        self.max = options.max;
        self.step = options.step;
        self.select_on_focus = options.select_on_focus;
        self.configuration = options.configuration.clone();
        self.style = options.style;
        self.hooks = Hooks::of(&options);
        self.sync_mirror();
        if changed {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    pub fn state(&self) -> TextFieldState {
        TextFieldState {
            focused: self.focused,
            focus_visible: self.focused && self.focus_visible,
            filled: !self.value.is_empty(),
            disabled: self.disabled,
            read_only: self.read_only,
            required: self.required,
            multi_line: self.multi_line,
            composing: self.composition.is_some(),
            placeholder_shown: self.value.is_empty()
                && self.placeholder.as_ref().is_some_and(|p| !p.is_empty()),
            expanded: self.expanded,
        }
    }

    /// The text, with any an input method is composing, `value`.
    pub fn value(&self) -> &SharedString {
        &self.value
    }

    pub fn len(&self) -> usize {
        self.value.len()
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    /// The value read as a number, `valueAsNumber`.
    pub fn value_as_number(&self) -> Option<f64> {
        parse_number(&self.value)
    }

    pub fn default_value(&self) -> &SharedString {
        &self.default_value
    }

    pub fn placeholder(&self) -> Option<&SharedString> {
        self.placeholder.as_ref()
    }

    pub fn kind(&self) -> TextFieldKind {
        self.kind
    }

    pub fn is_multi_line(&self) -> bool {
        self.multi_line
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn is_required(&self) -> bool {
        self.required
    }

    pub fn max_length(&self) -> Option<usize> {
        self.max_length
    }

    /// Whether typing and pasting reach it: not disabled or read-only.
    pub fn is_editable(&self) -> bool {
        !self.disabled && !self.read_only
    }

    /// Whether an input method is composing, the original's `isComposing`.
    /// A key handler leaves keys alone while it is.
    pub fn is_composing(&self) -> bool {
        self.composition.is_some()
    }

    /// The range of the value an input method is composing.
    pub fn composition_range(&self) -> Option<Range<usize>> {
        self.composition.as_ref().map(|composition| composition.range.clone())
    }

    /// Where the selection starts, `selectionStart`. Offsets are UTF-8 byte
    /// offsets into the value, on character boundaries.
    pub fn selection_start(&self) -> usize {
        self.start
    }

    /// Where it ends, `selectionEnd`. The caret when nothing is selected.
    pub fn selection_end(&self) -> usize {
        self.end
    }

    pub fn selection_direction(&self) -> SelectionDirection {
        self.direction
    }

    pub fn selection(&self) -> Range<usize> {
        self.start..self.end
    }

    pub fn selected_text(&self) -> &str {
        &self.value[self.start..self.end]
    }

    /// The end of the selection that moves: the caret.
    pub fn head(&self) -> usize {
        if self.direction == SelectionDirection::Backward {
            self.start
        } else {
            self.end
        }
    }

    /// The end of the selection that stays.
    pub fn anchor(&self) -> usize {
        if self.direction == SelectionDirection::Backward {
            self.end
        } else {
            self.start
        }
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Whether a popup the field controls is open.
    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    /// Says whether a popup the field controls is open, like a suggestion
    /// menu, so its arrow keys belong to the popup: what `aria-expanded`, or
    /// `data-open` on a text area, says in the original. Its ARIA is the
    /// component's, on the role it gives the part.
    pub fn set_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        if self.expanded != expanded {
            self.expanded = expanded;
            self.sync_mirror();
            cx.notify();
        }
    }

    /// Asks to submit the form the field is in, `form.requestSubmit()`,
    /// which `on_submit` does.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(hook) = self.hooks.submit.clone() {
            hook(self, window, cx);
        }
    }

    /// Writes the value from code, `value = ..`. No `input` fires, the caret
    /// goes to the end, and the undo history starts over, as in the browser.
    /// Line breaks go from a single-line field.
    pub fn set_value(
        &mut self,
        value: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = sanitize_line_breaks(&value.into(), self.multi_line);
        self.cancel_composition(window, cx);
        self.dirty = true;
        if value == self.value {
            return;
        }
        self.write_from_code(value, window, cx);
    }

    /// Puts the default value back, as a form reset does.
    pub fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = sanitize_line_breaks(&self.default_value, self.multi_line);
        self.cancel_composition(window, cx);
        self.dirty = false;
        if value == self.value {
            return;
        }
        self.write_from_code(value, window, cx);
    }

    fn write_from_code(&mut self, value: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_value(value);
        self.history.clear();
        self.committed = self.value.clone();
        self.user_edited = false;
        let end = self.value.len();
        self.set_selection_quiet(end, end, SelectionDirection::None);
        self.upstream = false;
        self.goal_x = None;
        cx.emit(TextFieldEvent::Write(self.value.clone()));
        if self.focused {
            window.notify_text_input_changed(&self.focus_handle);
        }
        self.changed(window, cx);
    }

    pub(crate) fn replace_value(&mut self, value: SharedString) {
        self.value = value;
        self.revision = self.revision.wrapping_add(1);
        let len = self.value.len();
        self.start = boundaries::floor_char(&self.value, self.start.min(len));
        self.end = boundaries::floor_char(&self.value, self.end.min(len));
        self.sync_mirror();
    }

    /// Selects a range, `setSelectionRange`. A start past the end moves to it.
    pub fn set_selection_range(
        &mut self,
        start: usize,
        end: usize,
        direction: SelectionDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_selection_quiet(start, end, direction);
        self.upstream = false;
        self.goal_x = None;
        self.history.close();
        if self.focused {
            window.notify_text_selection_changed(&self.focus_handle);
        }
        self.changed(window, cx);
    }

    /// Selects all the text, `select()`.
    pub fn select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let len = self.value.len();
        self.set_selection_range(0, len, SelectionDirection::None, window, cx);
    }

    /// Replaces a range with text as one step the user can undo, and puts
    /// the caret after it, the way the original inserts with `execCommand`.
    /// It's a program's edit: `input` fires with `user` false, and neither
    /// `beforeinput`, the filter nor the maximum length apply.
    pub fn replace_range(
        &mut self,
        range: Range<usize>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.commit_composition(window, cx);
        let source = ui_sdk_kernel::details::current_source().unwrap_or(ChangeSource::Programmatic);
        let input_type = if text.is_empty() {
            crate::InputType::DeleteContent
        } else {
            crate::InputType::InsertText
        };
        self.apply(
            crate::editing::Change {
                input_type,
                range,
                text: text.to_string(),
                source,
                user: false,
                group: crate::history::Group::Single,
                selection: None,
            },
            window,
            cx,
        )
    }

    /// Moves focus to it, `focus()`. A disabled field can't take it.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if !self.disabled {
            window.focus(&self.focus_handle, cx);
        }
    }

    /// Takes focus away from it, `blur()`.
    pub fn blur(&self, window: &mut Window, cx: &mut App) {
        if self.focus_handle.is_focused(window) {
            window.blur(cx);
        }
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    /// Where the caret at an offset was drawn last frame, in window
    /// coordinates, with no width: what a menu anchors to.
    pub fn caret_bounds(&self, offset: usize) -> Option<Bounds<Pixels>> {
        let offset = offset.min(self.value.len());
        self.geometry.as_ref().map(|geometry| geometry.caret_bounds(offset))
    }

    /// Where a range was drawn last frame, a rectangle per row, in window
    /// coordinates.
    pub fn range_bounds(&self, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        let Some(geometry) = self.geometry.as_ref() else {
            return Vec::new();
        };
        let start = geometry.display.to_display(range.start.min(self.value.len()));
        let end = geometry.display.to_display(range.end.min(self.value.len()));
        geometry
            .layout
            .range_rects(start..end)
            .into_iter()
            .map(|rect| Bounds::new(geometry.window(rect.origin), rect.size))
            .collect()
    }

    /// The area the text was drawn in last frame, in window coordinates.
    pub fn text_bounds(&self) -> Option<Bounds<Pixels>> {
        self.geometry.as_ref().map(|geometry| geometry.bounds)
    }

    /// The offset nearest a window position, from last frame's layout.
    pub fn offset_for_position(&self, position: Point<Pixels>) -> Option<usize> {
        self.hit(position).map(|(offset, _)| offset)
    }

    pub(crate) fn hit(&self, position: Point<Pixels>) -> Option<(usize, bool)> {
        let geometry = self.geometry.as_ref()?;
        let (shown, upstream) = geometry.layout.offset_at(geometry.local(position));
        let offset = boundaries::grapheme_start(&self.value, geometry.display.to_value(shown));
        Some((offset, upstream))
    }

    pub(crate) fn selection_tuple(&self) -> Selection {
        (self.start, self.end, self.direction)
    }

    pub(crate) fn set_selection_quiet(
        &mut self,
        start: usize,
        end: usize,
        direction: SelectionDirection,
    ) {
        let len = self.value.len();
        let end = boundaries::floor_char(&self.value, end.min(len));
        let start = boundaries::floor_char(&self.value, start.min(len)).min(end);
        self.start = start;
        self.end = end;
        self.direction = direction;
        self.sync_mirror();
    }

    /// What this frame shows for the value.
    pub(crate) fn display(&self) -> Display {
        if self.kind == TextFieldKind::Password && !self.multi_line {
            Display::masked(&self.value)
        } else {
            Display::plain(&self.value)
        }
    }

    // What it tells assistive technology its value is.
    fn a11y_value(&self) -> SharedString {
        self.display().text
    }

    fn key_context(&self) -> KeyContext {
        let mut context = KeyContext::default();
        context.add(KEY_CONTEXT);
        if self.multi_line {
            context.add("multiline");
        }
        if self.kind == TextFieldKind::Password {
            context.add("password");
        }
        context
    }

    /// Reports what an operation changed: a new frame with the caret shown
    /// and in view, and the selection, if it moved, once the update is over.
    pub(crate) fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Like `selectionchange`, the report comes after the call that moved
        // it, so a listener can update whatever made the call.
        if self.selection_tuple() != self.reported && !self.selection_pending {
            self.selection_pending = true;
            cx.defer_in(window, |field, window, cx| field.report_selection(window, cx));
        }
        self.autoscroll = true;
        if self.focused {
            self.restart_blink(window, cx);
            window.invalidate_character_coordinates();
        }
        cx.notify();
    }

    fn report_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selection_pending = false;
        let selection = self.selection_tuple();
        if selection == self.reported {
            return;
        }
        self.reported = selection;
        if let Some(hook) = self.hooks.selection_change.clone() {
            hook(self, window, cx);
        }
        cx.emit(TextFieldEvent::SelectionChange {
            range: self.start..self.end,
            direction: self.direction,
        });
        self.tell(ui_sdk_text::TextFieldEvent::Selection, window, cx);
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if self.start < self.end && self.kind != TextFieldKind::Password {
            cx.write_to_primary(gpui::ClipboardItem::new_string(
                self.selected_text().to_string(),
            ));
        }
    }

    pub(crate) fn before_input(
        &mut self,
        edit: &BeforeEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.tell(ui_sdk_text::TextFieldEvent::BeforeInput, window, cx);
        if let Some(hook) = self.hooks.before_input.clone() {
            hook(self, edit, window, cx);
        }
        !(edit.cancelable && edit.default_prevented())
    }

    pub(crate) fn fire_input(&mut self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(hook) = self.hooks.input.clone() {
            hook(self, &edit, window, cx);
        }
        // A commit reaches the text crate's listeners as the composition's
        // end, which reports the whole of it, as Chromium does.
        if edit.input_type != crate::InputType::InsertFromComposition {
            let event = ui_sdk_text::TextFieldEvent::Input {
                composing: edit.composing,
                user: edit.user,
            };
            self.tell(event, window, cx);
        }
        cx.emit(TextFieldEvent::Input(edit));
    }

    pub(crate) fn fire_composition_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tell(ui_sdk_text::TextFieldEvent::CompositionStart, window, cx);
        if let Some(hook) = self.hooks.composition_start.clone() {
            hook(self, window, cx);
        }
        cx.emit(TextFieldEvent::CompositionStart);
    }

    /// Reports the end of a composition. `by_code` when a write from code
    /// ended it, which the text crate's listeners hear as a program's input.
    pub(crate) fn fire_composition_end(
        &mut self,
        committed: String,
        by_code: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(hook) = self.hooks.composition_end.clone() {
            hook(self, &committed, window, cx);
        }
        let event = if by_code {
            ui_sdk_text::TextFieldEvent::Input {
                composing: false,
                user: false,
            }
        } else {
            ui_sdk_text::TextFieldEvent::CompositionEnd
        };
        self.tell(event, window, cx);
        cx.emit(TextFieldEvent::CompositionEnd(committed));
    }

    /// Reports what the user typed since the last commit, `change`.
    pub(crate) fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let edited = std::mem::take(&mut self.user_edited);
        if !edited || self.value == self.committed {
            return;
        }
        self.committed = self.value.clone();
        if let Some(hook) = self.hooks.change.clone() {
            hook(self, window, cx);
        }
        cx.emit(TextFieldEvent::Change(self.value.clone()));
    }

    pub(crate) fn restart_blink(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.caret_visible = true;
        self.blink_epoch = self.blink_epoch.wrapping_add(1);
        let epoch = self.blink_epoch;
        self.blink_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK_INTERVAL).await;
                let blinking = this.update(cx, |field, cx| {
                    if field.blink_epoch != epoch || !field.focused {
                        return false;
                    }
                    field.caret_visible = !field.caret_visible;
                    cx.notify();
                    true
                });
                if !matches!(blinking, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn focused_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focused = true;
        self.focus_visible = modality::focus_visible(window);
        // A field that just took focus isn't composing yet.
        ui_sdk_kernel::set_composing(window, self.composition.is_some(), cx);
        let reactivated = std::mem::take(&mut self.window_blurred);
        let pointer = std::mem::take(&mut self.pointer_focus);
        if !reactivated && !pointer {
            let tabbed =
                TAB_PRESSED.with(Cell::get) && modality::is_focus_from_keyboard(window, cx);
            if self.select_on_focus || (tabbed && !self.multi_line) {
                let len = self.value.len();
                self.set_selection_quiet(0, len, SelectionDirection::None);
            }
        }
        if let Some(hook) = self.hooks.focus.clone() {
            hook(self, window, cx);
        }
        cx.emit(TextFieldEvent::Focus);
        self.changed(window, cx);
    }

    fn blurred(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.window_blurred = !window.is_window_active();
        self.commit_composition(window, cx);
        self.focused = false;
        self.focus_visible = false;
        self.caret_visible = false;
        self.blink_task = None;
        self.drag = None;
        self.history.close();
        // A single-line field shows its start again once it lets focus go.
        if !self.multi_line {
            self.scroll.x = px(0.);
        }
        self.commit(window, cx);
        if let Some(hook) = self.hooks.blur.clone() {
            hook(self, window, cx);
        }
        self.tell(ui_sdk_text::TextFieldEvent::Blur, window, cx);
        cx.emit(TextFieldEvent::Blur);
        cx.notify();
    }

    // Runs once every listener and every other keystroke observer has had
    // the key, unless one stopped or prevented it, the way the browser's
    // default action follows `keydown`: a shortcut, which hears keys from an
    // observer too, takes its keys from the field however the two were made.
    fn keystroke(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        TAB_PRESSED.with(|tab| tab.set(event.keystroke.key == "tab"));
        self.refused_text = None;
        if event.action.is_some()
            || self.disabled
            || self.composition.is_some()
            || !self.focus_handle.is_focused(window)
        {
            return;
        }
        let os = cx.platform_info().os;
        let number = self.kind == TextFieldKind::Number && !self.multi_line;
        let command = keys::command_for(&event.keystroke, os, self.multi_line, number);
        let typed = event.keystroke.key_char.clone();
        let field = cx.weak_entity();
        // The observers after this one have the key first. The platform types
        // the key's text once they all have.
        window.defer(cx, move |window, cx| {
            field
                .update(cx, |field, cx| {
                    if window.default_prevented() {
                        // The platform still types the key's text; it goes
                        // too.
                        field.refused_text = typed;
                        return;
                    }
                    if let Some(command) = command {
                        field.run(command, window, cx);
                    }
                })
                .ok();
        });
    }

    /// The `root` part: the field itself. Style it like any part; the text
    /// inside draws with the part's text style and `TextFieldStyle`. Its role
    /// is a textbox, reported as accesskit's kind of text input: multi-line,
    /// password, search and so on. A component that's more, like a combobox,
    /// declares its own role over it.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<TextFieldState> {
        let field = this.read(cx);
        let state = field.state();
        let weak = this.downgrade();
        let element = TextFieldElement::new(this.clone());
        let snapshot = field.a11y.clone();
        let native_role = field.kind.role(field.multi_line);
        let aria_role = if field.kind == TextFieldKind::Search && !field.multi_line {
            AriaRole::Searchbox
        } else {
            AriaRole::Textbox
        };
        let part = Part::new(&field.instance, "root", None, state)
            .aria_role(aria_role)
            .focus_mode(FocusMode::Native, Some(&field.focus_handle), field.disabled)
            .aria(
                Aria::new()
                    .disabled(field.disabled)
                    .read_only(field.read_only)
                    .required(field.required),
            )
            .key_context(field.key_context())
            .aria_value(field.a11y_value())
            .when_some(field.placeholder.clone(), |part, placeholder| {
                part.aria_placeholder(placeholder)
            })
            .a11y_synthetic_children(move |builder| {
                accessibility::build(&snapshot, native_role, builder)
            })
            .when(!field.disabled, |part| part.cursor_text())
            .flex()
            .flex_col()
            .when(!field.multi_line, |part| part.justify_center())
            .on_mouse_down(MouseButton::Left, {
                let weak = weak.clone();
                move |event: &MouseDownEvent, window, cx| {
                    weak.update(cx, |field, cx| field.mouse_down(event, window, cx)).ok();
                }
            })
            .on_mouse_up(MouseButton::Left, {
                let weak = weak.clone();
                move |_: &MouseUpEvent, _, cx| {
                    weak.update(cx, |field, _| field.end_drag()).ok();
                }
            })
            .on_mouse_up_out(MouseButton::Left, {
                let weak = weak.clone();
                move |_: &MouseUpEvent, _, cx| {
                    weak.update(cx, |field, _| field.end_drag()).ok();
                }
            })
            .on_scroll({
                let weak = weak.clone();
                move |event: &ScrollEvent, window, cx| {
                    weak.update(cx, |field, cx| field.scroll_wheel(event, window, cx)).ok();
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &Copy, _, cx| {
                    weak.update(cx, |field, cx| field.copy(cx)).ok();
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &Cut, window, cx| {
                    weak.update(cx, |field, cx| field.cut(window, cx)).ok();
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &Paste, window, cx| {
                    weak.update(cx, |field, cx| field.paste(window, cx)).ok();
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &Undo, window, cx| {
                    weak.update(cx, |field, cx| field.undo(window, cx)).ok();
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &Redo, window, cx| {
                    weak.update(cx, |field, cx| field.redo(window, cx)).ok();
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &SelectAll, window, cx| {
                    weak.update(cx, |field, cx| field.select_all(window, cx)).ok();
                }
            })
            .on_a11y_action(AccessibleAction::SetValue, {
                let weak = weak.clone();
                move |data, window, cx| {
                    if let Some(value) = accessibility::value_of(data) {
                        weak.update(cx, |field, cx| field.a11y_set_value(value, window, cx)).ok();
                    }
                }
            })
            .on_a11y_action(AccessibleAction::ReplaceSelectedText, {
                let weak = weak.clone();
                move |data, window, cx| {
                    if let Some(text) = accessibility::value_of(data) {
                        weak.update(cx, |field, cx| field.a11y_replace(text, window, cx)).ok();
                    }
                }
            })
            .on_a11y_action(AccessibleAction::SetTextSelection, {
                let weak = weak.clone();
                move |data, window, cx| {
                    weak.update(cx, |field, cx| field.a11y_select(data, window, cx)).ok();
                }
            });
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let part = part.on_mouse_down(MouseButton::Middle, {
            let weak = weak.clone();
            move |event: &MouseDownEvent, window, cx| {
                weak.update(cx, |field, cx| field.paste_primary(event, window, cx)).ok();
            }
        });
        part.child(element)
    }
}

impl Focusable for TextField {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<TextFieldEvent> for TextField {}

/// Whether a text field has focus, so a key types text: the original's
/// `isTypingIn` for the focused element. Shortcuts and typeahead read it
/// before taking a key.
pub fn is_typing(window: &Window) -> bool {
    window
        .context_stack()
        .iter()
        .any(|context| context.contains(KEY_CONTEXT))
}

/// The text field that has focus in `window`, if one does.
pub fn focused_text_field(window: &Window) -> Option<Entity<TextField>> {
    FIELDS.with(|fields| {
        fields.borrow().iter().find_map(|(handle, field)| {
            let handle = handle.upgrade()?;
            if handle.is_focused(window) {
                field.upgrade()
            } else {
                None
            }
        })
    })
}

// What a value may hold: no line breaks in a single-line field, and only
// `\n` ones in a multi-line one.
pub(crate) fn sanitize_line_breaks(value: &SharedString, multi_line: bool) -> SharedString {
    if !value.contains('\r') && (multi_line || !value.contains('\n')) {
        return value.clone();
    }
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                if multi_line {
                    out.push('\n');
                }
            }
            '\n' => {
                if multi_line {
                    out.push('\n');
                }
            }
            c => out.push(c),
        }
    }
    out.into()
}

/// A number field's text read as a number: a floating-point number as HTML
/// writes them.
pub(crate) fn parse_number(text: &str) -> Option<f64> {
    let text = text.trim();
    let digits = text.strip_prefix('-').unwrap_or(text);
    if !digits.starts_with(|c: char| c.is_ascii_digit() || c == '.') || text.ends_with('.') {
        return None;
    }
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}
