//! Port of `foundation/kernel/roles.ts`: the roles a part can declare, and
//! what each asks of its slot: one list per need, read by `Part::aria_role`
//! and by development builds alike.
//!
//! The original types a slot by its role, so TypeScript asks for exactly what
//! the role needs. Rust parts declare the role with [`Part::aria_role`], which
//! sets gpui's accesskit role, and development builds check, once per slot,
//! what the part's node ends up with: the focus, press and keys the role
//! needs, the ARIA it doesn't take, and the states it can't work without.
//!
//! [`Part::aria_role`]: crate::Part::aria_role

use accesskit::Role as AccessRole;

/// A WAI-ARIA role a part can declare with `Part::aria_role`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Alert,
    AlertDialog,
    Application,
    Article,
    Banner,
    Blockquote,
    Button,
    Caption,
    Cell,
    Checkbox,
    Code,
    ColumnHeader,
    Combobox,
    Complementary,
    ContentInfo,
    Definition,
    Deletion,
    Dialog,
    Document,
    Emphasis,
    Feed,
    Figure,
    Form,
    Generic,
    Grid,
    GridCell,
    Group,
    Heading,
    Img,
    Insertion,
    Link,
    List,
    Listbox,
    ListItem,
    Log,
    Main,
    Marquee,
    Math,
    Menu,
    Menubar,
    MenuItem,
    MenuItemCheckbox,
    MenuItemRadio,
    Meter,
    Navigation,
    None,
    Note,
    Option,
    Paragraph,
    Presentation,
    ProgressBar,
    Radio,
    RadioGroup,
    Region,
    Row,
    RowGroup,
    RowHeader,
    Scrollbar,
    Search,
    Searchbox,
    Separator,
    Slider,
    SpinButton,
    Status,
    Strong,
    Subscript,
    Superscript,
    Switch,
    Tab,
    Table,
    TabList,
    TabPanel,
    Term,
    Textbox,
    Time,
    Timer,
    Toolbar,
    Tooltip,
    Tree,
    TreeGrid,
    TreeItem,
}

impl Role {
    /// Every role, in the original's order.
    pub const ALL: [Role; 81] = {
        use Role::*;
        [
            Alert, AlertDialog, Application, Article, Banner, Blockquote, Button, Caption, Cell,
            Checkbox, Code, ColumnHeader, Combobox, Complementary, ContentInfo, Definition,
            Deletion, Dialog, Document, Emphasis, Feed, Figure, Form, Generic, Grid, GridCell,
            Group, Heading, Img, Insertion, Link, List, Listbox, ListItem, Log, Main, Marquee,
            Math, Menu, Menubar, MenuItem, MenuItemCheckbox, MenuItemRadio, Meter, Navigation,
            None, Note, Option, Paragraph, Presentation, ProgressBar, Radio, RadioGroup, Region,
            Row, RowGroup, RowHeader, Scrollbar, Search, Searchbox, Separator, Slider,
            SpinButton, Status, Strong, Subscript, Superscript, Switch, Tab, Table, TabList,
            TabPanel, Term, Textbox, Time, Timer, Toolbar, Tooltip, Tree, TreeGrid, TreeItem,
        ]
    };

