//! Port of `components/file-input/file-input-api.ts`: a file input's state
//! and what it does: the value, the picker, form participation and the
//! attachments list. Drops and pastes are in
//! [`file_input_dropzone`](crate::file_input_dropzone), and the parts and
//! their state in [`file_input`](crate::file_input).

use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{App, Context, KeyDownEvent, PathPromptOptions, SharedString, Window};
use ui_sdk_focus::{Step, move_index, radio_step};
use ui_sdk_forms::{
    FileLike, FileRejection, FileRules, FormData, FormFile, FormValue, INPUT_AND_CHANGE,
    InternalsOptions, ValidatedFiles, ValueMissingKind, bind_internals, dispatch_change,
    is_disabled, keeps_value_on_reset, validate_files, value_missing_message,
};
use ui_sdk_kernel::modality::window_source;
use ui_sdk_kernel::{
    ChangeCallback, ChangeDetails, ChangeOptions, InputEvent, Key, Reason, after_update, dispatch,
};
use ui_sdk_platform::{announce, is_rtl};

pub use ui_sdk_forms::DragStatus;

use crate::file_drop::{file_of, picked};
use crate::file_input::{
    FileInput, FileInputChange, FileInputChangeOptions, FileInputOptions,
    FileInputValueChangeDetails,
};

/// A file the file input holds, standing in for a browser's `File`: what a
/// pick, drop or paste brought, with its name, type and size read from the
/// file system. Each one is a file of its own, as each `File` is an object,
/// so the same file picked twice is two files; clones are the same file. It
/// reads as the [`FormFile`] it holds.
#[derive(Clone)]
pub struct File {
    key: SharedString,
    file: FormFile,
}

// Files have no natural key, so each gets one when it's made, kept for as
// long as the file lives.
static FILE_COUNT: AtomicU64 = AtomicU64::new(0);

impl File {
    /// A file of what a form submits, like one an app passes in
    /// `default_value`.
    pub fn new(file: FormFile) -> Self {
        let count = FILE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        Self {
            key: format!("file-{count}").into(),
            file,
        }
    }

    /// The file at `path`, with its name, type and size read from the file
    /// system. A folder isn't a file.
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        file_of(path.as_ref()).map(Self::new)
    }

    /// A key for the file that stays the same while it lives, for lists
    /// keyed by file (`fileKey`).
    pub fn key(&self) -> &SharedString {
        &self.key
    }

    /// What a form submits for it.
    pub fn form_file(&self) -> &FormFile {
        &self.file
    }

    pub fn into_form_file(self) -> FormFile {
        self.file
    }
}

impl From<FormFile> for File {
    fn from(file: FormFile) -> Self {
        Self::new(file)
    }
}

impl Deref for File {
    type Target = FormFile;

    fn deref(&self) -> &FormFile {
        &self.file
    }
}

impl PartialEq for File {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for File {}

impl Hash for File {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("File")
            .field("key", &self.key)
            .field("name", &self.file.name)
            .field("type", &self.file.mime_type)
            .field("size", &self.file.size)
            .finish()
    }
}

impl FileLike for File {
    fn name(&self) -> &str {
        &self.file.name
    }

    fn mime_type(&self) -> &str {
        &self.file.mime_type
    }

    fn size(&self) -> u64 {
        self.file.size
    }

    fn relative_path(&self) -> Option<&str> {
        self.file.relative_path.as_deref()
    }
}

/// A key for a file that stays the same while the file lives, for lists
/// keyed by file.
pub fn file_key(file: &File) -> SharedString {
    file.key.clone()
}

// Browsers submit a named file input with no files as one empty file.
fn no_file() -> FormFile {
    FormFile {
        mime_type: "application/octet-stream".into(),
        ..FormFile::default()
    }
}

fn plural(count: usize) -> String {
    format!("{count} {}", if count == 1 { "file" } else { "files" })
}

