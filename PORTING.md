# Porting `@ui-sdk/core` to gpui

The original is Vercel's UI SDK: headless behaviors (keyboard, focus,
selection, dismissal, popups, drag and drop, and the accessibility they need)
and the components built from them. This workspace ports it to Rust for gpui,
headless like the original: no theme, no default styles. This guide is for
porting more of it, or porting upstream changes: how the original's DOM code
maps to gpui, and the conventions every crate follows.
[CONTRIBUTING.md](CONTRIBUTING.md) covers building and checking a change.

- Original source: `ref/ui-sdk/packages/core/src` (TypeScript), checked out as
  CONTRIBUTING.md describes, at the commit this port tracks (`855bc7f`).
- Original docs: `ref/ui-sdk/apps/docs/content/docs` (`core/` explains the
  kernel and behaviors; `react/components/*` shows each component in use).
- gpui: zed rev `2f808b3`, which Cargo checks out under
  `~/.cargo/git/checkouts/zed-*/2f808b3/crates/gpui`. Read
  `src/_accessibility.rs`, `src/_ownership_and_data_flow.rs`, `src/elements/div.rs`
  and `examples/` (`input.rs`, `tab_stop.rs`, `popover.rs`, `a11y.rs`,
  `drag_drop.rs`, `scrollable.rs`, `uniform_list.rs`) when you need an API.
- Reference port: `crates/kernel` and `crates/components/toggle`. Read both
  before writing a component.

## Rules

1. **No tests.** The port has no test suite and doesn't port the original's:
   no `#[test]`, `#[gpui::test]`, `#[cfg(test)]`, `tests/` directories,
   `test-support` features or test-only hooks like `debug_selector`. Skip the
   original's `*.test.ts`, `testing/` and `testing.ts`.