    /// The role's WAI-ARIA name, like `"menuitemcheckbox"`.
    pub fn as_str(self) -> &'static str {
        use Role::*;
        match self {
            Alert => "alert",
            AlertDialog => "alertdialog",
            Application => "application",
            Article => "article",
            Banner => "banner",
            Blockquote => "blockquote",
            Button => "button",
            Caption => "caption",
            Cell => "cell",
            Checkbox => "checkbox",
            Code => "code",
            ColumnHeader => "columnheader",
            Combobox => "combobox",
            Complementary => "complementary",
            ContentInfo => "contentinfo",
            Definition => "definition",
            Deletion => "deletion",
            Dialog => "dialog",
            Document => "document",
            Emphasis => "emphasis",
            Feed => "feed",
            Figure => "figure",
            Form => "form",
            Generic => "generic",
            Grid => "grid",
            GridCell => "gridcell",
            Group => "group",
            Heading => "heading",
            Img => "img",
            Insertion => "insertion",
            Link => "link",
            List => "list",
            Listbox => "listbox",
            ListItem => "listitem",
            Log => "log",
            Main => "main",
            Marquee => "marquee",
            Math => "math",
            Menu => "menu",
            Menubar => "menubar",
            MenuItem => "menuitem",
            MenuItemCheckbox => "menuitemcheckbox",
            MenuItemRadio => "menuitemradio",
            Meter => "meter",
            Navigation => "navigation",
            None => "none",
            Note => "note",
            Option => "option",
            Paragraph => "paragraph",
            Presentation => "presentation",
            ProgressBar => "progressbar",
            Radio => "radio",
            RadioGroup => "radiogroup",
            Region => "region",
            Row => "row",
            RowGroup => "rowgroup",
            RowHeader => "rowheader",
            Scrollbar => "scrollbar",
            Search => "search",
            Searchbox => "searchbox",
            Separator => "separator",
            Slider => "slider",
            SpinButton => "spinbutton",
            Status => "status",
            Strong => "strong",
            Subscript => "subscript",
            Superscript => "superscript",
            Switch => "switch",
            Tab => "tab",
            Table => "table",
            TabList => "tablist",
            TabPanel => "tabpanel",
            Term => "term",
            Textbox => "textbox",
            Time => "time",
            Timer => "timer",
            Toolbar => "toolbar",
            Tooltip => "tooltip",
            Tree => "tree",
            TreeGrid => "treegrid",
            TreeItem => "treeitem",
        }
    }

    /// The role a WAI-ARIA name names (`knownRoles`).
    pub fn from_name(name: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|role| role.as_str() == name)
    }

    /// The accesskit role gpui reports for it. `None` for roles with no node
    /// of their own: `generic`, `none` and `presentation`, and text-level
    /// roles accesskit has no role for.
    pub fn accesskit(self) -> Option<AccessRole> {
        use Role::*;
        Some(match self {
            Alert => AccessRole::Alert,
            AlertDialog => AccessRole::AlertDialog,
            Application => AccessRole::Application,
            Article => AccessRole::Article,
            Banner => AccessRole::Banner,
            Blockquote => AccessRole::Blockquote,
            Button => AccessRole::Button,
            Caption => AccessRole::Caption,
            Cell => AccessRole::Cell,
            Checkbox => AccessRole::CheckBox,
            Code => AccessRole::Code,
            ColumnHeader => AccessRole::ColumnHeader,
            Combobox => AccessRole::ComboBox,
            Complementary => AccessRole::Complementary,
            ContentInfo => AccessRole::ContentInfo,
            Definition => AccessRole::Definition,
            Deletion => AccessRole::ContentDeletion,
            Dialog => AccessRole::Dialog,
            Document => AccessRole::Document,
            Emphasis => AccessRole::Emphasis,
            Feed => AccessRole::Feed,
            Figure => AccessRole::Figure,
            Form => AccessRole::Form,
            Grid => AccessRole::Grid,
            GridCell => AccessRole::GridCell,
            Group => AccessRole::Group,
            Heading => AccessRole::Heading,
            Img => AccessRole::Image,
            Insertion => AccessRole::ContentInsertion,
            Link => AccessRole::Link,
            List => AccessRole::List,
            Listbox => AccessRole::ListBox,
            ListItem => AccessRole::ListItem,
            Log => AccessRole::Log,
            Main => AccessRole::Main,
            Marquee => AccessRole::Marquee,
            Math => AccessRole::Math,
            Menu => AccessRole::Menu,
            Menubar => AccessRole::MenuBar,
            MenuItem => AccessRole::MenuItem,
            MenuItemCheckbox => AccessRole::MenuItemCheckBox,
            MenuItemRadio => AccessRole::MenuItemRadio,
            Meter => AccessRole::Meter,
            Navigation => AccessRole::Navigation,
            Note => AccessRole::Note,
            Option => AccessRole::ListBoxOption,
            Paragraph => AccessRole::Paragraph,
            ProgressBar => AccessRole::ProgressIndicator,
            Radio => AccessRole::RadioButton,
            RadioGroup => AccessRole::RadioGroup,
            Region => AccessRole::Region,
            Row => AccessRole::Row,
            RowGroup => AccessRole::RowGroup,
            RowHeader => AccessRole::RowHeader,
            Scrollbar => AccessRole::ScrollBar,
            Search => AccessRole::Search,
            Searchbox => AccessRole::SearchInput,
            Separator => AccessRole::Splitter,
            Slider => AccessRole::Slider,
            SpinButton => AccessRole::SpinButton,
            Status => AccessRole::Status,
            Strong => AccessRole::Strong,
            Switch => AccessRole::Switch,
            Tab => AccessRole::Tab,
            Table => AccessRole::Table,
            TabList => AccessRole::TabList,
            TabPanel => AccessRole::TabPanel,
            Term => AccessRole::Term,
            Textbox => AccessRole::TextInput,
            Time => AccessRole::Time,
            Timer => AccessRole::Timer,
            Toolbar => AccessRole::Toolbar,
            Tooltip => AccessRole::Tooltip,
            Tree => AccessRole::Tree,
            TreeGrid => AccessRole::TreeGrid,
            TreeItem => AccessRole::TreeItem,
            Generic | None | Presentation | Subscript | Superscript => return std::option::Option::None,
        })
    }

    /// Whether the role takes focus, so its part says how (`focusableRoles`).
    /// Not `separator`, which takes focus only as a window splitter.
    pub fn takes_focus(self) -> bool {
        use Role::*;
        matches!(
            self,
            Button
                | Checkbox
                | Combobox
                | GridCell
                | Link
                | MenuItem
                | MenuItemCheckbox
                | MenuItemRadio
                | Option
                | Radio
                | Searchbox
                | Slider
                | SpinButton
                | Switch
                | Tab
                | Textbox
                | TreeItem
        )
    }

    /// Whether the role activates, so its part has a press (`pressableRoles`).
    pub fn activates(self) -> bool {
        use Role::*;
        matches!(
            self,
            Button
                | Checkbox
                | Link
                | MenuItem
                | MenuItemCheckbox
                | MenuItemRadio
                | Option
                | Radio
                | Switch
                | Tab
                | TreeItem
        )
    }

    /// Whether the role's pattern moves with keys, so its part lists the keys
    /// it handles (`keyedRoles`).
    pub fn moves_with_keys(self) -> bool {
        use Role::*;
        matches!(
            self,
            Combobox
                | Grid
                | Listbox
                | Menu
                | Menubar
                | RadioGroup
                | Slider
                | SpinButton
                | TabList
                | Toolbar
                | Tree
                | TreeGrid
        )
    }

    /// Whether the role can't be named: a label on it is ignored (`namelessRoles`).
    pub fn is_nameless(self) -> bool {
        use Role::*;
        matches!(
            self,
            Caption
                | Code
                | Deletion
                | Emphasis
                | Generic
                | Insertion
                | None
                | Paragraph
                | Presentation
                | Strong
                | Subscript
                | Superscript
        )
    }

    /// The states the role's pattern can't work without, which its part must
    /// always write (`requiredStates`).
    pub fn required_states(self) -> &'static [RoleAria] {
        use Role::*;
        match self {
            Checkbox | MenuItemCheckbox | MenuItemRadio | Radio | Switch => &[RoleAria::Checked],
            Combobox => &[RoleAria::Expanded],
            Meter | Slider => &[RoleAria::ValueNow],
            Option | Tab => &[RoleAria::Selected],
            _ => &[],
        }
    }

    /// Whether the role takes `attribute` (`ariaSupport`).
    pub fn takes(self, attribute: RoleAria) -> bool {
        attribute.roles().contains(&self)
    }

    /// "a" or "an", as warnings name it.
    pub fn article(self) -> &'static str {
        if self.as_str().starts_with(['a', 'e', 'i', 'o', 'u']) {
            "an"
        } else {
            "a"
        }
    }
}

