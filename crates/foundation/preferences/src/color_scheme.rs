//! Port of `foundation/preferences/color-scheme.ts`.
//!
//! A color scheme: what the app chose (`System`, `Light` or `Dark`), what the
//! system prefers, and what that resolves to. gpui reports the system's
//! preference as the window's appearance, in place of `prefers-color-scheme`:
//! `window.appearance()`, and its changes through
//! `cx.observe_window_appearance`. It always knows it, so a scheme is never
//! `"unknown"`, and there's no `initial` one to assume before hydration.

use std::rc::Rc;

use gpui::{App, AppContext as _, Context, Entity, Subscription, Window, WindowAppearance};
use ui_sdk_kernel::{ChangeCallback, ChangeDetails, ChangeOptions};
use ui_sdk_storage::ValueStorage;

use crate::preference::{Preference, PreferenceOptions, follow_in};

/// A color scheme the app can choose: `Light`, `Dark`, or `System`, which
/// follows the system's preference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColorSchemeValue {
    #[default]
    System,
    Light,
    Dark,
}

impl ColorSchemeValue {
    /// Every choice, in the original's order.
    pub const ALL: [ColorSchemeValue; 3] = [
        ColorSchemeValue::System,
        ColorSchemeValue::Light,
        ColorSchemeValue::Dark,
    ];

    /// The original's name for it, which a storage keeps: `"system"`,
    /// `"light"` or `"dark"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ColorSchemeValue::System => "system",
            ColorSchemeValue::Light => "light",
            ColorSchemeValue::Dark => "dark",
        }
    }
}

impl AsRef<str> for ColorSchemeValue {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// A scheme as the system knows it, from the window's appearance: `Light` or
/// `Dark`. A vibrant appearance is the scheme it's a variant of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SystemColorScheme {
    Light,
    Dark,
}

impl SystemColorScheme {
    /// The original's name for it: `"light"` or `"dark"`.
    pub fn as_str(self) -> &'static str {
        match self {
            SystemColorScheme::Light => "light",
            SystemColorScheme::Dark => "dark",
        }
    }
}

impl From<WindowAppearance> for SystemColorScheme {
    fn from(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => SystemColorScheme::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => SystemColorScheme::Dark,
        }
    }
}

/// Everything a color scheme knows, from `get`, `subscribe` and `follow`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ColorSchemeState {
    /// What the app chose. `System` follows the system's preference.
    pub value: ColorSchemeValue,
    /// What the system prefers: the window's appearance, which follows an
    /// appearance the app forces with `cx.set_window_appearance` instead.
    pub system: SystemColorScheme,
    /// What shows: `value`, or `system` when `value` is `System`.
    pub resolved: SystemColorScheme,
}

/// `on_system_value_change`: the system's preference it changed to.
pub type SystemValueChangeCallback = Rc<dyn Fn(&SystemColorScheme, &mut Window, &mut App)>;

/// Options for [`ColorScheme`].
#[derive(Clone, Default)]
pub struct ColorSchemeOptions {
    /// The scheme the app starts with before anything is saved, and after a
    /// saved one is removed. `System` unless set.
    pub default_value: ColorSchemeValue,
    /// The app's scheme, when the app controls it. A change asks
    /// `on_value_change`, and the app passes the new scheme back with
    /// `set_controlled_value`.
    pub value: Option<ColorSchemeValue>,
    /// Called before the app's scheme changes, with its reason: `Imperative`,
    /// or the one `set` was given, and `StorageSync` when a choice saved
    /// under the same storage arrives. `details.cancel()` keeps the current
    /// one. It runs inside the color scheme's update, so it may update any
    /// entity but the color scheme.
    pub on_value_change: Option<ChangeCallback<ColorSchemeValue>>,
    /// Called when the system's preference changes. It runs inside the color
    /// scheme's update, so it may update any entity but the color scheme.
    pub on_system_value_change: Option<SystemValueChangeCallback>,
    /// Where the choice is kept between runs, from `create_local_storage` or
    /// `create_session_storage`.
    pub storage: Option<ValueStorage>,
}

impl ColorSchemeOptions {
    pub fn default_value(mut self, value: ColorSchemeValue) -> Self {
        self.default_value = value;
        self
    }

    pub fn value(mut self, value: ColorSchemeValue) -> Self {
        self.value = Some(value);
        self
    }

    pub fn on_value_change(
        mut self,
        callback: impl Fn(&ColorSchemeValue, &ChangeDetails, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(callback));
        self
    }

