# Differences From the Original

This port follows `@ui-sdk/core` closely. The components have the original's parts, options, states and reasons, so the [UI SDK docs](https://ui-sdk.dev/docs) describe how each one behaves, and [Components](components.md) links each one's page there. The differences come from the platform: gpui has no DOM, no CSS and no browser, and there's no framework between your code and the components.

Each crate's API docs (`cargo doc --open -p ui-sdk`) list its drops and deviations. This page covers the differences shared across components.

## How the Concepts Map

| Original | Here |
| --- | --- |
| A component, like `<Select>` or a `definePrimitive` definition | A struct in a gpui entity, made once with `cx.new(\|cx\| Select::new(options, window, cx))` and kept in your view. It owns an `Instance`, which names its parts and is what `observe` and the agent bridge read. |
| Props on the root | An options struct with a builder method per option, in snake_case: `SwitchOptions::default().default_checked(true)`. `set_options` hands a component new options, as a re-render with new props does. |
| Parts (`<SelectTrigger>`, `createParts`) | Part functions that take the entity. `Select::trigger(&select, props, cx)` returns a `Part<SelectTriggerState>`: a `div` with the slot's role, focus, keys and ARIA already on it. A part there are many of takes a key, like a tab's value. |
| `data-slot` | `Part::slot()`, like `"select-trigger"`. |
| `data-*` attributes and CSS variables | Fields of the part's state struct, like `state.open`, `state.highlighted` or `state.percent`. |
| `className` and `style` | gpui's styling methods on the part, like `.px_3().rounded_md()`. |
| `className`, `style` or `children` as a function of state | `Part::with(\|el, state\| ..)`, or `part.state()`. |
| The `render` prop | No equivalent. Every part is a `div`; put links, images and other components inside it. A Button's `element` option says whether it acts as a native button, a link or something else. |
| Event props on a part, like `onClick` | gpui's listeners on the part: `.on_click(..)`, `.on_key_down(..)`, `.capture_key_down(..)`. |
| `defaultValue`, `value`, `onValueChange` | `default_value`, `value` and `on_value_change`, and the same for `default_open`, `checked`, `on_checked_change` and the rest. Pass a controlled value back with `set_controlled_value` where the component has one, or with `set_options`. |
| `details` | `ChangeDetails`. `reason` is a `Reason`, like `Reason::EscapeKey`, whose `as_str()` is `"escape-key"`; `source` is a `ChangeSource`; `event` is an `Option<InputEvent>` around gpui's event; and `cancel()` refuses the change. |
| Controllers and the `controller` option | The entity's own methods, like `Select::set_open` or `Switch::is_checked`, through `entity.update(cx, ..)` and `entity.read(cx)`. |
| A Promise an action returns | `ActionOutcome`: `Done`, `ActionOutcome::wait(future)` to stay pending until the future finishes, `ActionOutcome::failed(error)`, or `Canceled`. |
| Portals, `container` and the top layer | Popup content renders itself above the window, in gpui's `deferred(..)`, from wherever you put it in your tree. |
| `ref` and `element.focus()` | Focus handles. Methods like `Switch::focus` move focus to a component, and options that take an element in the original, like `initial_focus`, take a `FocusHandle`. |
| `aria-*` props | gpui's `aria_*` setters, like `aria_label`, and the kernel's `Aria` for the rest: `.aria(Aria::new().required(true))`. |
| An `id` on a part | `Part::with_element_id`. Input, Textarea, Password Input, OTP Input, Date Picker and Switch take an `id` option, which a `Label`'s `for_id` names. A `Label` names any other control by holding it, and a Field's label names the control in its Field. |
| `lang` and `dir` on the page | `set_page_locale` in `ui_sdk::foundation::text` and `set_direction` in `ui_sdk::foundation::platform`, for the whole app. A component with a `locale` option can still take its own. |
| `Intl` | ICU4X and its compiled CLDR data, in the number, date and text crates. |
| `localStorage` and `sessionStorage` | The storage crate. Local storage is `local-storage.json` in the directory `ui_sdk::init` names, in the platform's data directory (`~/Library/Application Support/<name>` on a Mac). Session storage lasts while the app runs. |
| `matchMedia` | `window.appearance()`, or the preferences crate's `ColorScheme`, for the color scheme, and the app-set `cx.reduce_motion()` flag for reduced motion; gpui does not read it from the system. |
| `<input>` and `<textarea>` | `ui_sdk::native::TextField`. Components that take text make their own. |
| `<form>`, `<fieldset>`, `<label>` and `FormData` | `Form`, `Fieldset`, `Label` and `FormData` in `ui_sdk::foundation::forms`. A form hands what it submits to its `on_submit`. |
| `<button type="submit">` | A `Button` with `.r#type(ButtonType::Submit)`. |
| Text an element renders | Props, like an item's `text` or `label` and a tooltip's `text`. gpui can't read text back from an element. |
| The page's screen reader announcer | `live_regions(window)`, rendered once in each window. |
| `<OverlayHost />` | A view of yours that renders `ui_sdk::page::overlays().entries()` by their kind. |
| Tab and Shift+Tab moving focus | `ui_sdk::init`, which binds them. |
| `console.warn`, and what `debug` logs | The `log` crate: development warnings with `log::warn!` in debug builds, and `debug`'s changes with `log::debug!`. Install a logger, like `env_logger`, to see them. |