/// How a part that takes focus gets it (`Focus`). `Roving`: one of a set
/// holds the tab stop, and keys move it. `TabStop`: always in the tab order.
/// `ActiveDescendant`: focus stays on another element, which points at the
/// part with an active descendant. `Native`: the element focuses on its own,
/// as a native control would.
pub type Focus = crate::part::FocusMode;

/// A key a part handles, by the original's `KeyboardEvent.key` name, like
/// `"ArrowDown"` or `" "`, or `"typeahead"` for typed characters that move to
/// a matching item. Modifiers are written as in shortcuts, like `"mod+a"`.
/// [`Key::name`](crate::Key::name) gives a gpui keystroke the same names.
pub type HandledKey = &'static str;

/// A need a part's role has that another element meets, with the reason, like
/// `Unneeded("The combobox's input handles them.")` (`{ none: "why" }`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unneeded(pub &'static str);

/// What a role asks of its part.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Need {
    /// It takes focus, so the part says how.
    Focus,
    /// It activates, so the part has a press.
    Press,
    /// It moves with keys, so the part lists the keys it handles.
    Keys,
    /// It lists keys, so it has a key handler. Rust parts list keys and
    /// attach the handler together, so this one never goes unmet.
    KeyDown,
}

impl Need {
    pub fn as_str(self) -> &'static str {
        match self {
            Need::Focus => "focus",
            Need::Press => "press",
            Need::Keys => "keys",
            Need::KeyDown => "keydown",
        }
    }
}

