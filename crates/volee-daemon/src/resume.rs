//! Resuming agents whose session ended (ADR 0006, delivery to stopped
//! agents).
//!
//! Claude Code stops idle background sessions after about an hour, and a
//! person can stop one too. When a message arrives for an agent volee
//! launched whose session ended, volee resumes the conversation where it
//! ran, with the pending messages as the prompt: from the repository's main
//! checkout, in the agent's worktree (`--worktree`, ADR 0013). The resumed
//! session is usually a fork with a new id; the daemon links it to the same
//! agent (ADR 0001), through its first hook or its handle in the listing.
//!
//! Resumes run off the state lock, one at a time per agent. A failed resume
//! is told to the human once, and not retried for a while unless a new
//! message arrives.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use volee_core::harness::HarnessKind;
use volee_core::message::{Message, MessageKind, Party, Receipt};
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::AgentState;
use volee_core::{Agent, AgentId, Event, EventKind, MessageId, Registry};
use volee_harness::SpawnSpec;
use volee_harness::hooks::format_messages_for_agent;

use crate::error::DaemonError;
use crate::launch;
use crate::server::Shared;

/// How long a failed resume waits before the same agent is tried again,
/// unless a new message arrives for it.
const RETRY_AFTER: Duration = Duration::from_secs(600);

/// Starts the resumer when the daemon has a harness.
pub(crate) fn spawn(shared: &Arc<Shared>) -> Option<JoinHandle<()>> {
    shared.harness.as_ref()?;
    Some(tokio::spawn(run(Arc::clone(shared))))
}

/// Resumes in progress and recent failures.
#[derive(Debug, Default)]
struct Book {
    running: HashSet<AgentId>,
    failed: HashMap<AgentId, Instant>,
}

