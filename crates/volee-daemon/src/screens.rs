//! Agents of harnesses that report no state (ADR 0022), and the agents
//! started elsewhere that no harness lists: OpenCode in the process table
//! (ADR 0023), and the panes a multiplexer says run another harness
//! (ADR 0030).
//!
//! An agent in a host whose harness has no hooks gets its state from its
//! screen, the last resort of ADR 0022: a screen that changed since the last
//! round is working, one that stayed the same for a while is idle, and a host
//! that recorded an end, or is gone, means stopped.
//!
//! OpenCode agents that volee does not host are found in the process table
//! and reconciled like any listing. A process that is gone stops its agent.
//! So are the agents a multiplexer recognizes in its panes, each keyed by
//! the process in the foreground of its pane.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use volee_core::harness::{HarnessKind, SessionKey};
use volee_core::reconcile::{ListedSession, Listing, gone, reconcile};
use volee_core::state::AgentState;
use volee_core::time::Timestamp;
use volee_core::{AgentId, Event, EventKind, Registry};

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

/// One round: the screens of hosted agents without hooks, then the agents
/// of the process table and of the multiplexers' panes. Returns the live
/// sessions they gave, for the search of the panes they may run in; `None`
/// when the table could not be read, in which case nothing can be said of
/// where they run.
pub(crate) async fn round(shared: &Arc<Shared>) -> Option<Vec<volee_mux::Hosted>> {
    hosted_states(shared).await;
    found_elsewhere(shared).await
}

/// One hosted agent, as a round sees it.
struct Hosted {
    agent: AgentId,
    host: String,
    state: AgentState,
    /// Stopped with no reason given, as a listing may have stopped it before
    /// its host was read: the host's end gives the reason.
    unexplained_stop: bool,
    followed: Followed,
    held: bool,
}

/// What gives the states of a hosted agent.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Followed {
    /// The hooks of its harness (Claude Code): the host only says whether it
    /// runs.
    ByHooks,
    /// Its screen: its harness reports no state.
    OnScreen,
    /// Its screen too, for a program started with `volee run`: once it
    /// ended, nothing brings it back, and its row leaves the list.
    OnScreenUntilItEnds,
}