/// What a part declares or has for its role's needs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Met {
    pub focus: bool,
    pub press: bool,
    pub keys: bool,
    pub keydown: bool,
}

/// The needs a role has that a part doesn't meet: `KeyDown` when it lists
/// keys with no key handler (`unmetNeeds`).
pub fn unmet_needs(role: Role, met: Met) -> Vec<Need> {
    let mut unmet = Vec::new();
    if role.takes_focus() && !met.focus {
        unmet.push(Need::Focus);
    }
    if role.activates() && !met.press {
        unmet.push(Need::Press);
    }
    if role.moves_with_keys() && !met.keys {
        unmet.push(Need::Keys);
    }
    if met.keys && !met.keydown {
        unmet.push(Need::KeyDown);
    }
    unmet
}

/// A role-specific ARIA attribute, from WAI-ARIA 1.2 with what subclass roles
/// inherit. Global ones, like `aria-label` and `aria-describedby`, aren't
/// listed: every role takes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RoleAria {
    ActiveDescendant,
    AutoComplete,
    Checked,
    ColCount,
    ColIndex,
    ColSpan,
    Expanded,
    Level,
    Modal,
    Multiline,
    MultiSelectable,
    Orientation,
    Placeholder,
    PosInSet,
    Pressed,
    ReadOnly,
    Required,
    RowCount,
    RowIndex,
    RowSpan,
    Selected,
    SetSize,
    Sort,
    ValueMax,
    ValueMin,
    ValueNow,
    ValueText,
}

