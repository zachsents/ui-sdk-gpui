//! Port of `components/file-input/file-drop.ts`: the files a drop or a
//! paste carries, read from the file system, which zone took a drop or a
//! paste, and whether a drag is over a zone.
//!
//! gpui hands files dragged in from the system as a drag of
//! [`ExternalPaths`], apart from the typed drags an app starts itself, so a
//! drag carries files exactly when its value is one: the original's
//! `carriesFiles`, and the page drags it tells apart from file drags. The
//! paths are known from the start of the drag. Each file's name, type and
//! size are read from the file system, where the original reads a `File`:
//! its type from its extension, as browsers type one.

use std::cell::Cell;
use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};

use gpui::{App, BackgroundExecutor, ClipboardEntry, ClipboardItem, ExternalPaths, Image, SharedString, Task};
use ui_sdk_forms::{FormFile, set_relative_path};

use crate::file_input_api::File;

/// A file's MIME type, from its extension, like `"image/png"`, the way a
/// browser types a `File`. Empty when nothing says.
pub fn mime_of(path: &Path) -> SharedString {
    mime_guess::from_path(path).first_raw().unwrap_or("").into()
}

fn form_file(path: &Path, metadata: &Metadata) -> FormFile {
    FormFile {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default()
            .into(),
        mime_type: mime_of(path),
        size: metadata.len(),
        path: Some(path.to_path_buf()),
        relative_path: None,
    }
}

/// The file at `path`, with its name, type and size, as a form submits it.
/// A folder isn't a file.
pub fn file_of(path: &Path) -> std::io::Result<FormFile> {
    let metadata = fs::metadata(path)?;
    if metadata.is_dir() {
        return Err(std::io::Error::other("a folder isn't a file"));
    }
    Ok(form_file(path, &metadata))
}

/// Whether a drag carries files from outside the app, rather than something
/// dragged within it: whether its value is the system's paths.
pub fn carries_files(drag: &dyn std::any::Any) -> bool {
    drag.is::<ExternalPaths>()
}

/// The MIME types of the files a drag carries, one for each dragged path,
/// which judge the drag before the drop. A folder has none, as a browser
/// reports it.
pub fn dragged_types(paths: &ExternalPaths) -> Vec<SharedString> {
    paths
        .paths()
        .iter()
        .map(|path| {
            if path.is_dir() {
                SharedString::default()
            } else {
                mime_of(path)
            }
        })
        .collect()
}

/// The files a paste carries, like a screenshot or files copied in the
/// system's file manager. Empty for text.
#[derive(Clone, Debug, Default)]
pub struct PastedFiles {
    /// Files on disk.
    pub paths: Vec<PathBuf>,
    /// Images held in memory, which become files once written.
    pub images: Vec<Image>,
}

impl PastedFiles {
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty() && self.images.is_empty()
    }
}

/// The files a paste carries: the clipboard's files and images, never its
/// text.
pub fn pasted_files(item: &ClipboardItem) -> PastedFiles {
    let mut pasted = PastedFiles::default();
    for entry in item.entries() {
        match entry {
            ClipboardEntry::ExternalPaths(paths) => {
                pasted.paths.extend(paths.paths().iter().cloned());
            }
            ClipboardEntry::Image(image) => pasted.images.push(image.clone()),
            ClipboardEntry::String(_) => {}
        }
    }
    pasted
}

// A pasted image as a file the app can read: written to the system's
// temporary folder, and named as a browser names a pasted image.
fn image_file(image: &Image) -> Option<FormFile> {
    let extension = image.format.extension();
    let path = std::env::temp_dir().join(format!("ui-sdk-paste-{}.{extension}", image.id));
    fs::write(&path, &image.bytes).ok()?;
    Some(FormFile {
        name: format!("image.{extension}").into(),
        mime_type: image.format.mime_type().into(),
        size: image.bytes.len() as u64,
        path: Some(path),
        relative_path: None,
    })
}

const SYSTEM_FILES: [&str; 3] = [".DS_Store", "Thumbs.db", "desktop.ini"];

/// The most files read from dropped folders, so a huge folder can't stall
/// the app.
pub const FOLDER_FILE_LIMIT: usize = 1000;

