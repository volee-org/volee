//! Agents of other harnesses that join through volee's MCP server
//! (ADR 0014).
//!
//! `volee mcp --join <name>` calls `agents.join` on its connection to the
//! daemon. The daemon finds the agent with that name, harness and directory
//! again, or registers it (launched outside volee, its harness named, role
//! independent), and records it idle. The agent is alive while a connection
//! that joined for it stays open: when the last one closes, it is stopped.
//! Its MCP server reports each tool call as its activity; no hooks report on
//! it, so messages reach it when it reads them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use volee_core::harness::HarnessKind;
use volee_core::message::{MessageKind, Party};
use volee_core::mission::MissionStatus;
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::AgentState;
use volee_core::supervisor::Notice;
use volee_core::workspace::{Location, Workspace};
use volee_core::{AgentId, Event, EventKind, MissionId};
use volee_proto::methods::{ActivityParams, Empty, JoinParams, Joined};

use crate::error::DaemonError;
use crate::server::Shared;
use crate::state::State;

/// The longest agent name volee accepts from a join.
const MAX_NAME: usize = 64;

/// The longest harness name volee accepts from a join.
const MAX_HARNESS: usize = 32;

/// The longest activity line volee keeps.
const MAX_ACTIVITY: usize = 200;

/// The detail of a joined agent's state while its MCP server runs.
const JOINED: &str = "joined through volee's MCP server";

/// The detail once its MCP server is gone.
const LEFT: &str = "its MCP server disconnected; it is back when it joins again";

/// The open connections that joined, by agent.
#[derive(Debug, Default)]
pub(crate) struct Presence(Mutex<HashMap<AgentId, usize>>);

impl Presence {
    fn arrive(&self, agent: AgentId) {
        *self.lock().entry(agent).or_default() += 1;
    }