## What's Not Here

### Server Rendering

A gpui app draws in its own window from the first frame, so nothing renders on a server or hydrates. What existed for that is gone: storage's inline `script` and cookie storage (`createCookieStorage`), `Preferences` and `preferencesScript`, a color scheme's `initial` value and its `"unknown"` system scheme, Tabs' streamed default, Token Input's `renderStaticHtml`, and reading what someone typed before the page's script ran. Storage reads are synchronous, so a saved value is there on the first frame.

### Framework Adapters

There's no React, Vue, Svelte, Solid or web components layer. The components are Rust structs whose parts you render from your own gpui views, so these are gone:

- `createParts`, `usePrimitive`, `usePart` and the `render` prop. Part functions return a `div` you style and fill.
- Controllers as separate objects: `createSelectController` and the rest, the `controller` option, and a trigger's `controller` prop. The entity you hold is the controller, and a trigger part renders anywhere you can reach the entity. Popover, Tooltip, Hover Card, Menu, Menubar and Alert Dialog keep their `controller` option and `create_*_controller`, for a handle made before the component, and `confirm()` uses Alert Dialog's.
- `container` and `PortalProps`. Content renders above the window through `deferred`.
- What adapters render for you: a menu's `items` as parts, a tree's data items, a waterfall's rows and axis ticks, a trigger menu's default list, and `<OverlayHost />`. Render the parts from the same data. A menu's `items` still registers its items' shortcuts.
- The readers for web components' attributes, like `valueAttribute`, `layoutAttribute` and `filterAttribute`.
- TypeScript's inferred value types, like `SelectGeneric`. Values are type parameters or `SharedString`.

### CSS

gpui styles elements with methods and has no stylesheets:

- `web.css`, the original's ready-made look, isn't here, and neither are the chevrons and check marks it draws in triggers and indicators. Put an icon in the part.
- Nothing needs the `ui-sdk` cascade layer or a Content Security Policy nonce.
- CSS anchor positioning. A popup's positioner places it on every frame.
- Scroll snap. A Carousel springs to its pages itself, and a sheet settles on its snap points itself.
- Smooth scrolling. gpui's scrolls land at once, like a press on a Scroll Area's track, which pages without gliding. A Scroll Area eases its own `scroll_to_end` and `scroll_to_start`, and its scroll-to-end button.
- `scroll-padding`. A list that reveals its highlighted item keeps no space clear around it.
- `field-sizing: content`. A Textarea grows with its `auto_resize` option.
- Selectors and marker attributes. An option that took a selector, like an `anchor`, `initialFocus` or a File Input's `paste_target`, takes a focus handle, a function that returns one, or an `Anchor`. A Drawer's content names its scroll areas and what a mouse press doesn't drag it from in `scroll_areas` and `no_drag`, where the original walks up from the press and reads `data-drawer-no-drag`. A Carousel has nothing in place of `data-carousel-no-drag`, though a text field in a slide keeps its presses from starting a drag.

