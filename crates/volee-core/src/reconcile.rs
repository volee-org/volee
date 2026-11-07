//! Reconciliation of a harness's session listing with the registry
//! (ADR 0004).
//!
//! A harness lists its sessions (for Claude Code, `claude agents --json
//! --all`). This module turns one listing into the events that bring the
//! registry in line with it. It is pure: the daemon runs the command, maps its
//! output to a [`Listing`], and persists the returned events.

use std::path::PathBuf;

use crate::agent::Agent;
use crate::event::{Event, EventKind};
use crate::harness::{HarnessKind, SessionKey, SessionOrigin};
use crate::ids::AgentId;
use crate::registry::Registry;
use crate::role::{LaunchedBy, Role};
use crate::state::AgentState;
use crate::time::Timestamp;
use crate::workspace::{Location, Workspace};

/// One session as a harness lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedSession {
    /// The session.
    pub key: SessionKey,
    /// Its display name, when it has one.
    pub name: Option<String>,
    /// Its working directory.
    pub cwd: PathBuf,
    /// Its state, mapped by the harness adapter.
    pub state: AgentState,
    /// Whether its process is alive.
    pub alive: bool,
    /// Whether it runs in the harness's background supervisor (as opposed to
    /// a terminal).
    pub background: bool,
}

/// Every session a harness reports at one point in time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    /// The harness that produced the listing.
    pub harness: HarnessKind,
    /// The sessions.
    pub sessions: Vec<ListedSession>,
    /// Whether finished sessions are included. Only a complete listing can
    /// prove that a session is gone.
    pub complete: bool,
}

/// Returns the events that bring `registry` in line with `listing`.
///
/// - A listed session that belongs to an agent updates the agent's state when
///   the coarse state differs. When both agree, the registry keeps its richer
///   state, which usually comes from hooks.
/// - A live listed session that nobody owns, running in the directory of an
///   agent volee launched and that has no session yet, is that agent's
///   session: the listing saw it before the session's first hook did.
/// - Any other live listed session that nobody owns becomes a new
///   `independent` agent. Finished sessions that nobody owns are history and
///   are skipped. Sessions of forgotten agents are always skipped.
/// - When a complete listing no longer contains an agent's current session,
///   the agent becomes `stopped` if it ran in a terminal (the session ended)
///   or `lost` if it ran in the background (the harness removed it).
pub fn reconcile(
    registry: &Registry,
    listing: &Listing,
    now: Timestamp,
    mut new_agent_id: impl FnMut() -> AgentId,
) -> Vec<Event> {
    let mut events = Vec::new();
    let mut adopted: Vec<AgentId> = Vec::new();
    for session in &listing.sessions {
        if registry.is_ignored(&session.key) {
            continue;
        }
        match registry.agent_for_session(&session.key) {
            Some(agent) => follow(agent, session, now, &mut events),
            None if session.alive => {
                if let Some(waiting) = awaiting_session(registry, session, &adopted) {
                    adopted.push(waiting.id);
                    adopt(waiting, session, now, &mut events);
                } else {
                    discover(new_agent_id(), session, now, &mut events);
                }
            }
            None => {}
        }
    }
    if listing.complete {
        vanished(registry, listing, now, &mut events);
    }
    events
}

/// A known agent's session is listed: the agent follows its coarse state.
fn follow(agent: &Agent, session: &ListedSession, now: Timestamp, events: &mut Vec<Event>) {
    let is_current = agent
        .current_session()
        .is_some_and(|link| link.key == session.key);
    if !is_current {
        return;
    }
    if agent.state.category() != session.state.category() {
        events.push(Event::new(
            now,
            EventKind::StateChanged {
                agent: agent.id,
                state: session.state.clone(),
                detail: None,
            },
        ));
    }
    if session.background && agent.location == Location::Unknown {
        events.push(Event::new(
            now,
            EventKind::LocationChanged {
                agent: agent.id,
                location: Location::Background,
            },
        ));
    }
}

