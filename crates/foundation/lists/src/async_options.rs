//! Port of `foundation/lists/async-options.ts`: options loaded for what the
//! user typed, like a search on the server. It waits until typing pauses,
//! cancels a request the next one replaces, so only the latest answer lands,
//! and keeps the last items while the next load runs. Pass its items, status
//! and `set_query` to a combobox with `filter` off, or use any data layer
//! instead.
//!
//! The original is a store with `subscribe`. Here it's an entity: make it
//! with [`create_async_options`], observe it with `cx.observe`, and call its
//! methods through `update`. Every change notifies.

use std::rc::Rc;
use std::time::Duration;

use futures::future::LocalBoxFuture;
use gpui::{App, AppContext, Context, Entity, SharedString, Task};
use ui_sdk_popups::AbortSignal;
use ui_sdk_text::js_trim;

/// Whether options are loading, loaded or failed (`AsyncOptionsStatus`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AsyncOptionsStatus {
    #[default]
    Idle,
    Loading,
    Error,
}

impl AsyncOptionsStatus {
    /// The original's name for it, like `"loading"`.
    pub fn as_str(self) -> &'static str {
        match self {
            AsyncOptionsStatus::Idle => "idle",
            AsyncOptionsStatus::Loading => "loading",
            AsyncOptionsStatus::Error => "error",
        }
    }
}

/// What async options hold (`AsyncOptionsState`).
#[derive(Clone, Debug, PartialEq)]
pub struct AsyncOptionsState<Item> {
    /// The text the items are for, or are loading for.
    pub query: SharedString,
    /// The loaded items. The previous ones stay while the next load runs.
    pub items: Rc<[Item]>,
    /// Whether a load is waiting for typing to pause or running, or the last
    /// one failed.
    pub status: AsyncOptionsStatus,
    /// Why the last load failed.
    pub error: Option<SharedString>,
    /// Whether `load_more` has another page to load.
    pub has_more: bool,
}

/// What `load` receives besides the text (`AsyncOptionsContext`).
#[derive(Clone, Debug)]
pub struct AsyncOptionsContext {
    /// Aborted when a newer load replaces this one.
    pub signal: AbortSignal,
    /// Which page to load: 0 first, then 1, 2… from `load_more`.
    pub page: usize,
}

/// What [`create_async_options`] takes (`AsyncOptionsOptions`).
#[derive(Clone, Debug)]
pub struct AsyncOptionsOptions {
    /// How long typing must pause before a load starts. Defaults to 200ms.
    pub debounce: Duration,
    /// The text to load for at first. Defaults to empty.
    pub query: SharedString,
    /// Whether to load for the first text right away, before anyone types.
    pub immediate: bool,
    /// How many characters the trimmed text needs before anything loads.
    /// Shorter text clears the items and loads nothing.
    pub min_length: usize,
}

impl Default for AsyncOptionsOptions {
    fn default() -> Self {
        Self {
            debounce: Duration::from_millis(200),
            query: SharedString::default(),
            immediate: false,
            min_length: 0,
        }
    }
}

impl AsyncOptionsOptions {
    pub fn debounce(mut self, debounce: Duration) -> Self {
        self.debounce = debounce;
        self
    }

    pub fn query(mut self, query: impl Into<SharedString>) -> Self {
        self.query = query.into();
        self
    }

    pub fn immediate(mut self, immediate: bool) -> Self {
        self.immediate = immediate;
        self
    }

    pub fn min_length(mut self, min_length: usize) -> Self {
        self.min_length = min_length;
        self
    }
}

/// What a load answers: its items, and whether there's another page.
#[derive(Clone, Debug, PartialEq)]
pub struct AsyncOptionsPage<Item> {
    pub items: Vec<Item>,
    pub has_more: bool,
}

impl<Item> From<Vec<Item>> for AsyncOptionsPage<Item> {
    fn from(items: Vec<Item>) -> Self {
        Self { items, has_more: false }
    }
}

/// Loads the items for a text: the text, which page, and the signal a newer
/// load aborts. It returns the work, which can run on the background
/// executor; its error is why it failed.
pub type AsyncOptionsLoad<Item> = Rc<
    dyn Fn(SharedString, AsyncOptionsContext, &mut App) -> LocalBoxFuture<'static, Result<AsyncOptionsPage<Item>, SharedString>>,
>;

/// Options loaded for a text, latest first (`createAsyncOptions`'s return).
pub struct AsyncOptions<Item: 'static> {
    load: AsyncOptionsLoad<Item>,
    debounce: Duration,
    min_length: usize,
    state: AsyncOptionsState<Item>,
    timer: Option<Task<()>>,
    controller: Option<AbortSignal>,
    running: Option<Task<()>>,
    page: usize,
}