### Browser Features

A browser provides these, and gpui has nothing to build them on:

- Autofill and password managers. Nothing fills a field or offers a saved password, and an OTP Input leaves no room for a password manager's badge.
- Restoring a form after a reload or a trip back through history. No component reports `Reason::Restore` for a form.
- A form's `action` and `method`. A `Form` hands its data to `on_submit`, and a Close part's `value` stands in for `method="dialog"`.
- The validation bubble. A `Form` hands the invalid control and its message to `on_report`, for you to show.
- Find in page and `hidden="until-found"`. `hidden_until_found` on an Accordion, Collapsible, Tabs, Tree or Navigation Menu keeps a closed panel in the page, hidden, as `keep_mounted` does, and `Reason::FindInPage` comes only from a call that passes it.
- Links that go somewhere on their own. A part that's a link carries its address, or says it's a link, and your own handler follows it. Pagination's `get_href` puts each URL in its part's state, list items take `link`, a toast's action has an `href` for you to open, menu items navigate in `on_select`, and tree items in `on_action`. Carousel indicators are always tabs.
- In text fields: spelling and grammar marks, the context menu, dragging text in or out, `inputmode`'s keyboard types, a search field clearing on Escape, and the platform's secure keyboard entry for passwords. A password field still masks its text, and never copies it, cuts it or hands it to the platform's text services.
- The eye dropper. Color Picker has no `eye-dropper` part or `pickFromScreen`, and nothing reports `Reason::EyeDropperPick`.
- Pointer lock. A Number Input's scrub stops where the pointer meets the screen's edge.
- In File Input: `capture`, which opens a phone's camera; a type filter in the system's file dialog, so a file `accept` rules out is rejected after it's picked; and the File and Directory Entries API, so a dropped folder is read from disk by its path.
- An Avatar image's `srcSet`, `sizes`, `crossOrigin`, `referrerPolicy` and lazy loading. gpui's `img` takes one source.
- Native `<select>`s in a Calendar's month and year parts. Put a select of your own inside each one, like a `ui_sdk::components::select::Select`, and call `Calendar::select` with its choice.
- iframes, along with the focus guards around them.
- `<meta name="theme-color">`. A sheet takes your window's chrome colors as a `SheetThemeColor`.
- The `pointer` and `hover` media queries. gpui doesn't report the device's pointer, so a `PointerKind` is what you tell it.
- The `languagechange` event. The system's language is read once.
- Storage events from other tabs. Every window shares the app's storage and hears its writes at once.
- More than one finger at a time. gpui follows one touch and doesn't recognize a touch pinch yet, so two fingers can't zoom a canvas or an Image Cropper. A trackpad's pinch zooms both.
- WebMCP's `registerTools`. Route a model's tool calls to `AgentBridge::call`, with `agent_tools` describing the tools.

### The Testing Kit

`@ui-sdk/core/testing` isn't ported. `checkPattern`, `checkDirection`, `mount` and `settle` drive a DOM, and `checkServer` checks server rendering, which nothing here does. The kit's `setPlatform` is `ui_sdk::foundation::platform::set_platform`.

### Developer Tools

The devtools hook isn't here, and neither is the console's `__UI_SDK__` (`changes()`, `layers()`, `shortcuts()`, `warnings()`) or strict mode, which turns warnings into errors. Name checks read `aria_label` and labelled-by associations, not drawn text or `text!` children. They cover meters, progress bars, timelines, carousels and several controls; each crate documents checks it omits.

## Behavior That Differs

### Text and Names

gpui can't read back the text an element draws, so what the original reads from the page is a prop here:

