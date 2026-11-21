//! Port of `foundation/popups/popup-behavior.ts`: how each kind of popup
//! behaves. Components start from a preset in [`popup_presets`] and override
//! what differs, so [`PopupLayer`](crate::PopupLayer) stays a short list of
//! wiring plus one name.

use ui_sdk_platform::OutsidePress;

use crate::popup::PopupModal;

/// How Tab moves through a popup. See [`PopupBehavior::tab`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PopupTab {
    /// Tab closes the popup and focus moves on from the trigger.
    Close,
    /// Tab moves in computed order, from the trigger into the popup and on to
    /// the page after it, and focus leaving closes the popup.
    Flow,
    /// Tab wraps inside the popup.
    Trap,
    /// gpui moves focus, in and out, and nothing closes.
    Native,
}

impl PopupTab {
    /// The original's name for it, like `"trap"`.
    pub fn as_str(self) -> &'static str {
        match self {
            PopupTab::Close => "close",
            PopupTab::Flow => "flow",
            PopupTab::Trap => "trap",
            PopupTab::Native => "native",
        }
    }
}

/// How Tab moves, fixed or picked from the modal effects, read live.
#[derive(Clone, Copy, Debug)]
pub enum TabSetting {
    Fixed(PopupTab),
    /// A function of the modal effects, like a popover's: it traps while it
    /// traps focus, and flows otherwise.
    Modal(fn(&PopupModal) -> PopupTab),
}

impl TabSetting {
    pub fn resolve(self, modal: &PopupModal) -> PopupTab {
        match self {
            TabSetting::Fixed(tab) => tab,
            TabSetting::Modal(pick) => pick(modal),
        }
    }
}

impl From<PopupTab> for TabSetting {
    fn from(tab: PopupTab) -> Self {
        TabSetting::Fixed(tab)
    }
}

/// Whether the window losing focus closes the popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PopupBlur {
    Always,
    /// Closes only when focus moved into an embedded frame outside it, so
    /// switching windows keeps it open. gpui embeds no frames, so it stays
    /// open whenever the window loses focus.
    Iframe,
    Never,
}

impl PopupBlur {
    /// The original's name for it, like `"iframe"`.
    pub fn as_str(self) -> &'static str {
        match self {
            PopupBlur::Always => "always",
            PopupBlur::Iframe => "iframe",
            PopupBlur::Never => "never",
        }
    }
}

/// Whether the open popup pauses keyboard shortcuts registered outside it,
/// fixed or picked from the modal effects, read as it shows.
#[derive(Clone, Copy, Debug)]
pub enum BlockingSetting {
    Fixed(bool),
    Modal(fn(&PopupModal) -> bool),
}

impl BlockingSetting {
    pub fn resolve(self, modal: &PopupModal) -> bool {
        match self {
            BlockingSetting::Fixed(blocking) => blocking,
            BlockingSetting::Modal(pick) => pick(modal),
        }
    }
}

impl From<bool> for BlockingSetting {
    fn from(blocking: bool) -> Self {
        BlockingSetting::Fixed(blocking)
    }
}

/// How a kind of popup behaves.
#[derive(Clone, Copy, Debug)]
pub struct PopupBehavior {
    /// `Close`: Tab closes the popup and focus moves on from the trigger.
    /// `Flow`: Tab moves in computed order, from the trigger into the popup
    /// and on to the page after it, and focus leaving closes the popup.
    /// `Trap`: Tab wraps inside the popup. `Native`: gpui moves focus, in and
    /// out, and nothing closes. A function picks from the modal effects, read
    /// live.
    pub tab: TabSetting,
    /// Whether the window losing focus closes the popup.
    pub blur: PopupBlur,
    /// An outside press dismisses at the press, or once it has also been
    /// released outside.
    pub outside_press: OutsidePress,
    /// Stops outside presses at this layer, so popups beneath stay open.
    pub shield: bool,
    /// Whether the trigger stays usable while the popup makes the window
    /// inert. A modal dialog's trigger doesn't, since nothing outside it may
    /// be used.
    pub keep_trigger: bool,
    /// Whether focus moves in for a popup that's open from the start, once
    /// it renders. A modal dialog has to hold focus; a popover leaves it
    /// alone.
    pub focus_on_mount: bool,
    /// Whether focus leaving a `Flow` popup closes it. A non-modal dialog
    /// stays.
    pub dismiss_on_focus_loss: bool,
    /// Whether the platform's back gesture closes the popup, with reason
    /// `Back`. Only the topmost one hears it.
    pub close_watcher: bool,
    /// Whether the open popup pauses keyboard shortcuts registered outside
    /// it. A function picks from the modal effects, read when it shows.
    pub blocking: BlockingSetting,
    /// Whether the popup ever moves focus: in when it opens, and back when it
    /// closes. A tooltip never does, since focus stays on its trigger.
    pub moves_focus: bool,
}

