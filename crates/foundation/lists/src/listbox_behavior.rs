//! Port of `foundation/lists/listbox-behavior.ts`: a highlight that moves
//! through options with the keys, the way the WAI-ARIA listbox and combobox
//! patterns ask, and options filtered and ranked by a query, for a component
//! of your own (`createListbox`).
//!
//! Make it in the component's constructor, keep it in a field, and wire the
//! option part as it renders:
//!
//! ```ignore
//! let option = ListboxOption::keyed(key.clone()).label(label);
//! if !this.listbox.shows(this, &option) {
//!     return None;
//! }
//! let attrs = this.listbox.attrs(&option);
//! let part = Part::new(&this.instance, "option", Some(&key), attrs.clone())
//!     .aria_role(Role::Option)
//!     .focus_mode(FocusMode::ActiveDescendant, None, false);
//! Some(this.listbox.bind(&option, attrs.apply(part), cx))
//! ```
//!
//! With virtual focus, the element that keeps focus calls
//! [`ListboxBehavior::keydown`] and points `Aria::active_descendant` at
//! [`ListboxBehavior::active_id`]. Choosing an option is yours: write it with
//! a `Controllable`, or a selection model for several.

use std::cell::{Cell, RefCell};
use std::fmt::Debug;
use std::rc::Rc;

use gpui::{App, Context, FocusHandle, KeyDownEvent, SharedString, Window};
use ui_sdk_focus::Step;
use ui_sdk_kernel::{Aria, InputEvent, Key, Part};
use ui_sdk_platform::is_apple;
use ui_sdk_text::is_typeahead_key;

use crate::listbox::create_filtered_listbox;
use crate::listbox_engine::{
    HighlightOptions, ItemKey, ItemMeta, ListData, ListFilter, ListFocus, ListHighlightDetails, ListHookFn,
    ListScroller, ListboxEngine, ListboxEngineConfig, WeakListboxEngine, item_key,
};
use crate::listbox_keyboard::list_step;
use crate::listbox_keys::option_id;

/// An option as [`create_listbox`] sees it: its key and the props of its
/// newest render (`ListboxOption`). Without `items`, `label`, `keywords`
/// and `disabled` describe the option.
#[derive(Clone, Debug, PartialEq)]
pub struct ListboxOption<T = SharedString> {
    /// The part's key: its `id`, else its `value`.
    pub key: SharedString,
    pub value: T,
    pub label: Option<SharedString>,
    /// The text it shows, which names it without a `label`.
    pub text: Option<SharedString>,
    pub keywords: Vec<SharedString>,
    pub disabled: bool,
}

