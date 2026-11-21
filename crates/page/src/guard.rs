//! Port of `page/agent/guard.ts`: `act` with a guard, which history's
//! restores pass, checked once `allow` answers, so a change someone makes
//! meanwhile supersedes the restore.
//!
//! The original keys the guarded entry point by the bridge's own `act` in a
//! `WeakMap`, so it survives object spreads and reactive proxies. A Rust
//! bridge is one type, so the guarded entry point is a crate method of it,
//! which `create_history` calls.

use gpui::{App, Task};

use crate::agent::{ActResult, AgentBridge, AgentStep};

pub(crate) use crate::agent::Guard;

/// Runs `steps` in the bridge's turn, refusing each once `allow` answered if
/// `guard` says the restore it's for was superseded.
pub(crate) fn guarded_act(agent: &AgentBridge, steps: Vec<AgentStep>, guard: Guard, cx: &mut App) -> Task<ActResult> {
    agent.act_guarded(steps, guard, cx)
}