/// A launched agent's first session, seen before its first hook.
fn adopt(agent: &Agent, session: &ListedSession, now: Timestamp, events: &mut Vec<Event>) {
    events.push(Event::new(
        now,
        EventKind::SessionLinked {
            agent: agent.id,
            session: session.key.clone(),
            origin: SessionOrigin::Started,
        },
    ));
    if agent.state.category() != session.state.category() {
        events.push(Event::new(
            now,
            EventKind::StateChanged {
                agent: agent.id,
                state: session.state.clone(),
                detail: None,
            },
        ));
    }
}

/// A live session nobody owns: a new independent agent.
fn discover(agent: AgentId, session: &ListedSession, now: Timestamp, events: &mut Vec<Event>) {
    let workspace = Workspace::at(&session.cwd);
    let name = session.name.clone().unwrap_or_else(|| workspace.label());
    events.push(Event::new(
        now,
        EventKind::AgentRegistered {
            agent,
            name,
            role: Role::Independent,
            harness: session.key.harness.clone(),
            launched_by: LaunchedBy::Outside,
            parent: None,
            mission: None,
            workspace,
            location: if session.background {
                Location::Background
            } else {
                Location::Terminal
            },
        },
    ));
    events.push(Event::new(
        now,
        EventKind::SessionLinked {
            agent,
            session: session.key.clone(),
            origin: SessionOrigin::Discovered,
        },
    ));
    events.push(Event::new(
        now,
        EventKind::StateChanged {
            agent,
            state: session.state.clone(),
            detail: None,
        },
    ));
}

/// Agents whose current session a complete listing no longer contains.
fn vanished(registry: &Registry, listing: &Listing, now: Timestamp, events: &mut Vec<Event>) {
    for agent in registry.agents() {
        if agent.harness != listing.harness {
            continue;
        }
        let Some(current) = agent.current_session() else {
            continue;
        };
        let listed = listing.sessions.iter().any(|s| s.key == current.key);
        if listed || matches!(agent.state, AgentState::Stopped | AgentState::Lost) {
            continue;
        }
        let (state, detail) = if agent.location == Location::Background {
            (AgentState::Lost, "the harness no longer lists this session")
        } else {
            (AgentState::Stopped, "the session ended")
        };
        events.push(Event::new(
            now,
            EventKind::StateChanged {
                agent: agent.id,
                state,
                detail: Some(detail.to_owned()),
            },
        ));
    }
}

