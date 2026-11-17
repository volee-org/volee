//! Typing pending messages into idle agents that a volee host holds.
//!
//! An agent volee launched hears its messages through its hooks (ADR 0006):
//! at the end of a turn, and, while it rests, through a hook that waits for
//! them up to `[agents] wake_timeout` (an hour by default). Past that, a
//! message for an agent still at its prompt had nobody to hand it over: it
//! stayed pending for as long as the agent rested, for days at times (found
//! on 2026-11-17, a question to an agent idle since the 12th). volee holds
//! the terminal of an agent in a volee host, so it types the messages at its
//! prompt, as a person would, once the agent has rested a while, no hook
//! waits for its messages, and nobody types into it: the host says whether a
//! client has its input. A host started before volee 0.18 cannot say it, and
//! is left alone: its agent hears its messages at the end of its next turn.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use volee_core::harness::HarnessKind;
use volee_core::state::AgentState;
use volee_core::time::Timestamp;
use volee_core::{Agent, AgentId, EventKind, MessageId};
use volee_host::proto::KeyPress;
use volee_host::{Connection, Role, Size};

use crate::server::Shared;

/// How long an agent has rested before volee types its messages. Its hook
/// registers its waiter within a second of its turn's end; the margin keeps
/// volee from typing over a turn that just ended or a person who just left.
pub(crate) const REST: Duration = Duration::from_secs(20);

/// How often the agents with pending messages are looked at, besides the
/// events that post one.
const SWEEP_EVERY: Duration = Duration::from_secs(10);

/// How long an attempt that did not type waits before the same agent is
/// tried again, unless a new message arrives for it.
const RETRY_AFTER: Duration = Duration::from_secs(60);

/// How long the program gets to take a key or a paste in.
const SETTLE: Duration = Duration::from_millis(400);

/// Who volee is, in the host's log.
const CLIENT: &str = "volee daemon (messages)";

/// Starts the typist.
pub(crate) fn spawn(shared: &Arc<Shared>) -> JoinHandle<()> {
    tokio::spawn(run(Arc::clone(shared)))
}

/// Typings in progress and recent attempts that did not type.
#[derive(Debug, Default)]
struct Book {
    typing: HashSet<AgentId>,
    skipped: HashMap<AgentId, Instant>,
}

