//! Port of `foundation/kernel/attrs.ts`: attributes every component writes
//! the same way, the state of a part that opens or checks, a value inside an
//! id, and the ARIA gpui's `aria_*` setters leave out.
//!
//! `data-open`, `data-closed`, `data-checked` and the like become fields of a
//! slot's state struct, which `Part::with` styles from. [`Checked`] is the
//! checked state every part with one shares.
//!
//! gpui's div sets a part's label, description, key shortcuts, selected,
//! expanded and toggled states, numeric value, value text, placeholder,
//! orientation, level, position in its set and row and column indices. An
//! [`Aria`] holds the rest a component writes, and [`Part::aria`] puts it on
//! a part: disabled, hidden, modal, invalid, required, read-only, busy,
//! current, the popup it opens, multiselectable, checked or pressed, live
//! regions, a role description, autocomplete, sorting, cell spans, and the
//! relations that point at other parts by their element ids. The part writes
//! them to its accesskit node as gpui builds it.
//!
//! [`Part::aria`]: crate::Part::aria

use accesskit::{Node, NodeId};
use gpui::{ElementId, SharedString};

/// A value made safe inside an id, like `format!("{id}-item-{}", id_part(value))`.
/// Values may hold spaces or `%`, which would break a reference to the id.
/// Whitespace and `%` are percent-encoded as UTF-8, as `encodeURIComponent`
/// does (`idPart`).
pub fn id_part(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if c == '%' || is_js_space(c) {
            let mut bytes = [0; 4];
            for byte in c.encode_utf8(&mut bytes).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

// What `\s` matches in a JavaScript regular expression.
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{0B}'
            | '\u{0C}'
            | '\r'
            | ' '
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// The marks an app writes on its own elements for the agent bridge: a change
/// a person must make (`data-agent-confirm`), a region agents stay out of
/// (`data-agent-hidden`) and values that never reach the model
/// (`data-agent-secret`). Each covers its element and everything inside it,
/// so one mark on a section covers every control in it. People see no
/// difference: the marks change no behavior, style or ARIA, and only the
/// bridge reads them. Put them on a part with `Part::agent_marks`, and on any
/// other element with an id with `part::with_agent_marks`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct AgentMarks {
    /// What the change does, which a person must approve, like "Charges $20
    /// to your card" (`data-agent-confirm`). Empty text still needs a person.
    pub confirm: Option<SharedString>,
    /// Agents never see what's inside (`data-agent-hidden`).
    pub hidden: bool,
    /// Values inside read `"[redacted]"` (`data-agent-secret`).
    pub secret: bool,
}

impl AgentMarks {
    pub fn new() -> Self {
        Self::default()
    }

    /// `data-agent-confirm`, with what the change does.
    pub fn confirm(mut self, text: impl Into<SharedString>) -> Self {
        self.confirm = Some(text.into());
        self
    }

    /// `data-agent-hidden`.
    pub fn hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }

    /// `data-agent-secret`.
    pub fn secret(mut self, secret: bool) -> Self {
        self.secret = secret;
        self
    }

    /// Whether it marks nothing.
    pub fn is_empty(&self) -> bool {
        self.confirm.is_none() && !self.hidden && !self.secret
    }

    /// `other` over this: its confirm text where it has one, and either's
    /// flags.
    pub fn merge(mut self, other: AgentMarks) -> Self {
        self.confirm = other.confirm.or(self.confirm);
        self.hidden |= other.hidden;
        self.secret |= other.secret;
        self
    }
}

/// Whether a part that opens is open: `data-open`, or `data-closed` (`openAttrs`).
/// Slot states carry it as `open: bool`; this names the pair.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct OpenState {
    pub open: bool,
}

impl OpenState {
    pub fn new(open: bool) -> Self {
        Self { open }
    }

    /// `data-closed`.
    pub fn closed(self) -> bool {
        !self.open
    }
}

/// Checked, unchecked or indeterminate: every part with a checked state
/// (`checkedAttrs`). Its ARIA is accesskit's `Toggled`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Checked {
    #[default]
    False,
    True,
    /// `"indeterminate"` in the original, `aria-checked="mixed"`.
    Indeterminate,
}