- List items say their text: `text` on Select and Listbox items, and `label` on the other lists' items. It's what typeahead and filtering match, and what a select's trigger shows once the item is chosen. Select, Listbox, Command, Tree and Trigger Menu items take it as their name for screen readers too. Combobox and Autocomplete items need an `aria_label` for that, and Menu items are named by their `text!(..)` children; a menu item without a `label` warns in development builds.
- A Tooltip's `text` option names or describes its trigger, and a Field error's `text` is what the field announces.
- A plain string child is drawn but never read by a screen reader. `text!(..)` makes text it reads, and gives a button, checkbox, radio, link or menu item its name. Other parts are named with `aria_label`, and a part that names or describes another, like a popup's title or description, carries its text as `aria_label`, since that's all it passes on.
- The agent bridge reads accessible names, not drawn text. Give a title, button or field an agent should read an `aria_label`.

### Values

- Values are Rust types. Components that take strings use `SharedString`: Accordion, Checkbox Group, Radio Group, Segmented Control, Tabs, Toggle Group, Navigation Menu, a menu's radio items and Trigger Menu. Select, Combobox and Listbox take their value type as a type parameter, any type with a key, like `Combobox<SharedString>`. Select and Listbox default to `SharedString`. A Slider is `Slider<f64>` for one thumb or `Slider<Vec<f64>>` for several.
- `null` is `None`. Where a controlled value can be `null`, the option is `Option<Option<T>>`: `None` leaves the component uncontrolled, and `Some(None)` controls it to nothing. Radio Group, Tabs, Number Input and Drawer's snap point are examples. Select, Combobox, Listbox and Tree express an empty single selection as `Single(None)`.
- Values the original types as a union are enums: `SelectValue` and `ListboxValue` (`Single` or `Multiple`), `CalendarValue` (`Empty`, `Single`, `Multiple` or `Range`), `TreeValue`, and `ResizableSize` (`Percent`, `Px` or `Rem`). An index of -1, like `on_highlight`'s, is `None`.
- Dates are ISO strings in an `ISODate`, which is a `SharedString`. Where the original's date functions throw a `RangeError`, the date crate's panic: on an ISO date that isn't one, a malformed locale tag, or a time zone that doesn't exist.
- Text offsets count UTF-8 bytes, and lengths like `max_length` count characters, where the original counts UTF-16 code units.
- Payloads are a type parameter, like `Popover<P>`, `()` by default. Return values, like what closed a dialog, are `SharedString`.

### Callbacks

- A callback runs inside its component's update, with `&mut Window` and `&mut App`. It can update any other entity, but reading or updating its own component panics, since gpui can't hand out an entity that's in the middle of an update. A Toggle Group's or Checkbox Group's `on_value_change` can't touch the toggle or checkbox that was pressed either. Update your own state in the callback, or defer a change to the component with `window.defer(cx, ..)`, which runs once the update is over.
- `details.event` is `None` for a method call, and for a change another change set off. `details.source` is never `ChangeSource::Pen`: gpui reports a pen as a mouse or a finger.
- Where the original adds to `details`, the port's details type derefs to `ChangeDetails` and carries the rest, like a Slider's `thumb`, a Color Picker's `color` and a Command's `index`. Or the callback takes it as an argument of its own, like an Autocomplete's chosen suggestion, and the return value `on_open_change_with_value` hears.
- Methods take `ChangeOptions` where the original takes `{ reason, event }`. `ChangeOptions::default()` reports `Imperative`.

### Timing and Event Order

gpui changes focus immediately, but delivers focus and blur events after drawing a frame. Containment checks also read the drawn frame. Work the original does a microtask after its elements bind waits for the frame here:

