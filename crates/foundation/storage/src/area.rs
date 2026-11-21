//! localStorage and sessionStorage themselves, which the browser provides and
//! `storage.rs` builds on.
//!
//! In gpui they're one store for the whole app, held in a gpui global. Local
//! storage lasts between runs, in a file the app names with [`init`] in the
//! platform's per-user data directory. Session storage lasts while the app
//! runs. Every window shares both, so a write reaches the readers in every
//! window, which hear it through the global's observers the way other tabs
//! hear the browser's `storage` event.
//!
//! The file is written on a background thread at the end of each update that
//! changed a value: to a temporary file beside it, then renamed over it, so
//! it's never half written. What hasn't reached it when the app quits is
//! written before it does.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use gpui::{App, Global, SharedString, Subscription};

/// The file local storage keeps its values in, in the app's directory.
const FILE_NAME: &str = "local-storage.json";

// Whether a write to local storage before `init` was reported this run.
static WARNED: AtomicBool = AtomicBool::new(false);

/// Where a value is kept: localStorage or sessionStorage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Lasts until it's removed, between runs, once [`init`] names its file.
    Local,
    /// Lasts while the app runs.
    Session,
}

impl Kind {
    /// The original's name for it: `"local"` or `"session"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Local => "local",
            Kind::Session => "session",
        }
    }

    /// Whether the area takes writes: local storage once [`init`] named its
    /// file, session storage always.
    pub fn available(self, cx: &App) -> bool {
        match self {
            Kind::Local => local_file(cx).is_some(),
            Kind::Session => true,
        }
    }

    /// The value kept under `key`, or `None` when nothing is (`getItem`).
    pub fn get_item(self, key: &str, cx: &App) -> Option<SharedString> {
        cx.try_global::<Areas>()?.area(self).values.get(key).cloned()
    }

    /// Keeps `value` under `key` (`setItem`). Returns false when the area is
    /// unavailable.
    pub fn set_item(self, key: &str, value: impl Into<SharedString>, cx: &mut App) -> bool {
        write(self, key, Some(value.into()), cx)
    }

    /// Removes the value under `key` (`removeItem`). Returns false when the
    /// area is unavailable.
    pub fn remove_item(self, key: &str, cx: &mut App) -> bool {
        write(self, key, None, cx)
    }
}

#[derive(Default)]
struct Area {
    values: BTreeMap<SharedString, SharedString>,
    // The writes each key took, so a subscriber can tell its key took one,
    // even of the value it already had.
    writes: HashMap<SharedString, u64>,
}

impl Area {
    fn wrote(&mut self, key: &str) {
        *self.writes.entry(key.into()).or_default() += 1;
    }
}

/// Both areas, for the whole app. A write through the global tells its
/// observers at the end of the update.
#[derive(Default)]
pub(crate) struct Areas {
    local: Area,
    session: Area,
    file: Option<LocalFile>,
}

impl Global for Areas {}

impl Areas {
    fn area(&self, kind: Kind) -> &Area {
        match kind {
            Kind::Local => &self.local,
            Kind::Session => &self.session,
        }
    }

    fn area_mut(&mut self, kind: Kind) -> &mut Area {
        match kind {
            Kind::Local => &mut self.local,
            Kind::Session => &mut self.session,
        }
    }
}

/// How many writes `key` took, which tells subscribers it took another.
pub(crate) fn writes(kind: Kind, key: &str, cx: &App) -> u64 {
    cx.try_global::<Areas>()
        .and_then(|areas| areas.area(kind).writes.get(key).copied())
        .unwrap_or(0)
}

// The file behind local storage, and the writes on their way to it.
struct LocalFile {
    name: String,
    path: PathBuf,
    // Whether a write is due at the end of this update.
    due: Cell<bool>,
    // The last snapshot sent to be written, and the last one that landed.
    // Snapshots land in the order they were taken, so one that finishes late
    // never replaces a newer one.
    sent: Cell<u64>,
    landed: Arc<Mutex<u64>>,
    _quit: Subscription,
}

fn local_file(cx: &App) -> Option<&LocalFile> {
    cx.try_global::<Areas>()?.file.as_ref()
}

/// Names the file local storage keeps its values in between runs:
/// `local-storage.json` in a directory called `name` in the platform's
/// per-user data directory, like `~/Library/Application Support/<name>` on a
/// Mac. Call it as the app starts. Local storage is unavailable until then,
/// and where the directory can't be made.
pub fn init(name: &str, cx: &mut App) {
    assert!(
        !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\']),
        "Local storage needs a directory name, like init(\"my-app\", cx), not {name:?}."
    );
    if let Some(file) = local_file(cx) {
        if cfg!(debug_assertions) && file.name != name {
            log::warn!(
                "[ui-sdk] Local storage already keeps its values under {:?}, so it ignores {name:?}.",
                file.name
            );
        }
        return;
    }
    let Some(dir) = dirs::data_dir().map(|dir| dir.join(name)) else {
        log::warn!("[ui-sdk] Local storage is unavailable: the platform has no data directory.");
        return;
    };
    if let Err(error) = fs::create_dir_all(&dir) {
        log::warn!("[ui-sdk] Local storage is unavailable: {}: {error}", dir.display());
        return;
    }
    let path = dir.join(FILE_NAME);
    let values = load(&path);
    let quit = cx.on_app_quit(|cx| {
        finish(cx);
        async {}
    });
    let areas = cx.default_global::<Areas>();
    // What was saved reaches anything that subscribed before.
    for key in values.keys() {
        areas.local.wrote(key);
    }
    areas.local.values = values;
    areas.file = Some(LocalFile {
        name: name.to_string(),
        path,
        due: Cell::new(false),
        sent: Cell::new(0),
        landed: Arc::new(Mutex::new(0)),
        _quit: quit,
    });
}

