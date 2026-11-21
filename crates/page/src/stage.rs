//! Port of `page/history/stage.ts`: trying changes on the page and taking
//! them back: an agent's suggestion shown on the real control while a person
//! looks at it, a preview of a preset, a "what would this look like". The
//! values go on through the bridge, like any agent step, and `revert` puts
//! back what each control had.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, AsyncApp, Task};
use ui_sdk_kernel::{AgentValue, json};

use crate::agent::{ActOptions, AgentBridge, AgentComponent, AgentStep, Here, QueryInput, snapshot};
use crate::registry::{self, setter_of};

/// One value to put on a control: its ref or label, the key, and the value.
#[derive(Clone, Debug, PartialEq)]
pub struct StagedChange {
    pub r#ref: String,
    /// `"value"` when left out, set with the component's `set<Key>` action.
    pub key: Option<String>,
    pub value: AgentValue,
}

impl StagedChange {
    pub fn new(r#ref: impl Into<String>, value: AgentValue) -> Self {
        Self {
            r#ref: r#ref.into(),
            key: None,
            value,
        }
    }

    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }
}

/// A change that went on, with the value it replaced.
#[derive(Clone, Debug, PartialEq)]
pub struct AppliedChange {
    pub r#ref: String,
    pub key: String,
    pub before: AgentValue,
    pub value: AgentValue,
}

/// What `stage_changes` put on the page, and how to take it back.
pub struct StagedChanges {
    agent: AgentBridge,
    applied: Vec<AppliedChange>,
    reverted: Rc<Cell<bool>>,
}

/// Puts values on the page's controls through the agent bridge, and resolves
/// with how to take them back. `key` defaults to `"value"`, set with the
/// component's `set<Key>` action. Changes that can't run are skipped, and
/// `revert` restores only the ones that did, in reverse order, and resolves
/// with the ones it couldn't put back, like a password field's, whose value
/// before reads `"[redacted]"`.
///
/// ```ignore
/// let staged = stage_changes(&agent, vec![StagedChange::new("Volume", json!(80)), StagedChange::new("Theme", json!("dark"))], cx).await;
/// // later
/// staged.revert(cx).await;
/// ```
pub fn stage_changes(agent: &AgentBridge, changes: Vec<StagedChange>, cx: &mut App) -> Task<StagedChanges> {
    let agent = agent.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let on_page = |list: Vec<AgentComponent>| -> Vec<AgentComponent> {
            list.into_iter()
                .filter(|component| !component.is_off_page() && component.component() != "more")
                .collect()
        };
        let components = on_page(cx.update(|cx| snapshot(&mut Here::none(), cx)));
        let matching = |list: &[AgentComponent], r#ref: &str| -> Option<AgentComponent> {
            list.iter()
                .find(|component| component.r#ref() == r#ref)
                .or_else(|| list.iter().find(|component| component.label() == Some(r#ref)))
                .cloned()
        };
        let mut applied = Vec::new();
        for change in changes {
            // A long page's snapshot stops at its limit, so a control past it
            // is looked up by ref, then name.
            let mut component = matching(&components, &change.r#ref);
            for input in [
                QueryInput {
                    within: Some(change.r#ref.clone()),
                    ..Default::default()
                },
                QueryInput {
                    text: Some(change.r#ref.clone()),
                    ..Default::default()
                },
            ] {
                if component.is_some() {
                    break;
                }
                let found = cx.update(|cx| agent.query(input, cx)).await;
                component = matching(&on_page(found.unwrap_or_default()), &change.r#ref);
            }
            let key = change.key.clone().unwrap_or_else(|| "value".into());
            let setter = setter_of(&key);
            let Some(component) = component.filter(|component| component.actions().contains(&setter.as_str())) else {
                continue;
            };
            let step = AgentStep::new(component.r#ref(), setter).args([change.value.clone()]);
            let result = cx
                .update(|cx| agent.act(vec![step], ActOptions::default(), cx))
                .await;
            if result.ok {
                applied.push(AppliedChange {
                    r#ref: component.r#ref().to_string(),
                    before: component.get(&key).cloned().unwrap_or(AgentValue::Null),
                    key,
                    value: change.value,
                });
            }
        }
        StagedChanges {
            agent,
            applied,
            reverted: Rc::new(Cell::new(false)),
        }
    })
}

impl StagedChanges {
    /// The changes that went on, each with the value it replaced.
    pub fn applied(&self) -> &[AppliedChange] {
        &self.applied
    }

    /// Puts back what each control had, and resolves with the changes it
    /// couldn't. Runs once.
    pub fn revert(&self, cx: &mut App) -> Task<Vec<AppliedChange>> {
        if self.reverted.replace(true) {
            return Task::ready(Vec::new());
        }
        let agent = self.agent.clone();
        let applied = self.applied.clone();
        cx.spawn(async move |cx: &mut AsyncApp| {
            let mut failed = Vec::new();
            for change in applied.into_iter().rev() {
                // A secret field's value before is hidden, and "[redacted]"
                // would land in it.
                let hidden = change.before == json!("[redacted]")
                    && registry::entry(&change.r#ref).is_some_and(|entry| entry.borrow().secret);
                let done = !hidden && {
                    let step = AgentStep::new(change.r#ref.clone(), setter_of(&change.key)).args([change.before.clone()]);
                    cx.update(|cx| agent.act(vec![step], ActOptions::default(), cx))
                        .await
                        .ok
                };
                if !done {
                    failed.push(change);
                }
            }
            failed
        })
    }
}