impl Checked {
    /// `data-checked`.
    pub fn is_checked(self) -> bool {
        self == Checked::True
    }

    /// `data-unchecked`.
    pub fn is_unchecked(self) -> bool {
        self == Checked::False
    }

    /// `data-indeterminate`.
    pub fn is_indeterminate(self) -> bool {
        self == Checked::Indeterminate
    }

    pub fn toggled(self) -> accesskit::Toggled {
        match self {
            Checked::False => accesskit::Toggled::False,
            Checked::True => accesskit::Toggled::True,
            Checked::Indeterminate => accesskit::Toggled::Mixed,
        }
    }
}

impl From<bool> for Checked {
    fn from(checked: bool) -> Self {
        if checked { Checked::True } else { Checked::False }
    }
}

/// `aria-current`: which of a set is the current one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Current {
    True,
    Page,
    Step,
    Location,
    Date,
    Time,
}

impl Current {
    fn accesskit(self) -> accesskit::AriaCurrent {
        match self {
            Current::True => accesskit::AriaCurrent::True,
            Current::Page => accesskit::AriaCurrent::Page,
            Current::Step => accesskit::AriaCurrent::Step,
            Current::Location => accesskit::AriaCurrent::Location,
            Current::Date => accesskit::AriaCurrent::Date,
            Current::Time => accesskit::AriaCurrent::Time,
        }
    }
}

/// `aria-haspopup`: what a trigger opens. `"true"` in the original is `Menu`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HasPopup {
    Menu,
    Listbox,
    Tree,
    Grid,
    Dialog,
}

impl HasPopup {
    fn accesskit(self) -> accesskit::HasPopup {
        match self {
            HasPopup::Menu => accesskit::HasPopup::Menu,
            HasPopup::Listbox => accesskit::HasPopup::Listbox,
            HasPopup::Tree => accesskit::HasPopup::Tree,
            HasPopup::Grid => accesskit::HasPopup::Grid,
            HasPopup::Dialog => accesskit::HasPopup::Dialog,
        }
    }
}

/// `aria-live`: how a screen reader reads changes to the part's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Live {
    Off,
    Polite,
    Assertive,
}

impl Live {
    fn accesskit(self) -> accesskit::Live {
        match self {
            Live::Off => accesskit::Live::Off,
            Live::Polite => accesskit::Live::Polite,
            Live::Assertive => accesskit::Live::Assertive,
        }
    }
}

/// `aria-invalid`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Invalid {
    True,
    Grammar,
    Spelling,
}

impl Invalid {
    fn accesskit(self) -> accesskit::Invalid {
        match self {
            Invalid::True => accesskit::Invalid::True,
            Invalid::Grammar => accesskit::Invalid::Grammar,
            Invalid::Spelling => accesskit::Invalid::Spelling,
        }
    }
}

/// `aria-autocomplete`: what typing in a field suggests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AutoComplete {
    Inline,
    List,
    Both,
}

impl AutoComplete {
    fn accesskit(self) -> accesskit::AutoComplete {
        match self {
            AutoComplete::Inline => accesskit::AutoComplete::Inline,
            AutoComplete::List => accesskit::AutoComplete::List,
            AutoComplete::Both => accesskit::AutoComplete::Both,
        }
    }
}

/// `aria-sort`: how a column or row header sorts its table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sort {
    Ascending,
    Descending,
    Other,
}

impl Sort {
    fn accesskit(self) -> accesskit::SortDirection {
        match self {
            Sort::Ascending => accesskit::SortDirection::Ascending,
            Sort::Descending => accesskit::SortDirection::Descending,
            Sort::Other => accesskit::SortDirection::Other,
        }
    }
}

/// The relations a part can point at other parts with, by their element ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Relation {
    LabelledBy,
    DescribedBy,
    Controls,
    Owns,
    Details,
    FlowTo,
    ActiveDescendant,
    ErrorMessage,
}

impl Relation {
    /// The attribute's name, like `"aria-labelledby"`.
    pub fn name(self) -> &'static str {
        match self {
            Relation::LabelledBy => "aria-labelledby",
            Relation::DescribedBy => "aria-describedby",
            Relation::Controls => "aria-controls",
            Relation::Owns => "aria-owns",
            Relation::Details => "aria-details",
            Relation::FlowTo => "aria-flowto",
            Relation::ActiveDescendant => "aria-activedescendant",
            Relation::ErrorMessage => "aria-errormessage",
        }
    }
}