    /// One connection that joined for `agent` closed: whether it was the
    /// last one.
    fn leave(&self, agent: AgentId) -> bool {
        let mut open = self.lock();
        match open.get_mut(&agent) {
            Some(count) if *count > 1 => {
                *count -= 1;
                false
            }
            Some(_) => {
                open.remove(&agent);
                true
            }
            None => false,
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<AgentId, usize>> {
        // A map of counters is valid after any statement: a poisoned lock
        // still holds a usable one.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// `agents.join`: finds the agent again or registers it, records it idle,
/// and counts the connection, which the caller releases with [`left`] when
/// it closes.
pub(crate) async fn join(shared: &Arc<Shared>, params: JoinParams) -> Result<Joined, DaemonError> {
    let name = checked_name(&params.name)?;
    let harness = HarnessKind::Other(checked_harness(&params.harness)?);
    if !params.cwd.is_absolute() {
        return Err(DaemonError::InvalidParams(format!(
            "the directory must be absolute, not {}",
            params.cwd.display()
        )));
    }
    let mut state = shared.state.lock().await;
    if let Some(mission) = params.mission {
        open_mission(&state, mission)?;
    }
    let now = state.now();
    let found = state
        .registry()
        .agents()
        .find(|agent| {
            agent.joined()
                && agent.name == name
                && agent.harness == harness
                && agent.workspace.cwd == params.cwd
        })
        .map(|agent| (agent.id, agent.mission, agent.state.is_live()));
    let mut events = Vec::new();
    let (agent, rejoined, previous_mission) = if let Some((agent, mission, live)) = found {
        if mission != params.mission {
            events.push(EventKind::MembershipChanged {
                agent,
                mission: params.mission,
            });
        }
        if !live {
            events.push(idle(agent));
        }
        (agent, true, mission)
    } else {
        let agent = state.new_agent_id();
        events.push(EventKind::AgentRegistered {
            agent,
            name: name.clone(),
            role: Role::Independent,
            harness: harness.clone(),
            launched_by: LaunchedBy::Outside,
            parent: None,
            mission: params.mission,
            workspace: Workspace::at(&params.cwd),
            location: Location::Unknown,
        });
        events.push(idle(agent));
        (agent, false, None)
    };
    let mut events: Vec<Event> = events
        .into_iter()
        .map(|kind| Event::new(now, kind))
        .collect();
    if let Some(mission) = params.mission
        && previous_mission != Some(mission)
    {
        events.extend(member_notice(&state, mission, &name, &harness));
    }
    state.commit(events)?;
    shared.presence.arrive(agent);
    tracing::info!(%agent, name, %harness, rejoined, "an agent joined through MCP");
    Ok(Joined { agent, rejoined })
}

/// `agents.activity`: records what a joined agent just did.
pub(crate) async fn activity(
    shared: &Arc<Shared>,
    params: ActivityParams,
) -> Result<Empty, DaemonError> {
    let mut state = shared.state.lock().await;
    let agent = state
        .registry()
        .agent(params.agent)
        .ok_or_else(|| DaemonError::NotFound(format!("unknown agent {}", params.agent)))?;
    if !agent.joined() {
        return Err(DaemonError::Refused(
            "only agents that joined through volee's MCP server report their own activity".into(),
        ));
    }
    let summary = one_line(&params.activity, MAX_ACTIVITY);
    if summary.is_empty() {
        return Ok(Empty {});
    }
    let now = state.now();
    state.commit(vec![Event::new(
        now,
        EventKind::ActivityRecorded {
            agent: params.agent,
            summary,
        },
    )])?;
    Ok(Empty {})
}

/// The connection that joined for `agents` closed: each agent whose last
/// connection it was is stopped.
pub(crate) async fn left(shared: &Arc<Shared>, agents: &[AgentId]) {
    let gone: Vec<AgentId> = agents
        .iter()
        .copied()
        .filter(|agent| shared.presence.leave(*agent))
        .collect();
    if gone.is_empty() {
        return;
    }
    let mut state = shared.state.lock().await;
    let events = stopped(&state, gone, LEFT);
    if !events.is_empty()
        && let Err(error) = state.commit(events)
    {
        tracing::warn!(%error, "cannot record that joined agents left");
    }
}

/// When the daemon starts, no connection is open yet: every joined agent
/// still recorded as alive is stopped until its MCP server joins again.
pub(crate) fn settle_at_start(state: &mut State) -> Result<usize, DaemonError> {
    let alive: Vec<AgentId> = state
        .registry()
        .agents()
        .filter(|agent| agent.joined() && agent.state.is_live())
        .map(|agent| agent.id)
        .collect();
    let events = stopped(state, alive, LEFT);
    let count = events.len();
    if count > 0 {
        state.commit(events)?;
    }
    Ok(count)
}

/// `agents`, still known and alive, stopped with `detail`.
fn stopped(state: &State, agents: Vec<AgentId>, detail: &str) -> Vec<Event> {
    let now = state.now();
    agents
        .into_iter()
        .filter(|agent| {
            state
                .registry()
                .agent(*agent)
                .is_some_and(|record| record.state.is_live())
        })
        .map(|agent| {
            Event::new(
                now,
                EventKind::StateChanged {
                    agent,
                    state: AgentState::Stopped,
                    detail: Some(detail.to_owned()),
                },
            )
        })
        .collect()
}

fn idle(agent: AgentId) -> EventKind {
    EventKind::StateChanged {
        agent,
        state: AgentState::Idle,
        detail: Some(JOINED.to_owned()),
    }
}

/// Refuses a mission that does not exist or is over.
fn open_mission(state: &State, mission: MissionId) -> Result<(), DaemonError> {
    let record = state
        .registry()
        .mission(mission)
        .ok_or_else(|| DaemonError::NotFound(format!("unknown mission {mission}")))?;
    if matches!(
        record.status,
        MissionStatus::Closed | MissionStatus::Aborted
    ) {
        return Err(DaemonError::Refused(format!(
            "mission {} is over: join an open mission, or none",
            record.title
        )));
    }
    Ok(())
}

/// Tells the mission's coordinator that a member joined.
fn member_notice(
    state: &State,
    mission: MissionId,
    name: &str,
    harness: &HarnessKind,
) -> Option<Event> {
    let coordinator = state.registry().mission(mission)?.coordinator?;
    let notice = Notice {
        to: vec![Party::Agent(coordinator)],
        kind: MessageKind::Note,
        body: format!(
            "{name} ({harness}) joined this mission through volee's MCP server. Mission members \
             can write to it; it reads its messages when it calls read_messages."
        ),
    };
    let message = crate::missions::notice_message(state, mission, notice)?;
    Some(Event::new(
        state.now(),
        EventKind::MessagePosted { message },
    ))
}

/// A name people and agents can type: one line, not empty, not too long.
fn checked_name(name: &str) -> Result<String, DaemonError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(DaemonError::InvalidParams(format!(
            "an agent's name has 1 to {MAX_NAME} characters, on one line"
        )));
    }
    Ok(name.to_owned())
}

/// A harness name such as `codex` or `gemini-cli`: lowercase letters,
/// digits, `.`, `_` and `-`. `claude-code` is taken: volee discovers those
/// sessions itself.
fn checked_harness(harness: &str) -> Result<String, DaemonError> {
    let valid = !harness.is_empty()
        && harness.len() <= MAX_HARNESS
        && harness
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if !valid {
        return Err(DaemonError::InvalidParams(format!(
            "a harness name has 1 to {MAX_HARNESS} lowercase letters, digits, `.`, `_` or `-`, \
             such as `codex` or `gemini-cli`"
        )));
    }
    if harness == HarnessKind::ClaudeCode.to_string() {
        return Err(DaemonError::Refused(
            "volee finds Claude Code sessions on its own: Claude Code agents do not join".into(),
        ));
    }
    Ok(harness.to_owned())
}

/// `text` on one line, without control characters, at most `max`
/// characters.
fn one_line(text: &str, max: usize) -> String {
    let line: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    line.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connection_count_reaches_zero_once() {
        let presence = Presence::default();
        let agent = AgentId::from_ulid(ulid::Ulid::from_parts(1, 1));
        presence.arrive(agent);
        presence.arrive(agent);
        assert!(!presence.leave(agent), "one connection is still open");
        assert!(presence.leave(agent), "the last one closed");
        assert!(!presence.leave(agent), "nothing left to close");
    }

    #[test]
    fn names_and_harnesses_are_checked() {
        assert_eq!(checked_name("  infra ").unwrap(), "infra");
        assert!(checked_name("").is_err());
        assert!(checked_name("two\nlines").is_err());
        assert!(checked_name(&"x".repeat(MAX_NAME + 1)).is_err());
        assert_eq!(checked_harness("gemini-cli").unwrap(), "gemini-cli");
        assert!(checked_harness("Codex").is_err());
        assert!(checked_harness("codex cli").is_err());
        assert!(checked_harness("").is_err());
        assert!(matches!(
            checked_harness("claude-code"),
            Err(DaemonError::Refused(_))
        ));
    }

    #[test]
    fn activity_is_one_short_line() {
        assert_eq!(
            one_line("  send_message\n to   infra ", 200),
            "send_message to infra"
        );
        assert_eq!(one_line("abcdef", 3), "abc");
        assert_eq!(one_line("\u{7}", 10), "");
    }
}