async fn run(shared: Arc<Shared>) {
    let book = Arc::new(Mutex::new(Book::default()));
    let mut events = shared.state.lock().await.subscribe();
    let mut sweep = tokio::time::interval(SWEEP_EVERY);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            received = events.recv() => match received {
                Ok(event) => {
                    if let EventKind::MessagePosted { message } = &event.event.kind {
                        // A new message gives an agent that was skipped a
                        // new chance at once.
                        {
                            let mut kept = book.lock().unwrap_or_else(PoisonError::into_inner);
                            for receipt in &message.receipts {
                                if let volee_core::message::Party::Agent(agent) = receipt.to {
                                    kept.skipped.remove(&agent);
                                }
                            }
                        }
                        type_pending(&shared, &book).await;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => type_pending(&shared, &book).await,
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = sweep.tick() => type_pending(&shared, &book).await,
        }
    }
}

/// Whether volee types `agent`'s messages now: `pending` of them wait, it is
/// Claude Code in a volee host, it has rested at its prompt for [`REST`] at
/// least, and no hook waits for its messages (`waits`).
pub(crate) fn ready(agent: &Agent, pending: usize, waits: bool, now: Timestamp) -> bool {
    let rest = u64::try_from(REST.as_millis()).unwrap_or(u64::MAX);
    pending > 0
        && !waits
        && agent.harness == HarnessKind::ClaudeCode
        && agent.location.host().is_some()
        && matches!(agent.state, AgentState::Idle | AgentState::Done)
        && agent.state_since.millis_until(now) >= rest
}

/// Types the pending messages of every agent that is ready for it.
async fn type_pending(shared: &Arc<Shared>, book: &Arc<Mutex<Book>>) {
    let ready: Vec<(AgentId, String)> = {
        let state = shared.state.lock().await;
        let now = state.now();
        let registry = state.registry();
        registry
            .agents()
            .filter(|agent| {
                ready(
                    agent,
                    registry.pending_for(agent.id).len(),
                    state.waits(agent.id),
                    now,
                )
            })
            .filter_map(|agent| Some((agent.id, agent.location.host()?.to_owned())))
            .collect()
    };
    for (agent, host) in ready {
        {
            let mut book = book.lock().unwrap_or_else(PoisonError::into_inner);
            let recent = book
                .skipped
                .get(&agent)
                .is_some_and(|at| at.elapsed() < RETRY_AFTER);
            if recent || !book.typing.insert(agent) {
                continue;
            }
        }
        let shared = Arc::clone(shared);
        let book = Arc::clone(book);
        tokio::spawn(async move {
            let outcome = deliver(&shared, agent, &host).await;
            let mut book = book.lock().unwrap_or_else(PoisonError::into_inner);
            book.typing.remove(&agent);
            match outcome {
                Ok(()) => {
                    book.skipped.remove(&agent);
                }
                Err(reason) => {
                    tracing::info!(agent = %agent, %reason, "messages left for the next turn");
                    book.skipped.insert(agent, Instant::now());
                }
            }
        });
    }
}

/// Types `agent`'s pending messages into its host, then marks delivered the
/// ones that are still pending.
async fn deliver(shared: &Arc<Shared>, agent: AgentId, host: &str) -> Result<(), String> {
    let Some((text, receipts)) = shared.state.lock().await.render_pending(agent) else {
        return Ok(());
    };
    let socket = crate::launch::hosts_dir(shared).join(format!("{host}.sock"));
    tokio::task::spawn_blocking(move || type_into(&socket, &text))
        .await
        .map_err(|error| error.to_string())??;
    let mut state = shared.state.lock().await;
    let still: HashSet<MessageId> = state
        .registry()
        .pending_for(agent)
        .iter()
        .map(|message| message.id)
        .collect();
    let receipts: Vec<_> = receipts
        .into_iter()
        .filter(|event| {
            matches!(&event.kind, EventKind::MessageDelivered { message, .. } if still.contains(message))
        })
        .collect();
    state
        .commit_only(receipts)
        .map_err(|error| error.to_string())?;
    tracing::info!(agent = %agent, "messages typed at the prompt of a resting agent");
    Ok(())
}

/// Types `text` at the prompt of the program in the host at `socket`, then
/// Enter, unless the program ended, the host cannot encode a paste or say
/// whether someone types into it, or someone does.
fn type_into(socket: &Path, text: &str) -> Result<(), String> {
    let (welcome, screen) = volee_host::snapshot(socket, CLIENT, Duration::from_secs(2))
        .map_err(|error| format!("its host does not answer: {error}"))?;
    if welcome.exit.is_some() {
        return Err("its program ended".to_owned());
    }
    if !welcome.keys {
        return Err("its host cannot take a paste".to_owned());
    }
    match welcome.owned {
        Some(false) => {}
        Some(true) => return Err("someone types into it".to_owned()),
        None => {
            return Err(
                "its host is older than volee 0.18 and cannot say whether someone types into it"
                    .to_owned(),
            );
        }
    }
    let normal = vim_normal_mode(welcome.size, &screen);
    let typing = Connection::open(socket, Role::Control, welcome.size, CLIENT)
        .map_err(|error| format!("its host does not answer: {error}"))?;
    if !typing.welcome.input {
        return Err("its host kept its input".to_owned());
    }
    let sender = typing
        .sender()
        .map_err(|error| format!("cannot write to its host: {error}"))?;
    let press = |key: &str| KeyPress {
        key: key.to_owned(),
        shift: false,
        alt: false,
        ctrl: false,
    };
    let written = (|| {
        if normal {
            // Claude Code's vim mode left in normal mode: insert first.
            sender.key(&press("i"))?;
            std::thread::sleep(SETTLE);
        }
        sender.paste(text)?;
        std::thread::sleep(SETTLE);
        sender.key(&press("enter"))?;
        std::thread::sleep(SETTLE);
        Ok::<(), std::io::Error>(())
    })();
    sender.close();
    written.map_err(|error| format!("cannot write to its host: {error}"))
}

/// Whether the last rows of a screen show Claude Code's vim mode in normal
/// mode.
fn vim_normal_mode(size: Size, screen: &[u8]) -> bool {
    let Ok(mut parsed) = volee_term::Screen::new(size.cols, size.rows, 0) else {
        return false;
    };
    parsed.feed(screen);
    parsed.text().is_ok_and(|text| {
        text.lines()
            .rev()
            .filter(|line| !line.trim().is_empty())
            .take(8)
            .any(|line| line.contains("-- NORMAL --"))
    })
}

#[cfg(test)]
mod tests {
    use ulid::Ulid;
    use volee_core::role::{LaunchedBy, Role};
    use volee_core::workspace::{Location, Workspace};
    use volee_core::{Event, Registry};

    use super::*;

    /// One agent of Claude Code at `location`, in `state` since `since`.
    fn agent(state: AgentState, since: i64, location: Location) -> Agent {
        let id = AgentId::from_ulid(Ulid::from_parts(1_000, 1));
        let events = [
            Event::new(
                Timestamp::from_millis(0),
                EventKind::AgentRegistered {
                    agent: id,
                    name: "demo-square".into(),
                    role: Role::Independent,
                    harness: HarnessKind::ClaudeCode,
                    launched_by: LaunchedBy::Human,
                    parent: None,
                    mission: None,
                    workspace: Workspace::at("/work/alpha"),
                    location,
                },
            ),
            Event::new(
                Timestamp::from_millis(since),
                EventKind::StateChanged {
                    agent: id,
                    state,
                    detail: None,
                },
            ),
        ];
        Registry::from_events(&events)
            .unwrap()
            .agent(id)
            .unwrap()
            .clone()
    }

    fn host() -> Location {
        Location::Host { id: "h1".into() }
    }

    #[test]
    fn a_resting_agent_in_a_host_gets_its_messages_typed() {
        let now = Timestamp::from_millis(1_000_000);
        let rested = agent(AgentState::Idle, 1_000_000 - 21_000, host());
        assert!(ready(&rested, 1, false, now));
        // Done is at its prompt too.
        let done = agent(AgentState::Done, 1, host());
        assert!(ready(&done, 2, false, now));
    }

    #[test]
    fn nothing_is_typed_while_a_hook_waits_or_the_agent_works_or_just_stopped() {
        let now = Timestamp::from_millis(1_000_000);
        let rested = agent(AgentState::Idle, 1, host());
        assert!(!ready(&rested, 0, false, now), "nothing pending");
        assert!(!ready(&rested, 1, true, now), "its hook waits for them");
        let working = agent(AgentState::Working, 1, host());
        assert!(!ready(&working, 1, false, now));
        let just = agent(AgentState::Idle, 1_000_000 - 5_000, host());
        assert!(!ready(&just, 1, false, now), "rested five seconds only");
    }

    #[test]
    fn nothing_is_typed_into_what_volee_does_not_hold() {
        let now = Timestamp::from_millis(1_000_000);
        for location in [Location::Terminal, Location::Background] {
            let agent = agent(AgentState::Idle, 1, location);
            assert!(!ready(&agent, 1, false, now));
        }
    }

    #[test]
    fn the_vim_normal_mode_is_read_on_the_last_rows() {
        let size = Size { cols: 40, rows: 6 };
        let normal = b"\x1b[1;1H> \x1b[6;1H  -- NORMAL --";
        assert!(vim_normal_mode(size, normal));
        let insert = b"\x1b[1;1H> \x1b[6;1H  -- INSERT --";
        assert!(!vim_normal_mode(size, insert));
        assert!(!vim_normal_mode(size, b"\x1b[1;1H> hello"));
    }
}