    pub fn on_system_value_change(
        mut self,
        callback: impl Fn(&SystemColorScheme, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_system_value_change = Some(Rc::new(callback));
        self
    }

    pub fn storage(mut self, storage: ValueStorage) -> Self {
        self.storage = Some(storage);
        self
    }
}

fn resolve(value: ColorSchemeValue, system: SystemColorScheme) -> SystemColorScheme {
    match value {
        ColorSchemeValue::System => system,
        ColorSchemeValue::Light => SystemColorScheme::Light,
        ColorSchemeValue::Dark => SystemColorScheme::Dark,
    }
}

/// The app's color scheme: a choice of `System`, `Light` or `Dark`, the
/// system's preference, and what shows. Create one per scope, like the app or
/// a dark sidebar, with `cx.new(|cx| ColorScheme::new(options, window, cx))`;
/// it reads the system's preference from that window. Views read it with
/// `get` in `render` and style by `resolved`.
pub struct ColorScheme {
    // The choice, kept in the storage.
    choice: Entity<Preference<ColorSchemeValue>>,
    system: SystemColorScheme,
    on_system_value_change: Option<SystemValueChangeCallback>,
    _subscriptions: [Subscription; 2],
}

impl ColorScheme {
    pub fn new(options: ColorSchemeOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ColorSchemeOptions {
            default_value,
            value,
            on_value_change,
            on_system_value_change,
            storage,
        } = options;
        let choice = cx.new(|cx| {
            Preference::named(
                "color-scheme",
                PreferenceOptions {
                    values: ColorSchemeValue::ALL.to_vec(),
                    default_value,
                    value,
                    storage,
                    on_value_change,
                },
                cx,
            )
        });
        let subscriptions = [
            // The choice changed, through `set` or its storage.
            cx.observe(&choice, |_, _, cx| cx.notify()),
            cx.observe_window_appearance(window, |this, window, cx| this.system_changed(window, cx)),
        ];
        Self {
            choice,
            system: window.appearance().into(),
            on_system_value_change,
            _subscriptions: subscriptions,
        }
    }

    /// The choices: `System`, `Light` and `Dark`.
    pub fn values(&self) -> &'static [ColorSchemeValue] {
        &ColorSchemeValue::ALL
    }

    /// The choice when nothing allowed is saved.
    pub fn default_value(&self, cx: &App) -> ColorSchemeValue {
        *self.choice.read(cx).default_value()
    }

    /// Where the choice is kept between runs.
    pub fn storage(&self, cx: &App) -> Option<ValueStorage> {
        self.choice.read(cx).storage().cloned()
    }

    /// What the color scheme knows now.
    pub fn get(&self, cx: &App) -> ColorSchemeState {
        let value = self.choice.read(cx).get().value;
        ColorSchemeState {
            value,
            system: self.system,
            resolved: resolve(value, self.system),
        }
    }

    /// Changes the app's scheme, through `on_value_change`, which can cancel
    /// it, and saves it. The reason defaults to `Imperative`. Returns whether
    /// it changed, or for a controlled scheme, whether the change was
    /// accepted.
    pub fn set(
        &mut self,
        value: ColorSchemeValue,
        options: ChangeOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.choice
            .update(cx, |choice, cx| choice.set(value, options, window, cx))
    }

    /// Passes the controlled scheme, or `None` to let the color scheme keep
    /// its own.
    pub fn set_controlled_value(&mut self, value: Option<ColorSchemeValue>, cx: &mut Context<Self>) {
        self.choice
            .update(cx, |choice, cx| choice.set_controlled_value(value, cx));
    }

    /// Hears every change, to the app's scheme or the system's preference,
    /// with the state after it. Dropping the subscription stops it.
    pub fn subscribe(
        this: &Entity<Self>,
        cx: &mut App,
        listener: impl Fn(&ColorSchemeState, &mut App) + 'static,
    ) -> Subscription {
        cx.observe(this, move |this, cx| {
            let state = this.read(cx).get(cx);
            listener(&state, cx);
        })
    }

    /// Keeps the field of an entity's state that `key` returns set to the
    /// color scheme's state: writes it at the end of this update and on
    /// every change after. Keep the subscription while it follows.
    pub fn follow<T: 'static>(
        this: &Entity<Self>,
        cx: &mut Context<T>,
        key: impl Fn(&mut T) -> &mut ColorSchemeState + 'static,
    ) -> Subscription {
        follow_in(this, |scheme, cx| scheme.get(cx), cx, key)
    }

    fn system_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let system = SystemColorScheme::from(window.appearance());
        if system == self.system {
            return;
        }
        self.system = system;
        if let Some(callback) = self.on_system_value_change.clone() {
            callback(&system, window, cx);
        }
        cx.notify();
    }
}
