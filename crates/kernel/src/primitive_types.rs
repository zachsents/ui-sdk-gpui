//! Port of `foundation/kernel/primitive-types.ts`: what a definition declares
//! beside its state, api and slots, as far as it means something in Rust.
//!
//! The original's types describe a definition to TypeScript and renderers:
//! slot props and their kinds, the context an api gets, the instance
//! adapters drive. Here a component is a struct in an entity: its fields are
//! the state, its methods the api, its part methods the slots. What's left
//! is what the agent bridge and `observe` read from a live component, its
//! definition's `agent`, `controlled` and `commits` keys, which a component
//! declares by implementing [`Agent`].
//!
//! The bridge reads values as JSON, the data its model speaks:
//! [`AgentValue`] is `serde_json::Value`, re-exported with its `json!` macro.

use std::fmt;

use gpui::{App, Context, SharedString, Window};

pub use serde_json::Value as AgentValue;
pub use serde_json::json;

/// What a definition's `agent` key declares besides its actions, items and
/// visibility, which [`Agent`]'s items say (`AgentSurface`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgentSurface {
    /// Its value is always one of its items, so text that names none is a
    /// mistake to refuse.
    pub closed: bool,
    /// Its closed items hide other components, like tabs, which a step can
    /// open first.
    pub reveals: bool,
    /// Its value is hidden from agents and `observe`, like a password's.
    pub secret: bool,
}

impl AgentSurface {
    pub const fn new() -> Self {
        Self {
            closed: false,
            reveals: false,
            secret: false,
        }
    }

    pub const fn closed(mut self, closed: bool) -> Self {
        self.closed = closed;
        self
    }

    pub const fn reveals(mut self, reveals: bool) -> Self {
        self.reveals = reveals;
        self
    }

    pub const fn secret(mut self, secret: bool) -> Self {
        self.secret = secret;
        self
    }
}

/// What an action an agent calls returns: the api method's result, like
/// `true` from a `setPressed` that went through, or an error the bridge
/// reports to the agent.
pub type AgentResult = Result<AgentValue, AgentError>;

/// What an action returns when its work goes on after it returns, like a
/// popup's `open()`, so the step doesn't wait for it.
pub const PENDING: &str = "pending";

/// Why a step on a component failed.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentError {
    /// The step's argument isn't what the action takes, like text for
    /// `setChecked`. `expected` reads like "true or false".
    Takes {
        action: SharedString,
        expected: &'static str,
        given: AgentValue,
    },
    /// The component has no such action.
    Unknown(SharedString),
    /// The component left before the step ran.
    Gone,
    /// The component refused the step, with why, like "takes a number".
    Refused(SharedString),
}

impl AgentError {
    pub fn refused(why: impl Into<SharedString>) -> Self {
        AgentError::Refused(why.into())
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::Takes {
                action,
                expected,
                given,
            } => write!(f, "{action} takes {expected}, not {given}."),
            AgentError::Unknown(action) => write!(f, "It takes no {action}."),
            AgentError::Gone => write!(f, "It left the page."),
            AgentError::Refused(why) => write!(f, "{why}"),
        }
    }
}

/// A step's arguments, read by the kind the action takes, the way the bridge
/// checks a `set<Key>` value against its option's kind (`valueFor`): text
/// converts where it reads as a number or a list, like "3" for a page, but
/// never to true or false.
#[derive(Clone, Copy, Debug)]
pub struct AgentArgs<'a> {
    action: &'a str,
    values: &'a [AgentValue],
}

impl<'a> AgentArgs<'a> {
    pub fn new(action: &'a str, values: &'a [AgentValue]) -> Self {
        Self { action, values }
    }