async fn run(shared: Arc<Shared>) {
    let book = Arc::new(Mutex::new(Book::default()));
    let (mut events, waiting) = {
        let state = shared.state.lock().await;
        let waiting: Vec<AgentId> = state
            .registry()
            .agents()
            .filter(|agent| needs_resume(state.registry(), agent))
            .map(|agent| agent.id)
            .collect();
        (state.subscribe(), waiting)
    };
    for agent in waiting {
        start(&shared, &book, agent, false);
    }
    loop {
        match events.recv().await {
            Ok(event) => {
                for (agent, new_message) in triggered(&event.event.kind) {
                    start(&shared, &book, agent, new_message);
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let waiting: Vec<AgentId> = {
                    let state = shared.state.lock().await;
                    state
                        .registry()
                        .agents()
                        .filter(|agent| needs_resume(state.registry(), agent))
                        .map(|agent| agent.id)
                        .collect()
                };
                for agent in waiting {
                    start(&shared, &book, agent, false);
                }
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// The agents an event may require to resume, and whether it brings a new
/// message for them.
fn triggered(kind: &EventKind) -> Vec<(AgentId, bool)> {
    match kind {
        EventKind::MessagePosted { message } => message
            .receipts
            .iter()
            .filter_map(|receipt| match receipt.to {
                Party::Agent(agent) => Some((agent, true)),
                Party::Human | Party::System => None,
            })
            .collect(),
        EventKind::StateChanged {
            agent,
            state: AgentState::Stopped | AgentState::Lost,
            ..
        } => vec![(*agent, false)],
        _ => Vec::new(),
    }
}

/// Whether volee should resume `agent` now: it launched it, its session
/// ended, and messages wait for it.
fn needs_resume(registry: &Registry, agent: &Agent) -> bool {
    agent.harness == HarnessKind::ClaudeCode
        && agent.launched_by != LaunchedBy::Outside
        && agent.current_session().is_some()
        && matches!(agent.state, AgentState::Stopped | AgentState::Lost)
        && agent.workspace.repo_root.is_some()
        && !registry.pending_for(agent.id).is_empty()
}

fn book(book: &Mutex<Book>) -> std::sync::MutexGuard<'_, Book> {
    // The book only holds sets; a poisoned lock is still consistent.
    book.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts a resume of `agent` unless one runs, or one failed recently and no
/// new message arrived since.
fn start(shared: &Arc<Shared>, ledger: &Arc<Mutex<Book>>, agent: AgentId, new_message: bool) {
    {
        let mut entries = book(ledger);
        if entries.running.contains(&agent) {
            return;
        }
        let recent_failure = entries
            .failed
            .get(&agent)
            .is_some_and(|at| at.elapsed() < RETRY_AFTER);
        if recent_failure && !new_message {
            return;
        }
        entries.running.insert(agent);
    }
    let shared = Arc::clone(shared);
    let ledger = Arc::clone(ledger);
    tokio::spawn(async move {
        let outcome = resume(&shared, agent).await;
        // The ledger's lock is released before anything else awaits.
        let first_failure = {
            let mut entries = book(&ledger);
            entries.running.remove(&agent);
            match &outcome {
                Ok(Some(_)) => {
                    entries.failed.remove(&agent);
                    false
                }
                Ok(None) => false,
                Err(_) => entries.failed.insert(agent, Instant::now()).is_none(),
            }
        };
        match outcome {
            Ok(Some(count)) => {
                tracing::info!(%agent, messages = count, "resumed an agent to deliver messages");
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%agent, %error, "cannot resume an agent");
                if first_failure {
                    alert_human(&shared, agent, &error).await;
                }
            }
        }
    });
}

/// What a resume needs, read under the lock.
struct Plan {
    session: volee_core::harness::SessionKey,
    spec: SpawnSpec,
    shown: Vec<MessageId>,
}

/// Resumes `agent` with its pending messages. `Ok(None)` when it no longer
/// needs it (a hook or another resume got there first).
async fn resume(shared: &Arc<Shared>, agent: AgentId) -> Result<Option<usize>, DaemonError> {
    let harness = shared
        .harness
        .clone()
        .ok_or_else(|| DaemonError::Unavailable("Claude Code is not available".into()))?;
    let (settings_file, mcp_config_file) = launch::write_agent_files(shared, &harness, agent)?;
    let plan = {
        let state = shared.state.lock().await;
        let registry = state.registry();
        let Some(record) = registry.agent(agent) else {
            return Ok(None);
        };
        if !needs_resume(registry, record) {
            return Ok(None);
        }
        plan(shared, registry, record, settings_file, mcp_config_file)
    };
    let Some(plan) = plan else {
        return Ok(None);
    };
    let Plan {
        session,
        spec,
        shown,
    } = plan;
    let resumed = {
        let harness = Arc::clone(&harness);
        launch::blocking(move || harness.resume(&session, &spec)).await??
    };
    let mut state = shared.state.lock().await;
    state.expect_session(resumed.handle, agent, resumed.origin);
    let now = state.now();
    let mut events: Vec<Event> = shown
        .iter()
        .map(|message| {
            Event::new(
                now,
                EventKind::MessageDelivered {
                    message: *message,
                    to: Party::Agent(agent),
                },
            )
        })
        .collect();
    events.push(Event::new(
        now,
        EventKind::StateChanged {
            agent,
            state: AgentState::Launching,
            detail: Some(format!(
                "resumed to deliver {} message{}",
                shown.len(),
                if shown.len() == 1 { "" } else { "s" }
            )),
        },
    ));
    state.commit(events)?;
    Ok(Some(shown.len()))
}

fn plan(
    shared: &Shared,
    registry: &Registry,
    agent: &Agent,
    settings_file: std::path::PathBuf,
    mcp_config_file: std::path::PathBuf,
) -> Option<Plan> {
    let session = agent.current_session()?.key.clone();
    let root = agent.workspace.repo_root.clone()?;
    let worktree = agent
        .workspace
        .worktree
        .as_deref()
        .and_then(|worktree| launch::worktree_name(&root, worktree));
    let pending = registry.pending_for(agent.id);
    let rendered = format_messages_for_agent(pending.iter().copied(), |id| {
        registry.agent(id).map(|sender| sender.name.clone())
    });
    let shown = pending
        .iter()
        .take(rendered.shown)
        .map(|message| message.id)
        .collect();
    let disallowed_tools = if agent.role == Role::Coordinator {
        ["Edit", "Write", "NotebookEdit"]
            .map(str::to_owned)
            .to_vec()
    } else {
        Vec::new()
    };
    let model = agent.model.clone().or_else(|| {
        shared
            .settings
            .model_for(agent.workspace.repo.as_deref())
            .map(str::to_owned)
    });
    Some(Plan {
        session,
        spec: SpawnSpec {
            cwd: root,
            name: agent.name.clone(),
            model,
            prompt: rendered.text,
            settings_file,
            mcp_config_file,
            permission_mode: launch::permission_mode(shared),
            allowed_tools: Vec::new(),
            disallowed_tools,
            append_system_prompt: Some(launch::instructions_for(registry, agent)),
            worktree,
        },
        shown,
    })
}

/// Tells the human, in the agent's thread with them, that a message could
/// not reach the agent.
async fn alert_human(shared: &Arc<Shared>, agent: AgentId, error: &DaemonError) {
    let mut state = shared.state.lock().await;
    let registry = state.registry();
    let Some(record) = registry.agent(agent) else {
        return;
    };
    let name = record.name.clone();
    let thread = record
        .mission
        .and_then(|mission| registry.mission(mission))
        .map(|mission| mission.thread)
        .or_else(|| {
            registry
                .thread_between(Party::System, Party::Human)
                .map(|thread| thread.id)
        });
    let now = state.now();
    let mut events = Vec::new();
    let thread = if let Some(thread) = thread {
        thread
    } else {
        let id = state.new_thread_id();
        events.push(Event::new(
            now,
            EventKind::ThreadOpened {
                thread: volee_core::message::Thread {
                    id,
                    title: "volee".to_owned(),
                    mission: None,
                    participants: vec![Party::System, Party::Human],
                    opened_at: now,
                },
            },
        ));
        id
    };
    events.push(Event::new(
        now,
        EventKind::MessagePosted {
            message: Message {
                id: state.new_message_id(),
                thread,
                from: Party::System,
                kind: MessageKind::Alert,
                body: format!(
                    "Messages wait for {name}, whose session ended, and volee could not resume it: \
                     {error}. Take it over with `volee attach {name}`, or forget it."
                ),
                refs: Vec::new(),
                in_reply_to: None,
                at: now,
                receipts: vec![Receipt {
                    to: Party::Human,
                    delivered_at: None,
                    read_at: None,
                }],
            },
        },
    ));
    if let Err(error) = state.commit(events) {
        tracing::error!(%error, "cannot tell the human about a failed resume");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_to_agents_and_stops_trigger_a_check() {
        let agent = AgentId::from_ulid(ulid::Ulid::from_parts(1, 1));
        let stopped = EventKind::StateChanged {
            agent,
            state: AgentState::Lost,
            detail: None,
        };
        assert_eq!(triggered(&stopped), vec![(agent, false)]);
        let working = EventKind::StateChanged {
            agent,
            state: AgentState::Working,
            detail: None,
        };
        assert_eq!(triggered(&working), vec![]);
    }
}