impl<T> ListboxOption<T> {
    pub fn new(key: impl Into<SharedString>, value: T) -> Self {
        Self {
            key: key.into(),
            value,
            label: None,
            text: None,
            keywords: Vec::new(),
            disabled: false,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn text(mut self, text: impl Into<SharedString>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn keywords<K: Into<SharedString>>(mut self, keywords: impl IntoIterator<Item = K>) -> Self {
        self.keywords = keywords.into_iter().map(Into::into).collect();
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl ListboxOption<SharedString> {
    /// An option whose value is its key.
    pub fn keyed(key: impl Into<SharedString>) -> Self {
        let key = key.into();
        Self::new(key.clone(), key)
    }
}

/// Hears the highlight move, with where the option is, inside the
/// component's update.
pub type ListboxHighlightFn<C> =
    Rc<dyn Fn(&mut C, Option<&SharedString>, &ListHighlightDetails, &mut Window, &mut Context<C>)>;

/// What [`create_listbox`] takes (`ListboxBehaviorOptions`). The original's
/// `key`, the state key that holds the highlight, is the behavior's own
/// here: [`ListboxBehavior::highlighted`].
pub struct ListboxBehaviorOptions<C, T = SharedString> {
    /// `Real` moves focus to the highlighted option. `Virtual` keeps it where
    /// it is, like in an input that points an active descendant at
    /// [`ListboxBehavior::active_id`]. Defaults to `Real`.
    pub focus: ListFocus,
    /// The options as data: items, plain strings, groups and separators.
    /// Without it, the bound parts are the options, in page order.
    pub items: Option<Rc<dyn Fn(&C) -> ListData<T>>>,
    /// The text options are filtered by. Empty shows every option.
    pub query: Option<Rc<dyn Fn(&C) -> SharedString>>,
    /// How `query` narrows the options. A function that returns a number
    /// ranks them too, best first. Defaults to `Contains`.
    pub filter: Option<Rc<dyn Fn(&C) -> ListFilter<T>>>,
    /// Whether moving past either end wraps around. Defaults to true.
    pub r#loop: Option<Rc<dyn Fn(&C) -> bool>>,
    /// The list's focus handle, which takes focus back when its focused
    /// option leaves, as the original finds the listbox around an option.
    pub container: Option<Rc<dyn Fn(&C) -> Option<FocusHandle>>>,
    /// The scroller options reveal in, as the original finds the scrolling
    /// element around one.
    pub scroll_root: Option<Rc<dyn Fn(&C) -> Option<ListScroller>>>,
    /// Runs when the highlight moves, with the option's index among those
    /// that match, for a virtualizer to scroll to.
    pub on_highlight: Option<ListboxHighlightFn<C>>,
    /// Runs when the highlight reaches the last option, so the next page can
    /// load.
    pub on_end: Option<ListHookFn<C>>,
}

impl<C, T> Default for ListboxBehaviorOptions<C, T> {
    fn default() -> Self {
        Self {
            focus: ListFocus::Real,
            items: None,
            query: None,
            filter: None,
            r#loop: None,
            container: None,
            scroll_root: None,
            on_highlight: None,
            on_end: None,
        }
    }
}

impl<C, T> ListboxBehaviorOptions<C, T> {
    pub fn focus(mut self, focus: ListFocus) -> Self {
        self.focus = focus;
        self
    }

    pub fn items(mut self, items: impl Fn(&C) -> ListData<T> + 'static) -> Self {
        self.items = Some(Rc::new(items));
        self
    }

    pub fn query(mut self, query: impl Fn(&C) -> SharedString + 'static) -> Self {
        self.query = Some(Rc::new(query));
        self
    }

    pub fn filter(mut self, filter: impl Fn(&C) -> ListFilter<T> + 'static) -> Self {
        self.filter = Some(Rc::new(filter));
        self
    }

    pub fn r#loop(mut self, r#loop: impl Fn(&C) -> bool + 'static) -> Self {
        self.r#loop = Some(Rc::new(r#loop));
        self
    }

    pub fn container(mut self, container: impl Fn(&C) -> Option<FocusHandle> + 'static) -> Self {
        self.container = Some(Rc::new(container));
        self
    }

    pub fn scroll_root(mut self, scroll_root: impl Fn(&C) -> Option<ListScroller> + 'static) -> Self {
        self.scroll_root = Some(Rc::new(scroll_root));
        self
    }

    pub fn on_highlight(
        mut self,
        callback: impl Fn(&mut C, Option<&SharedString>, &ListHighlightDetails, &mut Window, &mut Context<C>) + 'static,
    ) -> Self {
        self.on_highlight = Some(Rc::new(callback));
        self
    }

    pub fn on_end(mut self, callback: impl Fn(&mut C, &mut Window, &mut Context<C>) + 'static) -> Self {
        self.on_end = Some(Rc::new(callback));
        self
    }
}

/// What an option part takes from [`ListboxBehavior::attrs`]: its element
/// id, whether it's highlighted (`data-highlighted`), whether it's disabled
/// (`aria-disabled`), and whether it takes focus itself, with real focus
/// (`tabindex="-1"`, through [`ListboxBehavior::focus_handle`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ListboxOptionAttrs {
    pub id: SharedString,
    pub focusable: bool,
    pub highlighted: bool,
    pub disabled: bool,
}

impl ListboxOptionAttrs {
    /// Puts the id and `aria-disabled` on an option part.
    pub fn apply<S>(&self, part: Part<S>) -> Part<S> {
        part.with_element_id(self.id.clone())
            .aria(Aria::new().disabled(self.disabled))
    }
}

/// What [`create_listbox`] returns (`ListboxBehavior`). Cloning it gives
/// another handle to the same behavior.
pub struct ListboxBehavior<C: 'static, T: 'static = SharedString> {
    engine: ListboxEngine<C, T>,
    id: SharedString,
    focus: ListFocus,
    // The end `move` placed the highlight at, which it keeps while options
    // render after a query change, until anything else moves it.
    placed: Rc<Cell<Option<Step>>>,
    queued: Rc<Cell<bool>>,
}

impl<C: 'static, T: 'static> Clone for ListboxBehavior<C, T> {
    fn clone(&self) -> Self {
        Self {
            engine: self.engine.clone(),
            id: self.id.clone(),
            focus: self.focus,
            placed: self.placed.clone(),
            queued: self.queued.clone(),
        }
    }
}

// An empty list highlights nothing, so Enter and the active descendant
// never reach an option the query hid.
fn place<C, T>(
    engine: &ListboxEngine<C, T>,
    this: &mut C,
    step: Step,
    event: Option<InputEvent>,
    window: &mut Window,
    cx: &mut Context<C>,
) where
    C: 'static,
    T: Clone + PartialEq + Debug + 'static,
{
    let key = engine.step_key(this, step, None);
    if key != engine.highlighted() {
        engine.highlight(this, key, HighlightOptions::event(event), window, cx);
    }
}

/// A highlight that moves through options with the keys, and options
/// filtered and ranked by a query (`createListbox`). `id` is the
/// component's instance id, which option ids start with.
pub fn create_listbox<C, T>(
    id: impl Into<SharedString>,
    options: ListboxBehaviorOptions<C, T>,
    window: &Window,
    cx: &mut Context<C>,
) -> ListboxBehavior<C, T>
where
    C: 'static,
    T: ItemKey + Clone + PartialEq + Debug + 'static,
{
    let focus = options.focus;
    let placed: Rc<Cell<Option<Step>>> = Rc::new(Cell::new(None));
    let queued = Rc::new(Cell::new(false));
    let ranked = options.filter.clone();
    let mut config = ListboxEngineConfig::new(|_: &C, value: &T| item_key(value, None))
        .focus(move |_| focus)
        .skip_disabled(true)
        .active(|_| true);
    if let Some(items) = options.items.clone() {
        config = config.items(move |this| Some(items(this)));
    }
    let filter = options.filter.clone();
    config = config.filter(move |this| Some(filter.as_ref().map_or(ListFilter::Contains, |filter| filter(this))));
    if let Some(query) = options.query.clone() {
        config = config.query(move |this| query(this));
    }
    config = config.rank(move |this| ranked.as_ref().is_some_and(|filter| filter(this).is_custom()));
    if let Some(r#loop) = options.r#loop.clone() {
        config = config.r#loop(move |this| r#loop(this));
    }
    if let Some(container) = options.container.clone() {
        config = config.container(move |this| container(this));
    }
    if let Some(scroll_root) = options.scroll_root.clone() {
        config = config.scroll_root(move |this| scroll_root(this));
    }
    if let Some(on_highlight) = options.on_highlight.clone() {
        config = config.on_highlight(move |this, key, index, event, window, cx| {
            let details = ListHighlightDetails {
                index,
                event: event.cloned(),
            };
            on_highlight(this, key, &details, window, cx);
        });
    }
    if let Some(on_end) = options.on_end.clone() {
        config = config.on_last_reached(move |this, window, cx| on_end(this, window, cx));
    }
    let engine_cell: Rc<RefCell<Option<WeakListboxEngine<C, T>>>> = Rc::new(RefCell::new(None));
    {
        let placed = placed.clone();
        let queued = queued.clone();
        let engine_cell = engine_cell.clone();
        config = config.on_register(move |_this, _key, window, cx| {
            if placed.get().is_none() || queued.get() {
                return;
            }
            queued.set(true);
            let placed = placed.clone();
            let queued = queued.clone();
            let engine = engine_cell.borrow().clone();
            cx.defer_in(window, move |this, window, cx| {
                queued.set(false);
                if let (Some(step), Some(engine)) = (placed.get(), engine.and_then(|engine| engine.upgrade())) {
                    place(&engine, this, step, None, window, cx);
                }
            });
        });
    }
    let engine = create_filtered_listbox(config, window, cx);
    *engine_cell.borrow_mut() = Some(engine.downgrade());
    // Focus that lands on an option another way, like Tab or a press, moves
    // the highlight there. gpui tells where focus is when it draws, so the
    // list reads it once the options have rendered.
    if focus == ListFocus::Real {
        let weak = engine.downgrade();
        let hook_placed = placed.clone();
        engine.set_settled_hook(Some(Rc::new(move |this: &mut C, window: &mut Window, cx: &mut Context<C>| {
            let Some(engine) = weak.upgrade() else {
                return;
            };
            let Some(focused) = engine.focused(window) else {
                return;
            };
            if engine.highlighted().as_ref() != Some(&focused) {
                hook_placed.set(None);
                engine.highlight(this, Some(focused), HighlightOptions::without_focus(None), window, cx);
            }
        })));
    }
    ListboxBehavior {
        engine,
        id: id.into(),
        focus,
        placed,
        queued,
    }
}

fn meta_of<T: Clone>(option: &ListboxOption<T>) -> ItemMeta<T> {
    ItemMeta {
        value: Some(option.value.clone()),
        label: option.label.clone(),
        text: option.text.clone(),
        keywords: option.keywords.clone(),
        disabled: option.disabled,
        group: None,
        pinned: false,
        index: None,
        link: false,
    }
}

impl<C, T> ListboxBehavior<C, T>
where
    C: 'static,
    T: Clone + PartialEq + Debug + 'static,
{
    /// The engine underneath, for what the behavior doesn't wrap.
    pub fn engine(&self) -> &ListboxEngine<C, T> {
        &self.engine
    }

    /// The highlighted option's key.
    pub fn highlighted(&self) -> Option<SharedString> {
        self.engine.highlighted()
    }

    /// Joins an option as it renders (`bind`): registers it and tracks its
    /// element. With real focus, the option takes focus through
    /// [`focus_handle`](Self::focus_handle).
    pub fn bind<S>(&self, option: &ListboxOption<T>, part: Part<S>, cx: &App) -> Part<S> {
        self.engine.bind_item(option.key.clone(), meta_of(option), part, cx)
    }

    /// The focus handle an option tracks with real focus, out of the tab
    /// order (`tabindex="-1"`): `FocusMode::Roving { tab_stop: false }`.
    pub fn focus_handle(&self, key: &SharedString, cx: &App) -> FocusHandle {
        self.engine.focus_handle(key, cx)
    }

    /// An option's attributes: its id, `data-highlighted` and
    /// `aria-disabled`, and whether it takes focus itself, with real focus.
    pub fn attrs(&self, option: &ListboxOption<T>) -> ListboxOptionAttrs {
        ListboxOptionAttrs {
            id: option_id(&self.id, &option.key),
            focusable: self.focus == ListFocus::Real,
            highlighted: self.engine.highlighted().as_ref() == Some(&option.key),
            disabled: option.disabled,
        }
    }

    /// The id of the highlighted option, for an active descendant with
    /// virtual focus.
    pub fn active_id(&self) -> Option<SharedString> {
        self.engine.highlighted().map(|key| option_id(&self.id, &key))
    }

    /// Moves the highlight for the arrows, PageUp and PageDown, and with
    /// real focus also Home, End and typing an option's name. Returns the
    /// key it moved to, or `None` for a key it leaves alone, like Enter. A
    /// key it takes calls `window.prevent_default()`.
    pub fn keydown(
        &self,
        this: &mut C,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<C>,
    ) -> Option<SharedString> {
        let current = self.engine.highlighted();
        let key = Key::of(&event.keystroke);
        let real = self.focus == ListFocus::Real;
        let to = if real && (key == Key::Home || key == Key::End) {
            self.engine
                .step_key(this, if key == Key::Home { Step::First } else { Step::Last }, None)
        } else if real && is_typeahead_key(&event.keystroke) && (key != Key::Space || self.engine.typing()) {
            Some(self.engine.search(this, &key, current.as_deref())?.key)
        } else {
            let step = list_step(&event.keystroke, is_apple(cx))?;
            self.engine.step_key(this, step, None)
        };
        window.prevent_default();
        self.placed.set(None);
        self.engine.highlight(
            this,
            to.clone(),
            HighlightOptions::event(Some(InputEvent::from(event))),
            window,
            cx,
        );
        to
    }

    /// Highlights an option, or none with `None`.
    pub fn highlight(
        &self,
        this: &mut C,
        key: Option<SharedString>,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<C>,
    ) {
        self.placed.set(None);
        self.engine.highlight(this, key, HighlightOptions::event(event), window, cx);
    }

    /// Moves the highlight by a step, or to the first or last option, and to
    /// none when no option matches. The first or last stays there as options
    /// render, like after the query changed.
    pub fn r#move(
        &self,
        this: &mut C,
        step: Step,
        event: Option<InputEvent>,
        window: &mut Window,
        cx: &mut Context<C>,
    ) {
        self.placed
            .set(matches!(step, Step::First | Step::Last).then_some(step));
        place(&self.engine, this, step, event, window, cx);
    }

    /// The data to render now: `items` filtered by `query`. Empty without
    /// `items`.
    pub fn items(&self, this: &C) -> ListData<T> {
        self.engine
            .visible_data(this)
            .map(|visible| visible.list)
            .unwrap_or_default()
    }

    /// Whether a bound option matches `query`. Options from `items` always
    /// do, since `items` already left the others out.
    pub fn shows(&self, this: &C, option: &ListboxOption<T>) -> bool {
        self.engine.matches(this, &option.key, &meta_of(option))
    }

    /// How many options match `query`.
    pub fn count(&self, this: &C) -> usize {
        self.engine.count(this)
    }
}
