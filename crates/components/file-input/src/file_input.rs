//! Port of `components/file-input/file-input.ts`: a file input, the files a
//! user chose, dropped or pasted, as its value. Every part is optional. A
//! trigger alone is an upload button; add a drop zone for drops and pastes,
//! and a list for the attachments, each removable. The trigger opens the
//! system's file dialog, and the root takes part in a form through the
//! file input's internals, as a form-associated custom element does. This
//! module is the public api and the parts; the behavior is in
//! `file_input_api.rs` and `file_input_dropzone.rs`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::ops::Deref;
use std::rc::Rc;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyDownEvent, SharedString,
    StatefulInteractiveElement, Subscription, Task, Window,
};
use ui_sdk_forms::files::FileValidator;
use ui_sdk_forms::{Accept, Association, ElementInternals, FileRejection, Node, associate};
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{
    Agent, AgentAction, AgentError, AgentItem, AgentState, AgentValue, Aria, ChangeDetails,
    ChangeOptions, CommitDetails, Controllable, InputEvent, Instance, Part, Parts, Reason, held,
};

use crate::file_input_api::{DragStatus, File, Settings, settings};
use crate::file_input_dropzone::{Zone, bind_dropzone, paste_targets};

/// Why a file input's value wants to change: `Picker`, `Drop`, `Paste`,
/// `RemovePress`, `Reset` or `Imperative`.
pub type FileInputValueChangeReason = Reason;

/// What a change did, beside the new value.
#[derive(Clone, Debug, PartialEq)]
pub struct FileInputChange {
    /// Why it changed.
    pub reason: FileInputValueChangeReason,
    /// The files that joined the value. Files that failed the checks are in
    /// `rejected` instead.
    pub added: Vec<File>,
    /// The files that left the value, including the one a new file replaces
    /// without `multiple`.
    pub removed: Vec<File>,
    /// The files that didn't pass the checks, each with its reasons. They
    /// never join the value.
    pub rejected: Vec<FileRejection<File>>,
}

/// What `on_value_change` receives after the files: the reason, source and
/// event, with `cancel()`, as every change's details have, and what the
/// change added, removed and rejected.
pub struct FileInputValueChangeDetails<'a> {
    details: &'a ChangeDetails,
    /// The files that joined the value.
    pub added: &'a [File],
    /// The files that left the value, including the one a new file replaces
    /// without `multiple`.
    pub removed: &'a [File],
    /// The files that didn't pass the checks, each with its reasons.
    pub rejected: &'a [FileRejection<File>],
}

impl<'a> FileInputValueChangeDetails<'a> {
    pub(crate) fn new(details: &'a ChangeDetails, change: &'a FileInputChange) -> Self {
        Self {
            details,
            added: &change.added,
            removed: &change.removed,
            rejected: &change.rejected,
        }
    }

    /// The change's own details, with its reason, source and event.
    pub fn details(&self) -> &ChangeDetails {
        self.details
    }
}

impl Deref for FileInputValueChangeDetails<'_> {
    type Target = ChangeDetails;

    fn deref(&self) -> &ChangeDetails {
        self.details
    }
}

impl fmt::Debug for FileInputValueChangeDetails<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileInputValueChangeDetails")
            .field("details", self.details)
            .field("added", &self.added)
            .field("removed", &self.removed)
            .field("rejected", &self.rejected)
            .finish()
    }
}

/// Options for every write method: the reason and event `on_value_change`
/// receives.
pub type FileInputChangeOptions = ChangeOptions;

