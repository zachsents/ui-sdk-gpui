//! Port of `components/label/label.ts`: a label that names any control, by
//! its `for_id` or by holding it, and lets a press on the label reach it.
//!
//! A native `<label>` names only a labelable element, like an input or a
//! button; this one names anything else itself too, like a radio group or a
//! slider. Here both are the forms crate's [`Label`](ui_sdk_forms::Label),
//! which stands in for `<label>`: it finds its control in the page, names it
//! through the control's internals, and activates it when pressed. The part
//! puts it in the page with a role, so the control's `labelled_by` relation
//! points at a node assistive technology reads.

use gpui::{App, ClickEvent, Context, Entity, Role, SharedString, StatefulInteractiveElement, Window};
use ui_sdk_forms::{LabelOptions as FormLabelOptions, Node, NodeKind, activate_control};
use ui_sdk_kernel::part::with_role;
use ui_sdk_kernel::{Agent, AgentState, InputEvent, Instance, Part};

/// The options `Label` takes.
#[derive(Clone, Debug, Default)]
pub struct LabelOptions {
    /// The id of the control it names (`htmlFor`). Without one, it names the
    /// control inside it.
    pub for_id: Option<SharedString>,
    /// Its own id, which the label's element takes, so a control's
    /// `labelled_by` can name it (the instance's `id` in the original).
    pub id: Option<SharedString>,
    /// What it reads as: its name for assistive technology, and its control's
    /// name where its part isn't in the page.
    pub text: SharedString,
    /// Logs every change with its reason, in development builds.
    pub debug: bool,
}

impl LabelOptions {
    pub fn for_id(mut self, id: impl Into<SharedString>) -> Self {
        self.for_id = Some(id.into());
        self
    }

    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn text(mut self, text: impl Into<SharedString>) -> Self {
        self.text = text.into();
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }
}

fn form_options(options: &LabelOptions) -> FormLabelOptions {
    FormLabelOptions {
        id: options.id.clone(),
        for_id: options.for_id.clone(),
        text: options.text.clone(),
    }
}

/// A label. Its entity is its controller.
pub struct Label {
    instance: Instance,
    label: ui_sdk_forms::Label,
    id: Option<SharedString>,
}

impl Label {
    pub fn new(options: LabelOptions, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut instance = Instance::new("label").with_agent(cx);
        instance.debug = options.debug;
        let label = ui_sdk_forms::Label::new(form_options(&options));
        // The control it names can render after it, or be replaced, like a
        // group behind a condition: the label hears it once a frame draws.
        label.node().set_observer(Some(cx.entity_id()));
        Self {
            instance,
            label,
            id: options.id,
        }
    }

    /// Hands it new options, as a framework re-renders with new props. Its
    /// control hears a new `for_id` or text once the current update is done.
    pub fn set_options(&mut self, options: LabelOptions, cx: &mut Context<Self>) {
        let changed = self.label.text() != options.text
            || self.label.for_id() != options.for_id
            || self.id != options.id;
        self.instance.debug = options.debug;
        self.label.set_options(form_options(&options), cx);
        self.id = options.id;
        if changed {
            cx.notify();
        }
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    /// The forms label it renders, which its control's internals find.
    pub fn label(&self) -> &ui_sdk_forms::Label {
        &self.label
    }

    /// The control it names, as of the last frame (`ownControl`).
    pub fn control(&self) -> Option<Node> {
        self.label.control()
    }

    /// The `root` part: the label. A press on it activates its control the
    /// way a native label does: checks a checkbox, a switch or a radio,
    /// focuses a group's checked radio, and focuses anything else. A press on
    /// the control itself, or on another control inside the label, is
    /// theirs, and a disabled control ignores it. Assistive technology's
    /// click on the label reaches the control too.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<()> {
        let label = this.read(cx);
        let text = label.label.text();
        let forms = label.label.clone();
        let tracked = label.label.clone();
        let part = Part::new(&label.instance, "root", None, ())
            .map_el(|el| with_role(tracked.track(el), Role::Label))
            .on_press(move |event, window, cx| {
                let Some(control) = forms.control() else {
                    return;
                };
                if control.is_disabled() || presses_inside(&event, forms.node(), &control) {
                    return;
                }
                activate_control(&control, Some(event), window, cx);
            });
        let part = match &label.id {
            Some(id) => part.with_element_id(id.clone()),
            None => part,
        };
        if text.is_empty() {
            part
        } else {
            part.aria_label(text)
        }
    }
}

// A press on the control, or on another control inside the label, is
// theirs, as on a native label.
fn presses_inside(event: &InputEvent, label: &Node, control: &Node) -> bool {
    let InputEvent::Click(click) = event else {
        return false;
    };
    if matches!(click, ClickEvent::Keyboard(_)) {
        return false;
    }
    let position = click.position();
    let hit = |node: &Node| node.bounds().is_some_and(|bounds| bounds.contains(&position));
    hit(control)
        || label
            .descendants()
            .iter()
            .any(|node| node.kind() == NodeKind::Control && hit(node))
}

/// Agents never list it: the control it names carries its text
/// (`agent.shown`).
impl Agent for Label {
    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new()
    }

    fn agent_shown(&self, _cx: &App) -> bool {
        false
    }
}
