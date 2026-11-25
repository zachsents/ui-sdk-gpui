# Agent Bridge

The bridge lives in `ui_sdk::page`. The upstream [Connect an Agent reference](https://ui-sdk.dev/docs/agents/connect-an-agent/reference) documents its tools and JSON results; this page covers the Rust integration.

Snippets assume a gpui view with `use gpui::prelude::*;`; component types come from `ui_sdk::components::<name>`. Fields such as `self.agent` and `self.region` belong to that view.

Run the [bridge example](../crates/page/examples/agent.rs) with `cargo run -p ui-sdk-page --example agent`. It includes a form, snapshot viewer, action runner, change log and confirmation mark.

## Create the Bridge

After `ui_sdk::init`, create an `AgentBridge` and keep it in your view. Clones share the bridge:

```rust
use ui_sdk::page::{AgentBridge, CreateAgentOptions, create_agent};

let agent = create_agent(CreateAgentOptions::default(), cx);
```

Components register through `Instance::with_agent`; no per-component registration is needed. Components created before the bridge are included too.

The bridge reads `aria_label` and the `aria_label` of associated label parts. Drawn text and `text!` children don't reach it. Name the parts an agent should identify:

```rust
Input::root(&self.name, cx).aria_label("Project name")
Select::label(&self.region, cx).aria_label("Region").child("Region")
```

Pass list choices as `items` when the agent needs them before the list renders; [Items as Data](using-components.md#items-as-data) shows the Rust types.

## Read and Change Components

Inside a view method, take a snapshot with the calling window and app:

```rust
use ui_sdk::page::components_value;

let components = self.agent.snapshot(window, cx);
println!("{:#}", components_value(&components));
```

`snapshot` reads across windows immediately. `components_value` converts its typed result to `AgentValue`, a re-export of `serde_json::Value`.

`act` and `query` return gpui tasks. Await them inside `cx.spawn`; keep or detach the outer task so it runs:

```rust
use ui_sdk::kernel::json;
use ui_sdk::page::{ActOptions, AgentStep};

let step = AgentStep::new("Region", "setValue").args([json!("North America")]);
let result = self.agent.act(vec![step], ActOptions::default(), cx);
cx.spawn(async move |_, _| println!("{:#}", result.await.to_value()))
    .detach();
```

For a typed query:

```rust
use ui_sdk::page::{QueryInput, query_result_value};

let input = QueryInput {
    within: Some("Region".into()),
    text: Some("Asia".into()),
    ..Default::default()
};
let result = self.agent.query(input, cx);
cx.spawn(async move |_, _| println!("{:#}", query_result_value(&result.await)))
    .detach();
```

`act` and `query` queue in call order. Dropping their task cancels the call. Synchronous changes reach component callbacks with `ChangeSource::Agent`. In async actions, retain `details.source` and wrap later mutations in `ui_sdk::kernel::with_source(source, || ..)` to preserve attribution. A callback can refuse a change with `details.cancel()`, a component's value in `on_before_value_change`.

## Connect a Model

`agent_tools()` returns each tool's `name`, `description` and `input_schema`. Map those fields to your model API's tool format. `AgentTool::to_value()` returns the MCP shape: `name`, `description`, `inputSchema`.

Dispatch a model's tool call from the main thread, then await the task without blocking gpui:

```rust
let output = self.agent.call(&tool_name, &tool_input, cx);
cx.spawn(async move |_, _| {
    let result = output.await;
    // Send result back through your model client's tool-result message.
    println!("{result}");
})
.detach();
```

`tool_name` is a string and `tool_input` an `AgentValue`. The bridge supplies no model client or transport. Await each call before dispatching the next if order matters: `call("snapshot", ..)` reads immediately, even while an action waits for permission. HTTP requests must yield to the app; run a blocking client on `cx.background_executor()`.

## Permissions

`CreateAgentOptions::allow` runs in the target component's window, with the app. It returns `Allowed::Now(bool)` (or `bool.into()`) for an immediate answer, or `Allowed::later(future)` to wait.

This hook uses gpui's prompt for a step carrying a confirm mark:

```rust
use gpui::PromptLevel;
use ui_sdk::page::Allowed;

let agent = create_agent(
    CreateAgentOptions::default().allow(|step, _context, window, cx| match &step.confirm {
        None => true.into(),
        Some(confirm) => {
            let answer = window.prompt(PromptLevel::Warning, confirm, None, &["Allow", "Deny"], cx);
            Allowed::later(async move { answer.await == Ok(0) })
        }
    }),
    cx,
);
```

An `allow` hook must check `step.confirm` if confirm marks should require approval. Without a hook, the bridge refuses marked steps. The hook's context carries an abort signal for closing a custom prompt on cancellation or timeout (`CreateAgentOptions::timeout`, five minutes by default). To cancel from your app, pass a signal in `ActOptions`, then abort its `AbortController`.

Marks attach to parts with `agent_marks`:

```rust
use ui_sdk::page::AgentMarks;

Button::root(&self.delete, cx)
    .aria_label("Delete project")
    .agent_marks(AgentMarks::new().confirm("Delete project"))
    .child("Delete project")
```

For any element with an id, use `with_agent_marks`:

```rust
use ui_sdk::page::with_agent_marks;

with_agent_marks(div().id("billing").children(billing_fields), AgentMarks::new().hidden(true))
```

`hidden(true)` excludes a subtree from the agent. `secret(true)` redacts its values while allowing the agent to fill them. See the upstream reference for mark inheritance, automatic secret fields and confirmation semantics.

## Watch Changes

`subscribe` runs inside a component update, without an app argument. Collect changes there and drain them before the model's next turn:

```rust
use std::cell::RefCell;
use std::rc::Rc;
use ui_sdk::kernel::ChangeSource;
use ui_sdk::page::ObservedChange;

let changes: Rc<RefCell<Vec<ObservedChange>>> = Rc::default();
let subscription = agent.subscribe(
    {
        let changes = changes.clone();
        move |change| {
            if change.source != ChangeSource::Agent {
                changes.borrow_mut().push(change.clone());
            }
        }
    },
    cx,
);
```

Store both `changes` and `subscription` in the view. Dropping the subscription removes the listener. `ObservedChange::to_value()` produces the JSON to send to a model.

For application records without a bridge, `observe` returns the same kind of subscription, including changes inside hidden marks:

```rust
use ui_sdk::page::{ObserveOptions, observe};

observe(
    |change| println!("{}", change.to_value()),
    ObserveOptions::default().within("settings"),
    cx,
)
.detach();
```

History is a handle too:

```rust
use ui_sdk::page::{HistoryOptions, create_history};

let history = create_history(&agent, HistoryOptions::default(), cx);
```

Keep it to record changes. `history.undo_source(ChangeSource::Agent, cx)` and `history.seek(n, cx)` return tasks; retain, await or detach them. Restores go through the bridge's permissions and component callbacks. See the upstream reference for history and staging semantics.

## Your Own Components

A component you build joins the bridge the way the library's do: its constructor hands the entity to its instance with `Instance::with_agent`, and it implements the kernel's `Agent` trait. Here's a star rating whose one action is `rate`:

```rust
use gpui::{App, Context, Entity, Window};
use ui_sdk::kernel::roles::Role;
use ui_sdk::kernel::{Agent, AgentAction, AgentState, ChangeOptions, Controllable, Instance, Part};

pub struct Stars {
    instance: Instance,
    value: Controllable<f64>,
}

impl Stars {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            instance: Instance::new("stars").with_agent(cx),
            value: Controllable::new("value", 0.0),
        }
    }

    pub fn rate(&mut self, value: f64, options: ChangeOptions, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let changed = self.value.change(&self.instance, value, options, window, cx);
        if changed {
            cx.notify();
        }
        changed
    }

    pub fn root(this: &Entity<Self>, cx: &App) -> Part<f64> {
        let stars = this.read(cx);
        Part::new(&stars.instance, "root", None, *stars.value.get()).aria_role(Role::Group)
    }
}

impl Agent for Stars {
    const ACTIONS: &'static [AgentAction<Self>] = &[AgentAction::new("rate", |stars, args, window, cx| {
        let value = args.number(0)?;
        Ok(stars.rate(value, ChangeOptions::default(), window, cx).into())
    })];

    fn agent_state(&self, _cx: &App) -> AgentState {
        AgentState::new().with("value", *self.value.get())
    }
}
```

Here's what each piece does:

- `ACTIONS` lists what an agent can call, by the name the agent uses. Each action calls the component's own method with the step's arguments, which `AgentArgs` reads by kind (`number`, `string`, `boolean`, `list`). A wrong kind fails the step with what it takes, like `rate on Your rating (c1) takes a number, not "lots".`, and returning `Err(AgentError::refused(...))` fails it with your own message.
- The bridge runs the action as the agent, so the change `Controllable::change` makes reports `ChangeSource::Agent`, to `observe` and to any change callback the component takes.
- `agent_state` returns the state under the original's key names. The snapshot shows `value`, `open` and `checked`, or the keys in `CONTROLLED` when you set it, and reads `disabled` and `readOnly` to know when the component can't act, and `pending` and `error` for work still running or failed.

Render its root with the stars inside, and name it with `aria_label("Your rating")` like any component. On a page of its own, the snapshot lists it as:

```json
[{ "ref": "c1", "component": "stars", "actions": ["rate"], "label": "Your rating", "value": 0.0 }]
```

and `AgentStep::new("Your rating", "rate").args([json!(4)])` rates it 4.

A component with items says more through the trait. `SURFACE` says whether its value is always one of its items (`closed`), whether its closed items hide other components, like tabs (`reveals`), and whether its value is secret. `agent_items` returns its items as data, and `agent_shown` hides it from the agent until it matters, like a field while it has no errors. `AgentAction::unlisted` adds an action that runs without being listed, like `toggle` beside `setPressed`. [Toggle](../crates/components/toggle/src/toggle.rs) is a short, complete example, and every component in the library implements the trait.

Two types share the name `AgentAction`: the kernel's is what a component declares, and `ui_sdk::page::AgentAction` is the step `allow` decides.