/// The ARIA a part writes beyond gpui's `aria_*` setters. Build it with its
/// methods, which take `false` and `None` as "leave it out", and put it on a
/// part with `Part::aria`. Relations name the element ids of other parts,
/// like `instance.element_id("title", None)`; ones that name no part in the
/// page are left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Aria {
    pub disabled: bool,
    pub hidden: bool,
    pub modal: bool,
    pub invalid: Option<Invalid>,
    pub required: bool,
    pub read_only: bool,
    pub busy: bool,
    pub current: Option<Current>,
    pub has_popup: Option<HasPopup>,
    pub multiselectable: bool,
    /// `aria-checked`, for checkbox, radio, switch and menu item roles.
    pub checked: Option<Checked>,
    /// `aria-pressed`, for a button.
    pub pressed: Option<Checked>,
    /// `aria-valuetext`: the value as a person reads it, like "3 of 5".
    pub value_text: Option<SharedString>,
    pub live: Option<Live>,
    /// `aria-atomic`: a live region reads all of itself on a change.
    pub atomic: bool,
    pub role_description: Option<SharedString>,
    pub autocomplete: Option<AutoComplete>,
    pub sort: Option<Sort>,
    pub row_span: Option<usize>,
    pub column_span: Option<usize>,
    /// The label written when none of `labelled_by` is in the page, like a
    /// popup named by its trigger when it renders no title.
    pub fallback_label: Option<SharedString>,
    pub labelled_by: Vec<ElementId>,
    pub described_by: Vec<ElementId>,
    pub controls: Vec<ElementId>,
    pub owns: Vec<ElementId>,
    pub details: Vec<ElementId>,
    pub flow_to: Vec<ElementId>,
    /// `aria-activedescendant` on a container that keeps focus, by the
    /// element id of the item it points at.
    pub active_descendant: Option<ElementId>,
    pub error_message: Option<ElementId>,
}

impl Aria {
    pub fn new() -> Self {
        Self::default()
    }