- Work that needs parts on the page, like a list settling its highlight, a group moving its tab stop, a form finding its controls or a Field finding its control, runs once the frame that rendered them is drawn. A Field's ARIA reaches its control a frame after the field reads it, and a parent checkbox without `all_values` shows its state from the frame after its group's checkboxes draw.
- Focus landing on a part, or leaving it, is read once the frame is drawn, so a blur arrives a frame late. A Date Picker commits a typed date a frame after focus leaves its input.
- A controlled value you pass back arrives with your next render, where the original's arrives a microtask later. An OTP Input waits a frame for a typed code to come back.
- Some changes wait for the update that caused them to end: a popup that a controlled `open` opens mounts after it, a toast added inside another component's update shows after it, and a nested dialog closes after the update that closes the dialog around it.
- Some `set_options` and `set_controlled_*` methods take the window, like Collapsible's `set_controlled_open`, since a change they bring, like a panel's exit or a field's new text, starts at once.
- Handlers added to a part run after its own handlers. A focused item's key handler runs before its container's, so `window.prevent_default()` there can stop container behavior. Use `.capture_key_down(..)` on an ancestor to intercept a key before the part. Screen-reader and agent presses run only the part's own press, not your `.on_click`; put application actions in the component's action callback.
- Shortcuts hear a key after the focused element and gpui's key bindings. A key something prevented or stopped, or one an action took, runs no shortcut.

### Keyboard, Focus and Pointer

- `ui_sdk::init` enables window-wide Tab and Shift+Tab navigation. Components that handle Tab themselves, such as modal focus traps, work without it.
- Buttons and triggers activate on Enter and Space as the key comes up, which is gpui's keyboard click, and not while a modifier is held. A browser's button acts on Enter as it goes down. Submenu triggers also open on key down, and Shift+Enter or Shift+Space picks a calendar day on key down.
- Focus never scrolls anything into view. A list reveals its highlighted item by scrolling the part that holds the items, like a Listbox's root or a Select's content, so give that part the height limit and `overflow_y_scroll()`. A Tree reveals an item in the `scroller` handle you pass it, and a Waterfall pages by its `scroller`'s height.
- gpui allows one `on_hover` per element, and parts that react to the pointer resting on them already have theirs: the trigger and content of a menu, tooltip, popover or hover card, a navigation menu's trigger and viewport (and its content without a viewport), a select's trigger, a rating's root and a calendar's days. A second `on_hover` on one of them panics in debug builds and replaces the part's own in release builds. Style hover with `.hover(..)`, or from the part's state.

### Popups and Hidden Content

- Popup content renders above the window from wherever you put it in your tree, so it can sit next to its trigger. Declare a backdrop before its content, so it paints underneath.
- Content takes every press, hover and scroll over it, as an element on top does in a browser. Content shown in place, with `inline`, covers the elements it's laid out in too, so they stop hearing the pointer over it.
- gpui can't make part of a window inert. A modal popup paints a blocker over the rest of the window that takes every press, hover and scroll, and traps Tab. gpui can't hide the rest of the window from assistive technology either, so a modal dialog says `aria-modal` instead.
- While a popup animates out, its content and backdrop still take presses, where the original's let them through.
- Parts the original makes inert say so in their state instead, for you to leave their controls out of the tab order: a Carousel's slides out of view, a Resizable panel collapsed to nothing, and a row's hidden swipe actions.
- `toast(cx)` and `confirm()` show nothing on their own. Render a `ToastRegion` for the toasts, and attach an Alert Dialog of yours to `confirm()` through `Confirm::controller`. Without those, both open into the overlay registry, which a view of yours renders from `overlays().entries()`, as each framework's `<OverlayHost />` does in the original.

### Animation

- The original waits for the CSS transitions and animations it finds on a part. Here a part animates with the timing you give it, which stands for the transition its CSS would declare: `timing` on a popup's content props, `panel_timing` on Accordion, Collapsible and Tabs, `indicator_timing` on Checkbox and Radio Group, `group_timing` on Tree, and `timing` on Toast Region and Navigation Menu viewport. Without timing, timed parts show and hide at once; drawers and other sheets still use their own spring.
- Style the animation from the part's `presence` state. `starting` and `ending` are the original's `data-starting-style` and `data-ending-style`, and `progress` runs from 0 at the starting or ending look to 1 when it's open. Accordion, Collapsible and Tree group states also carry `state.panel.height`; Tabs panels do not.
- Animations run on gpui's animation frames. Reduced motion is gpui's app-set `cx.reduce_motion()` flag; set it with `cx.set_reduce_motion(..)`. Timed parts jump to their end. Sheets instead settle in 200 ms or fade in 150 ms.

### Forms

