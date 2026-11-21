//! Port of `foundation/text/accessible-name.ts`.
//!
//! A part's accessible name, close to how browsers compute it
//! (<https://www.w3.org/TR/accname-1.2/#computation-steps>), so popups, the
//! agent bridge, `observe` and development warnings agree on it.
//!
//! The original reads the name off the DOM: `aria-labelledby`, `aria-label`,
//! `<label>`s, an image's `alt`, an icon's `<title>`, text content and
//! `title`. gpui elements keep none of it where it can be read back, so a name
//! comes from what a part reports: a [`NameSources`] holds the same sources,
//! filled from what the component knows, like its options' `aria_label`, the
//! labels the forms crate associates with it, and the text its `Parts` entry
//! registers. The rules are the original's: the order sources are taken in,
//! which roles their content names, hidden content naming nothing, nested
//! names and fields inside labels, and collapsed whitespace.
//!
//! Roles are the kernel's ([`Role`]), what every part declares with
//! `aria_role`. Dropped: reading roles off tag names (`button`, `a[href]`,
//! `input` types, `dialog`, headings), since every part declares its own.

use gpui::SharedString;
use ui_sdk_kernel::roles::Role;

use crate::r#match::is_js_space;

/// What kind of element a piece of content is, where it changes the name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum NameKind {
    #[default]
    Element,
    /// A text field, a select or a text area, which adds nothing to the name
    /// of what contains it, so a label doesn't repeat the name of the field it names.
    Field,
    /// An icon, whose drawing isn't text: only its own name counts.
    Icon,
}

/// Where a part's name comes from, as it reports them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NameSources {
    /// Its role, which decides whether its content names it.
    pub role: Option<Role>,
    /// What kind of element it is, when it's inside another's content.
    pub kind: NameKind,
    /// Hidden from everyone, like `hidden` or `aria-hidden`. A hidden element
    /// can still name another through `labelled_by`, and then gives all its text.
    pub hidden: bool,
    /// The elements `aria-labelledby` points at, in order.
    pub labelled_by: Vec<NameSources>,
    /// Its `aria-label`.
    pub aria_label: Option<SharedString>,
    /// The labels that name it, like a field's `<label>`s.
    pub labels: Vec<NameSources>,
    /// What an image or an icon is called by its own markup: an image's alt
    /// text or an icon's title.
    pub embedded: Option<SharedString>,
    /// What it contains: text, and the elements inside with names of their own.
    pub content: Vec<NameContent>,
    /// Its `title`, the name of last resort.
    pub title: Option<SharedString>,
}

/// A piece of what an element contains.
#[derive(Clone, Debug, PartialEq)]
pub enum NameContent {
    Text(SharedString),
    Element(NameSources),
}

impl NameSources {
    /// Sources for a part with `role`.
    pub fn new(role: Role) -> Self {
        Self {
            role: Some(role),
            ..Default::default()
        }
    }

    /// An element whose content is `text` alone, like a label or a title part.
    pub fn text(text: impl Into<SharedString>) -> Self {
        Self::default().child_text(text)
    }

    pub fn role(mut self, role: Role) -> Self {
        self.role = Some(role);
        self
    }

    pub fn kind(mut self, kind: NameKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }

    /// Adds an element `aria-labelledby` points at.
    pub fn labelled_by(mut self, label: NameSources) -> Self {
        self.labelled_by.push(label);
        self
    }

    pub fn aria_label(mut self, label: impl Into<SharedString>) -> Self {
        self.aria_label = Some(label.into());
        self
    }

    /// Adds a label that names it, like a `<label>` of a field.
    pub fn label(mut self, label: NameSources) -> Self {
        self.labels.push(label);
        self
    }

    pub fn embedded(mut self, name: impl Into<SharedString>) -> Self {
        self.embedded = Some(name.into());
        self
    }

    /// Adds text to its content.
    pub fn child_text(mut self, text: impl Into<SharedString>) -> Self {
        self.content.push(NameContent::Text(text.into()));
        self
    }

    /// Adds an element to its content.
    pub fn child(mut self, child: NameSources) -> Self {
        self.content.push(NameContent::Element(child));
        self
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }
}

/// Roles a name can come from the element's text for. The others need
/// `aria-label`, `aria-labelledby`, a label element or `title`.
pub const NAMED_BY_CONTENT: [Role; 17] = [
    Role::Button,
    Role::Cell,
    Role::Checkbox,
    Role::ColumnHeader,
    Role::GridCell,
    Role::Heading,
    Role::Link,
    Role::MenuItem,
    Role::MenuItemCheckbox,
    Role::MenuItemRadio,
    Role::Option,
    Role::Radio,
    Role::Row,
    Role::RowHeader,
    Role::Switch,
    Role::Tab,
    Role::TreeItem,
];

/// Whether a role's content names it (`namedByContent.has(role)`).
pub fn named_by_content(role: Role) -> bool {
    NAMED_BY_CONTENT.contains(&role)
}