/// Loads options for a text, latest first (`createAsyncOptions`). `load`
/// returns the items, or a page of them with whether there's more.
pub fn create_async_options<Item: Clone + 'static>(
    load: impl Fn(SharedString, AsyncOptionsContext, &mut App) -> LocalBoxFuture<'static, Result<AsyncOptionsPage<Item>, SharedString>>
    + 'static,
    options: AsyncOptionsOptions,
    cx: &mut App,
) -> Entity<AsyncOptions<Item>> {
    cx.new(|cx| AsyncOptions::new(Rc::new(load), options, cx))
}

impl<Item: Clone + 'static> AsyncOptions<Item> {
    /// The options for `load`, made in their entity.
    pub fn new(load: AsyncOptionsLoad<Item>, options: AsyncOptionsOptions, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            load,
            debounce: options.debounce,
            min_length: options.min_length,
            state: AsyncOptionsState {
                query: options.query,
                items: Vec::new().into(),
                status: AsyncOptionsStatus::Idle,
                error: None,
                has_more: false,
            },
            timer: None,
            controller: None,
            running: None,
            page: 0,
        };
        if options.immediate {
            this.run(0, cx);
        }
        this
    }

    /// The current items, text and status (`getState`).
    pub fn get_state(&self) -> &AsyncOptionsState<Item> {
        &self.state
    }

    fn set(&mut self, patch: impl FnOnce(&mut AsyncOptionsState<Item>), cx: &mut Context<Self>) {
        patch(&mut self.state);
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.timer = None;
        if let Some(controller) = self.controller.take() {
            controller.abort(cx);
        }
        self.running = None;
    }

    fn too_short(&self, text: &str) -> bool {
        js_trim(text).chars().count() < self.min_length
    }

    fn clear(&mut self, text: SharedString, cx: &mut Context<Self>) {
        self.stop(cx);
        self.set(
            |state| {
                state.query = text;
                state.items = Vec::new().into();
                state.status = AsyncOptionsStatus::Idle;
                state.error = None;
                state.has_more = false;
            },
            cx,
        );
    }

    fn run(&mut self, next: usize, cx: &mut Context<Self>) {
        if self.too_short(&self.state.query) {
            let query = self.state.query.clone();
            self.clear(query, cx);
            return;
        }
        if let Some(controller) = self.controller.take() {
            controller.abort(cx);
        }
        let current = AbortSignal::new();
        self.controller = Some(current.clone());
        self.page = next;
        self.set(
            |state| {
                state.status = AsyncOptionsStatus::Loading;
                state.error = None;
            },
            cx,
        );
        let work = (self.load)(
            self.state.query.clone(),
            AsyncOptionsContext {
                signal: current.clone(),
                page: next,
            },
            cx,
        );
        self.running = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                if current.is_aborted() {
                    return;
                }
                this.controller = None;
                match result {
                    Ok(page) => this.set(
                        |state| {
                            state.items = if next == 0 {
                                page.items.into()
                            } else {
                                state.items.iter().cloned().chain(page.items).collect::<Vec<_>>().into()
                            };
                            state.status = AsyncOptionsStatus::Idle;
                            state.has_more = page.has_more;
                        },
                        cx,
                    ),
                    Err(error) => this.set(
                        |state| {
                            state.status = AsyncOptionsStatus::Error;
                            state.error = Some(error);
                        },
                        cx,
                    ),
                }
            })
            .ok();
        }));
    }

    /// Loads for new text once typing pauses, replacing any load still
    /// running (`setQuery`).
    pub fn set_query(&mut self, next: impl Into<SharedString>, cx: &mut Context<Self>) {
        let next = next.into();
        if next == self.state.query && (self.state.status == AsyncOptionsStatus::Loading || self.timer.is_some()) {
            return;
        }
        if self.too_short(&next) {
            self.clear(next, cx);
            return;
        }
        self.stop(cx);
        // Loading from the first key press: while typing pauses, the last
        // items aren't an answer to the new text, so nothing says "no match".
        self.set(
            |state| {
                state.query = next;
                state.status = AsyncOptionsStatus::Loading;
                state.error = None;
            },
            cx,
        );
        let debounce = self.debounce;
        self.timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(debounce).await;
            this.update(cx, |this, cx| {
                this.timer = None;
                this.run(0, cx);
            })
            .ok();
        }));
    }

    /// Loads the current text again, now.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.stop(cx);
        self.run(0, cx);
    }

    /// Loads the next page and appends it, when `load` said there's more
    /// (`loadMore`).
    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.state.status == AsyncOptionsStatus::Loading || !self.state.has_more {
            return;
        }
        let next = self.page + 1;
        self.run(next, cx);
    }

    /// Stops any pending or running load.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.stop(cx);
        if self.state.status == AsyncOptionsStatus::Loading {
            self.set(|state| state.status = AsyncOptionsStatus::Idle, cx);
        }
    }
}