/// What a component passes to change its preset's behavior. `None` leaves
/// the preset's value.
#[derive(Clone, Copy, Debug, Default)]
pub struct PopupBehaviorOverrides {
    pub tab: Option<TabSetting>,
    pub blur: Option<PopupBlur>,
    pub outside_press: Option<OutsidePress>,
    pub shield: Option<bool>,
    pub keep_trigger: Option<bool>,
    pub focus_on_mount: Option<bool>,
    pub dismiss_on_focus_loss: Option<bool>,
    pub close_watcher: Option<bool>,
    pub blocking: Option<BlockingSetting>,
    pub moves_focus: Option<bool>,
}

impl PopupBehaviorOverrides {
    pub fn tab(mut self, tab: impl Into<TabSetting>) -> Self {
        self.tab = Some(tab.into());
        self
    }

    pub fn blur(mut self, blur: PopupBlur) -> Self {
        self.blur = Some(blur);
        self
    }

    pub fn outside_press(mut self, outside_press: OutsidePress) -> Self {
        self.outside_press = Some(outside_press);
        self
    }

    pub fn shield(mut self, shield: bool) -> Self {
        self.shield = Some(shield);
        self
    }

    pub fn keep_trigger(mut self, keep: bool) -> Self {
        self.keep_trigger = Some(keep);
        self
    }

    pub fn focus_on_mount(mut self, focus: bool) -> Self {
        self.focus_on_mount = Some(focus);
        self
    }

    pub fn dismiss_on_focus_loss(mut self, dismiss: bool) -> Self {
        self.dismiss_on_focus_loss = Some(dismiss);
        self
    }

    pub fn close_watcher(mut self, close_watcher: bool) -> Self {
        self.close_watcher = Some(close_watcher);
        self
    }

    pub fn blocking(mut self, blocking: impl Into<BlockingSetting>) -> Self {
        self.blocking = Some(blocking.into());
        self
    }

    pub fn moves_focus(mut self, moves_focus: bool) -> Self {
        self.moves_focus = Some(moves_focus);
        self
    }
}

const PLAIN: PopupBehavior = PopupBehavior {
    tab: TabSetting::Fixed(PopupTab::Close),
    blur: PopupBlur::Always,
    outside_press: OutsidePress::PointerDown,
    shield: false,
    keep_trigger: true,
    focus_on_mount: false,
    dismiss_on_focus_loss: true,
    close_watcher: false,
    blocking: BlockingSetting::Fixed(false),
    moves_focus: true,
};

fn trap_or_flow(modal: &PopupModal) -> PopupTab {
    if modal.trap_focus {
        PopupTab::Trap
    } else {
        PopupTab::Flow
    }
}

fn inert_and_lock(modal: &PopupModal) -> bool {
    modal.inert_and_lock
}

/// The kinds of popup, by the name of their row in [`popup_presets`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PopupBehaviorName {
    Menu,
    Select,
    Combobox,
    Popover,
    Dialog,
    Tooltip,
    HoverCard,
    NavigationMenu,
}

impl PopupBehaviorName {
    /// The original's name for it, like `"hoverCard"`.
    pub fn as_str(self) -> &'static str {
        match self {
            PopupBehaviorName::Menu => "menu",
            PopupBehaviorName::Select => "select",
            PopupBehaviorName::Combobox => "combobox",
            PopupBehaviorName::Popover => "popover",
            PopupBehaviorName::Dialog => "dialog",
            PopupBehaviorName::Tooltip => "tooltip",
            PopupBehaviorName::HoverCard => "hoverCard",
            PopupBehaviorName::NavigationMenu => "navigationMenu",
        }
    }
}