/// The agent volee launched in `session`'s directory that still waits for
/// its first session, if any.
fn awaiting_session<'a>(
    registry: &'a Registry,
    session: &ListedSession,
    adopted: &[AgentId],
) -> Option<&'a Agent> {
    registry.agents().find(|agent| {
        agent.harness == session.key.harness
            && agent.launched_by != LaunchedBy::Outside
            && agent.current_session().is_none()
            && agent.state.is_live()
            && agent.workspace.cwd == session.cwd
            && !adopted.contains(&agent.id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::InputRequest;
    use ulid::Ulid;

    fn ids() -> impl FnMut() -> AgentId {
        let mut next = 0;
        move || {
            next += 1;
            AgentId::from_ulid(Ulid::from_parts(1_000, next))
        }
    }

    fn listed(id: &str, state: AgentState, alive: bool) -> ListedSession {
        ListedSession {
            key: SessionKey::claude(id),
            name: Some(format!("session {id}")),
            cwd: PathBuf::from("/work/alpha"),
            state,
            alive,
            background: true,
        }
    }

    fn listing(sessions: Vec<ListedSession>) -> Listing {
        Listing {
            harness: HarnessKind::ClaudeCode,
            sessions,
            complete: true,
        }
    }

    fn apply_all(registry: &mut Registry, events: &[Event]) {
        for event in events {
            registry.apply(event).unwrap();
        }
    }

    /// Reconciles, applies the events and returns them.
    fn sync(registry: &mut Registry, listing: &Listing, millis: i64) -> Vec<Event> {
        let events = reconcile(registry, listing, Timestamp::from_millis(millis), ids());
        apply_all(registry, &events);
        events
    }

    #[test]
    fn a_live_unknown_session_becomes_an_independent_agent() {
        let mut registry = Registry::new();
        let events = reconcile(
            &registry,
            &listing(vec![listed("aaaa1111", AgentState::Working, true)]),
            Timestamp::from_millis(10),
            ids(),
        );
        apply_all(&mut registry, &events);
        let agent = registry
            .agent_for_session(&SessionKey::claude("aaaa1111"))
            .unwrap();
        assert_eq!(agent.role, Role::Independent);
        assert_eq!(agent.state, AgentState::Working);
        assert_eq!(agent.location, Location::Background);
        assert_eq!(agent.name, "session aaaa1111");
    }

    #[test]
    fn finished_unknown_sessions_are_history() {
        let registry = Registry::new();
        let events = reconcile(
            &registry,
            &listing(vec![listed("old00000", AgentState::Done, false)]),
            Timestamp::from_millis(10),
            ids(),
        );
        assert_eq!(events, []);
    }

    #[test]
    fn reconciling_twice_changes_nothing() {
        let mut registry = Registry::new();
        let snapshot = listing(vec![listed("aaaa1111", AgentState::Idle, true)]);
        let first = reconcile(&registry, &snapshot, Timestamp::from_millis(1), ids());
        apply_all(&mut registry, &first);
        let second = reconcile(&registry, &snapshot, Timestamp::from_millis(2), ids());
        assert!(second.is_empty(), "{second:?}");
    }

    #[test]
    fn a_richer_state_of_the_same_category_is_kept() {
        let mut registry = Registry::new();
        let start = listing(vec![listed("aaaa1111", AgentState::Working, true)]);
        sync(&mut registry, &start, 1);
        let agent = registry
            .agent_for_session(&SessionKey::claude("aaaa1111"))
            .unwrap()
            .id;
        let rich = AgentState::NeedsInput {
            request: InputRequest::Permission {
                tool: Some("Bash".into()),
            },
        };
        registry
            .apply(&Event::new(
                Timestamp::from_millis(2),
                EventKind::StateChanged {
                    agent,
                    state: rich.clone(),
                    detail: Some("Bash(cargo test)".into()),
                },
            ))
            .unwrap();
        let coarse = listing(vec![listed(
            "aaaa1111",
            AgentState::NeedsInput {
                request: InputRequest::Permission { tool: None },
            },
            true,
        )]);
        let events = reconcile(&registry, &coarse, Timestamp::from_millis(3), ids());
        assert!(events.is_empty(), "{events:?}");
        assert_eq!(registry.agent(agent).unwrap().state, rich);
    }

    #[test]
    fn a_vanished_background_session_is_lost_not_done() {
        let mut registry = Registry::new();
        let start = listing(vec![listed("aaaa1111", AgentState::Working, true)]);
        sync(&mut registry, &start, 1);
        let events = reconcile(
            &registry,
            &listing(vec![]),
            Timestamp::from_millis(2),
            ids(),
        );
        apply_all(&mut registry, &events);
        let agent = registry
            .agent_for_session(&SessionKey::claude("aaaa1111"))
            .unwrap();
        assert_eq!(agent.state, AgentState::Lost);
        assert!(agent.detail.as_deref().unwrap().contains("no longer lists"));
    }

    #[test]
    fn a_vanished_terminal_session_is_stopped() {
        let mut registry = Registry::new();
        let mut session = listed("bbbb2222", AgentState::Working, true);
        session.background = false;
        sync(&mut registry, &listing(vec![session]), 1);
        sync(&mut registry, &listing(vec![]), 2);
        let agent = registry
            .agent_for_session(&SessionKey::claude("bbbb2222"))
            .unwrap();
        assert_eq!(agent.state, AgentState::Stopped);
    }

    #[test]
    fn an_incomplete_listing_never_marks_agents_lost() {
        let mut registry = Registry::new();
        let start = listing(vec![listed("aaaa1111", AgentState::Working, true)]);
        sync(&mut registry, &start, 1);
        let partial = Listing {
            complete: false,
            ..listing(vec![])
        };
        assert_eq!(
            reconcile(&registry, &partial, Timestamp::from_millis(2), ids()),
            []
        );
    }

    #[test]
    fn forgotten_sessions_never_come_back() {
        let mut registry = Registry::new();
        let snapshot = listing(vec![listed("aaaa1111", AgentState::Idle, true)]);
        sync(&mut registry, &snapshot, 1);
        let agent = registry
            .agent_for_session(&SessionKey::claude("aaaa1111"))
            .unwrap()
            .id;
        registry
            .apply(&Event::new(
                Timestamp::from_millis(2),
                EventKind::AgentForgotten { agent },
            ))
            .unwrap();
        assert_eq!(
            reconcile(&registry, &snapshot, Timestamp::from_millis(3), ids()),
            []
        );
    }

    #[test]
    fn only_the_current_session_drives_the_state() {
        let mut registry = Registry::new();
        sync(
            &mut registry,
            &listing(vec![listed("old11111", AgentState::Idle, true)]),
            1,
        );
        let agent = registry
            .agent_for_session(&SessionKey::claude("old11111"))
            .unwrap()
            .id;
        registry
            .apply(&Event::new(
                Timestamp::from_millis(2),
                EventKind::SessionLinked {
                    agent,
                    session: SessionKey::claude("new22222"),
                    origin: SessionOrigin::Forked,
                },
            ))
            .unwrap();
        let both = listing(vec![
            listed("old11111", AgentState::Stopped, false),
            listed("new22222", AgentState::Idle, true),
        ]);
        let events = reconcile(&registry, &both, Timestamp::from_millis(3), ids());
        assert!(
            events.is_empty(),
            "the old session must not stop the agent: {events:?}"
        );
    }

    fn launched(registry: &mut Registry, n: u128, cwd: &str) -> AgentId {
        let agent = AgentId::from_ulid(Ulid::from_parts(9_000, n));
        registry
            .apply(&Event::new(
                Timestamp::from_millis(1),
                EventKind::AgentRegistered {
                    agent,
                    name: format!("worker {n}"),
                    role: Role::Worker,
                    harness: HarnessKind::ClaudeCode,
                    launched_by: LaunchedBy::Human,
                    parent: None,
                    mission: None,
                    workspace: Workspace::at(cwd),
                    location: Location::Background,
                },
            ))
            .unwrap();
        agent
    }

    #[test]
    fn a_launched_agent_adopts_the_session_in_its_directory() {
        let mut registry = Registry::new();
        let agent = launched(&mut registry, 1, "/work/alpha");
        let events = reconcile(
            &registry,
            &listing(vec![listed("s-1", AgentState::Working, true)]),
            Timestamp::from_millis(5),
            ids(),
        );
        assert!(
            events
                .iter()
                .all(|event| !matches!(event.kind, EventKind::AgentRegistered { .. }))
        );
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            EventKind::SessionLinked { agent: linked, origin: SessionOrigin::Started, .. }
                if *linked == agent
        )));
        for event in &events {
            registry.apply(event).unwrap();
        }
        assert!(
            registry
                .agent(agent)
                .unwrap()
                .owns_session(&SessionKey::claude("s-1"))
        );
        // The next listing changes nothing.
        let again = reconcile(
            &registry,
            &listing(vec![listed("s-1", AgentState::Working, true)]),
            Timestamp::from_millis(6),
            ids(),
        );
        assert_eq!(again, vec![]);
    }

    #[test]
    fn only_one_session_is_adopted_and_others_become_agents() {
        let mut registry = Registry::new();
        launched(&mut registry, 1, "/work/alpha");
        let events = reconcile(
            &registry,
            &listing(vec![
                listed("s-1", AgentState::Working, true),
                listed("s-2", AgentState::Working, true),
            ]),
            Timestamp::from_millis(5),
            ids(),
        );
        let registered = events
            .iter()
            .filter(|event| matches!(event.kind, EventKind::AgentRegistered { .. }))
            .count();
        assert_eq!(registered, 1, "the second session is someone else");
    }

    #[test]
    fn a_session_elsewhere_is_never_adopted() {
        let mut registry = Registry::new();
        launched(&mut registry, 1, "/work/beta");
        let events = reconcile(
            &registry,
            &listing(vec![listed("s-1", AgentState::Working, true)]),
            Timestamp::from_millis(5),
            ids(),
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event.kind, EventKind::AgentRegistered { .. }))
        );
    }
}
