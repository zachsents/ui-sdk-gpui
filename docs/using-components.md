# Using Components

Rust/gpui mechanics for the components in [Getting Started](getting-started.md). For component behavior, use the [UI SDK docs](https://ui-sdk.dev/docs); [Components](components.md) links each component to its upstream page and Rust example.

The snippets assume `use gpui::prelude::*;` and gpui's `div`, `px`, `rgb` and `text`, and run inside a view like the one Getting Started builds, with its `window` and `cx`. Most leave out the `use` lines for a component's own types, which come from `ui_sdk::components::<name>`, like `ui_sdk::components::popover::{Popover, PopoverOptions}`.

## Components and Entities

Make a component once, in your view's constructor, and keep its entity in a field:

```rust
use ui_sdk::components::switch::{Switch, SwitchOptions};

let wifi = cx.new(|cx| {
    Switch::new(
        SwitchOptions::default().id("wifi").default_checked(true),
        window,
        cx,
    )
});
```

Every component takes an options struct like `SwitchOptions`. Start from `Default` and call a method for each option you set; the fields are public too. An option named after a Rust keyword keeps its raw identifier, like `ButtonOptions::r#type` or `TabsOptions::r#loop`. Don't make components in `render`: every frame would get a new one, with its state starting over.

To change options later, hand the component a whole new struct with `set_options`:

```rust
self.wifi.update(cx, |switch, cx| {
    switch.set_options(SwitchOptions::default().id("wifi").disabled(airplane_mode), cx);
});
```

It replaces every option, callbacks included, so pass those again. A new `default_*` value doesn't move the current one; on a form control, it changes what a form reset returns to. Some components' `set_options` take the window as well.

The entity is also the component's controller. Its methods are the original's api and controller methods in snake_case, like `set_checked`, `toggle`, `set_value`, `set_open` and `focus`. Methods that change state take options that say why, `ChangeOptions` for most, and the window, so call them with `update` from a handler that has the window:

```rust
use ui_sdk::kernel::ChangeOptions;

let off = div().id("off").on_click(cx.listener(|this, _, window, cx| {
    this.wifi.update(cx, |switch, cx| {
        switch.set_checked(false, ChangeOptions::default(), window, cx);
    });
}));
```

`ChangeOptions::default()` reports the change with the reason `Imperative`, and `ChangeOptions::reason(..)` with another. A call goes through the same change callback as a click, so it can be canceled, and it returns `false` when the change was canceled or changed nothing. Reads take only the app, like `self.wifi.read(cx).is_checked()`. Popover, Tooltip, Hover Card, Menu, Menubar and Alert Dialog also keep a `controller` option, made with `create_popover_controller()` and the like, for code that needs a handle before the component exists.

## Parts

A part function builds one of a component's elements from its entity, like `Switch::root(&self.wifi, cx)` or `Slider::thumb(&self.volume, 0, cx)`. Call them in `render` on every frame, as you would build any element. The view that renders a component's parts draws again whenever the component changes, as [Show the Settings](getting-started.md#show-the-settings) explains.

Some part functions take more than the entity:

- Props, with what the original passes that part, like `SelectItemProps` or `PopoverContentProps`.
- A key or an index, for a part there can be several of, like a menu's items, a dialog's triggers or a slider's thumbs. Each key makes its own element id, so render the part for a key once per frame.

A part that's only there some of the time comes back as an `Option`: a popup's content while it's open, a collapsible's panel, a field's error once it shows. Add those with `.children(..)`.

A part function returns a `Part<State>`: a `div` with the part's element id, role, focus handling, press and key handlers and ARIA already on it, and the part's state beside it. `Part` implements gpui's `Styled`, `InteractiveElement`, `StatefulInteractiveElement` and `ParentElement`, so you style it, add handlers and give it children like any `div`. A few parts come in a type of their own around a `Part`, like a popup's content (`PopupContent`) and a tooltip's trigger (`TooltipTrigger`), which style the same way.

`with` styles a part from its state, and `state()` reads it:

```rust
let trigger = Popover::trigger(&self.details, "details", PopoverTriggerProps::default(), cx)
    .with(|el, state| el.when(state.open, |el| el.bg(rgb(0xe5e7eb))))
    .px_3()
    .py_1()
    .rounded_md()
    .child(text!("Details"));
let open = trigger.state().open;
```

`with` hands you the part's `Stateful<Div>` and its state and runs right away, so a style set after it wins over one set inside it. The state holds what the original writes as data attributes and CSS variables. Where the original's CSS selects `[data-slot="popover-trigger"][data-open]`, you read `state.open` on `Popover::trigger`'s part, and `slot()` returns that slot name, `"popover-trigger"`. Most part states are structs named after their component and part, like `PopoverTriggerState`, and rustdoc lists their fields.

A handler added to a part runs after that part's own handler. A focused item's key handler still runs before its container's, so `window.prevent_default()` there can stop the container's arrow-key behavior. To intercept a key before a part handles it, use `capture_key_down` on an ancestor. Screen-reader and agent presses call only the part's own press handler; put application actions in the component's action callback, not an added `.on_click`.

## Styling and Animation

Parts come with no styles. Style them with gpui's methods, and from their state with `with`, which carries what the original's CSS reads:

- Data attributes are fields: `data-open` is `state.open`, `data-highlighted` is `state.highlighted`, and a popup's `data-side` is the `side` in its content's `state.placement`.
- CSS variables are values: `available` and `anchor_size` in a popup content's `state.placement` (`--available-height`, `--anchor-width`), or a collapsible panel's `state.panel.height`.

gpui's interaction styles work on parts as on any element: `hover`, `active`, `focus`, and `focus_visible`, which applies while the part has focus and the last input was the keyboard, like CSS's `:focus-visible`. Here's a toggle styled from its state, with a border that shows keyboard focus:

```rust
Toggle::root(&self.bold, cx)
    .aria_label("Bold")
    .with(|el, state| el.when(state.pressed, |el| el.bg(rgb(0x1f2937)).text_color(rgb(0xffffff))))
    .px_3()
    .py_1()
    .border_1()
    .border_color(rgb(0xd1d5db))
    .rounded_md()
    .focus_visible(|style| style.border_color(rgb(0x2563eb)))
    .child("B")
```

In a list, style the highlighted item from `state.highlighted` rather than `hover`, since the highlight follows the arrow keys as well as the pointer. gpui allows one `on_hover` listener per element, and parts that react to the pointer resting on them, like a tooltip's trigger, already have one; [Differences From the Original](differences.md#keyboard-focus-and-pointer) lists them.

### Animate Opening and Closing

gpui has no CSS transitions to wait for, so a part that animates in and out takes the timing its CSS would declare, a `PresenceTiming`, and plays it on animation frames. Popup content takes one in its props:

```rust
use gpui::ease_in_out;
use ui_sdk::foundation::motion::{PresenceTiming, Timing};

let timing = PresenceTiming::new(Timing::ms(150).easing(ease_in_out));
let content = Popover::content(&self.notes, PopoverContentProps::default().timing(timing), cx).map(|content| {
    content
        .with(|el, state| el.opacity(state.presence.progress))
        .p_3()
        .bg(rgb(0xffffff))
});
```

`state.presence` stands in for the original's `data-starting-style` and `data-ending-style`. Its `progress` eases from 0 to 1 as the popup opens and back to 0 as it closes, `phase` says whether it's `Entering`, `Open`, `Exiting` or `Closed`, and `starting` and `ending` mark the enter's first frame and the exit. The content stays in the window until the exit finishes, with `state.open` already false, and `on_open_change_complete` runs then. When gpui's `cx.reduce_motion()` flag is set, the timing counts as zero. gpui doesn't read that flag from the system; your app sets it with `cx.set_reduce_motion(..)`. gpui styles have no transforms, so animate opacity, a margin or a size rather than a scale.

Other parts take timing the same way. A dialog's backdrop takes it as an argument, `Dialog::backdrop(&self.share, Some(timing), cx)`. Collapsible, Accordion and Tabs panels take `panel_timing` in their options, and a collapsible's or an accordion's panel state also has the `height` it animates through. Checkbox and Radio Group indicators take `indicator_timing`.

## State and Events

A component keeps its own state unless you control it, and its change callbacks hear every change either way.

### Start or Control a Value

`default_*` options pick where a component starts, like `default_checked`, `default_pressed`, `default_open` or `default_value`. From then on it keeps its own value.

Pass the value itself instead, like `pressed` or `open`, to control it. The component then shows the value you pass and asks for changes through its callback, and you pass the next value back. Here the view keeps a toggle's state, as `crates/components/toggle/examples/toggle.rs` does:

```rust
let this = cx.entity().downgrade();
let italic = cx.new(|cx| {
    Toggle::new(
        ToggleOptions::default()
            .pressed(false)
            .on_pressed_change(move |pressed, _details, _window, cx| {
                let pressed = *pressed;
                this.update(cx, |view, cx| {
                    view.italic_pressed = pressed;
                    cx.notify();
                })
                .ok();
            }),
        window,
        cx,
    )
});
```

The callback stores the requested value in the view, and the view's `render` passes it back:

```rust
let pressed = self.italic_pressed;
self.italic.update(cx, |toggle, cx| toggle.set_controlled_pressed(Some(pressed), cx));
```

If the view kept passing `false`, the toggle would stay off however it's pressed. Each component's `set_controlled_*` takes `Some(value)` to control it, and `None` hands the value back to the component, which keeps the last one. A value that can be empty, like Tabs' or a Radio Group's, has an `Option` inside, so `Some(None)` controls it with nothing selected. Select, Combobox, Autocomplete and Trigger Menu have no `set_controlled_*` methods: pass their `value` or `open` in their options through `set_options`.

Debug builds warn when a component asked for a value and didn't get it back within a second, and when a controlled value goes back to `None`. The warnings need a logger to show, as [Log Changes](#log-changes) explains.

### Understand a Change

A change callback receives the requested value, `details`, the window and the app, as in `on_checked_change(|checked, details, window, cx| ..)`. `details` is a `ChangeDetails`:

| Field | What It Tells You |
| --- | --- |
| `reason` | What asked for the change, a `Reason` like `TriggerPress` or `EscapeKey`. Each callback's rustdoc lists the reasons it reports, and `reason.as_str()` gives the original's name, like `"escape-key"`. |
| `source` | How the person caused it, a `ChangeSource`: `Mouse`, `Touch`, `Keyboard`, `Virtual` (a click with no pointer or key behind it, like a screen reader's), `Programmatic` or `Agent`. |
| `event` | The gpui event behind it, as an `InputEvent`, or `None` for a call. |
| `cancel()` | Refuses the change. |

For example, keep a popover open when someone clicks outside it:

```rust
use ui_sdk::kernel::Reason;

let details = cx.new(|cx| {
    Popover::new(
        PopoverOptions::default().on_before_open_change(|open, details, _window, _cx| {
            if !*open && details.reason == Reason::OutsidePress {
                details.cancel();
            }
        }),
        window,
        cx,
    )
});
```

Escape and the trigger still close it. Canceling works whether the value is controlled or not. A change is refused in its `on_before_*` callback, like `on_before_open_change` here, since `on_open_change` hears the change once it has happened ([When Callbacks Run](#when-callbacks-run)). A few callbacks get details of their own, which dereference to `ChangeDetails` and add a field, like a slider's `details.thumb`.

### Commit a Value

Components you change in steps also say when the interaction ends. A slider's `on_value_change` runs on every step of a drag, and `on_value_commit` runs once, when the pointer lifts or the key comes up, which is the time to save:

```rust
let volume = cx.new(|cx| {
    Slider::new(
        SliderOptions::default()
            .default_value(50.0)
            .on_value_commit(|value, _details, _window, _cx| println!("volume {value}")),
        window,
        cx,
    )
});
```

Number Input, Color Picker and OTP Input have `on_value_commit` too, Resizable has `on_layout_commit` and Image Cropper `on_crop_commit`. Commits can't be canceled.

### When Callbacks Run

A component's callbacks, like `on_value_change`, `on_open_change`, `on_checked_change` and `on_value_commit`, run once the update that set them off is over, so they can update any entity: the view that holds the component, and the component itself. That holds for a click or typing and for your own calls: when the view clears an input it holds with `clear`, the input's `on_value_change` can update the view, which is still in the middle of its update when it calls `clear`, since the callback runs once that update is over too. They hear a change after it happened, so `details.cancel()` there does nothing, and debug builds warn about it.

To refuse a change, or look at it before it's taken, use its `on_before_*` callback, like `on_before_value_change` or `on_before_open_change`, which every change that can be refused has. It runs while gpui is updating the component, before the change, and `details.cancel()` keeps things as they were. Toggle Group and Checkbox Group's `on_before_value_change` also run inside the update of the toggle or checkbox that changed. An `on_before_*` callback can update any other entity, but reading or updating the component itself, or whatever called into it, panics, since gpui doesn't allow that while an entity is being updated. To change the component from there, defer the change with `window.defer(cx, ..)`, which runs once the update is over.

Handlers whose return value the component uses, like an action returning an `ActionOutcome`, run at the press, inside the update, since what they return decides what the part does next.

### Log Changes

In debug builds, pass `debug(true)` in a component's options to log each change with its reason and source. Pressing a toggle logs a line like this:

```text
[ui-sdk] toggle#ui-sdk-3 pressed: true (press, mouse)
```

These lines go through the `log` crate, at the debug level, as do the warnings debug builds print, at the warn level, like a controlled value that never came back or a control with no name. gpui installs no logger, so install one at the start of `main`, like `env_logger` 0.11, which gpui's own examples use:

```rust
env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
```

That shows the warnings. To see the `debug` lines too, run with `RUST_LOG=warn,ui_sdk_kernel=debug`. The gallery's `crates/gallery/src/main.rs` sets up its logger the same way.

## Accessible Names

gpui can't read back drawn text. Give controls an `aria_label` and draw their visible label as a plain string:

```rust
Toggle::root(&self.bold, cx).aria_label("Bold").child("Bold")
```

A part that names or describes another passes on its own `aria_label`. Use the same pattern for popup titles, descriptions, field labels and group labels:

```rust
Dialog::title(&self.share, cx).aria_label("Share link").child("Share link")
```

The standalone `Label` component takes a `text` option instead; [Quick Start](../README.md#quick-start) shows its `for_id` association. List items also need their `text` or `label` prop for typeahead and filtering. Combobox and Autocomplete items need `aria_label` as well. Tooltips take their accessible text in `TooltipOptions::text`.

For text a screen reader should read on its own, use gpui's `text!(..)`. Plain string children aren't accessibility nodes. Some roles derive screen-reader names from `text!` children, but the agent bridge and development name checks can't; `aria_label` works for all three.

## Popups

Popovers, dialogs, menus, selects, tooltips and the other popups render their content above the window wherever you put it in your tree. The content part puts itself in gpui's `deferred(..)`, at the priority popups share, so it needs no portal and can sit right next to its trigger. Placed popups, like popovers, menus and tooltips, are laid out beside their trigger. A dialog's content and a backdrop are laid out in a box the size of the window, where `.m_auto()` centers the content; declare a backdrop before its content, so it paints below it. While content shows, it takes presses, hovers and scrolls from whatever is under it.

Call the content's part function on every frame, even while it returns `None`: that's how the popup hears its props before it opens. `keep_mounted(true)` in the content's props keeps it in the window while closed, hidden, so what's inside keeps its state, and `inline(true)` renders it in place, in its parent's layout, and leaves placing it to you.

### Present as a Sheet

Popover, Dialog, Menu, Select, Combobox, Autocomplete and Date Picker take a `presentation` option. `PresentationSetting::Sheet` slides the popup up from the bottom of the window, where it can be swiped away, and `Auto` does that on small touch screens, deciding each time it opens. While it presents as a sheet, the component's `viewport` part is `Some`: it's the window-sized element the sheet slides in, with the dim behind the sheet. Put the content inside it:

```rust
let content = Popover::content(&self.notes, PopoverContentProps::default(), cx)
    .map(|content| content.p_3().bg(rgb(0xffffff)).rounded_t_lg());
let content = content.map(|content| match Popover::viewport(&self.notes, cx) {
    Some(viewport) => viewport.flex().flex_col().justify_end().child(content).into_any_element(),
    None => content.into_any_element(),
});
```

A Drawer is always a sheet.

### Wait for a Popup to Close

A popup's `open` method returns a `PopupPending`, a future that resolves once the popup closes, with why it closed (`reason` and `source`) and what closed it reported (`return_value`, like a dialog Close part's `value`):

```rust
use ui_sdk::foundation::popups::PopupOpenOptions;

let ask = div().id("ask").on_click(cx.listener(|this, _, window, cx| {
    let closed = this.details.update(cx, |popover, cx| popover.open(PopupOpenOptions::default(), window, cx));
    cx.spawn(async move |_, _| {
        let result = closed.await;
        println!("{}", result.reason.as_str());
    })
    .detach();
}));
```

## Lists and Menus

Select, Combobox, Autocomplete, Listbox, Command, Menu, Tree and Trigger Menu show their options as item parts you render. gpui can't read what an item draws, so each item says what it is in its props.

### Items

An item takes its value and its text, like `SelectItemProps::new("dark").text("Dark")`. Choosing it makes its value the component's; its text is what typing and filtering match, what the trigger shows once it's chosen, and its name ([Accessible Names](#accessible-names) lists how other lists differ). Items keep the order you render them in, which is the order the arrow keys follow, and you style them from `state.highlighted` and `state.selected`. [Add a Select](getting-started.md#add-a-select) renders a list of items this way.

Typing while a list has focus moves the highlight to the next item whose text starts with what you typed. Typing on a select's closed trigger chooses that item.

To group rendered items, put them in a group part with a label part, and name the group in each item's props by the group's key:

```rust
Select::group(&self.font, "sans", cx)
    .child(Select::group_label(&self.font, "sans", cx).aria_label("Sans serif").child("Sans serif"))
    .child(Select::item(&self.font, SelectItemProps::new("inter").text("Inter").group("sans"), cx).child("Inter"))
```

`Select::separator(&self.font, key, cx)` is the separator part to style between groups. `crates/components/select/examples/select.rs` has groups, several values, a search field and a sheet.

### Items as Data

When your options are data, pass them as `items`, a `ListData` of `ListItem`s, `ListGroup`s and separators. The component searches and filters them without your rendering each one, and its content's state hands back the ones to show:

```rust
use ui_sdk::components::combobox::{ListData, ListItem, ListNode};

let countries: ListData = COUNTRIES
    .iter()
    .map(|&(value, label)| ListItem::new(SharedString::from(value), label))
    .collect();
let country: Entity<Combobox<SharedString>> = cx.new(|cx| Combobox::new(ComboboxOptions::default().items(countries), window, cx));
```

In `render`, make an item part for each one the content's state lists:

```rust
let picker = &self.country;
let content = Combobox::content(picker, ComboboxContentProps::default(), cx).map(|content| {
    let items = content.state().items.clone();
    content.child(Combobox::list(picker, cx).children(items.iter().filter_map(|node| match node {
        ListNode::Item(item) => Some(
            Combobox::item(picker, ComboboxItemProps::new(item.value.clone()).label(item.label.clone()), cx)
                .aria_label(item.label.clone())
                .child(item.label.clone()),
        ),
        _ => None,
    })))
});
```

`state().items` holds the items that match what's typed. `ListItem` also takes `keywords` for search and `disabled`, and `ListGroup::new([..]).label(..)` and `ListNode::separator()` group and separate them. Pass the same `ListData` each time rather than building it again: the component recognizes it by identity and keeps what it worked out.

## Forms

The forms crate stands in for the browser's forms. Give a control a `name`, render it inside a form, and the form submits its value under that name:

```rust
use ui_sdk::components::button::{Button, ButtonOptions, ButtonType};
use ui_sdk::components::field::{Field, FieldErrorProps, FieldOptions};
use ui_sdk::components::input::{Input, InputOptions, TextFieldKind};
use ui_sdk::foundation::forms::{Form, FormOptions};

let form = Form::new(
    FormOptions::default()
        .on_submit(|data, _details, _window, _cx| println!("{}", data.to_url_encoded()))
        .on_report(|report, _window, _cx| println!("{}", report.message)),
);
let email = cx.new(|cx| Field::new(FieldOptions::default().name("email"), window, cx));
let email_input = cx.new(|cx| {
    Input::new(
        InputOptions::default().kind(TextFieldKind::Email).required(true),
        window,
        cx,
    )
});
let save = cx.new(|cx| Button::new(ButtonOptions::default().r#type(ButtonType::Submit), window, cx));
```

A `Form` isn't an entity. Keep the handle in a field, and render the controls inside its `root()`:

```rust
let error = Field::error(&self.email, "error", FieldErrorProps::new(), cx)
    .map(|error| error.with(|el, state| el.child(state.message.clone())));
let email = Field::root(&self.email, cx)
    .flex()
    .flex_col()
    .child(Field::label(&self.email, cx).aria_label("Email").child("Email"))
    .child(Input::root(&self.email_input, cx).border_1())
    .children(error);
self.form
    .root()
    .child(email)
    .child(Button::root(&self.save, cx).child(text!("Save")))
```

Its callbacks receive `FormData` and validation reports; `on_report` is where your app displays the message a browser would show in a validation bubble. A control without its own `name` inherits the Field's. A native text field of your own joins through `Field::control`; see the [Field example](../crates/components/field/examples/field.rs).

## Focus and Keyboard

gpui doesn't move through the window's tab stops with Tab on its own. `ui_sdk::init` binds Tab and Shift+Tab to move through tab stops in the order they render, as a browser does, and leaves Tab with Control, Alt or ⌘ alone. Parts that take focus track focus handles of their own, and an element of yours joins the order with gpui's `track_focus(&handle.tab_stop(true))`. To move focus to a component from code, call its `focus(window, cx)` method.

### Keyboard Shortcuts

Register shortcuts of your own with `register_shortcut`, and keep the guard it returns in a field, since dropping it unregisters the shortcut:

```rust
use ui_sdk::foundation::shortcuts::{RegisteredShortcut, ShortcutOptions, register_shortcut};

let search: RegisteredShortcut = register_shortcut(
    "mod+k",
    |_event, _window, _cx| println!("search"),
    ShortcutOptions::default(),
    cx,
);
```

`ShortcutOptions` can scope it to a popup with `layer`, let it run in text fields with `allow_in_input`, or let it repeat while the keys are held.

## Saved State

`ui_sdk::init("my-app", cx)` names where local storage keeps its values: `local-storage.json`, in a directory of that name in the platform's per-user data directory (`~/Library/Application Support/my-app` on a Mac). Until it's called, local storage is unavailable.

`create_local_storage(id)` makes a handle to one value in local storage, which lasts between runs, and `create_session_storage(id)` to one in session storage, which lasts while the app runs. Both reach every window. Read and write the value yourself with `get`, `set` and `remove`, or give the handle to something that saves its own state, like a color scheme:

```rust
use ui_sdk::foundation::preferences::{ColorScheme, ColorSchemeOptions, SystemColorScheme};
use ui_sdk::foundation::storage::create_local_storage;

let scheme = cx.new(|cx| {
    ColorScheme::new(
        ColorSchemeOptions::default().storage(create_local_storage("color-scheme")),
        window,
        cx,
    )
});
cx.observe(&scheme, |_, _, cx| cx.notify()).detach();
```

A `ColorScheme` holds the app's choice (`System`, `Light` or `Dark`), what the system prefers, which is the window's appearance, and what that resolves to. It isn't a component with parts, so the view observes it to draw again when it changes, and reads it in `render`:

```rust
let dark = self.scheme.read(cx).get(cx).resolved == SystemColorScheme::Dark;
```

It reads the saved choice as it's made, so the first frame already has it, and `set` saves a new one. Another color scheme with the same storage, in any window, hears the change with the reason `StorageSync`. A `Preference` does the same for one value from a list of your own, and a Resizable group saves its layout in local storage under its `auto_save_id`.

## Keep Going

- [Components](components.md) lists every component with its crate, its example and the original's page for it.
- [Differences From the Original](differences.md) maps the original's concepts to this port's, and lists what behaves differently and what was left out.
- [Agent Bridge](agent-bridge.md) shows how an AI agent can read and change the components in your windows.