/// `on_value_change`: the files, and what the change did.
pub type FileInputValueChange =
    Rc<dyn Fn(&[File], &FileInputValueChangeDetails<'_>, &mut Window, &mut App)>;

/// `on_reject`: the files that failed the checks, each with its reasons.
pub type FileInputReject = Rc<dyn Fn(&[FileRejection<File>], &CommitDetails, &mut Window, &mut App)>;

/// `announcement`: what screen readers hear after a change, or `None` for
/// nothing.
pub type FileInputAnnouncement = Rc<dyn Fn(&FileInputChange) -> Option<String>>;

/// The options `FileInput` takes.
#[derive(Clone, Default)]
pub struct FileInputOptions {
    /// The files to accept, like the `accept` attribute: MIME types
    /// (`"image/png"`), wildcards (`"image/*"`) and extensions (`".pdf"`),
    /// in a comma-separated list or one token each. Without it, any file. It
    /// checks files from the picker, drops and pastes; an extension matches
    /// the file name, whatever type the system reports.
    pub accept: Option<Accept>,
    /// The most files it holds. When too many come, the first ones that pass
    /// the other checks are accepted until it's full, and the rest are
    /// rejected with `TooManyFiles`.
    pub max_files: Option<usize>,
    /// The largest size accepted, in bytes.
    pub max_size: Option<u64>,
    /// The smallest size accepted, in bytes.
    pub min_size: Option<u64>,
    /// A check of your own: a reason, like `"duplicate"`, rejects the file,
    /// and `None` accepts it. The slice holds the files of the same pick,
    /// drop or paste accepted before this one. It runs after the type and
    /// size checks, and its reason reaches `on_reject` like theirs.
    pub validate: Option<FileValidator<File>>,
    /// The files, to control the file input.
    pub value: Option<Vec<File>>,
    /// The files at first, when uncontrolled. A form reset returns to them.
    /// None by default.
    pub default_value: Option<Vec<File>>,
    /// Runs when files are added or removed, with what changed in the
    /// details: `details.reason` is `Picker`, `Drop`, `Paste`, `RemovePress`,
    /// `Reset` or `Imperative`. It runs once the file input's update is over,
    /// so it may update any entity, the file input too.
    pub on_value_change: Option<FileInputValueChange>,
    /// Runs as files are asked to be added or removed, before they are,
    /// inside the file input's update, with what changed in the details:
    /// `details.cancel()` stops it. It may update any entity but the file
    /// input.
    pub on_before_value_change: Option<FileInputValueChange>,
    /// Runs when files fail the checks, after `on_value_change`, with every
    /// reason each failed, even when none were accepted. It runs once the file
    /// input's update is over, so it may update any entity, the file input
    /// too.
    pub on_reject: Option<FileInputReject>,
    /// Holds more than one file. New files join the ones it has, up to
    /// `max_files` in all. Without it, a new file replaces the one it has.
    pub multiple: bool,
    /// Picks a whole folder, with every file inside it. Turns on `multiple`.
    pub directory: bool,
    /// The name the files submit under. Without one, nothing is submitted.
    pub name: Option<SharedString>,
    /// The id of a form to belong to, when it isn't inside that form.
    pub form: Option<SharedString>,
    /// Blocks form submission until it holds a file, with the browser's own
    /// message.
    pub required: bool,
    /// Disables it like a native control: the trigger can't be focused or
    /// used, drops and pastes are ignored, and nothing is submitted.
    pub disabled: bool,
    /// What screen readers hear after a change. Return `None` for nothing.
    /// "2 files accepted. 1 file rejected.", or "photo.png removed." by
    /// default.
    pub announcement: Option<FileInputAnnouncement>,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl FileInputOptions {
    pub fn accept(mut self, accept: impl Into<Accept>) -> Self {
        self.accept = Some(accept.into());
        self
    }

    pub fn max_files(mut self, max_files: usize) -> Self {
        self.max_files = Some(max_files);
        self
    }

    pub fn max_size(mut self, max_size: u64) -> Self {
        self.max_size = Some(max_size);
        self
    }

    pub fn min_size(mut self, min_size: u64) -> Self {
        self.min_size = Some(min_size);
        self
    }

    pub fn validate(
        mut self,
        validate: impl Fn(&File, &[File]) -> Option<SharedString> + 'static,
    ) -> Self {
        self.validate = Some(Rc::new(validate));
        self
    }

    pub fn value(mut self, value: Vec<File>) -> Self {
        self.value = Some(value);
        self
    }

    pub fn default_value(mut self, value: Vec<File>) -> Self {
        self.default_value = Some(value);
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&[File], &FileInputValueChangeDetails<'_>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_before_value_change(
        mut self,
        callback: impl Fn(&[File], &FileInputValueChangeDetails<'_>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_before_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_reject(
        mut self,
        callback: impl Fn(&[FileRejection<File>], &CommitDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_reject = Some(Rc::new(callback));
        self
    }

    pub fn multiple(mut self, multiple: bool) -> Self {
        self.multiple = multiple;
        self
    }

    pub fn directory(mut self, directory: bool) -> Self {
        self.directory = directory;
        self
    }

    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn form(mut self, form: impl Into<SharedString>) -> Self {
        self.form = Some(form.into());
        self
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn announcement(
        mut self,
        announcement: impl Fn(&FileInputChange) -> Option<String> + 'static,
    ) -> Self {
        self.announcement = Some(Rc::new(announcement));
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// Where a drop zone takes drops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FileInputDropzoneTarget {
    /// Files dropped on the zone.
    #[default]
    Element,
    /// Files dropped anywhere in the window that no other zone takes. The
    /// zone is dragging while files are over the window, so it can be an
    /// overlay over all of it.
    Document,
}

/// Where a drop zone takes pasted files from, on top of pastes while focus
/// is inside it.
#[derive(Clone)]
pub enum FileInputPasteTarget {
    /// Pastes anywhere in the window.
    Document,
    /// Pastes while focus is in the element with this focus handle, or
    /// inside it, like a prompt's text area.
    Element(FocusHandle),
    /// The element, from a function called when the paste happens, so the
    /// element can render after the zone.
    Function(Rc<dyn Fn(&App) -> Option<FocusHandle>>),
}

impl FileInputPasteTarget {
    /// The element a function returns when the paste happens.
    pub fn function(target: impl Fn(&App) -> Option<FocusHandle> + 'static) -> Self {
        FileInputPasteTarget::Function(Rc::new(target))
    }
}

impl From<FocusHandle> for FileInputPasteTarget {
    fn from(handle: FocusHandle) -> Self {
        FileInputPasteTarget::Element(handle)
    }
}

impl fmt::Debug for FileInputPasteTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileInputPasteTarget::Document => f.write_str("Document"),
            FileInputPasteTarget::Element(handle) => f.debug_tuple("Element").field(handle).finish(),
            FileInputPasteTarget::Function(_) => f.write_str("Function"),
        }
    }
}

/// The props a drop zone takes. They're read when each drag, drop or paste
/// happens, so new ones apply at once.
#[derive(Clone, Debug, Default)]
pub struct FileInputDropzoneProps {
    /// Where it takes drops: on itself by default, or anywhere in the window
    /// with `Document`.
    pub target: FileInputDropzoneTarget,
    /// Somewhere else to take pasted files from, on top of pastes while focus
    /// is inside the zone: the whole window, or an element, like a prompt's
    /// text area. Text pastes are left alone.
    pub paste_target: Option<FileInputPasteTarget>,
}

impl FileInputDropzoneProps {
    pub fn target(mut self, target: FileInputDropzoneTarget) -> Self {
        self.target = target;
        self
    }

    pub fn paste_target(mut self, target: impl Into<FileInputPasteTarget>) -> Self {
        self.paste_target = Some(target.into());
        self
    }
}

/// What the root's, the trigger's and the list's `with` and the app read.
#[derive(Clone, Debug, PartialEq)]
pub struct FileInputState {
    /// The files it holds.
    pub value: Vec<File>,
    /// Whether it's disabled, by itself, its field or a fieldset
    /// (`data-disabled`).
    pub disabled: bool,
    /// Whether a form won't submit while it holds no files (`data-required`).
    pub required: bool,
    /// Whether files are being dragged over one of its drop zones
    /// (`data-dragging` on the root).
    pub dragging: bool,
}

/// What a drop zone's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileInputDropzoneState {
    /// Whether files are being dragged over it, or over the window for a
    /// window-wide zone (`data-dragging`).
    pub dragging: bool,
    /// Whether the dragged files look acceptable, from their types and count
    /// (`data-drag-accept`).
    pub drag_accept: bool,
    /// Whether a dragged file has a type `accept` rules out, or there are too
    /// many (`data-drag-reject`).
    pub drag_reject: bool,
    /// Whether the file input is disabled, by itself, its field or a
    /// fieldset, so drops and pastes are ignored (`data-disabled`).
    pub disabled: bool,
}

/// What an item's `with` reads.
#[derive(Clone, Debug, PartialEq)]
pub struct FileInputItemState {
    /// The file this item shows.
    pub file: File,
    /// Where it is in the value, from 0. `None` for a file the value doesn't
    /// hold.
    pub index: Option<usize>,
    /// Whether the file input is disabled, so the item can't be removed
    /// (`data-disabled`).
    pub disabled: bool,
}

/// What a remove button's `with` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileInputItemRemoveState {
    /// Whether the file input is disabled, so the button can't remove its
    /// file (`data-disabled`).
    pub disabled: bool,
}

// The keys an item handles: the arrows, Home and End move between items, and
// Backspace and Delete remove its file.
const ITEM_KEYS: &[&str] = &[
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "Home",
    "End",
    "Backspace",
    "Delete",
];

/// A file input. Its entity is its controller: `get_value`, `set_value`,
/// `add_files`, `remove_file`, `clear`, `open`, `focus` and `is_dragging`
/// work from anywhere, and a trigger rendered anywhere opens its picker.
pub struct FileInput {
    pub(crate) instance: Instance,
    pub(crate) value: Controllable<Vec<File>>,
    // Where a form reset returns.
    pub(crate) initial: Vec<File>,
    pub(crate) has_default: bool,
    pub(crate) settings: Settings,
    pub(crate) accept: Option<Accept>,
    pub(crate) max_files: Option<usize>,
    pub(crate) max_size: Option<u64>,
    pub(crate) min_size: Option<u64>,
    pub(crate) validate: Option<FileValidator<File>>,
    pub(crate) on_value_change: Option<FileInputValueChange>,
    pub(crate) on_before_value_change: Option<FileInputValueChange>,
    pub(crate) on_reject: Option<FileInputReject>,
    pub(crate) announcement: Option<FileInputAnnouncement>,
    pub(crate) form_disabled: bool,
    // The drop zones files are over, by key, with how the dragged files look.
    pub(crate) drags: HashMap<SharedString, DragStatus>,
    // The item that takes Tab into the list, by key. The first one until
    // another is focused.
    pub(crate) active_item: Option<SharedString>,
    pub(crate) internals: ElementInternals,
    pub(crate) trigger_handle: FocusHandle,
    // The trigger's place in the page, where a validation message points.
    pub(crate) trigger_node: Node,
    // The items in page order, with their focus handles.
    pub(crate) items: Parts<SharedString, ()>,
    pub(crate) removes: RefCell<HashMap<SharedString, FocusHandle>>,
    // The drop zones, by key, in the order they first rendered.
    pub(crate) zones: RefCell<Vec<(SharedString, Rc<Zone>)>>,
    // The system's file dialog while it's open.
    pub(crate) picking: Option<Task<()>>,
    // Files being read from dropped folders and pasted images.
    pub(crate) reading: HashMap<u64, Task<()>>,
    pub(crate) next_read: u64,
    _subscriptions: Vec<Subscription>,
}

impl FileInput {
    pub fn new(options: FileInputOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let first = options
            .value
            .clone()
            .or_else(|| options.default_value.clone())
            .unwrap_or_default();
        let initial = options.default_value.clone().unwrap_or_else(|| first.clone());
        let mut value = Controllable::new("value", first);
        value.set_controlled(options.value.clone());
        let mut instance = Instance::new("file-input").with_agent(cx);
        instance.debug = options.debug;
        let internals = ElementInternals::new(cx);
        let trigger_handle = cx.focus_handle();
        // A submit that finds it invalid focuses the trigger.
        internals.set_focus_handle(Some(trigger_handle.clone()));
        // The trigger is a plain button, so a field names it by its label
        // and then the trigger's own text, like "Browse files".
        internals.set_named_by_content(Some(instance.element_id("trigger", None)));
        // A label press clicks the trigger, which opens the picker, as a
        // native label clicks a plain button it names.
        internals.on_activate(cx, |input: &mut Self, _event, window, cx| {
            input.open(window, cx);
        });
        let association = associate(
            &internals,
            Association::new()
                .reset(cx, |input: &mut Self, window, cx| input.reset(window, cx))
                .disabled(cx, |input: &mut Self, disabled, _window, cx| {
                    input.set_form_disabled(disabled, cx)
                }),
        );
        let pastes = paste_targets(window, cx);
        let input = Self {
            instance,
            value,
            initial,
            has_default: options.default_value.is_some(),
            settings: settings(&options),
            accept: options.accept,
            max_files: options.max_files,
            max_size: options.max_size,
            min_size: options.min_size,
            validate: options.validate,
            on_value_change: options.on_value_change,
            on_before_value_change: options.on_before_value_change,
            on_reject: options.on_reject,
            announcement: options.announcement,
            form_disabled: false,
            drags: HashMap::new(),
            active_item: None,
            internals,
            trigger_handle,
            trigger_node: Node::element(),
            items: Parts::new(),
            removes: RefCell::new(HashMap::new()),
            zones: RefCell::new(Vec::new()),
            picking: None,
            reading: HashMap::new(),
            next_read: 0,
            _subscriptions: vec![association, pastes],
        };
        input.sync_form();
        input
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`). A new `default_value` changes what a form reset returns to.
    pub fn set_options(&mut self, options: FileInputOptions, cx: &mut Context<Self>) {
        let settings = settings(&options);
        let mut changed = self.settings != settings;
        if let Some(default_value) = &options.default_value {
            self.initial = default_value.clone();
        }
        self.has_default = options.default_value.is_some();
        changed |= self.value.set_controlled(options.value);
        self.settings = settings;
        self.accept = options.accept;
        self.max_files = options.max_files;
        self.max_size = options.max_size;
        self.min_size = options.min_size;
        self.validate = options.validate;
        self.on_value_change = options.on_value_change;
        self.on_before_value_change = options.on_before_value_change;
        self.on_reject = options.on_reject;
        self.announcement = options.announcement;
        self.instance.debug = options.debug;
        self.sync_form();
        if changed {
            cx.notify();
        }
    }

    /// Passes the controlled files, or `None` to let the file input keep its
    /// own.
    pub fn set_controlled_value(&mut self, value: Option<Vec<File>>, cx: &mut Context<Self>) {
        if self.value.set_controlled(value) {
            self.sync_form();
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// What it takes part in forms with, for a field or form around it.
    pub fn internals(&self) -> &ElementInternals {
        &self.internals
    }

    pub fn state(&self) -> FileInputState {
        FileInputState {
            value: self.value.get().clone(),
            disabled: self.is_disabled(),
            required: self.settings.required,
            dragging: !self.drags.is_empty(),
        }
    }

    pub(crate) fn dropzone_state(&self, key: &SharedString) -> FileInputDropzoneState {
        let status = self.drags.get(key).copied();
        FileInputDropzoneState {
            dragging: status.is_some(),
            drag_accept: status == Some(DragStatus::Accept),
            drag_reject: status == Some(DragStatus::Reject),
            disabled: self.is_disabled(),
        }
    }

    // The item that takes Tab into the list: the last one focused, else the
    // first.
    fn is_active_item(&self, key: &SharedString) -> bool {
        let value = self.value.get();
        if let Some(active) = &self.active_item
            && value.iter().any(|file| file.key() == active)
        {
            return active == key;
        }
        value.first().is_some_and(|file| file.key() == key)
    }

    fn remove_handle(&self, key: &SharedString, cx: &App) -> FocusHandle {
        self.removes
            .borrow_mut()
            .entry(key.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// The `root` part, which holds the others and takes part in the form:
    /// render it around them, since it's what the form, a fieldset or a
    /// label around the file input finds.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<FileInputState> {
        let input = this.read(cx);
        input.sync_form();
        let entity = this.downgrade();
        Part::new(&input.instance, "root", None, input.state())
            .map_el(|el| input.internals.track(el))
            .on_props(move |_window, cx| {
                // Zones that left the page leave their drags behind.
                let Some(entity) = entity.upgrade() else {
                    return;
                };
                let input = entity.read(cx);
                let gone = |key: &SharedString| !held(&input.instance, "dropzone", Some(key));
                let stale = input.zones.borrow().iter().any(|(key, _)| gone(key))
                    || input.drags.keys().any(gone);
                if stale {
                    entity.update(cx, |input, cx| input.prune_zones(cx));
                }
            })
    }

    /// The `trigger` part: the button that opens the system's file dialog.
    /// It's what a label or a field around the file input names, a field by
    /// its label and then the trigger's own name. Give it text, or an
    /// `aria_label`. Disabled, by the file input, a fieldset or its field,
    /// it can't be focused or used.
    pub fn trigger(this: &Entity<Self>, cx: &App) -> Part<FileInputState> {
        let input = this.read(cx);
        let state = input.state();
        // What the field writes on its control: its names, descriptions,
        // invalid state and disabled. A button takes no required or read-only
        // state; the root reports them to the form.
        let mut aria = input.internals.aria();
        aria.required = false;
        aria.read_only = false;
        let disabled = aria.disabled || state.disabled;
        let entity = this.downgrade();
        Part::new(&input.instance, "trigger", None, state)
            .aria_role(Role::Button)
            .focus_mode(FocusMode::Native, Some(&input.trigger_handle), disabled)
            .aria(aria.disabled(disabled))
            .map_el(|el| input.trigger_node.track(el))
            .on_press(move |_event, window, cx| {
                entity.update(cx, |input, cx| input.open(window, cx)).ok();
            })
    }

    /// A `dropzone` part, by a key of its own: it takes files dropped on it,
    /// or anywhere in the window with a `Document` target, and files pasted
    /// while focus is inside it or in its `paste_target`. It has no role and
    /// no tab stop: keep a trigger beside it for keyboards and screen
    /// readers. A window-wide zone hears drags only while it paints, so hide
    /// one with no size or opacity rather than `invisible`.
    pub fn dropzone(
        this: &Entity<Self>,
        key: impl Into<SharedString>,
        props: FileInputDropzoneProps,
        cx: &App,
    ) -> Part<FileInputDropzoneState> {
        let input = this.read(cx);
        let key = key.into();
        let part = Part::new(&input.instance, "dropzone", Some(&key), input.dropzone_state(&key));
        bind_dropzone(part, this, key, props, cx)
    }

    /// The `list` part: the files, as a list. Render an item for each file
    /// inside it, in order, since it starts each frame's order of items.
    /// Name it with `aria_label`.
    pub fn list(this: &Entity<Self>, cx: &App) -> Part<FileInputState> {
        let input = this.read(cx);
        input.items.retain_bound();
        input.items.begin();
        // Remove buttons of files that left go with them.
        let present: HashSet<&SharedString> = input.value.get().iter().map(File::key).collect();
        input.removes.borrow_mut().retain(|key, _| present.contains(key));
        Part::new(&input.instance, "list", None, input.state()).aria_role(Role::List)
    }

    /// An `item` part, for one of the files: focusable, with one tab stop for
    /// the list, the last item focused or else the first. Arrow keys, Home
    /// and End move between items, and Backspace or Delete removes the
    /// focused one.
    pub fn item(this: &Entity<Self>, file: &File, cx: &App) -> Part<FileInputItemState> {
        let input = this.read(cx);
        let key = file.key().clone();
        input.items.register(key.clone(), ());
        let handle = input.items.focus_handle(&key, cx);
        let state = FileInputItemState {
            file: file.clone(),
            index: input.value.get().iter().position(|each| each == file),
            disabled: input.is_disabled(),
        };
        let tab_stop = input.is_active_item(&key);
        let keys = this.downgrade();
        let watched = this.downgrade();
        let own = handle.clone();
        let focused = handle.clone();
        let pressed = file.clone();
        Part::new(&input.instance, "item", Some(&key), state)
            .aria_role(Role::ListItem)
            .focus_mode(FocusMode::Roving { tab_stop }, Some(&handle), false)
            .on_keys(ITEM_KEYS, move |event: &KeyDownEvent, window, cx| {
                // Only keys pressed on the item itself, not on a control
                // inside it.
                if !own.is_focused(window) {
                    return;
                }
                keys.update(cx, |input, cx| input.item_keydown(event, &pressed, window, cx))
                    .ok();
            })
            .on_props(move |window, cx| {
                // Focus moving into the item makes it the one Tab enters on.
                if !focused.contains_focused(window, cx) {
                    return;
                }
                let Some(entity) = watched.upgrade() else {
                    return;
                };
                if entity.read(cx).active_item.as_ref() != Some(&key) {
                    entity.update(cx, |input, cx| input.set_active_item(key, cx));
                }
            })
    }

    /// An `item-remove` part, for one of the files: removes its file. It's
    /// named for the file, "Remove photo.png", and stays out of the tab
    /// order, since Backspace on the item does the same.
    pub fn item_remove(this: &Entity<Self>, file: &File, cx: &App) -> Part<FileInputItemRemoveState> {
        let input = this.read(cx);
        let disabled = input.is_disabled();
        let handle = (!disabled).then(|| input.remove_handle(file.key(), cx));
        let entity = this.downgrade();
        let target = file.clone();
        Part::new(
            &input.instance,
            "item-remove",
            Some(file.key()),
            FileInputItemRemoveState { disabled },
        )
        .aria_role(Role::Button)
        .focus_mode(FocusMode::Native, handle.as_ref(), true)
        .aria_label(SharedString::from(format!("Remove {}", file.name)))
        .aria(Aria::new().disabled(disabled))
        .on_press(move |event: InputEvent, window, cx| {
            entity
                .update(cx, |input, cx| input.remove_item(&target, Some(event), window, cx))
                .ok();
        })
    }
}

impl Focusable for FileInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.trigger_handle.clone()
    }
}

// A step can't pass a file, so setting and adding take none: only an empty
// list, which clears or adds nothing.
fn takes_files(args: &ui_sdk_kernel::AgentArgs) -> Result<(), AgentError> {
    match args.value(0) {
        AgentValue::Array(files) if files.is_empty() => Ok(()),
        _ => Err(AgentError::refused(
            "It takes files, and a step's text isn't one. Leave choosing a file to the person. removeFile and clear take the files it holds.",
        )),
    }
}

/// `agent: { actions: ["setValue", "addFiles", "removeFile", "clear",
/// "open"], items }`: its items are the files it holds, by key and name, and
/// `removeFile` names one by either.
impl Agent for FileInput {
    const ACTIONS: &'static [AgentAction<Self>] = &[
        AgentAction::new("setValue", |input, args, window, cx| {
            takes_files(args)?;
            Ok(input.set_value(Vec::new(), ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("addFiles", |input, args, window, cx| {
            takes_files(args)?;
            Ok(input.add_files(Vec::new(), ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("removeFile", |input, args, window, cx| {
            let named = args.string(0)?;
            let value = input.get_value();
            let file = value
                .iter()
                .find(|file| *file.key() == named)
                .or_else(|| value.iter().find(|file| file.name == named))
                .cloned();
            let Some(file) = file else {
                let names: Vec<&str> = value.iter().map(|file| file.name.as_ref()).collect();
                let names = if names.is_empty() {
                    "none".to_string()
                } else {
                    names.join(", ")
                };
                return Err(AgentError::refused(format!("No file \"{named}\". Its files: {names}.")));
            };
            Ok(input.remove_file(&file, ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("clear", |input, _args, window, cx| {
            Ok(input.clear(ChangeOptions::default(), window, cx).into())
        }),
        AgentAction::new("open", |input, _args, window, cx| Ok(input.open(window, cx).into())),
    ];

    fn agent_state(&self, _cx: &App) -> AgentState {
        let value: Vec<AgentValue> = self
            .get_value()
            .iter()
            .map(|file| AgentValue::from(file.key().to_string()))
            .collect();
        AgentState::new()
            .with("value", value)
            .with("disabled", self.is_disabled())
            .with("required", self.settings.required)
            .with("multiple", self.settings.multiple)
    }

    fn agent_items(&self, _cx: &App) -> Option<Vec<AgentItem>> {
        Some(
            self.get_value()
                .iter()
                .map(|file| AgentItem::new(file.key().clone(), file.name.clone()))
                .collect(),
        )
    }
}