- Validation messages are Chromium's, in English, whatever the app's language. `set_value_missing_message` replaces the missing-value ones, and a Field error's `text` shows your own.
- Controls inside a hidden panel, like a closed Accordion item or Collapsible kept in the page, aren't drawn, so they leave their form until the panel shows. A browser still submits a hidden input.
- An Input of kind `Number` holding invalid numeric text submits that text, where a browser submits an empty value. Number Input submits a number or an empty string.
- Every control reports to its form itself, without hidden inputs. A checkbox in a Checkbox Group submits only under a `name` of its own, where React's hidden inputs use the group's.

### Language and Storage

- The page's language is the app's: what `set_page_locale` set, else the system's. One section can't have a `lang` of its own, but a component's `locale` option can.
- ICU4X formats as browsers do, with a few gaps in its data. Units it has no names for, like celsius and the digital units, read as CLDR's English in every locale, a compound unit like `kilometer-per-hour` can read "km/hr" where a browser has "km/h", and there's no `usage: "search"` collation. Number options a browser throws on fall back to the closest valid ones, with a warning in debug builds.
- Local storage keeps nothing until `ui_sdk::init` names its file. Writes before that are dropped; the first logs a warning in debug builds.
- Local storage is written to its file in the background after each change, so a write that fails is logged rather than returned. Another running copy of the app doesn't hear your writes, and the last one to write wins.
- A preference reads its saved value as it's created, so it reports no `Restore` change when something first subscribes.

### The Agent Bridge

- `act` and `query` return gpui `Task`s and queue in call order. Dropping one stops its call. `snapshot` reads immediately across windows; its window argument is the calling window. `call` dispatches to these APIs, so a snapshot tool call does not wait in the action queue.
- `allow` answers with an `Allowed`, now or later, and runs in the component's window, so it can show your own confirm. `AbortSignal` and `AbortController` stand in for the web's.
- [Agent Bridge](agent-bridge.md) covers the Rust integration.

## Known Gaps

- A drag session started by a synthesized tap's mouse press can report mixed touch and mouse sources, such as on a Carousel viewport. Slider claims the touch drag directly and reports `Touch` for both change and commit.
- gpui can't do some things CSS does, which shows in how you style the components:
  - A `div` can't scale, rotate or move after layout. There's no press-scale on a button, and a popup can't scale in from its `transform_origin`; animate its opacity, size or position from `presence.progress` instead. Parts the original moves with `translate` place themselves with offsets and margins: give a slider thumb a size along its track, and center a color picker thumb with negative margins half its size. Only an `svg` takes a transformation, like a chevron that turns as its select opens.
  - There's no z-index. Elements paint in the order they render, and only `deferred` content goes above the rest. Overlapping slider thumbs paint in render order, though a press still lands on the one the original raises, and a focus ring drawn past its element's edge sits under the elements that render after it.
  - There's no `::before` or `::after`. Decorations, check marks and larger hit areas are child elements you render.
  - There's no letter-spacing. Text keeps its font's own spacing.
  - Clipping is rectangular, so `overflow_hidden()` on a rounded part doesn't round what's inside. Round an Avatar's image part itself.
  - Gradients take two stops. A Color Picker's channel and area backgrounds come as layered gradient elements, `ChannelGradient::element` and `AreaGradient::element`, to put first in their part.
  - gpui draws no scrollbars. A scrolling element shows one only through a Scroll Area's scrollbar parts.
  - Layout and text run left to right. `set_direction(Direction::Rtl, cx)` mirrors what the components decide, like arrow keys, the side a popup opens on and a text field's `Start` and `End` alignment, but it doesn't reverse a row of your own (use `flex_row_reverse()`), and text in a field doesn't run right to left.
- It's been built and run on macOS only. gpui supports Linux and Windows, but this port hasn't been built there. gpui sends touch events and long presses on iOS and the web, and the back gesture only on iOS, so the touch paths, like dragging a drawer with a finger or long-pressing for a context menu, haven't run.
- There's no automated test suite, and the original's tests weren't ported. Every component example has been run in the gallery.