// The values the file holds. A missing file holds none. One that can't be
// read or parsed holds none too, and the next write replaces it.
fn load(path: &Path) -> BTreeMap<SharedString, SharedString> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return BTreeMap::new(),
        Err(error) => {
            log::warn!("[ui-sdk] Couldn't read local storage from {}: {error}", path.display());
            return BTreeMap::new();
        }
    };
    match serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text) {
        Ok(saved) => saved
            .into_iter()
            .filter_map(|(key, value)| match value {
                serde_json::Value::String(value) => Some((key.into(), value.into())),
                _ => None,
            })
            .collect(),
        Err(error) => {
            log::warn!("[ui-sdk] Couldn't parse local storage in {}: {error}", path.display());
            BTreeMap::new()
        }
    }
}

// Keeps or removes the value under `key`, which every subscriber hears.
fn write(kind: Kind, key: &str, value: Option<SharedString>, cx: &mut App) -> bool {
    if !kind.available(cx) {
        if cfg!(debug_assertions) && !WARNED.swap(true, Ordering::Relaxed) {
            log::warn!(
                "[ui-sdk] Local storage is unavailable, so nothing is saved. Name its file with ui_sdk_storage::init as the app starts."
            );
        }
        return false;
    }
    let area = cx.default_global::<Areas>().area_mut(kind);
    let changed = match value {
        Some(value) => area.values.insert(key.into(), value.clone()).as_ref() != Some(&value),
        None => area.values.remove(key).is_some(),
    };
    area.wrote(key);
    if kind == Kind::Local && changed {
        save_soon(cx);
    }
    true
}

// Saves at the end of this update, so a run of writes saves once.
fn save_soon(cx: &mut App) {
    let Some(file) = local_file(cx) else {
        return;
    };
    if file.due.replace(true) {
        return;
    }
    cx.defer(|cx| {
        if let Some(snapshot) = snapshot(cx) {
            cx.background_executor()
                .spawn(async move { snapshot.write() })
                .detach();
        }
    });
}

// Writes what hasn't landed yet before the app quits, since work on
// background threads stops with it.
fn finish(cx: &App) {
    let unsaved = local_file(cx)
        .is_some_and(|file| file.due.get() || *lock(&file.landed) < file.sent.get());
    if unsaved && let Some(snapshot) = snapshot(cx) {
        snapshot.write();
    }
}

// What the file should hold now, numbered after the last one sent.
struct Snapshot {
    path: PathBuf,
    contents: String,
    number: u64,
    landed: Arc<Mutex<u64>>,
}

fn snapshot(cx: &App) -> Option<Snapshot> {
    let areas = cx.try_global::<Areas>()?;
    let file = areas.file.as_ref()?;
    file.due.set(false);
    let contents = match serde_json::to_string_pretty(&areas.local.values) {
        Ok(contents) => contents,
        Err(error) => {
            log::warn!("[ui-sdk] Couldn't save local storage: {error}");
            return None;
        }
    };
    let number = file.sent.get() + 1;
    file.sent.set(number);
    Some(Snapshot {
        path: file.path.clone(),
        contents,
        number,
        landed: file.landed.clone(),
    })
}

impl Snapshot {
    // Writes a temporary file beside the file and renames it over it, unless
    // a newer snapshot landed first.
    fn write(self) {
        let temp = self
            .path
            .with_extension(format!("{}-{}.tmp", std::process::id(), self.number));
        let landed = self.write_through(&temp);
        if !matches!(landed, Ok(true)) {
            fs::remove_file(&temp).ok();
        }
        if let Err(error) = landed {
            log::warn!(
                "[ui-sdk] Couldn't save local storage to {}: {error}",
                self.path.display()
            );
        }
    }

    fn write_through(&self, temp: &Path) -> io::Result<bool> {
        {
            let mut file = fs::File::create(temp)?;
            file.write_all(self.contents.as_bytes())?;
            file.sync_all()?;
        }
        let mut landed = lock(&self.landed);
        if *landed >= self.number {
            return Ok(false);
        }
        fs::rename(temp, &self.path)?;
        *landed = self.number;
        Ok(true)
    }
}

fn lock(landed: &Mutex<u64>) -> MutexGuard<'_, u64> {
    landed.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
