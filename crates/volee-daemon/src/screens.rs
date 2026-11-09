//! Agents of harnesses that report no state (ADR 0022), and the OpenCode
//! agents started elsewhere (ADR 0023).
//!
//! An agent in a host whose harness has no hooks gets its state from its
//! screen, the last resort of ADR 0022: a screen that changed since the last
//! round is working, one that stayed the same for a while is idle, and a host
//! that recorded an end, or is gone, means stopped.
//!
//! OpenCode agents that volee does not host are found in the process table
//! and reconciled like any listing. A process that is gone stops its agent.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use volee_core::harness::HarnessKind;
use volee_core::reconcile::reconcile;
use volee_core::role::LaunchedBy;
use volee_core::state::AgentState;
use volee_core::{AgentId, Event, EventKind};

use crate::server::Shared;

/// How long a screen stays the same before its agent counts as idle.
const IDLE_AFTER: Duration = Duration::from_secs(6);

/// What the last rounds saw of each screen.
#[derive(Debug, Default)]
pub(crate) struct Screens {
    seen: Mutex<HashMap<AgentId, (u64, Instant)>>,
}

impl Screens {
    /// Records a screen and says whether it changed since `IDLE_AFTER`.
    fn changed(&self, agent: AgentId, screen: &[u8]) -> bool {
        let mut hasher = DefaultHasher::new();
        screen.hash(&mut hasher);
        let hash = hasher.finish();
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        match seen.get(&agent) {
            Some((last, _)) if *last != hash => {
                seen.insert(agent, (hash, Instant::now()));
                true
            }
            Some((_, since)) => since.elapsed() < IDLE_AFTER,
            None => {
                seen.insert(agent, (hash, Instant::now()));
                true
            }
        }
    }
}

/// One round: the screens of hosted agents without hooks, then OpenCode's
/// processes.
pub(crate) async fn round(shared: &Arc<Shared>) {
    hosted_states(shared).await;
    opencode_processes(shared).await;
}

async fn hosted_states(shared: &Arc<Shared>) {
    let hosted: Vec<(AgentId, String, AgentState)> = {
        let state = shared.state.lock().await;
        state
            .registry()
            .agents()
            .filter(|agent| agent.harness != HarnessKind::ClaudeCode)
            .filter(|agent| agent.state.is_live() || agent.state == AgentState::Launching)
            .filter_map(|agent| {
                Some((
                    agent.id,
                    agent.location.host()?.to_owned(),
                    agent.state.clone(),
                ))
            })
            .collect()
    };
    let dir = crate::launch::hosts_dir(shared);
    for (agent, host, current) in hosted {
        let socket = dir.join(format!("{host}.sock"));
        let read = tokio::task::spawn_blocking(move || {
            volee_host::snapshot(&socket, "volee daemon screens", Duration::from_secs(2))
        })
        .await;
        let (next, detail) = match read {
            Ok(Ok((welcome, _))) if welcome.exit.is_some() => {
                (AgentState::Stopped, Some("its program ended".to_owned()))
            }
            Ok(Ok((_, screen))) => {
                if shared.screens.changed(agent, &screen) {
                    (AgentState::Working, None)
                } else {
                    (AgentState::Idle, None)
                }
            }
            _ => (
                AgentState::Stopped,
                Some("its volee host is gone".to_owned()),
            ),
        };
        if next.category() != current.category() {
            let mut state = shared.state.lock().await;
            let now = state.now();
            let _ = state.commit(vec![Event::new(
                now,
                EventKind::StateChanged {
                    agent,
                    state: next,
                    detail,
                },
            )]);
        }
    }
}

async fn opencode_processes(shared: &Arc<Shared>) {
    let listed = tokio::task::spawn_blocking(|| {
        volee_harness::opencode::list_processes(&volee_harness::SystemRunner)
    })
    .await;
    let Ok(Ok(listing)) = listed else {
        return;
    };
    let mut state = shared.state.lock().await;
    let now = state.now();
    state.sessions_mut().record(&listing);
    let mut events = reconcile(state.registry(), &listing, now, || state.new_agent_id());
    // The listing is never complete: an agent started elsewhere whose
    // process is gone stops here.
    let running: HashSet<_> = listing
        .sessions
        .iter()
        .map(|session| &session.key)
        .collect();
    let kind = volee_harness::opencode::kind();
    for agent in state.registry().agents() {
        let gone = agent.harness == kind
            && agent.launched_by == LaunchedBy::Outside
            && agent.location.host().is_none()
            && agent.state.is_live()
            && agent
                .current_session()
                .is_some_and(|link| !running.contains(&link.key));
        if gone {
            events.push(Event::new(
                now,
                EventKind::StateChanged {
                    agent: agent.id,
                    state: AgentState::Stopped,
                    detail: Some("its process ended".to_owned()),
                },
            ));
        }
    }
    if !events.is_empty()
        && let Err(error) = state.commit(events)
    {
        tracing::error!(%error, "cannot apply OpenCode's processes");
    }
}