    /// The action the arguments are for, like `"setValue"`.
    pub fn action(&self) -> &'a str {
        self.action
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// The argument at `index`, as given. `Null` when it's left out.
    pub fn value(&self, index: usize) -> &'a AgentValue {
        static NULL: AgentValue = AgentValue::Null;
        self.values.get(index).unwrap_or(&NULL)
    }

    /// Whether the argument at `index` is left out or null.
    pub fn is_missing(&self, index: usize) -> bool {
        self.value(index).is_null()
    }

    fn takes(&self, expected: &'static str, index: usize) -> AgentError {
        AgentError::Takes {
            action: self.action.to_string().into(),
            expected,
            given: self.value(index).clone(),
        }
    }

    /// `true` or `false`, never text that reads like one.
    pub fn boolean(&self, index: usize) -> Result<bool, AgentError> {
        self.value(index)
            .as_bool()
            .ok_or_else(|| self.takes("true or false", index))
    }

    /// A number, or text that reads as one, like `"3"`.
    pub fn number(&self, index: usize) -> Result<f64, AgentError> {
        let number = match self.value(index) {
            AgentValue::Number(number) => number.as_f64(),
            AgentValue::String(text) if !text.trim().is_empty() => text.trim().parse::<f64>().ok(),
            _ => None,
        };
        number
            .filter(|number| !number.is_nan())
            .ok_or_else(|| self.takes("a number", index))
    }

    /// Text, or a number or true or false as its text.
    pub fn string(&self, index: usize) -> Result<SharedString, AgentError> {
        match self.value(index) {
            AgentValue::String(text) => Ok(text.clone().into()),
            AgentValue::Number(number) => Ok(number.to_string().into()),
            AgentValue::Bool(value) => Ok(value.to_string().into()),
            _ => Err(self.takes("text", index)),
        }
    }

    /// A list of texts, or one text as a list of one.
    pub fn list(&self, index: usize) -> Result<Vec<SharedString>, AgentError> {
        match self.value(index) {
            AgentValue::Array(items) => items
                .iter()
                .map(|item| match item {
                    AgentValue::String(text) => Ok(text.clone().into()),
                    AgentValue::Number(number) => Ok(number.to_string().into()),
                    _ => Err(self.takes("a list of texts", index)),
                })
                .collect(),
            AgentValue::String(text) => Ok(vec![text.clone().into()]),
            AgentValue::Null => Ok(Vec::new()),
            _ => Err(self.takes("a list of texts", index)),
        }
    }
}

/// One of a component's items as an agent sees it: its value, as the key the
/// component's state holds, and its label. Groups and a tree's branches are
/// flattened, separators left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentItem {
    pub value: SharedString,
    pub label: SharedString,
    /// A person can't choose it now.
    pub disabled: bool,
}

impl AgentItem {
    pub fn new(value: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            disabled: false,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A component's state as the bridge reads it (`instance.get()`): its keys by
/// the original's names, like `"open"`, `"value"` or `"readOnly"`, with
/// their values as data. Include every key the original's state has that the
/// bridge reads: the controlled keys, and `disabled`, `readOnly`, `pending`,
/// `error`, `min`, `max`, `step`, `placeholder`, `query`, `multiple`,
/// `required` and `errors` when the component has them. Values that are
/// items hold their keys, the `value` of their [`AgentItem`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentState(Vec<(&'static str, AgentValue)>);

impl AgentState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets `key`.
    pub fn with(mut self, key: &'static str, value: impl Into<AgentValue>) -> Self {
        self.set(key, value);
        self
    }

    pub fn set(&mut self, key: &'static str, value: impl Into<AgentValue>) {
        let value = value.into();
        match self.0.iter_mut().find(|(name, _)| *name == key) {
            Some(entry) => entry.1 = value,
            None => self.0.push((key, value)),
        }
    }

    pub fn get(&self, key: &str) -> Option<&AgentValue> {
        self.0
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &AgentValue)> {
        self.0.iter().map(|(name, value)| (*name, value))
    }

    /// The state as a JSON object.
    pub fn to_value(&self) -> AgentValue {
        AgentValue::Object(
            self.0
                .iter()
                .map(|(name, value)| (name.to_string(), value.clone()))
                .collect(),
        )
    }
}

/// An api method an agent may call, by the original's name, like
/// `"setValue"`, and what runs it. The bridge calls it as the agent, so
/// every change it makes reports `ChangeSource::Agent`.
pub struct AgentAction<T> {
    pub name: &'static str,
    /// Whether the component lists it among its actions. An unlisted one,
    /// like `toggle` beside `setPressed`, still runs as a step's action.
    pub listed: bool,
    pub run: fn(&mut T, &AgentArgs, &mut Window, &mut Context<T>) -> AgentResult,
}

impl<T> AgentAction<T> {
    /// An action the component lists, like `agent.actions` does.
    pub const fn new(
        name: &'static str,
        run: fn(&mut T, &AgentArgs, &mut Window, &mut Context<T>) -> AgentResult,
    ) -> Self {
        Self {
            name,
            listed: true,
            run,
        }
    }