impl RoleAria {
    /// The attribute's name, like `"aria-checked"`.
    pub fn name(self) -> &'static str {
        use RoleAria::*;
        match self {
            ActiveDescendant => "aria-activedescendant",
            AutoComplete => "aria-autocomplete",
            Checked => "aria-checked",
            ColCount => "aria-colcount",
            ColIndex => "aria-colindex",
            ColSpan => "aria-colspan",
            Expanded => "aria-expanded",
            Level => "aria-level",
            Modal => "aria-modal",
            Multiline => "aria-multiline",
            MultiSelectable => "aria-multiselectable",
            Orientation => "aria-orientation",
            Placeholder => "aria-placeholder",
            PosInSet => "aria-posinset",
            Pressed => "aria-pressed",
            ReadOnly => "aria-readonly",
            Required => "aria-required",
            RowCount => "aria-rowcount",
            RowIndex => "aria-rowindex",
            RowSpan => "aria-rowspan",
            Selected => "aria-selected",
            SetSize => "aria-setsize",
            Sort => "aria-sort",
            ValueMax => "aria-valuemax",
            ValueMin => "aria-valuemin",
            ValueNow => "aria-valuenow",
            ValueText => "aria-valuetext",
        }
    }

    /// The roles that take the attribute (`ariaSupport`).
    pub fn roles(self) -> &'static [Role] {
        use Role::*;
        match self {
            RoleAria::ActiveDescendant => &[
                Application, Combobox, Grid, Group, Listbox, Menu, Menubar, RadioGroup, Row,
                Searchbox, SpinButton, TabList, Textbox, Toolbar, Tree, TreeGrid,
            ],
            RoleAria::AutoComplete => &[Combobox, Searchbox, Textbox],
            RoleAria::Checked => &[
                Checkbox, MenuItemCheckbox, MenuItemRadio, Option, Radio, Switch, TreeItem,
            ],
            RoleAria::ColCount => &[Grid, Table, TreeGrid],
            RoleAria::ColIndex => &[Cell, ColumnHeader, GridCell, Row, RowHeader],
            RoleAria::ColSpan => &[Cell, ColumnHeader, GridCell, RowHeader],
            RoleAria::Expanded => &[
                Application, Button, Checkbox, ColumnHeader, Combobox, GridCell, Link, Listbox,
                MenuItem, MenuItemCheckbox, MenuItemRadio, Row, RowHeader, Switch, Tab, TreeItem,
            ],
            RoleAria::Level => &[Heading, ListItem, Row, TreeItem],
            RoleAria::Modal => &[AlertDialog, Dialog],
            RoleAria::Multiline => &[Searchbox, Textbox],
            RoleAria::MultiSelectable => &[Grid, Listbox, TabList, Tree, TreeGrid],
            RoleAria::Orientation => &[
                Listbox, Menu, Menubar, RadioGroup, Scrollbar, Separator, Slider, TabList,
                Toolbar, Tree, TreeGrid,
            ],
            RoleAria::Placeholder => &[Searchbox, Textbox],
            RoleAria::PosInSet | RoleAria::SetSize => &[
                Article, ListItem, MenuItem, MenuItemCheckbox, MenuItemRadio, Option, Radio, Row,
                Tab, TreeItem,
            ],
            RoleAria::Pressed => &[Button],
            RoleAria::ReadOnly => &[
                Checkbox, ColumnHeader, Combobox, Grid, GridCell, Listbox, MenuItemCheckbox,
                MenuItemRadio, RadioGroup, RowHeader, Searchbox, Slider, SpinButton, Switch,
                Textbox,
            ],
            RoleAria::Required => &[
                Checkbox, ColumnHeader, Combobox, GridCell, Listbox, RadioGroup, RowHeader,
                Searchbox, SpinButton, Switch, Textbox, Tree, TreeGrid,
            ],
            RoleAria::RowCount => &[Grid, Table, TreeGrid],
            RoleAria::RowIndex => &[Cell, ColumnHeader, GridCell, Row, RowHeader],
            RoleAria::RowSpan => &[Cell, ColumnHeader, GridCell, RowHeader],
            RoleAria::Selected => &[ColumnHeader, GridCell, Option, Row, RowHeader, Tab, TreeItem],
            RoleAria::Sort => &[ColumnHeader, RowHeader],
            RoleAria::ValueMax | RoleAria::ValueMin | RoleAria::ValueNow | RoleAria::ValueText => {
                &[Meter, ProgressBar, Scrollbar, Separator, Slider, SpinButton]
            }
        }
    }
}

/// What a part's node writes, for [`aria_problems`]: the role-specific
/// attributes, and whether it's named.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub attributes: Vec<RoleAria>,
    /// A label, or a labelled-by relation.
    pub named: bool,
}

/// What's wrong with the ARIA a part writes for its role.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AriaProblems {
    /// Attributes the role doesn't take, by name, so assistive technology
    /// ignores them. `"aria-label"` on a role that can't be named.
    pub not_allowed: Vec<&'static str>,
    /// States the role requires that the part writes without.
    pub missing: Vec<RoleAria>,
}

impl AriaProblems {
    pub fn is_empty(&self) -> bool {
        self.not_allowed.is_empty() && self.missing.is_empty()
    }
}

/// The ARIA attributes a part writes that its role doesn't take, and the
/// states the role requires that it writes without, like `aria-checked` on a
/// checkbox (`ariaProblems`).
pub fn aria_problems(role: Role, written: &Written) -> AriaProblems {
    let mut problems = AriaProblems::default();
    for &attribute in &written.attributes {
        if !role.takes(attribute) {
            problems.not_allowed.push(attribute.name());
        }
    }
    if written.named && role.is_nameless() {
        problems.not_allowed.push("aria-label");
    }
    problems.missing = role
        .required_states()
        .iter()
        .copied()
        .filter(|state| !written.attributes.contains(state))
        .collect();
    problems
}
