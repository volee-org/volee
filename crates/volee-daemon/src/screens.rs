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

/// How many rounds in a row a host must not answer before its agent counts
/// as stopped: one slow answer proves nothing.
const GONE_AFTER: u32 = 2;

/// What the last rounds saw of each host.
#[derive(Debug, Default)]
pub(crate) struct Screens {
    seen: Mutex<HashMap<AgentId, (u64, Instant)>>,
    misses: Mutex<HashMap<AgentId, u32>>,
}

impl Screens {
    /// Records a screen and says whether it changed in the last
    /// `IDLE_AFTER`; `None` the first time, when nothing is known yet.
    fn changed(&self, agent: AgentId, screen: &[u8]) -> Option<bool> {
        let mut hasher = DefaultHasher::new();
        screen.hash(&mut hasher);
        let hash = hasher.finish();
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        match seen.get(&agent) {
            Some((last, _)) if *last != hash => {
                seen.insert(agent, (hash, Instant::now()));
                Some(true)
            }
            Some((_, since)) => Some(since.elapsed() < IDLE_AFTER),
            None => {
                seen.insert(agent, (hash, Instant::now()));
                None
            }
        }
    }

    /// Counts a round where the host of `agent` did not answer; true once it
    /// missed `GONE_AFTER` rounds in a row.
    fn missed(&self, agent: AgentId) -> bool {
        let mut misses = self.misses.lock().unwrap_or_else(PoisonError::into_inner);
        let count = misses.entry(agent).or_insert(0);
        *count += 1;
        *count >= GONE_AFTER
    }

    fn answered(&self, agent: AgentId) {
        self.misses
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&agent);
    }
}

/// One round: the screens of hosted agents without hooks, then OpenCode's
/// processes.
pub(crate) async fn round(shared: &Arc<Shared>) {
    hosted_states(shared).await;
    opencode_processes(shared).await;
}

/// One hosted agent, as a round sees it.
struct Hosted {
    agent: AgentId,
    host: String,
    state: AgentState,
    /// Its harness has hooks (Claude Code): they give its states, the host
    /// only says whether it runs.
    hooked: bool,
    held: bool,
}

/// Follows every agent in a host: whether its host runs and, for a harness
/// without hooks, what its screen does. A program that ended, or a host that
/// stopped answering for two rounds, stops the agent; a host that runs
/// brings back an agent wrongly shown stopped.
async fn hosted_states(shared: &Arc<Shared>) {
    let hosted: Vec<Hosted> = {
        let state = shared.state.lock().await;
        state
            .registry()
            .agents()
            .filter(|agent| !matches!(agent.state, AgentState::Done | AgentState::Failed))
            .filter_map(|agent| {
                Some(Hosted {
                    agent: agent.id,
                    host: agent.location.host()?.to_owned(),
                    state: agent.state.clone(),
                    hooked: agent.harness == HarnessKind::ClaudeCode,
                    held: agent.hold.is_some(),
                })
            })
            .collect()
    };
    let dir = crate::launch::hosts_dir(shared);
    for Hosted {
        agent,
        host,
        state: current,
        hooked,
        held,
    } in hosted
    {
        let socket = dir.join(format!("{host}.sock"));
        let read = tokio::task::spawn_blocking(move || {
            volee_host::snapshot(&socket, "volee daemon screens", Duration::from_secs(2))
        })
        .await;
        let running = current.is_live() || current == AgentState::Launching;
        let (next, detail) = if let Ok(Ok((welcome, screen))) = read {
            shared.screens.answered(agent);
            if welcome.exit.is_some() {
                if !running {
                    continue;
                }
                (AgentState::Stopped, Some("its program ended".to_owned()))
            } else if !running && !held {
                // Its host still runs it: it never stopped.
                (
                    AgentState::Idle,
                    Some("its volee host still runs it".to_owned()),
                )
            } else if hooked {
                continue;
            } else {
                match shared.screens.changed(agent, &screen) {
                    Some(true) => (AgentState::Working, None),
                    Some(false) => (AgentState::Idle, None),
                    None => continue,
                }
            }
        } else {
            if !running || !shared.screens.missed(agent) {
                continue;
            }
            (
                AgentState::Stopped,
                Some("its volee host is gone".to_owned()),
            )
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