// Where a file sits below `root`, with `/` between its folders.
fn relative(path: &Path, root: &Path) -> String {
    let inside = path.strip_prefix(root).unwrap_or(path);
    inside
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

// Reads one dropped entry: a file, or a folder and everything in it. A
// symbolic link to a folder is left out, so a link to a folder above can't
// loop.
fn visit(path: &Path, root: &Path, files: &mut Vec<File>, limit: usize, skip_system: bool) {
    if files.len() >= limit {
        return;
    }
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    let metadata = if metadata.file_type().is_symlink() {
        match fs::metadata(path) {
            Ok(target) if target.is_file() => target,
            _ => return,
        }
    } else {
        metadata
    };
    if metadata.is_file() {
        let name = path.file_name().map(|name| name.to_string_lossy());
        if skip_system && name.is_some_and(|name| SYSTEM_FILES.contains(&name.as_ref())) {
            return;
        }
        let mut file = form_file(path, &metadata);
        set_relative_path(&mut file, relative(path, root));
        files.push(File::new(file));
        return;
    }
    if !metadata.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    children.sort();
    for child in children {
        if files.len() >= limit {
            return;
        }
        visit(&child, root, files, limit, skip_system);
    }
}

/// Reads files from dropped entries, walking into folders, up to `limit`
/// files, and skipping system files like `.DS_Store`. Each file remembers
/// its path inside the folder, from the dropped folder's own name, for
/// `relative_path`. Entries that can't be read are left out.
pub fn read_entries(paths: &[PathBuf], limit: usize) -> Vec<File> {
    let mut files = Vec::new();
    for path in paths {
        let root = path.parent().unwrap_or(Path::new(""));
        visit(path, root, &mut files, limit, true);
    }
    files
}

/// Every file inside chosen folders, as a browser's folder picker gives
/// them: all of them, each with its path from the chosen folder's name.
pub fn read_folders(paths: &[PathBuf]) -> Vec<File> {
    let mut files = Vec::new();
    for path in paths {
        let root = path.parent().unwrap_or(Path::new(""));
        visit(path, root, &mut files, usize::MAX, false);
    }
    files
}

/// The files a drop, a paste or a pick brings: right away, or once they're
/// read.
pub enum DroppedFiles {
    Ready(Vec<File>),
    /// Files read in the background, like the ones inside dropped folders.
    Reading(Task<Vec<File>>),
}

fn files_at(paths: &[PathBuf]) -> Vec<File> {
    paths
        .iter()
        .filter_map(|path| file_of(path).ok())
        .map(File::new)
        .collect()
}

/// The files a drop carries. Without folders, that's the files themselves,
/// right away. With folders, every file inside them once they're read, in
/// the background, plain files read like the rest.
pub fn dropped_files(paths: &ExternalPaths, cx: &App) -> DroppedFiles {
    let paths = paths.paths().to_vec();
    if !paths.iter().any(|path| path.is_dir()) {
        return DroppedFiles::Ready(files_at(&paths));
    }
    DroppedFiles::Reading(
        cx.background_executor()
            .spawn(async move { read_entries(&paths, FOLDER_FILE_LIMIT) }),
    )
}

/// The files a paste carries: files on disk right away, and images once
/// they're written to the temporary folder, in the background.
pub fn pasted(pasted: PastedFiles, cx: &App) -> DroppedFiles {
    let PastedFiles { paths, images } = pasted;
    let mut files = files_at(&paths);
    if images.is_empty() {
        return DroppedFiles::Ready(files);
    }
    DroppedFiles::Reading(cx.background_executor().spawn(async move {
        files.extend(images.iter().filter_map(image_file).map(File::new));
        files
    }))
}

/// The files a picker chose, read in the background: every file inside the
/// folders a folder picker chose, or the chosen files themselves.
pub fn picked(paths: Vec<PathBuf>, directory: bool, executor: &BackgroundExecutor) -> Task<Vec<File>> {
    executor.spawn(async move {
        if directory {
            read_folders(&paths)
        } else {
            files_at(&paths)
        }
    })
}

thread_local! {
    // A zone took the drop or paste being handled now.
    static CLAIMED: Cell<bool> = const { Cell::new(false) };
}

/// Marks the drop or paste being handled now as taken by a zone, until the
/// update handling it is done.
pub fn claim(cx: &mut App) {
    if !CLAIMED.with(|claimed| claimed.replace(true)) {
        cx.defer(|_| CLAIMED.with(|claimed| claimed.set(false)));
    }
}

/// Whether a zone already took the drop or paste being handled now.
pub fn is_claimed() -> bool {
    CLAIMED.with(Cell::get)
}

/// Whether a drag of files is over a zone. gpui reports a drag over an
/// element by hit testing, children and all, so it's over the zone while a
/// frame finds it under the pointer, and the original's count of the
/// children it entered, which keeps the state from flickering across them,
/// isn't needed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DragTracker {
    over: bool,
}

impl DragTracker {
    pub fn is_over(&self) -> bool {
        self.over
    }

    /// The drag moved, over the zone or not. Returns the new state when it
    /// changed.
    pub fn moved(&mut self, over: bool) -> Option<bool> {
        (self.over != over).then(|| {
            self.over = over;
            over
        })
    }

    /// The drag ended. Returns whether it was over the zone.
    pub fn reset(&mut self) -> bool {
        std::mem::take(&mut self.over)
    }
}

/// A tracker for a zone with no drag over it.
pub fn create_drag_tracker() -> DragTracker {
    DragTracker::default()
}