/// What a file input announces after a change, like "2 files accepted. 1
/// file rejected." or "photo.png removed." `None` when there's nothing to
/// say.
pub fn describe_change(change: &FileInputChange) -> Option<String> {
    if change.reason == Reason::Reset {
        return None;
    }
    let mut parts = Vec::new();
    if !change.added.is_empty() {
        parts.push(format!("{} accepted.", plural(change.added.len())));
    }
    if !change.rejected.is_empty() {
        parts.push(format!("{} rejected.", plural(change.rejected.len())));
    }
    if !change.removed.is_empty() && change.added.is_empty() {
        parts.push(if change.removed.len() == 1 {
            format!("{} removed.", change.removed[0].name)
        } else {
            format!("{} removed.", plural(change.removed.len()))
        });
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// What the options set beside the value and the rules.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Settings {
    pub(crate) disabled: bool,
    pub(crate) required: bool,
    pub(crate) name: Option<SharedString>,
    pub(crate) form: Option<SharedString>,
    pub(crate) multiple: bool,
    pub(crate) directory: bool,
}

pub(crate) fn settings(options: &FileInputOptions) -> Settings {
    Settings {
        disabled: options.disabled,
        required: options.required,
        name: options.name.clone(),
        form: options.form.clone(),
        multiple: options.multiple || options.directory,
        directory: options.directory,
    }
}

impl FileInput {
    /// Whether it's disabled, by itself, its field or a fieldset around it.
    /// A field disables it as the original's adapters pass the field's
    /// `disabled` into it.
    pub fn is_disabled(&self) -> bool {
        is_disabled(
            self.settings.disabled || self.internals.control_attrs().disabled,
            self.form_disabled,
        )
    }

    // The rules for the next files. With `multiple`, `max_files` counts the
    // files it holds, so a full list rejects new ones, and its drags show it.
    pub(crate) fn rules(&self) -> FileRules<File> {
        let multiple = self.settings.multiple;
        let room = match self.max_files {
            Some(max_files) if multiple => Some(max_files.saturating_sub(self.value.get().len())),
            max_files => max_files,
        };
        FileRules {
            accept: self.accept.clone(),
            multiple: Some(multiple),
            max_files: room,
            max_size: self.max_size,
            min_size: self.min_size,
            validate: self.validate.clone(),
        }
    }

    // The name the files submit under: its own, or its field's.
    fn form_name(&self) -> Option<SharedString> {
        self.settings
            .name
            .clone()
            .filter(|name| !name.is_empty())
            .or_else(|| self.internals.control_attrs().name)
    }

    /// Reports the files to its form through its internals, as a
    /// form-associated custom element does: each file under its name, or one
    /// empty file when it holds none, and a missing value while it's
    /// required and empty. The parts call it as they render, and changes
    /// call it themselves.
    pub(crate) fn sync_form(&self) {
        let value = match self.form_name() {
            Some(name) => {
                let mut data = FormData::new();
                let files = self.value.get();
                if files.is_empty() {
                    data.append(name, no_file());
                } else {
                    for file in files {
                        data.append(name.clone(), file.form_file().clone());
                    }
                }
                FormValue::Data(data)
            }
            None => FormValue::None,
        };
        bind_internals(
            &self.internals,
            InternalsOptions {
                value,
                name: self.settings.name.clone(),
                value_missing: self.settings.required && self.value.get().is_empty(),
                message: value_missing_message(ValueMissingKind::File),
                invalid: None,
                anchor: self
                    .trigger_node
                    .is_connected()
                    .then(|| self.trigger_node.clone()),
                disabled: self.settings.disabled,
                read_only: false,
                required: self.settings.required,
                barred: false,
                form: self.settings.form.clone(),
            },
        );
    }

    // `Input` and `Change` from the control, so form listeners and fields
    // hear a change. A native file input fires them for a pick itself; with
    // none here, a pick sends them too. A form reset sends none.
    fn echo_change(&self, window: &mut Window, cx: &mut App) {
        dispatch_change(Some(self.internals.node()), INPUT_AND_CHANGE, window, cx);
    }

    // Asks for the new value, with what the change added, removed and
    // rejected in `on_before_value_change`'s and `on_value_change`'s details.
    fn change_value(
        &mut self,
        next: Vec<File>,
        change: &Rc<FileInputChange>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let with_change = |callback: Option<crate::file_input::FileInputValueChange>| {
            callback.map(|callback| {
                let change = change.clone();
                Rc::new(
                    move |value: &Vec<File>, details: &ChangeDetails, window: &mut Window, cx: &mut App| {
                        let details = FileInputValueChangeDetails::new(details, &change);
                        callback(value, &details, window, cx)
                    },
                ) as ChangeCallback<Vec<File>>
            })
        };
        self.value.set_on_change(with_change(self.on_value_change.clone()));
        self.value.set_on_before_change(with_change(self.on_before_value_change.clone()));
        let changed = self.value.change(&self.instance, next, options, window, cx);
        if changed {
            cx.notify();
        }
        changed
    }

    // Reports a change with what it added, removed and rejected, then
    // announces it, unless it was canceled. A batch that changes nothing
    // because every file was rejected still reaches `on_reject` and the
    // announcement.
    pub(crate) fn commit(
        &mut self,
        next: Vec<File>,
        options: ChangeOptions,
        added: Vec<File>,
        removed: Vec<File>,
        rejected: Vec<FileRejection<File>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let reason = options.reason.unwrap_or(Reason::Imperative);
        let change = Rc::new(FileInputChange {
            reason,
            added,
            removed,
            rejected,
        });
        if cfg!(debug_assertions) && self.instance.debug {
            log::debug!(
                "[ui-sdk] file-input#{} {} added, {} removed, {} rejected ({})",
                self.instance.id(),
                change.added.len(),
                change.removed.len(),
                change.rejected.len(),
                reason.as_str()
            );
        }
        let differs = *self.value.get() != next;
        let changed = differs && self.change_value(next, &change, options.clone(), window, cx);
        // After the change, so a handler that clears messages on a change
        // doesn't clear the ones for this batch's rejections.
        if !change.rejected.is_empty()
            && let Some(callback) = self.on_reject.clone()
        {
            let rejected = change.rejected.clone();
            after_update(window, cx, move |window, cx| {
                dispatch(Some(&callback), rejected.as_slice(), options, window, cx);
            });
        }
        // A canceled change leaves the form's files as they were, and says
        // nothing.
        self.sync_form();
        if differs && !changed {
            return false;
        }
        if changed && reason != Reason::Reset {
            self.echo_change(window, cx);
        }
        let message = match &self.announcement {
            Some(announcement) => announcement(&change),
            None => describe_change(&change),
        };
        if let Some(message) = message.filter(|message| !message.is_empty()) {
            announce(&message, window, cx);
        }
        changed
    }

    // Runs files through the rules. With `multiple` they join the value, up
    // to `max_files` in all; without it, the first one replaces it, and a
    // batch with nothing accepted keeps the file it has.
    pub(crate) fn receive(
        &mut self,
        files: Vec<File>,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.is_disabled() {
            return false;
        }
        let ValidatedFiles { accepted, rejected } = validate_files(&files, &self.rules());
        let multiple = self.settings.multiple;
        if !multiple && accepted.is_empty() {
            let value = self.value.get().clone();
            return self.commit(value, options, Vec::new(), Vec::new(), rejected, window, cx);
        }
        let current = if multiple {
            self.value.get().clone()
        } else {
            Vec::new()
        };
        let added: Vec<File> = accepted
            .into_iter()
            .filter(|file| !current.contains(file))
            .collect();
        let removed: Vec<File> = if multiple {
            Vec::new()
        } else {
            self.value
                .get()
                .iter()
                .filter(|file| !added.contains(file))
                .cloned()
                .collect()
        };
        let mut next = current;
        next.extend(added.iter().cloned());
        self.commit(next, options, added, removed, rejected, window, cx)
    }

    fn remove(&mut self, file: &File, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.is_disabled() || !self.value.get().contains(file) {
            return false;
        }
        let next: Vec<File> = self
            .value
            .get()
            .iter()
            .filter(|each| *each != file)
            .cloned()
            .collect();
        self.commit(next, options, Vec::new(), vec![file.clone()], Vec::new(), window, cx)
    }

    // Replaces the files as they are, without the checks.
    fn replace(&mut self, files: Vec<File>, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let value = self.value.get();
        let added = files
            .iter()
            .filter(|file| !value.contains(file))
            .cloned()
            .collect();
        let removed = value
            .iter()
            .filter(|file| !files.contains(file))
            .cloned()
            .collect();
        self.commit(files, options, added, removed, Vec::new(), window, cx)
    }

    // A removed item's focus goes to the next one, else the one before, else
    // the trigger, once the file has gone. A controlled value comes back once
    // the app renders it, and one its owner ignored keeps the item and its
    // focus.
    pub(crate) fn remove_item(
        &mut self,
        file: &File,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let list = self.items.keys();
        let index = list.iter().position(|key| key == file.key());
        let had_focus = self
            .items
            .existing_focus_handle(file.key())
            .is_some_and(|handle| index.is_some() && handle.contains_focused(window, cx));
        let neighbour = index
            .and_then(|index| {
                list.get(index + 1)
                    .or_else(|| index.checked_sub(1).and_then(|before| list.get(before)))
            })
            .and_then(|key| self.items.existing_focus_handle(key));
        let options = ChangeOptions {
            reason: Some(Reason::RemovePress),
            event,
            source: None,
        };
        if !self.remove(file, options, window, cx) || !had_focus {
            return;
        }
        let trigger = self.trigger_handle.clone();
        let move_focus = move |kept: bool, window: &mut Window, cx: &mut App| {
            if kept {
                return;
            }
            match neighbour.filter(|neighbour| neighbour.is_rendered(window)) {
                Some(neighbour) => window.focus(&neighbour, cx),
                None if trigger.is_rendered(window) => window.focus(&trigger, cx),
                None => {}
            }
        };
        if !self.value.is_controlled() {
            move_focus(self.value.get().contains(file), window, cx);
            return;
        }
        // The app passes the value back as it renders the next frame.
        let this = cx.weak_entity();
        let file = file.clone();
        window.on_next_frame(move |window, _cx| {
            window.on_next_frame(move |window, cx| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                let kept = this.read(cx).value.get().contains(&file);
                move_focus(kept, window, cx);
            });
        });
    }

    // Arrow keys move between items, and Backspace and Delete remove the
    // focused one. The item calls it for keys pressed on itself.
    pub(crate) fn item_keydown(
        &mut self,
        event: &KeyDownEvent,
        file: &File,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.alt || modifiers.control || modifiers.platform {
            return;
        }
        let key = Key::of(&event.keystroke);
        if key == Key::Backspace || key == Key::Delete {
            window.prevent_default();
            self.remove_item(file, Some(InputEvent::KeyDown(event.clone())), window, cx);
            return;
        }
        let step = radio_step(&key, is_rtl(cx)).map(Step::By).or(match key {
            Key::Home => Some(Step::First),
            Key::End => Some(Step::Last),
            _ => None,
        });
        let Some(step) = step else {
            return;
        };
        let list = self.items.keys();
        let index = list
            .iter()
            .position(|each| each == file.key())
            .map_or(-1, |index| index as isize);
        let Some(target) = list
            .get(move_index(index, step, list.len(), false))
            .and_then(|key| self.items.existing_focus_handle(key))
        else {
            return;
        };
        window.prevent_default();
        window.focus(&target, cx);
    }

    /// The files it holds.
    pub fn get_value(&self) -> &[File] {
        self.value.get()
    }

    /// Replaces the files, without checking them. Returns false when
    /// canceled. `details.reason` is `Imperative` unless the options say.
    pub fn set_value(
        &mut self,
        files: Vec<File>,
        options: FileInputChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.replace(files, options, window, cx)
    }

    /// Runs files through the checks and adds the accepted ones, like a
    /// drop. Returns false when disabled or canceled.
    pub fn add_files(
        &mut self,
        files: Vec<File>,
        options: FileInputChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.receive(files, options, window, cx)
    }

    /// Removes one file. Returns false when it isn't there, or the change is
    /// canceled.
    pub fn remove_file(
        &mut self,
        file: &File,
        options: FileInputChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.remove(file, options, window, cx)
    }

    /// Removes every file.
    pub fn clear(&mut self, options: FileInputChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.set_value(Vec::new(), options, window, cx)
    }

    /// Opens the system's file dialog, for folders with `directory`, more
    /// than one file with `multiple`. The files chosen go through the checks
    /// with `Picker`. Returns false when disabled, or while a dialog it
    /// opened is still open.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.is_disabled() || self.picking.is_some() {
            return false;
        }
        let directory = self.settings.directory;
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: !directory,
            directories: directory,
            multiple: self.settings.multiple,
            prompt: None,
        });
        self.picking = Some(cx.spawn_in(window, async move |this, cx| {
            let paths = match chosen.await {
                Ok(Ok(Some(paths))) => paths,
                _ => {
                    this.update(cx, |this, _cx| this.picking = None).ok();
                    return;
                }
            };
            let files = picked(paths, directory, cx.background_executor()).await;
            this.update_in(cx, |this, window, cx| {
                this.picking = None;
                // A pick has no pointer or key behind it, so it's how the
                // window was used last, like the press that opened it.
                let options = ChangeOptions::reason(Reason::Picker).source(window_source(window));
                this.receive(files, options, window, cx);
            })
            .ok();
        }));
        true
    }

    /// Moves focus to the trigger, when one is in the page.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if self.trigger_handle.is_rendered(window) {
            window.focus(&self.trigger_handle, cx);
        }
    }

    /// Whether files are being dragged over one of its drop zones.
    pub fn is_dragging(&self) -> bool {
        !self.drags.is_empty()
    }

    pub(crate) fn set_active_item(&mut self, key: SharedString, cx: &mut Context<Self>) {
        if self.active_item.as_ref() != Some(&key) {
            self.active_item = Some(key);
            cx.notify();
        }
    }

    pub(crate) fn set_drag(&mut self, key: &SharedString, status: Option<DragStatus>, cx: &mut Context<Self>) {
        if self.drags.get(key).copied() == status {
            return;
        }
        match status {
            Some(status) => {
                self.drags.insert(key.clone(), status);
            }
            None => {
                self.drags.remove(key);
            }
        }
        cx.notify();
    }

    // What a form reset does: back to the files it started with, unless it's
    // controlled with no default.
    pub(crate) fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let controlled = self.value.is_controlled().then_some(());
        let default = self.has_default.then_some(());
        if !keeps_value_on_reset(controlled.as_ref(), default.as_ref()) {
            let initial = self.initial.clone();
            self.replace(initial, ChangeOptions::reason(Reason::Reset), window, cx);
        }
    }

    pub(crate) fn set_form_disabled(&mut self, form_disabled: bool, cx: &mut Context<Self>) {
        if self.form_disabled != form_disabled {
            self.form_disabled = form_disabled;
            self.sync_form();
            cx.notify();
        }
    }
}
