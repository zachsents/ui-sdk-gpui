//! Port of `components/file-input`: the files someone chooses, drops or
//! pastes, checked against the app's rules and submitted with a form.
//! Uploading them is the app's.
//!
//! `file-input.ts` → [`file_input`], `file-input-api.ts` →
//! [`file_input_api`], `file-input-dropzone.ts` → [`file_input_dropzone`],
//! `file-drop.ts` → [`file_drop`]. The parts are [`FileInput::root`],
//! [`FileInput::trigger`], [`FileInput::dropzone`], [`FileInput::list`],
//! [`FileInput::item`] and [`FileInput::item_remove`]; an item and its
//! remove button take the [`File`] they show, the original's
//! `FileInputItemProps`.
//!
//! A [`File`] stands in for a browser's `File`: the name, type and size read
//! from the file system, and the path. Each is a file of its own, keyed by
//! [`File::key`] (`fileKey`), so the same file picked twice is two files, as
//! two `File` objects are. The trigger opens the system's file dialog
//! (`prompt_for_paths`), with `directory` for folders. Files dragged in from
//! the system are gpui's [`ExternalPaths`](gpui::ExternalPaths) drags; a
//! paste is the platform's paste keys, or the text field's `Paste` action,
//! with the clipboard's files and images. The root takes part in a form
//! through its `ElementInternals`, as a custom element does in the original:
//! each file under the name, one empty file when it holds none, and a
//! missing value while it's required and empty, with the trigger as the
//! control a label or field names and the message points at. A field names
//! the trigger by its label and then the trigger's own name, as it does a
//! plain button, and disables the file input with it.
//!
//! # Dropped, and why
//!
//! - `createFileInputController`, the `controller` option and the trigger's
//!   `FileInputTriggerProps`: the entity is the controller, and a trigger
//!   rendered anywhere opens its file input's dialog.
//! - The `input` slot, the hidden `<input type="file">`, and what kept it in
//!   step (`setInputFiles`, `keepsFiles`, `writeInput`, `bindInput`,
//!   `echoing`, `hiddenInputStyle`, the `associated` flag): gpui has no
//!   native file input, so every file input reports through its internals.
//! - `capture`, which opens a phone's camera: gpui's file dialog has none,
//!   and desktop browsers ignore it too.
//! - `guardFileDrops`, `isFileInput`, the page-drag flag and `dropEffect`: a
//!   drop no one takes does nothing in gpui, there are no native file inputs,
//!   gpui's drags are typed, and the system's drag feedback isn't the app's.
//! - The File and Directory Entries API (`DroppedEntry`): dropped folders
//!   are read from the file system, by path.
//! - `zoneOf`: gpui hands a drop to the innermost element under the pointer
//!   that takes one.
//! - A CSS selector as a `paste_target`: gpui has no selectors; pass the
//!   element's focus handle, or a function that returns it.
//! - Restoring files after going back: gpui has no history.
//!
//! # Deviations
//!
//! - gpui's file dialog has no type filter, so files `accept` rules out can
//!   be chosen, and are rejected after the pick with `FileType`.
//! - `Input` and `Change` go out after a pick too, since no native input
//!   fires them; a form reset sends none.
//! - [`FileInput::open`] returns false while a dialog it opened is still
//!   open, as nothing opens then.
//! - An element inside a zone that takes `ExternalPaths` drops itself takes
//!   them before the zone, where the original's zone listens first.
//! - A window-wide zone hears drags only while it paints: hide one with no
//!   size or opacity, not `invisible`. It takes a drop no zone took, even one
//!   an app's own drop handler took.
//! - A removed item's focus moves on once the app renders a controlled value
//!   back, the frame after the change, rather than a microtask later.
//! - A file's type comes from its extension, through `mime_guess`, as
//!   browsers type most files; a folder has none.

pub mod file_drop;
pub mod file_input;
pub mod file_input_api;
pub mod file_input_dropzone;

pub use file_drop::{
    DragTracker, DroppedFiles, FOLDER_FILE_LIMIT, PastedFiles, carries_files, claim,
    create_drag_tracker, dragged_types, dropped_files, file_of, is_claimed, mime_of, pasted,
    pasted_files, picked, read_entries, read_folders,
};
pub use file_input::{
    FileInput, FileInputAnnouncement, FileInputChange, FileInputChangeOptions,
    FileInputDropzoneProps, FileInputDropzoneState, FileInputDropzoneTarget,
    FileInputItemRemoveState, FileInputItemState, FileInputOptions, FileInputPasteTarget,
    FileInputReject, FileInputState, FileInputValueChange, FileInputValueChangeDetails,
    FileInputValueChangeReason,
};
pub use file_input_api::{DragStatus, File, describe_change, file_key};
pub use ui_sdk_forms::{
    FileLike, FileRejection, FileRejectionReason, FileRules, format_file_size, relative_path,
};