    /// `aria-disabled`, and the native `disabled` a control would carry.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// `aria-hidden`: the part and everything in it are left out for
    /// assistive technology.
    pub fn hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }

    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    /// `aria-invalid="true"` when `invalid`.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid.then_some(Invalid::True);
        self
    }

    /// `aria-invalid` with what's wrong.
    pub fn invalid_as(mut self, invalid: impl Into<Option<Invalid>>) -> Self {
        self.invalid = invalid.into();
        self
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }

    pub fn current(mut self, current: impl Into<Option<Current>>) -> Self {
        self.current = current.into();
        self
    }

    pub fn has_popup(mut self, popup: impl Into<Option<HasPopup>>) -> Self {
        self.has_popup = popup.into();
        self
    }

    pub fn multiselectable(mut self, multiselectable: bool) -> Self {
        self.multiselectable = multiselectable;
        self
    }

    pub fn checked(mut self, checked: impl Into<Checked>) -> Self {
        self.checked = Some(checked.into());
        self
    }

    pub fn pressed(mut self, pressed: impl Into<Checked>) -> Self {
        self.pressed = Some(pressed.into());
        self
    }

    pub fn value_text(mut self, text: impl Into<SharedString>) -> Self {
        self.value_text = Some(text.into());
        self
    }

    pub fn live(mut self, live: impl Into<Option<Live>>) -> Self {
        self.live = live.into();
        self
    }

    pub fn atomic(mut self, atomic: bool) -> Self {
        self.atomic = atomic;
        self
    }

    pub fn role_description(mut self, description: impl Into<SharedString>) -> Self {
        self.role_description = Some(description.into());
        self
    }

    pub fn autocomplete(mut self, autocomplete: impl Into<Option<AutoComplete>>) -> Self {
        self.autocomplete = autocomplete.into();
        self
    }

    pub fn sort(mut self, sort: impl Into<Option<Sort>>) -> Self {
        self.sort = sort.into();
        self
    }

    pub fn row_span(mut self, span: usize) -> Self {
        self.row_span = Some(span);
        self
    }

    pub fn column_span(mut self, span: usize) -> Self {
        self.column_span = Some(span);
        self
    }

    pub fn fallback_label(mut self, label: impl Into<SharedString>) -> Self {
        self.fallback_label = Some(label.into());
        self
    }

    /// Adds a part that names this one.
    pub fn labelled_by(mut self, id: impl Into<ElementId>) -> Self {
        self.labelled_by.push(id.into());
        self
    }

    /// Adds a part that describes this one.
    pub fn described_by(mut self, id: impl Into<ElementId>) -> Self {
        self.described_by.push(id.into());
        self
    }

    /// Adds a part this one controls, like a trigger's popup.
    pub fn controls(mut self, id: impl Into<ElementId>) -> Self {
        self.controls.push(id.into());
        self
    }

    /// Adds a part this one owns that isn't inside it.
    pub fn owns(mut self, id: impl Into<ElementId>) -> Self {
        self.owns.push(id.into());
        self
    }

    pub fn details(mut self, id: impl Into<ElementId>) -> Self {
        self.details.push(id.into());
        self
    }

    pub fn flow_to(mut self, id: impl Into<ElementId>) -> Self {
        self.flow_to.push(id.into());
        self
    }

    /// The item a container that keeps focus points at.
    pub fn active_descendant(mut self, id: Option<impl Into<ElementId>>) -> Self {
        self.active_descendant = id.map(Into::into);
        self
    }

    pub fn error_message(mut self, id: Option<impl Into<ElementId>>) -> Self {
        self.error_message = id.map(Into::into);
        self
    }

    /// `other` over this: its flags and values where it sets them, its
    /// labelled-by in place of this one's when it names any, as an author's
    /// own label does (`authorOwned`), and the other relations of both.
    pub fn merge(mut self, other: Aria) -> Self {
        self.disabled |= other.disabled;
        self.hidden |= other.hidden;
        self.modal |= other.modal;
        self.required |= other.required;
        self.read_only |= other.read_only;
        self.busy |= other.busy;
        self.multiselectable |= other.multiselectable;
        self.atomic |= other.atomic;
        self.invalid = other.invalid.or(self.invalid);
        self.current = other.current.or(self.current);
        self.has_popup = other.has_popup.or(self.has_popup);
        self.checked = other.checked.or(self.checked);
        self.pressed = other.pressed.or(self.pressed);
        self.value_text = other.value_text.or(self.value_text);
        self.live = other.live.or(self.live);
        self.role_description = other.role_description.or(self.role_description);
        self.autocomplete = other.autocomplete.or(self.autocomplete);
        self.sort = other.sort.or(self.sort);
        self.row_span = other.row_span.or(self.row_span);
        self.column_span = other.column_span.or(self.column_span);
        self.fallback_label = other.fallback_label.or(self.fallback_label);
        if !other.labelled_by.is_empty() {
            self.labelled_by = other.labelled_by;
        }
        self.described_by.extend(other.described_by);
        self.controls.extend(other.controls);
        self.owns.extend(other.owns);
        self.details.extend(other.details);
        self.flow_to.extend(other.flow_to);
        self.active_descendant = other.active_descendant.or(self.active_descendant);
        self.error_message = other.error_message.or(self.error_message);
        self
    }

    /// The role-specific attributes this sets, for the role checks.
    pub(crate) fn role_attributes(&self) -> Vec<crate::roles::RoleAria> {
        use crate::roles::RoleAria;
        let mut written = Vec::new();
        if self.checked.is_some() {
            written.push(RoleAria::Checked);
        }
        if self.pressed.is_some() {
            written.push(RoleAria::Pressed);
        }
        if self.modal {
            written.push(RoleAria::Modal);
        }
        if self.multiselectable {
            written.push(RoleAria::MultiSelectable);
        }
        if self.read_only {
            written.push(RoleAria::ReadOnly);
        }
        if self.required {
            written.push(RoleAria::Required);
        }
        if self.autocomplete.is_some() {
            written.push(RoleAria::AutoComplete);
        }
        if self.sort.is_some() {
            written.push(RoleAria::Sort);
        }
        if self.row_span.is_some() {
            written.push(RoleAria::RowSpan);
        }
        if self.column_span.is_some() {
            written.push(RoleAria::ColSpan);
        }
        if self.active_descendant.is_some() {
            written.push(RoleAria::ActiveDescendant);
        }
        written
    }

    /// Writes the states and values to `node`, and each relation whose
    /// targets `resolve` finds in the tree.
    pub fn write(&self, node: &mut Node, resolve: &dyn Fn(&ElementId, Relation) -> Option<NodeId>) {
        self.write_states(node);
        self.write_relations(node, resolve);
    }

    /// Whether it has a relation to write, or a label for when its
    /// labelled-by targets aren't there.
    pub fn has_relations(&self) -> bool {
        !self.labelled_by.is_empty()
            || self.fallback_label.is_some()
            || !self.described_by.is_empty()
            || !self.controls.is_empty()
            || !self.owns.is_empty()
            || !self.details.is_empty()
            || !self.flow_to.is_empty()
            || self.active_descendant.is_some()
            || self.error_message.is_some()
    }

    /// Writes the states and values to `node`, leaving out the relations.
    pub fn write_states(&self, node: &mut Node) {
        if self.disabled {
            node.set_disabled();
        }
        if self.hidden {
            node.set_hidden();
        }
        if self.modal {
            node.set_modal();
        }
        if let Some(invalid) = self.invalid {
            node.set_invalid(invalid.accesskit());
        }
        if self.required {
            node.set_required();
        }
        if self.read_only {
            node.set_read_only();
        }
        if self.busy {
            node.set_busy();
        }
        if let Some(current) = self.current {
            node.set_aria_current(current.accesskit());
        }
        if let Some(popup) = self.has_popup {
            node.set_has_popup(popup.accesskit());
        }
        if self.multiselectable {
            node.set_multiselectable();
        }
        if let Some(checked) = self.checked.or(self.pressed) {
            node.set_toggled(checked.toggled());
        }
        if let Some(text) = &self.value_text {
            node.set_value(text.to_string());
        }
        if let Some(live) = self.live {
            node.set_live(live.accesskit());
        }
        if self.atomic {
            node.set_live_atomic();
        }
        if let Some(description) = &self.role_description {
            node.set_role_description(description.to_string());
        }
        if let Some(autocomplete) = self.autocomplete {
            node.set_auto_complete(autocomplete.accesskit());
        }
        if let Some(sort) = self.sort {
            node.set_sort_direction(sort.accesskit());
        }
        if let Some(span) = self.row_span {
            node.set_row_span(span);
        }
        if let Some(span) = self.column_span {
            node.set_column_span(span);
        }
    }

    /// Writes each relation whose targets `resolve` finds in the tree, and
    /// the fallback label when no labelled-by target is there.
    pub fn write_relations(&self, node: &mut Node, resolve: &dyn Fn(&ElementId, Relation) -> Option<NodeId>) {
        let all = |ids: &[ElementId], relation| -> Vec<NodeId> {
            ids.iter().filter_map(|id| resolve(id, relation)).collect()
        };
        let labels = all(&self.labelled_by, Relation::LabelledBy);
        if labels.is_empty() {
            if let Some(label) = &self.fallback_label
                && node.label().is_none()
            {
                node.set_label(label.to_string());
            }
        } else {
            node.set_labelled_by(labels);
        }
        let described = all(&self.described_by, Relation::DescribedBy);
        if !described.is_empty() {
            node.set_described_by(described);
        }
        let controls = all(&self.controls, Relation::Controls);
        if !controls.is_empty() {
            node.set_controls(controls);
        }
        let owns = all(&self.owns, Relation::Owns);
        if !owns.is_empty() {
            node.set_owns(owns);
        }
        let details = all(&self.details, Relation::Details);
        if !details.is_empty() {
            node.set_details(details);
        }
        let flow_to = all(&self.flow_to, Relation::FlowTo);
        if !flow_to.is_empty() {
            node.set_flow_to(flow_to);
        }
        if let Some(target) = self
            .active_descendant
            .as_ref()
            .and_then(|id| resolve(id, Relation::ActiveDescendant))
        {
            node.set_active_descendant(target);
        }
        if let Some(target) = self
            .error_message
            .as_ref()
            .and_then(|id| resolve(id, Relation::ErrorMessage))
        {
            node.set_error_message(target);
        }
    }
}
