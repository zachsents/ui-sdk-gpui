//! Port of `components/field/field-set.ts`: a field set groups related fields
//! under a legend.
//!
//! It's the forms crate's [`Fieldset`], the stand-in for a native
//! `<fieldset>`, so `disabled` disables every control inside it the way the
//! browser does, except those in its legend, and leaves them out of the
//! form. Fields inside it mark their parts disabled too. Its legend is the
//! forms crate's [`Legend`].
//!
//! Its parts are a field's, so their slots read `field-set`, `field-legend`,
//! and `field-description` for a description of the whole group. The field
//! group lays fields out, with no role and no state: a field set is what
//! names and disables a group.

use gpui::{App, Context, Entity, Role, SharedString, Window};
use ui_sdk_forms::{Fieldset, Legend};
use ui_sdk_kernel::bound_keys::bound;
use ui_sdk_kernel::part::with_role;
use ui_sdk_kernel::prelude::AriaRole;
use ui_sdk_kernel::{Agent, AgentState, Aria, Instance, Part, key_while_bound};

/// The options `FieldSet` takes.
#[derive(Clone, Debug, Default)]
pub struct FieldSetOptions {
    /// Disables every control inside it, except those in its legend, and
    /// leaves them out of the form.
    pub disabled: bool,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl FieldSetOptions {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// What every part's `with` and the app read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FieldSetState {
    /// Whether the field set is disabled, by itself or by a field set
    /// around it.
    pub disabled: bool,
}

/// A field set. Its entity is its controller.
pub struct FieldSet {
    instance: Instance,
    disabled: bool,
    fieldset: Fieldset,
    legend: Legend,
}

impl FieldSet {
    pub fn new(options: FieldSetOptions, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("field").with_agent(cx);
        instance.debug = options.debug;
        let fieldset = Fieldset::new();
        // A disabled field set around this one disables it too.
        fieldset.node().set_observer(Some(cx.entity_id()));
        fieldset.set_disabled(options.disabled, cx);
        Self {
            instance,
            disabled: options.disabled,
            fieldset,
            legend: Legend::new(SharedString::default()),
        }
    }

    /// Hands it new options, as a framework re-renders with new props
    /// (`sync`).
    pub fn set_options(&mut self, options: FieldSetOptions, cx: &mut Context<Self>) {
        let changed = self.disabled != options.disabled;
        self.disabled = options.disabled;
        self.fieldset.set_disabled(options.disabled, cx);
        self.instance.debug = options.debug;
        if changed {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The fieldset it renders, which the controls inside it find.
    pub fn fieldset(&self) -> &Fieldset {
        &self.fieldset
    }

    pub fn state(&self) -> FieldSetState {
        FieldSetState {
            disabled: self.disabled || self.fieldset.node().form_disabled(),
        }
    }

    /// The `set` part: the group, named by its legend and described by its
    /// descriptions while they're in the page. A legend first in the set
    /// names it natively in the original; this also covers a legend that
    /// isn't.
    pub fn set(this: &Entity<Self>, cx: &App) -> Part<FieldSetState> {
        let set = this.read(cx);
        let state = set.state();
        let instance = &set.instance;
        let mut aria = Aria::new().disabled(state.disabled);
        if bound(instance, "legend") {
            aria = aria.labelled_by(instance.element_id("legend", None));
        }
        for key in key_while_bound(instance, "description") {
            aria = aria.described_by(instance.element_id("description", Some(key.as_ref())));
        }
        Part::new(instance, "set", None, state)
            .aria_role(AriaRole::Group)
            .map_el(|el| set.fieldset.track(el))
            .aria(aria)
    }

    /// The `legend` part, which names the set. Controls inside it stay
    /// enabled while the set is disabled. Give it its text as an
    /// `aria_label`.
    pub fn legend(this: &Entity<Self>, cx: &App) -> Part<FieldSetState> {
        let set = this.read(cx);
        Part::new(&set.instance, "legend", None, set.state())
            .map_el(|el| with_role(set.legend.track(el), Role::Legend))
    }

    /// A `description` part, by a key of the app's own, right inside the set
    /// rather than in one of its fields: it describes the group, so screen
    /// readers say it as focus enters. Give it its text as an `aria_label`.
    pub fn description(this: &Entity<Self>, key: impl Into<SharedString>, cx: &App) -> Part<FieldSetState> {
        let set = this.read(cx);
        let key: SharedString = key.into();
        Part::new(&set.instance, "description", Some(&key), set.state())
            .map_el(|el| with_role(el, Role::Label))
    }
}

/// `agent: { shown: () => false }`: agents read the fields inside it.
impl Agent for FieldSet {
    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new().with("disabled", self.state().disabled)
    }

    fn agent_shown(&self, _cx: &App) -> bool {
        false
    }
}

/// The options `FieldGroup` takes.
#[derive(Clone, Debug, Default)]
pub struct FieldGroupOptions {
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl FieldGroupOptions {
    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

/// A field group, which only lays fields out. Its entity is its controller.
pub struct FieldGroup {
    instance: Instance,
}

impl FieldGroup {
    pub fn new(options: FieldGroupOptions, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("field").with_agent(cx);
        instance.debug = options.debug;
        Self { instance }
    }

    /// Hands it new options, as a framework re-renders with new props.
    pub fn set_options(&mut self, options: FieldGroupOptions, _cx: &mut Context<Self>) {
        self.instance.debug = options.debug;
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The `group` part, which stacks the fields. It has no role and no
    /// state.
    pub fn group(this: &Entity<Self>, cx: &App) -> Part<()> {
        Part::new(&this.read(cx).instance, "group", None, ())
    }
}

/// `agent: { shown: () => false }`: agents read the fields inside it.
impl Agent for FieldGroup {
    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
    }

    fn agent_shown(&self, _cx: &App) -> bool {
        false
    }
}
