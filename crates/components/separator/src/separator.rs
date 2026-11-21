//! Port of `components/separator/separator.ts`: a line between groups of
//! content, announced as a separator unless it's decorative.

use gpui::{App, Context, Entity, Orientation, StatefulInteractiveElement, Window};

use ui_sdk_kernel::roles::Role;
use ui_sdk_kernel::{Agent, AgentState, Instance, Part};

/// Which way the line runs: gpui's `Orientation`, horizontal or vertical.
pub type SeparatorOrientation = Orientation;

/// The options `Separator` takes.
#[derive(Clone, Debug)]
pub struct SeparatorOptions {
    /// Which way the line runs. A vertical separator divides items in a row.
    /// Horizontal by default.
    pub orientation: SeparatorOrientation,
    /// Hides the separator from assistive technology with the `none` role,
    /// for a line that's only visual. Leave it off when the separator divides
    /// content a screen reader should hear as separate, like groups of menu
    /// items.
    pub decorative: bool,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl Default for SeparatorOptions {
    fn default() -> Self {
        Self {
            orientation: SeparatorOrientation::Horizontal,
            decorative: false,
            debug: false,
        }
    }
}

impl SeparatorOptions {
    pub fn orientation(mut self, orientation: SeparatorOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    pub fn decorative(mut self, decorative: bool) -> Self {
        self.decorative = decorative;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What the root part's `with` and the app read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeparatorState {
    /// Which way the line runs.
    pub orientation: SeparatorOrientation,
}

/// A separator. Every key follows the options.
pub struct Separator {
    instance: Instance,
    orientation: SeparatorOrientation,
    decorative: bool,
}

impl Separator {
    pub fn new(options: SeparatorOptions, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("separator").with_agent(cx);
        instance.debug = options.debug;
        Self {
            instance,
            orientation: options.orientation,
            decorative: options.decorative,
        }
    }

    /// Hands it new options, as a framework re-renders with new props (`sync`).
    pub fn set_options(&mut self, options: SeparatorOptions, cx: &mut Context<Self>) {
        let changed =
            self.orientation != options.orientation || self.decorative != options.decorative;
        self.orientation = options.orientation;
        self.decorative = options.decorative;
        self.instance.debug = options.debug;
        if changed {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    pub fn state(&self) -> SeparatorState {
        SeparatorState {
            orientation: self.orientation,
        }
    }

    /// Which way the line runs.
    pub fn orientation(&self) -> SeparatorOrientation {
        self.orientation
    }

    /// Whether it's only visual, so assistive technology skips it.
    pub fn is_decorative(&self) -> bool {
        self.decorative
    }

    /// The `root` part: the line itself. Give it a background, a height when
    /// it's horizontal and a width when it's vertical. A vertical one takes
    /// its row's height with `self_stretch`.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<SeparatorState> {
        let separator = this.read(cx);
        let state = separator.state();
        let part = Part::new(&separator.instance, "root", None, state);
        // A separator is structure, so it's never focusable. One that resizes
        // panels is a splitter, which needs a value and is another part.
        if separator.decorative {
            part.aria_role(Role::None)
        } else {
            part.aria_role(Role::Separator)
                .aria_orientation(state.orientation)
        }
    }
}

/// `agent: { shown: () => false }`: agents never list it, since it has
/// nothing to read or change.
impl Agent for Separator {
    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
    }

    fn agent_shown(&self, _cx: &App) -> bool {
        false
    }
}