/// The behavior of each kind of popup (`popupPresets`).
pub fn popup_presets(name: PopupBehaviorName) -> PopupBehavior {
    match name {
        // Tab closes it, like a native menu, and so does the window losing focus.
        PopupBehaviorName::Menu => PLAIN,
        // Like a native select: Tab closes it and focus moves on from the trigger.
        PopupBehaviorName::Select => PLAIN,
        // Focus stays in the input while the list is open, so Tab is gpui's.
        // The combobox closes the list itself when focus leaves its input.
        PopupBehaviorName::Combobox => PopupBehavior {
            tab: TabSetting::Fixed(PopupTab::Native),
            ..PLAIN
        },
        // Part of the window's Tab order unless modal, and it stays open while
        // you switch windows, so work in it can continue.
        PopupBehaviorName::Popover => PopupBehavior {
            tab: TabSetting::Modal(trap_or_flow),
            blur: PopupBlur::Iframe,
            ..PLAIN
        },
        // Waits for the user whatever has focus. A modal one shields the
        // layers beneath, holds focus from the start, keeps its trigger inert,
        // pauses the window's shortcuts and closes on the back gesture.
        // Outside presses count once released, so a text selection dragged out
        // of a field doesn't close it.
        PopupBehaviorName::Dialog => PopupBehavior {
            tab: TabSetting::Modal(trap_or_flow),
            blur: PopupBlur::Never,
            outside_press: OutsidePress::Click,
            shield: true,
            keep_trigger: false,
            focus_on_mount: true,
            dismiss_on_focus_loss: false,
            close_watcher: true,
            blocking: BlockingSetting::Modal(inert_and_lock),
            ..PLAIN
        },
        // Never takes focus and has nothing to Tab to, so gpui moves focus
        // past it. Escape and an outside press close it, and so does leaving
        // the window.
        PopupBehaviorName::Tooltip => PopupBehavior {
            tab: TabSetting::Fixed(PopupTab::Native),
            moves_focus: false,
            ..PLAIN
        },
        // Part of the window's Tab order after its trigger, like a non-modal
        // popover, and focus leaving closes it. It's a passing preview, so
        // leaving the window does too.
        PopupBehaviorName::HoverCard => PopupBehavior {
            tab: TabSetting::Fixed(PopupTab::Flow),
            ..PLAIN
        },
        // Site navigation: Tab flows from a trigger into its panel and on to
        // the next item, focus leaving the navigation closes it, and switching
        // windows leaves it as it was.
        PopupBehaviorName::NavigationMenu => PopupBehavior {
            tab: TabSetting::Fixed(PopupTab::Flow),
            blur: PopupBlur::Never,
            ..PLAIN
        },
    }
}

/// The preset's behavior, with any override passed beside it winning
/// (`resolveBehavior`).
pub fn resolve_behavior(preset: PopupBehaviorName, overrides: &PopupBehaviorOverrides) -> PopupBehavior {
    let base = popup_presets(preset);
    PopupBehavior {
        tab: overrides.tab.unwrap_or(base.tab),
        blur: overrides.blur.unwrap_or(base.blur),
        outside_press: overrides.outside_press.unwrap_or(base.outside_press),
        shield: overrides.shield.unwrap_or(base.shield),
        keep_trigger: overrides.keep_trigger.unwrap_or(base.keep_trigger),
        focus_on_mount: overrides.focus_on_mount.unwrap_or(base.focus_on_mount),
        dismiss_on_focus_loss: overrides
            .dismiss_on_focus_loss
            .unwrap_or(base.dismiss_on_focus_loss),
        close_watcher: overrides.close_watcher.unwrap_or(base.close_watcher),
        blocking: overrides.blocking.unwrap_or(base.blocking),
        moves_focus: overrides.moves_focus.unwrap_or(base.moves_focus),
    }
}