/// A part's ARIA role name, like `"button"` or `"menuitemcheckbox"`, as the
/// original's `roleOf` reads it off an element; empty without a role.
pub fn role_of(role: Option<Role>) -> &'static str {
    role.map_or("", Role::as_str)
}

// The names of the elements `labelled_by` points at, each by its
// `aria_label` or its content.
fn labelled_by_of(sources: &NameSources) -> String {
    sources
        .labelled_by
        .iter()
        .map(|label| match &label.aria_label {
            Some(name) => name.to_string(),
            None => content_of(label, true, label.hidden),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// What an image or an icon is called by its own markup.
fn embedded_name(sources: &NameSources) -> String {
    sources.embedded.as_deref().unwrap_or_default().to_string()
}

// What an element inside another's content adds to its name
// (https://www.w3.org/TR/accname-1.2/#step2): its `labelled_by`, unless this
// is already one's text, its `aria_label`, its image's alt, its content, then
// its title. A field inside a label adds nothing, so a label doesn't repeat
// the name of the field it names.
fn name_within(sources: &NameSources, labelling: bool, hidden: bool) -> String {
    if sources.kind == NameKind::Field {
        return String::new();
    }
    let labelled = if labelling { String::new() } else { labelled_by_of(sources) };
    let label = sources
        .aria_label
        .as_deref()
        .map(|label| label.trim_matches(is_js_space))
        .unwrap_or_default();
    let own = if !labelled.is_empty() {
        labelled
    } else if !label.is_empty() {
        label.to_string()
    } else {
        embedded_name(sources)
    };
    if !own.is_empty() {
        return format!(" {own} ");
    }
    // An icon's drawing isn't text.
    if sources.kind == NameKind::Icon {
        return String::new();
    }
    let content = content_of(sources, labelling, hidden);
    if !content.is_empty() {
        return content;
    }
    match sources.title.as_deref() {
        Some(title) if !title.is_empty() => format!(" {title} "),
        _ => String::new(),
    }
}

// The text an element's content names it with, without its hidden
// descendants. A hidden element can still name another through
// `labelled_by`, and then gives all its text, as browsers do.
fn content_of(sources: &NameSources, labelling: bool, hidden: bool) -> String {
    let mut text = String::new();
    for node in &sources.content {
        match node {
            NameContent::Text(value) => text.push_str(value),
            NameContent::Element(child) if hidden || !child.hidden => {
                text.push_str(&name_within(child, labelling, hidden));
            }
            NameContent::Element(_) => {}
        }
    }
    text
}

/// Text with each run of whitespace one space, and none around it, as the
/// page shows it (`text.replace(/\s+/g, " ").trim()`).
pub fn collapse(text: &str) -> String {
    let mut collapsed = String::with_capacity(text.len());
    for word in text.split(is_js_space).filter(|word| !word.is_empty()) {
        if !collapsed.is_empty() {
            collapsed.push(' ');
        }
        collapsed.push_str(word);
    }
    collapsed
}

// The labels' content.
fn labels_of(sources: &NameSources) -> String {
    sources
        .labels
        .iter()
        .map(|label| content_of(label, false, label.hidden))
        .collect::<Vec<_>>()
        .join(" ")
}

// Where a name comes from, in the order browsers take them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    LabelledBy,
    AriaLabel,
    Labels,
    Embedded,
    Content,
    Title,
}

const NAME_SOURCES: [Source; 6] = [
    Source::LabelledBy,
    Source::AriaLabel,
    Source::Labels,
    Source::Embedded,
    Source::Content,
    Source::Title,
];

// The same sources with labels last, for asking only whether there's a name.
const LABELS_LAST: [Source; 6] = [
    Source::LabelledBy,
    Source::AriaLabel,
    Source::Embedded,
    Source::Content,
    Source::Title,
    Source::Labels,
];

fn read(source: Source, sources: &NameSources) -> String {
    match source {
        Source::LabelledBy => labelled_by_of(sources),
        Source::AriaLabel => sources.aria_label.as_deref().unwrap_or_default().to_string(),
        Source::Labels => labels_of(sources),
        Source::Embedded => embedded_name(sources),
        Source::Content => {
            if sources.role.is_some_and(named_by_content) {
                content_of(sources, false, sources.hidden)
            } else {
                String::new()
            }
        }
        Source::Title => sources.title.as_deref().unwrap_or_default().to_string(),
    }
}

fn first_name(sources: &NameSources, order: &[Source]) -> String {
    order
        .iter()
        .map(|source| collapse(&read(*source, sources)))
        .find(|name| !name.is_empty())
        .unwrap_or_default()
}

/// An approximate accessible name: what `labelled_by`, `aria_label`, labels,
/// an image's alt, content for roles named by it, or `title` give, with
/// whitespace collapsed. Content counts its images' alt, its icons' names and
/// its elements' own labels.
pub fn accessible_name(sources: &NameSources) -> String {
    first_name(sources, &NAME_SOURCES)
}

/// Whether a part has an accessible name, reading its labels only when
/// nothing else names it.
pub fn has_accessible_name(sources: &NameSources) -> bool {
    !first_name(sources, &LABELS_LAST).is_empty()
}