    /// An action another one covers, left out of the list to keep it short,
    /// like `toggle` beside `setPressed` or `open` beside `setOpen`, or one
    /// the bridge calls itself, like `setOpen` to find a button in a closed
    /// popup.
    pub const fn unlisted(
        name: &'static str,
        run: fn(&mut T, &AgentArgs, &mut Window, &mut Context<T>) -> AgentResult,
    ) -> Self {
        Self {
            name,
            listed: false,
            run,
        }
    }
}

impl<T> Clone for AgentAction<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for AgentAction<T> {}

impl<T> fmt::Debug for AgentAction<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentAction")
            .field("name", &self.name)
            .field("listed", &self.listed)
            .finish()
    }
}

/// What the agent bridge and `observe` read from a live component: its
/// definition's `agent`, `controlled` and `commits` keys, its state and
/// items. Every component implements it, and hands its entity to its
/// instance with `Instance::with_agent(cx)` as it's made.
pub trait Agent: Sized + 'static {
    /// Whether its value is always one of its items, whether its closed
    /// items hide other components, and whether its value is secret.
    const SURFACE: AgentSurface = AgentSurface::new();

    /// The api methods an agent may call (`agent.actions`), with what runs
    /// each. A definition without `agent.actions` lists its api methods named
    /// like a change, as the bridge would take them: `set*`, and `open`,
    /// `close`, `toggle`, `select`, `clear`, `next`, `previous`, `first`,
    /// `last`, `play`, `pause`, `show`, `hide`, `expand`, `collapse`,
    /// `increment`, `decrement`, `reset`, `submit`, `add`, `remove`, `insert`
    /// and the names they start. A synonym another action covers is
    /// unlisted: `open`, `close`, `show` and `hide` beside `setOpen`,
    /// `toggle` beside `setOpen`, `setChecked` or `setPressed`, `select`
    /// beside `setValue`.
    const ACTIONS: &'static [AgentAction<Self>] = &[];

    /// The state keys a user can control (`controlled`), which the bridge
    /// reports and `observe` follows. `None` takes whichever of `open`,
    /// `value` and `checked` the state has.
    const CONTROLLED: Option<&'static [&'static str]> = None;

    /// The state keys an interaction changes in steps and then commits, like
    /// a slider's `value` (`commits`). `observe` reports each once per commit.
    const COMMITS: &'static [&'static str] = &[];

    /// Its state as the bridge reads it.
    fn agent_state(&self, cx: &App) -> AgentState;

    /// Its items as data (`agent.items`), when they're its own, like a
    /// select's options or a tree's nodes, including those a list shows
    /// while its query is empty. `None` when it has no items, or only the
    /// ones it renders.
    fn agent_items(&self, _cx: &App) -> Option<Vec<AgentItem>> {
        None
    }

    /// Whether an agent sees it now (`agent.shown`), like a field only while
    /// it has errors.
    fn agent_shown(&self, _cx: &App) -> bool {
        true
    }

    /// Whether it takes a value that names none of its items now, like a
    /// combobox with `allow_custom_value` (`internal.allowCustomValue`).
    fn agent_custom_value(&self, _cx: &App) -> bool {
        false
    }
}
