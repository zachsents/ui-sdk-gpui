//! Port of `components/resizable/resizable-storage.ts`: saved layouts, for
//! groups with an `auto_save_id`. One entry per group, holding a layout for
//! each set of panels it has had, so panels that come and go each get back
//! the sizes they were left at.

use std::rc::Rc;

use gpui::{App, SharedString};
use serde_json::{Map, Value};
use ui_sdk_storage::{Kind, create_local_storage};

/// Where layouts are saved: local storage by default. Anything with the
/// same two methods works, like session storage (`Kind::Session`), which
/// lasts while the app runs.
pub trait ResizableStorage {
    /// The text kept under `key`, if any (`getItem`).
    fn get_item(&self, key: &str, cx: &App) -> Option<SharedString>;

    /// Keeps `value` under `key` (`setItem`).
    fn set_item(&self, key: &str, value: &str, cx: &mut App);
}

impl ResizableStorage for Kind {
    fn get_item(&self, key: &str, cx: &App) -> Option<SharedString> {
        Kind::get_item(*self, key, cx)
    }

    fn set_item(&self, key: &str, value: &str, cx: &mut App) {
        Kind::set_item(*self, key, value.to_string(), cx);
    }
}

/// The key a group saves under, from its `auto_save_id` (`storageKey`).
pub fn storage_key(auto_save_id: &str) -> String {
    format!("ui-sdk-resizable:{auto_save_id}")
}

// Where a group saves: the storage it was given, or local storage, which is
// skipped while it can't be written, like before the app named its file.
// Saving is a convenience.
enum Store<'a> {
    Given(&'a Rc<dyn ResizableStorage>),
    Local,
}

fn store<'a>(storage: Option<&'a Rc<dyn ResizableStorage>>, key: &str, cx: &App) -> Option<Store<'a>> {
    match storage {
        Some(storage) => Some(Store::Given(storage)),
        None => create_local_storage(key.to_string())
            .available(cx)
            .then_some(Store::Local),
    }
}

fn read(store: &Store<'_>, key: &str, cx: &App) -> Option<SharedString> {
    match store {
        Store::Given(storage) => storage.get_item(key, cx),
        Store::Local => create_local_storage(key.to_string()).get(cx),
    }
}

fn entries(store: &Store<'_>, key: &str, cx: &App) -> Map<String, Value> {
    let text = read(store, key, cx);
    match serde_json::from_str::<Value>(text.as_deref().unwrap_or("{}")) {
        Ok(Value::Object(saved)) => saved,
        _ => Map::new(),
    }
}

/// The layout saved for these panels, if there is one that fits them.
pub fn load_layout(
    auto_save_id: &str,
    ids: &[SharedString],
    storage: Option<&Rc<dyn ResizableStorage>>,
    cx: &App,
) -> Option<Vec<f64>> {
    let key = storage_key(auto_save_id);
    let store = store(storage, &key, cx)?;
    let panels = ids.iter().map(|id| id.as_ref()).collect::<Vec<_>>().join(",");
    let Value::Array(layout) = entries(&store, &key, cx).remove(&panels)? else {
        return None;
    };
    let layout: Vec<f64> = layout
        .iter()
        .map(|size| size.as_f64().filter(|size| size.is_finite()))
        .collect::<Option<_>>()?;
    (layout.len() == ids.len()).then_some(layout)
}

/// Saves the layout for these panels, beside the ones saved for other sets
/// of panels.
pub fn save_layout(
    auto_save_id: &str,
    ids: &[SharedString],
    layout: &[f64],
    storage: Option<&Rc<dyn ResizableStorage>>,
    cx: &mut App,
) {
    let key = storage_key(auto_save_id);
    let Some(store) = store(storage, &key, cx) else {
        return;
    };
    let mut saved = entries(&store, &key, cx);
    let panels = ids.iter().map(|id| id.as_ref()).collect::<Vec<_>>().join(",");
    let sizes = layout
        .iter()
        .map(|size| Value::from((size * 1000.0).round() / 1000.0))
        .collect();
    saved.insert(panels, Value::Array(sizes));
    let text = Value::Object(saved).to_string();
    // Full or blocked storage keeps the layout for this run only.
    match store {
        Store::Given(storage) => storage.set_item(&key, &text, cx),
        Store::Local => {
            create_local_storage(key).set(text, cx);
        }
    }
}