impl Followed {
    fn of(agent: &volee_core::Agent) -> Self {
        if agent.harness == HarnessKind::ClaudeCode {
            Self::ByHooks
        } else if agent.generic() {
            Self::OnScreenUntilItEnds
        } else {
            Self::OnScreen
        }
    }
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
            .filter(|agent| agent.state != AgentState::Failed)
            .filter_map(|agent| {
                Some(Hosted {
                    agent: agent.id,
                    host: agent.location.host()?.to_owned(),
                    state: agent.state.clone(),
                    unexplained_stop: agent.state == AgentState::Stopped && agent.detail.is_none(),
                    followed: Followed::of(agent),
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
        unexplained_stop,
        followed,
        held,
    } in hosted
    {
        let socket = dir.join(format!("{host}.sock"));
        let read = tokio::task::spawn_blocking(move || {
            volee_host::snapshot(&socket, "volee daemon screens", Duration::from_secs(2))
        })
        .await;
        let running = current.is_live() || current == AgentState::Launching;
        // A stop that came without a reason gets the one its host recorded.
        let mut explain = false;
        let (next, detail) = if let Ok(Ok((welcome, screen))) = read {
            shared.screens.answered(agent);
            if let Some(exit) = &welcome.exit {
                if !running && !unexplained_stop {
                    if followed == Followed::OnScreenUntilItEnds {
                        expire(shared, agent).await;
                    }
                    continue;
                }
                explain = !running;
                (
                    AgentState::Stopped,
                    Some(ended(exit, welcome.size, &screen)),
                )
            } else if !running && !held {
                // Its host still runs it: it never stopped.
                (
                    AgentState::Idle,
                    Some("its volee host still runs it".to_owned()),
                )
            } else if followed == Followed::ByHooks {
                continue;
            } else {
                match shared.screens.changed(agent, &screen) {
                    Some(true) => (AgentState::Working, None),
                    Some(false) => (AgentState::Idle, None),
                    None => continue,
                }
            }
        } else {
            if !running {
                if followed == Followed::OnScreenUntilItEnds {
                    expire(shared, agent).await;
                }
                continue;
            }
            if !shared.screens.missed(agent) {
                continue;
            }
            (
                AgentState::Stopped,
                Some("its volee host is gone".to_owned()),
            )
        };
        if explain || next.category() != current.category() {
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

/// Takes the agent of a program started with `volee run` out of the list
/// once its program has been ended for a while: nothing resumes it, so there
/// is nothing left to do with its row. It stays long enough for its end to
/// be seen, like a session volee only watched (ADR 0027).
async fn expire(shared: &Arc<Shared>, agent: AgentId) {
    let mut state = shared.state.lock().await;
    let now = state.now();
    let expired = state
        .registry()
        .agent(agent)
        .filter(|record| record.generic() && !record.state.is_live())
        .and_then(|record| gone(record, now));
    if let Some(event) = expired
        && let Err(error) = state.commit(vec![event])
    {
        tracing::error!(%error, "cannot take an ended program out of the list");
    }
}

/// The detail of an agent whose hosted program ended. A program that failed
/// gets its status and the last line it printed, so that its error (Claude
/// Code refusing a repository it does not trust yet, for example) shows
/// without taking the host over.
fn ended(exit: &volee_host::Exit, size: volee_host::Size, screen: &[u8]) -> String {
    let status = match (exit.code, &exit.signal) {
        (_, Some(signal)) => format!("signal {signal}"),
        (Some(code), None) if code != 0 => format!("exit {code}"),
        _ => return "its program ended".to_owned(),
    };
    match last_line(size, screen) {
        Some(line) => format!("its program failed ({status}): {line}"),
        None => format!("its program failed ({status})"),
    }
}

/// The last line of a host's screen that is not blank, at most 300
/// characters long.
fn last_line(size: volee_host::Size, screen: &[u8]) -> Option<String> {
    let mut parsed = volee_term::Screen::new(size.cols, size.rows, 0).ok()?;
    parsed.feed(screen);
    let text = parsed.text().ok()?;
    let line = text.lines().map(str::trim).rfind(|line| !line.is_empty())?;
    Some(line.chars().take(300).collect())
}

/// The agents started elsewhere that no harness lists for volee: OpenCode
/// in the process table (ADR 0023), and the panes a multiplexer says run a
/// harness volee has no listing for (ADR 0030). Each source is a listing of
/// its own harness, never complete; an agent whose process a source that
/// answered no longer names has ended.
async fn found_elsewhere(shared: &Arc<Shared>) -> Option<Vec<volee_mux::Hosted>> {
    // The process table. Without one (a test), nothing is said of OpenCode.
    let table = match shared.processes.clone() {
        None => None,
        Some(runner) => {
            let listed = tokio::task::spawn_blocking(move || {
                volee_harness::opencode::list_processes(&*runner)
            })
            .await;
            let Ok(Ok(listing)) = listed else {
                return None;
            };
            Some(listing)
        }
    };
    let table_read = table.is_some();
    // The panes. A multiplexer that could not tell says nothing of the
    // agents it named before: they stay as they are.
    let in_panes = crate::panes::agents(shared).await;
    let mut listings: Vec<Listing> = table.into_iter().collect();
    listings.extend(pane_listings(in_panes.as_deref().unwrap_or_default()));
    // A process that runs in the host of an agent started with `volee run`
    // is that agent (ADR 0029), whatever the name of the volee program that
    // started the host.
    let in_hosts = crate::launch::run_programs(shared).await;
    if !in_hosts.is_empty() {
        for listing in &mut listings {
            listing
                .sessions
                .retain(|session| session.pid.is_none_or(|pid| !in_hosts.contains(&pid)));
        }
    }
    let live: Vec<volee_mux::Hosted> = listings
        .iter()
        .flat_map(|listing| crate::panes::hosted(&listing.sessions))
        .collect();
    let mut state = shared.state.lock().await;
    let now = state.now();
    if in_panes.is_some() {
        listings.extend(emptied(state.registry(), &listings));
    }
    let mut events = Vec::new();
    for listing in &listings {
        state.sessions_mut().record(listing);
        events.extend(reconcile(state.registry(), listing, now, || {
            state.new_agent_id()
        }));
    }
    events.extend(no_longer_named(
        state.registry(),
        &listings,
        table_read,
        in_panes.is_some(),
        now,
    ));
    if !events.is_empty()
        && let Err(error) = state.commit(events)
    {
        tracing::error!(%error, "cannot apply the agents found elsewhere");
    }
    Some(live)
}

/// The agents multiplexers recognize in their panes, as listings: one per
/// harness, each agent keyed by its process, like an OpenCode agent found
/// without a session id. volee reads no state for them: they are idle while
/// they run.
fn pane_listings(agents: &[volee_mux::PaneAgent]) -> Vec<Listing> {
    let mut by_harness: BTreeMap<&str, Vec<ListedSession>> = BTreeMap::new();
    for agent in agents {
        let key = SessionKey {
            harness: HarnessKind::Other(agent.harness.clone()),
            id: format!("pid-{}", agent.pid),
        };
        by_harness
            .entry(agent.harness.as_str())
            .or_default()
            .push(ListedSession {
                alive: true,
                pid: Some(agent.pid),
                ..ListedSession::new(key, agent.cwd.clone(), AgentState::Idle)
            });
    }
    by_harness
        .into_iter()
        .map(|(harness, sessions)| Listing {
            harness: HarnessKind::Other(harness.to_owned()),
            sessions,
            complete: false,
        })
        .collect()
}

/// An empty listing for each harness whose agents multiplexers named, and
/// name no more: a harness whose last agent is gone has no listing this
/// round, and its empty one replaces what the last round recorded.
fn emptied(registry: &Registry, listings: &[Listing]) -> Vec<Listing> {
    let opencode = volee_harness::opencode::kind();
    let named: BTreeSet<&str> = registry
        .agents()
        .filter(|agent| agent.watched() && agent.harness != opencode)
        .filter(|agent| {
            !listings
                .iter()
                .any(|listing| listing.harness == agent.harness)
        })
        .filter_map(|agent| match &agent.harness {
            HarnessKind::Other(harness) => Some(harness.as_str()),
            HarnessKind::ClaudeCode => None,
        })
        .collect();
    named
        .into_iter()
        .map(|harness| Listing {
            harness: HarnessKind::Other(harness.to_owned()),
            sessions: Vec::new(),
            complete: false,
        })
        .collect()
}

/// What happens to the agents started elsewhere whose process no listing
/// names any more. The listings are never complete: such an agent stops
/// here, then leaves the list like any agent volee only watches (ADR 0027).
/// A source that did not answer says nothing of its agents: the process
/// table for OpenCode (`table_read`), the multiplexers for the harnesses
/// they recognize in their panes (`panes_read`).
fn no_longer_named(
    registry: &Registry,
    listings: &[Listing],
    table_read: bool,
    panes_read: bool,
    now: Timestamp,
) -> Vec<Event> {
    let opencode = volee_harness::opencode::kind();
    let running: HashSet<&SessionKey> = listings
        .iter()
        .flat_map(|listing| &listing.sessions)
        .map(|session| &session.key)
        .collect();
    let mut events = Vec::new();
    for agent in registry.agents() {
        // What names the agents of this harness, and whether it answered.
        let asked = match &agent.harness {
            HarnessKind::ClaudeCode => false,
            harness if *harness == opencode => table_read,
            HarnessKind::Other(_) => panes_read,
        };
        let absent = asked
            && agent.watched()
            && agent
                .current_session()
                .is_some_and(|link| !running.contains(&link.key));
        if !absent {
            continue;
        }
        if agent.state.is_live() {
            events.push(Event::new(
                now,
                EventKind::StateChanged {
                    agent: agent.id,
                    state: AgentState::Stopped,
                    detail: Some("its process ended".to_owned()),
                },
            ));
        } else {
            events.extend(gone(agent, now));
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: volee_host::Size = volee_host::Size { cols: 80, rows: 6 };

    fn exit(code: Option<u32>, signal: Option<&str>) -> volee_host::Exit {
        volee_host::Exit {
            code,
            signal: signal.map(str::to_owned),
            at: 0,
        }
    }

    #[test]
    fn a_failed_program_shows_its_last_line() {
        let screen =
            b"\x1b[2J\x1b[HError creating worktree: Workspace trust not yet accepted.\r\n\r\n";
        assert_eq!(
            ended(&exit(Some(1), None), SIZE, screen),
            "its program failed (exit 1): Error creating worktree: Workspace trust not yet accepted."
        );
        assert_eq!(
            ended(&exit(None, Some("SIGKILL")), SIZE, b""),
            "its program failed (signal SIGKILL)"
        );
    }

    #[test]
    fn a_program_that_exits_cleanly_just_ended() {
        assert_eq!(
            ended(&exit(Some(0), None), SIZE, b"Goodbye\r\n"),
            "its program ended"
        );
    }
}