2. **Dependencies.** Add a third-party dependency in the crate's own
   `Cargo.toml` with an explicit version. A crate depends on what its original
   module imports, never in a cycle: `drag-and-drop` must not depend on
   `selection` (the original's `isSameKeys` import goes the other way here),
   and nothing below a component depends on a component. Don't change the
   gpui rev in a port; that's a change of its own.
3. **Verify by building and running**: `cargo check -p ui-sdk-<name> --examples`
   with no warnings, then run the example or the gallery section and try it
   with the keyboard. Debug builds report what the kernel can check, so run
   one with a logger (the gallery has one).
4. **Port behavior, faithfully.** Every option, state key, reason, keyboard
   interaction, focus move, timing, edge case and ARIA role/state the original
   has. Read each TypeScript file in full before writing its Rust. When gpui
   lacks a DOM feature, find the gpui way to the same behavior (see the mapping
   below). Drop only what has no meaning here (server rendering, hydration,
   framework adapters, web components, CSS-only concerns, `hidden="until-found"`)
   and list each drop, with why, in the crate's `lib.rs` doc.
5. **Mirror the tree.** One Rust module per TypeScript file, in snake_case
   (`roving-focus.ts` → `roving_focus.rs`), definitions in the original's
   order where practical. `index.ts` becomes the `lib.rs` re-exports.
6. **Write like the original.** Port its comments and doc comments, adapted to
   gpui, in its register: short plain sentences that say what a thing is or
   why. Names are the snake_case of the original's; types keep their names.
7. **UI copy** in examples: labels, data, and placeholders of a word or two.
   No captions, hints or empty states that narrate what something does.

## Workspace

| Directory | Crate | Ports |
| --- | --- | --- |
| `crates/kernel` | `ui-sdk-kernel` | `foundation/kernel` |
| `crates/foundation/<m>` | `ui-sdk-<m>` | `foundation/<m>` |
| `crates/components/<c>` | `ui-sdk-<c>` | `components/<c>`; `menu` and `menubar` share `ui-sdk-menu` |
| `crates/page` | `ui-sdk-page` | `page` |
| `crates/native` | `ui-sdk-native` | what the browser provides that the original builds on, like `<input>` |

Each crate's `Cargo.toml` depends on what the original imports
(`ui-sdk-<x>.workspace = true`).

## Crate Notes

Read a crate's `lib.rs` doc before using it from another crate; it lists
its deviations from the original. What other crates need to know:

- `ui-sdk-kernel`: see Kernel Conventions below.
- `ui-sdk-storage`, `ui-sdk-preferences`: an app calls
  `ui_sdk_storage::init("app-name", cx)` once; `create_local_storage(key)` and
  `Kind::Local.get_item(key, cx)` stand in for `localStorage`.
- `ui-sdk-motion`, `ui-sdk-disclosure`: `Presence` exposes its phases
  (`PresenceState`) for parts to style; while `inert` is set a part skips its
  own handlers and focus, since gpui can't make a subtree inert. `Panel`: call
  `panel.sync(..)` after every change to state or options, and put
  `panel.track(el)` on the panel part. A sheet calls `track_animation` on its
  popup's presence before it exits. `Presence::stop` is the original's cleanup: a
  part animating out eases back to open, and a stop and an enter in one
  update count as one change. Reduced motion is gpui's
  `cx.reduce_motion()`.
- `ui-sdk-platform`: `Region` stands in for a DOM element (bounds, hitbox,
  focus handle; `Region::measure(el)`), and containment asks about a `Target`
  (a press point or a focus handle). Layers are one stack per window
  (`Layer::new`, `push_layer`, `escape_layer`, `layer_contains`, ..; guards pop
  on drop); each layer's content paints `listener()` every frame, and its
  owner calls `set_active(false)` on close and `set_exiting` while animating
  out. Positioning: `position(anchor, options)` gives a `Positioner`; render
  `positioned(&positioner, content)` inside `deferred(..)` at `LAYER_PRIORITY`
  (not `anchored`, which would undo flip and shift), make content `occlude()`,
  and `stop()` it on hide. `announce(&str, window, cx)` speaks through live
  regions an app renders once per window with `live_regions(window)`.
  Shortcuts: `matches_shortcut`, `format_shortcut`, `aria_keyshortcuts`,
  `is_apple(cx)`. `prefers_reduced_motion` reads `cx.reduce_motion()`.
- `ui-sdk-text`: the app-wide locale (`set_page_locale`, `page_locale`,
  `locale_of`); matching (`fold_text`, `match_ranges`, `match_segments`,
  `fuzzy_filter`); `Typeahead::search(&key, labels, current)`; accessible names
  from a `NameSources`; the `TextField` trait the native text field implements,
  and the editor helpers on it. Text ranges are UTF-8 byte offsets.
- `ui-sdk-number`: `format_number` and `NumberFormatOptions` (every
  `Intl.NumberFormat` option, through ICU4X), `parse_number`, and the steps:
  `step_from_key`, `snap_to_step`, `step_value`, `clamp`, `round_to`, `wrap`.
- `ui-sdk-gauge`: `gauge_settings`, `format_gauge_value`, `gauge_aria(..).apply(part)`,
  `indicator_style(p).apply(el, rtl)`.
- `ui-sdk-focus`: `RovingFocus::new(options, window, cx)`: the container calls
  `begin(cx)` before its items render, each item calls
  `bind(key, &handle, RovingMember { disabled, focusable, preferred, claims }, cx)`
  (returns whether it holds the tab stop), and key handlers call
  `keydown(event, &key, window, cx)` → `Option<RovingMoved>`. Groups such as a
  toggle group or toolbar implement `RovingScope`. Also `GridFocus`,
  `FocusRecovery`, `keep_focus`, `press_keeps_focus`, `nearest_in_direction`.
  Tab order: `tabbables`, `next_tabbable`, `tab_within(root, backward, wrap)`
  for focus traps; they step focus through the window, so call them from
  event handlers, never from render. Apps call `bind_tab_keys(cx)` once; a
  handler that takes Tab calls `cx.stop_propagation()` or
  `window.prevent_default()`. `Orientation` is gpui's. `check_with(preferred, cx)`
  moves the roving stop at once from what the component says is preferred now.
- `ui-sdk-forms`: controls find their form, fieldsets and labels through the
  element tree, a frame late. A control makes `ElementInternals::new(cx)`,
  `associate`s it (or `on_reset`/`on_disabled`), puts `track` on its element
  and `.aria(internals.aria())` on its part, and calls its binder
  (`bind_internals`, `bind_hidden_input`, `bind_value_input`) on each render and
  after each change, as the original's `sync`. `Form`, `Fieldset`, `Legend`,
  `Label`, `FormData` stand in for the HTML elements; disabled state:
  `is_disabled`, `watch_form_disabled`; validity: `value_missing_message`,
  `set_component_validity`, custom validity; labels: `labels_of`,
  `name_from_labels`, `activate_control`, `set_control_attrs`. Checkbox and
  switch build on `ToggleControl` (and its `ToggleGroup` trait); text fields
  join through `TextFieldApi` over a `TextControl`. Buttons register
  `set_button(Some(ButtonType::Submit))` and call `activate_button` after a
  press nothing prevented. A form's validation bubble goes to its
  `on_report`.
  A control's `is_disabled` also reads the `disabled` its field writes on it
  (`internals.control_attrs().disabled`), as the original's adapters pass a
  field's `disabled` to its control.
- `ui-sdk-native`: `TextField::new(options, window, cx)` and
  `TextField::root(&entity, cx)` stand in for `<input>` and `<textarea>`
  (kinds Text, Search, Email, Tel, Url, Number, Password; multi-line with
  rows and auto-resize). It has the `HTMLInputElement` surface (`value`,
  `set_value`, `selection_start`/`end`, `set_selection_range`, `select`,
  `focus`, `blur`), `replace_range` (one undoable program edit), `caret_bounds`
  for anchoring menus, and `set_expanded` for comboboxes. Events are hooks
  (`on_before_input` with `prevent_default`, `on_input` with an `Edit`,
  `on_change`, `on_selection_change`, `on_focus`, `on_blur`, composition,
  `on_submit`) that run inside the field's update, also sent as
  `TextFieldEvent` to `cx.subscribe`. The field edits from a keystroke
  observer after every listener, so a component's `on_key_down` stops an edit
  with `window.prevent_default()` or `cx.stop_propagation()`. `TextFieldHandle`
  implements `ui_sdk_text::TextField`. Components declare their own role
  (combobox, spinbutton) over the field's.
  Its style takes `text_align(TextFieldAlign::{Start, Center, End})` and
  `width_ch(f32)`; font features (like `tnum`) come from the part's text
  style. `on_before_input`'s `BeforeEdit.data` is the text before
  `max_length` cuts it, and `inserted` is what actually goes in.
- `ui-sdk-color`: `Color` (converts to gpui's `Rgba`, `Hsla`, `Fill`),
  `parse_color`, `convert_color`, `format_color`, gamut, contrast, mixing.
- `ui-sdk-date`: `ISODate` and its arithmetic, `month_grid`,
  `first_day_of_week`, `date_formatter`/`format_date` (browser-matching
  `Intl.DateTimeFormat` through ICU4X), `month_names`, `weekday_names`,
  `today_date`, `date_pattern`, `parse_date`, `date_time_field_name`.
- `ui-sdk-image`: `ImageCrop`, `crop_image` (cuts an SVG at its own size),
  `load_image`, `use_image(source, window, cx)` (a per-frame
  loading/loaded/failed status), `use_natural_size` (an SVG's own size),
  `size_of` (decoded pixels).
- `ui-sdk-drag-and-drop`, `ui-sdk-selection`, `ui-sdk-canvas`: behaviors are
  cheap-to-clone handles a component keeps in a field; calls that change
  state take the component after cloning the handle
  (`self.reorder.clone().keydown(self, event, &key, window, cx)`), and changes
  reach the component through a `set` option carrying `ChangeOptions` for
  `Controllable::change`. Pointer sessions (`create_drag_session`, `bind_drag`,
  `DRAG_THRESHOLD`, `is_drag_press`, `repeat_while_held`): the component
  paints `session.listener()` every frame, so it hears moves and the release
  anywhere; `on_pointer_down(el, handler)` hears mouse presses and touch
  drags. `swallow_next_click(until_release, window, cx)` must be called in the
  release's capture phase (a session's `on_end`, `capture_any_mouse_up`), or
  on the press with `until_release`, with a session listener or
  `swallow_listener()` painted. Escape cancels a drag. Also `create_reorder`,
  `create_transfer`, `create_tree_reorder`, `create_selection`,
  `create_key_selection`, `create_lasso`, `create_cell_range`,
  `bind_indicator`, `direction_between`, `create_viewport`, `create_connect`.
  `DragSession::for_entity(weak)` makes a session without a `Context`;
  `on_pointer_down` keeps an element's `absolute()`, so it needs no wrapper.
- `ui-sdk-lists` (list components): `warn_if_unnamed` needs the part's
  `keep_name()` from its first render; use `icon_part(instance, slot, ..)`
  for any icon slot and call `check_icon_item` from the item;
  `set_settled_hook` allows one hook per engine; tree-shaped data implements
  `TreeData`; set `IndicatorOptions::scroll` whenever the list scrolls. Link
  items are clicked from code with `press_of(..).click(..)`, which runs only
  the part's `on_press` handlers, so apps follow links in `on_press`.
- `ui-sdk-popups`: a component's entity implements `PopupHost` (`instance()`,
  `popup()`), builds a `Popup<T>` with `create_popup(instance, CreatePopupOptions { .. }, window, cx)`
  and calls `Popup::set_options(PopupOptions)` each render; `open` returns a
  `PopupPending` future, `close`/`toggle` a bool. Parts: `popup_trigger`
  (`popup_trigger_with(TriggerHandlers)`), `popup_content`/`bind_popup_content`,
  `popup_viewport`, `popup_title`, `popup_description`, `popup_close`,
  `popup_arrow`, `popup_backdrop`, `window_box`. Presets: `dialog_preset`,
  `alert_dialog_preset`, `drawer_preset`, `popover_preset`, `menu_preset`,
  `select_preset`, `combobox_preset`, `date_picker_preset`,
  `trigger_menu_preset`, `tooltip_preset`, `hover_card_preset`. Lower level
  for menus, toasts and navigation: `PopupLayer`, `create_stepped_popup`,
  `create_hover_intent`, `create_safe_area`, `bridge_between`,
  `create_dismissal`, `block_outside`, `register_popup_element`/`owns_event`.
  Sheets: `create_presentation`, `SheetConfig`, `present_sheet`, `attach_sheet`
  (`Sheet::set_scroll_areas`, `set_no_drag`, `snap_to`); a menu in a sheet
  renders through `drilled_into`. Modality blocks the page with hitboxes;
  `measureScrollerEarly` and `prepareSheet` are gone.
  Rules: `bind(&T)` reads the host as the content binds,
  `contains(target, event)` gets the press being decided (`None` for focus),
  `on_bind`/`on_unbind` hear content joining and leaving the page; `Popup::set_trigger_key`; `PopupLayer::listener()` for content a
  component draws itself; `TriggerHandlers.expanded` leaves out
  `aria-expanded`; `popup_backdrop_with(this, state, ..)` for a backdrop with
  the component's state. Popup content blocks presses, hovers and scrolls in
  every presentation. A sheet's `on_layout` runs as it measures; the
  scroll-snap host takes the pointer over the window unless the sheet rests
  where the window stays usable.
- `ui-sdk-toolbar`: a toggle joins a toolbar by taking `Toolbar::scope` (or
  `scope_in(group)`), calling `scope.bind(key, &handle, RovingMember::new().disabled(own), cx)`
  each render and using the result as `FocusMode::Roving { tab_stop }`,
  treating itself as disabled (but focusable, `aria-disabled`) while
  `scope.disabled(cx)`, sending key presses to `scope.keydown` and reading
  `scope.orientation(cx)`. A toggle group inside a toolbar binds its toggles
  into the toolbar's scope instead of its own roving focus.

## The Component Model

A `definePrimitive` definition becomes a struct held in an `Entity`:

- **State** is its fields; **api** is its methods; it implements `Focusable`
  when it takes focus. It owns one `Instance::new("<name>")` (ids, slot names,
  debug logging, and the registry `observe` and the agent bridge read).
- **Constructor**: `pub fn new(options: XOptions, window: &mut Window, cx: &mut Context<Self>) -> Self`.
  Apps create it with `cx.new(|cx| X::new(options, window, cx))`.
- **Options** are an `XOptions` struct: public fields, `Default`, and a builder
  method per field named as the field (`.pressed(true)`,
  `.on_pressed_change(|value, details, window, cx| ..)`). Callbacks are
  `Option<Rc<dyn Fn(..)>>`. `set_options(options, cx)` hands it new options the
  way a framework re-renders with new props (the original's `sync`) and
  notifies only when what renders changed.
- **The entity is its controller.** `createXController` and the `controller`
  option exist because the DOM instance is out of the app's reach; here the
  app holds the entity and calls its methods. Keep any controller behavior
  that is more than a method call.
- **Controlled state** is a `Controllable<T>` per changeable key
  (`kernel/change.rs`): `value`, `defaultValue` and `onValueChange` become
  `Controllable::new(key, default)`, `set_controlled`, `set_on_change`. Every
  change an interaction or api call makes goes through
  `Controllable::change(&instance, value, ChangeOptions, window, cx)`, which
  resolves the source, runs the callback, honors `cancel()`, and tells the
  watchers, as `createChange` does. Reasons come from the shared `Reason`
  enum; each `on_*_change` option documents its component's subset. Writes no
  one can refuse go through `set_held` and `change::log`.
  Commits go through `Controllable::commit`, or `commit_value` for a value
  the app hasn't passed back yet.
- **Callbacks run once the component's update is over.** They get
  `&mut Window` and `&mut App` and may update any entity, the component too.
  `Controllable` runs `on_change` and `on_commit` that way; a callback the
  component calls itself goes through `after_update(window, cx, ..)`. A
  change that can be refused gets an `on_before_*` callback, which runs inside
  the update, before the change, and can `cancel()`: `set_on_before_change`
  on a `Controllable`, `dispatch_then` for an event callback. Handlers whose
  return value the component reads, like actions, run inside the update.
- **Slots are part methods** returning a `Part<State>`:
  `pub fn root(this: &Entity<Self>, cx: &App) -> Part<ToggleState>`, and for a
  slot there are many of, the key too:
  `pub fn trigger(this: &Entity<Self>, value: impl Into<SharedString>, cx: &App) -> Part<TabsTriggerState>`.
  A `Part` is a `Stateful<Div>` that already has the slot's role, ARIA, focus
  behavior and handlers, with the slot's state beside it. The app styles it and
  adds children like any div; `Part::with(|el, state| ..)` styles it from
  state, which replaces the original's CSS on `data-*` attributes and CSS
  variables. A slot's state struct, `<Component><Slot>State`, holds what its
  `state` exposes, derives `Clone, Debug, PartialEq`.
- **Handlers** on parts hold a `WeakEntity` and call
  `entity.update(cx, |this, cx| ..)`.
- **Many parts of one slot** (items, options, tabs) register in a
  `Parts<K>` (`kernel/elements.rs`): render order is page order (`begin`
  before rendering them, `register` each), and it keeps each one's focus
  handle and, through `measure`, its bounds.
- **Generic values**: where the original is generic (`PrimitiveGeneric`,
  `Select<T>`), use a type parameter `T: Clone + PartialEq + Debug + 'static`;
  where it takes strings, `SharedString`.

## Kernel Conventions

**Roles, focus, press, keys** (`kernel/roles.rs`, `kernel/part.rs`):
- Declare a part's role with `.aria_role(roles::Role::X)`, never gpui's `.role()`.
- A role that takes focus calls `.focus_mode(FocusMode::.., Some(&handle), disabled_unfocusable)`;
  items a focused container points at use `FocusMode::ActiveDescendant`.
- A role that activates uses `.on_press(handler)`: click, tap, Enter or Space,
  and a screen reader's click.
- A role that moves with keys uses `.on_keys(&["ArrowDown", ..], handler)`,
  which skips keys an input method is composing.
- When another element meets one of these needs, `.unneeded(Need::Keys, "why")`.
  Debug builds warn about unmet needs, ARIA the role doesn't take, and missing
  required states.

**ARIA**:
- gpui's `aria_*` setters for label, description, selected, expanded, toggled,
  value, placeholder, orientation, level, set position and row/column.
- Everything else through `.aria(Aria::new()..)`: `disabled`, `hidden`,
  `modal`, `invalid`, `required`, `read_only`, `busy`, `current`, `has_popup`,
  `multiselectable`, `checked(Checked)`, `pressed`, `value_text`,
  `live`/`atomic`, `role_description`, `autocomplete`, `sort`, spans.
- Relations name other parts by `instance.element_id(slot, key)`:
  `labelled_by`, `described_by`, `controls` (only while the target renders),
  `owns`, `details`, `flow_to`, `error_message`, `active_descendant` (on the
  container that keeps focus), and `fallback_label` for when no labelled-by
  target is on the page. The kernel writes them after the part's subtree is
  drawn, and only to targets already in the tree or laid out this frame. A
  custom element finds a target with `part::node_id_of` in prepaint and
  writes its relations in `a11y_synthetic_children`.
- Elements that aren't parts get `part::with_aria(el, aria)`.
- `data-*` attributes become fields of the slot's state struct; checked states
  use `Checked`.
- Whether a part is on the page: `flag_while_bound`, `key_while_bound`,
  `part::held`.
- A part that carries ARIA needs a role: gpui makes no accessibility node for
  a div without one, so its ARIA, `hidden` included, goes nowhere.
  Never give one gpui's `GenericContainer`: gpui asserts against it in debug
  builds and the app panics (the kernel's `with_role` ignores it). A
  decorative part the original hides with `aria-hidden` takes `Group` with
  `Aria::new().hidden(true)`.
- Name parts for screen readers and the agent with `aria_label`: text children
  don't name a div in gpui. Visible text a screen reader should read goes in
  gpui's `text!(..)` (or `Text::new(id, ..)`), which makes a label node; a
  plain string child makes none.
- How accesskit names things: a node's own `aria_label` wins outright; else
  each `labelled_by` target gives its *value* if its role is `Label` (the
  kernel copies a `Label` part's `aria_label` into its value), or its own
  label otherwise; with neither, buttons, checkboxes, radios, links and menu
  items take the text of their `text!` descendants. The kernel describes a
  part from its `described_by` targets automatically (each target's name, as
  accesskit reads a labelled-by target), so description parts carry their
  text as `aria_label`; gpui can't read their `text!` children.

**The agent surface**: every component implements `Agent` and makes its
instance with `Instance::new(name).with_agent(cx)` (see the toggle).
`with_agent` also makes the component's `cx.notify()` redraw the views its
parts draw in, cached ones included. An entity that has parts but isn't a
component uses `Instance::with_entity(cx)`; a custom element that draws an
entity's state outside its parts calls `instance.record_view(window)`.
- `ACTIONS` uses the original's names: `AgentAction::new("setValue", |this, args, window, cx| ..)`
  calls the api method with `ChangeOptions::default()` and returns its result
  with `.into()`. Read arguments with `args.boolean`, `number`, `string`, `list`.
- Add as `AgentAction::unlisted` the api's synonyms (`toggle`, `open`,
  `close`, `show`, `hide`, `select`), and any `setOpen`, `open` or `setValue`
  the original doesn't list, since the bridge calls those itself. With no
  `agent.actions` in the original, list its change-named api methods.
- `CONTROLLED` and `COMMITS` copy the definition's `controlled` and `commits`;
  `SURFACE` copies `closed`, `reveals` and `secret`.
- `agent_state` uses the original's key names (`readOnly`, ..): the controlled
  keys, plus `disabled`, `pending`, `error`, `min`, `max`, `step`,
  `placeholder`, `query`, `multiple`, `required` and `errors` where the state
  has them. Items appear as their keys. `agent_items` returns a flat list of
  `AgentItem`s; long-running actions return `PENDING`.

**Other**:
- Every options struct has a `debug` field, which sets `instance.debug`.
- A controllable value type the kernel can't read as data (not a string,
  number, bool or list of them) registers its conversion once with
  `watchers::register_data::<T>(to_data)`, which the agent bridge and history
  read. Its `Debug` stays a Rust value's.
- A part over a native input gives its `type` and `autocomplete` with
  `Part::input_attrs(..)`; the agent bridge keeps password, one-time-code
  and `cc-*` values secret by them.
- Never call `window.refresh()` where it can run often: it re-renders every
  view without caches, and a key pressed meanwhile waits for it. Notify the
  view that changed.
- Shortcuts that skip held keys read `ui_sdk_kernel::key_held(&keystroke)`.
  Native fields edit only after every keystroke observer has heard the key.
- A popup that's animating out reads as gone to the agent bridge through the
  platform's `in_exiting_layer`; its content's layer needs a focus-tracking
  `Region` measured on the element that paints `listener()`.
- Dismissals use `DismissReason`, `is_dismissal` and `DismissAttemptDetails`.
- Callbacks that aren't state go through `dispatch`.
- Actions use `run_action` and `AnnouncePending` (with the platform crate's
  announce).
- Teardown runs in `cx.on_release(|this, cx| while_destroying(|| ..))`. A
  change the original reports at unmount (outside its destroying flag) goes
  in `cx.on_release_in(window, ..)`, outside `while_destroying`.
- Components that ask `is_focus_from_keyboard` call `track_input` in their
  constructor. Popups move focus with `move_focus` and `restore_focus`.
- Add a `Controller` only where it does more than call a method.
- Composition: `ui_sdk_kernel::events::is_composing(window, cx)`.
- Subscribe to focus with `ui_sdk_kernel::focus_events::{on_focus, on_focus_in,
  on_blur, on_focus_out, on_focus_lost, on_focus_in_from, on_focus_change}`
  (`on_focus_in_from` adds where focus came from; `on_focus_change` is the
  document's `focusin`/`focusout`), never gpui's `cx.on_focus` and
  friends directly. gpui calls focus listeners at the end of its draw, where
  `cx.notify()` asks for no frame, so what a listener changes would show only
  when something else redraws; the kernel's versions run the listener right
  after the draw.
- More generally, gpui drops a `cx.notify()` made while it draws (layout,
  prepaint, paint, focus listeners): no frame is scheduled and observers
  don't run. Report what an element learns while drawing (a load status, a
  measurement that changes state) through `window.defer(cx, ..)` or
  `cx.defer_in(window, ..)`, and notify from there.
- A component that isn't itself a view hears its parts come and go
  (`bound()`) by observing its own entity with `cx.observe_in`.
- Orientation options are `pub type XOrientation = gpui::Orientation`.
- Decide a part's presence, ARIA or style during layout with
  `Part::on_layout(|layout, window, cx| ..)` (`present`, `aria`, `style`).
  Wrap a `Part` in a type of the slot's name only when the slot holds another
  element the app styles, like `AvatarImage` around gpui's `img`.
- `Part::map_state` changes a part's state type. `Part::keep_name()` with
  `part::name_of(&instance, slot, key)` reads back the label an app gave a part.
  `part::press_of(&instance, slot, key)` and `PartPress::click(event, window, cx)`
  click a part from code, the original's `el.click()`.

## DOM → gpui

| Original | gpui |
| --- | --- |
| `press` on a slot, `click` | `on_click`: gpui turns Enter and Space on a focused element into a keyboard click (`ClickEvent::Keyboard`); `kernel::part::on_press` |
| `pointerdown/move/up` | `on_mouse_down/move/up(MouseButton::Left, ..)`; outside presses: `on_mouse_down_out`; drags that leave the element: `window.on_mouse_event` registered while painting, or `capture_any_mouse_up` |
| `pointerType` | gpui hands a tap over as a mouse event: make the `InputEvent` with `InputEvent::click`, `mouse_down` or `mouse_up` and the window, which tell taps apart; parts' `on_press` already does |
| dragging data between elements | gpui's typed drag: `on_drag`, `on_drop`, `drag_over` |
| `keydown`, `event.key` | `on_key_down` / `capture_key_down`, matched with `ui_sdk_kernel::Key::of(&event.keystroke)`, which names keys as the original does; modifiers in `event.keystroke.modifiers` (`platform` is ⌘ on a Mac) |
| `stopPropagation` / `preventDefault` | `cx.stop_propagation()` / `window.prevent_default()` |
| `focus()`, `blur()` | `window.focus(&handle, cx)`, `window.blur()` |
| `focus`/`blur`/`focusin`/`focusout` | `ui_sdk_kernel::focus_events::{on_focus, on_blur, on_focus_in, on_focus_out}(&handle, window, cx, ..)`, subscribed in the constructor and kept in a field (never gpui's `cx.on_focus` family: see Kernel Conventions) |
| `:focus-visible`, input modality | `kernel::modality::{focus_visible, window_source}`; gpui's `focus_visible` style |
| `tabindex` | `FocusHandle::tab_stop(bool)` and `tab_index(n)`, `track_focus(&handle)`, `tab_group()`; gpui moves focus with `window.focus_next(cx)`/`focus_prev(cx)` |
| `aria-activedescendant` | `Aria::active_descendant(id)` on the container that keeps focus (gpui's own `aria_active_descendant()` goes on the item and only counts while an ancestor holds focus) |
| roles, ARIA | `.aria_role(roles::Role::..)`, gpui's `.aria_*`, and the kernel's `.aria(Aria::new()..)`: see Kernel Conventions |
| document order | render order, through `Parts` |
| `getBoundingClientRect`, `ResizeObserver` | `Parts::measure` / `Measured`: bounds recorded at prepaint, read the next frame |
| `scrollIntoView`, scroll position | `ScrollHandle` (`track_scroll`, `scroll_to_item`, `set_offset`, `bounds`) |
| `setTimeout` | `cx.spawn_in(window, async move \|this, cx\| { cx.background_executor().timer(d).await; this.update_in(cx, \|this, window, cx\| ..).ok(); })`; keep the `Task` in a field, dropping it cancels |
| `requestAnimationFrame` | `window.request_animation_frame()`, `window.on_next_frame(..)` |
| CSS transitions | state that parts expose, and the motion crate |
| portals, top layer, `popover` | `deferred(positioned(&positioner, content))` at the platform crate's `LAYER_PRIORITY` |
| `dir`, `isRTL(el)` | `ui_sdk_platform::is_rtl(cx)`: the app's direction |
| `Intl.*` | ICU4X 2.x (`icu` or its component crates, compiled data) where the original formats, parses, collates or segments by locale |
| `localStorage` | the storage crate |
| `matchMedia` | `window.appearance()` and the preferences crate |
| `<input>`, `<textarea>` | `ui-sdk-native`'s text field |
| `<form>`, hidden inputs, validity, `<label for>` | the forms crate |
| clipboard | `cx.write_to_clipboard(ClipboardItem::new_string(..))`, `cx.read_from_clipboard()` |
| `<input type="file">` | `cx.prompt_for_paths(PathPromptOptions { .. })` |
| `#dev` warnings | `log::warn!` behind `cfg!(debug_assertions)` |
| `isBrowser`, server rendering | there is always a window: drop the checks |

## Examples

Each component crate gets `examples/<component>.rs`, a window showing the
component in its main configurations with minimal styling, like
`crates/components/toggle/examples/toggle.rs`. Add
`[dev-dependencies] gpui_platform.workspace = true`. Check it with
`--examples` and run it.

The gallery (`crates/gallery`) compiles every example verbatim as one of its
sections, so keep to this shape: the view is `struct Example`, made by
`fn new(window: &mut Window, cx: &mut Context<Self>) -> Self`, rendered by
`impl Render for Example` with a `.size_full()` root that doesn't stretch its
controls (`.items_start()` on rows), and opened by a `fn main()` that starts
at the beginning of a line and is the file's last item. Set up nothing in
`main` beyond opening the window; the gallery calls only `Example::new`. A
new example needs an entry in `SECTIONS` in `crates/gallery/build.rs`.
