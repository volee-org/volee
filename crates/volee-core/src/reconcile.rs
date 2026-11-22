//! What a harness lists, and how a listing moves the agents a mission
//! knows (v2, step 8).
//!
//! A harness lists its sessions (for Claude Code, `claude agents --json
//! --all`). The daemon's agents view is the one reader of that listing and
//! the one list of sessions a person sees. The 0.x discovery, which turned
//! every listed session into an agent of its own, is gone: [`reconcile`]
//! only follows the agents the registry already holds (the ones a mission
//! launched or that joined through MCP). It is pure: the daemon maps the
//! listing to a [`Listing`] and persists the returned events.

use std::path::PathBuf;

use crate::agent::Agent;
use crate::event::{Event, EventKind};
use crate::harness::{HarnessKind, SessionKey, SessionOrigin};
use crate::ids::AgentId;
use crate::registry::Registry;
use crate::role::LaunchedBy;
use crate::state::AgentState;
use crate::time::Timestamp;
use crate::workspace::Location;

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
    /// The harness's handle for commands on the session (for Claude Code,
    /// the `id` of a background session). Sessions without one, such as
    /// Claude Code's interactive sessions, cannot be stopped, removed,
    /// previewed or attached through the harness.
    pub handle: Option<String>,
    /// Its process id, while the process is alive and the harness reports
    /// it.
    pub pid: Option<u32>,
    /// When it started, when the harness reports it.
    pub started_at: Option<Timestamp>,
}

impl ListedSession {
    /// A session known by its key, directory and state; it is neither alive
    /// nor in the background, and has no handle, process id or start time
    /// until the caller sets them.
    pub fn new(key: SessionKey, cwd: impl Into<PathBuf>, state: AgentState) -> Self {
        Self {
            key,
            name: None,
            cwd: cwd.into(),
            state,
            alive: false,
            background: false,
            handle: None,
            pid: None,
            started_at: None,
        }
    }
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

/// How long an agent volee only watches stays in the registry after its
/// session is gone, in milliseconds: long enough to see that it ended, short
/// enough that the list shows the present.
pub const GONE_GRACE_MS: u64 = 60_000;

/// Returns the events that bring the agents `registry` holds in line with
/// `listing`. It never creates an agent.
///
/// - A listed session that belongs to an agent updates the agent's state when
///   the coarse state differs. When both agree, the registry keeps its richer
///   state, which usually comes from hooks.
/// - A live listed session that nobody owns, running in the directory of an
///   agent volee is launching or resuming (state `launching`, and no session
///   of its own listed), is that agent's session: the listing saw it before
///   the session's first hook did. It is a fork when the agent already had a
///   session.
/// - Any other listed session that nobody owns is left to the agents view.
/// - When a complete listing no longer contains an agent's current session,
///   the agent becomes `stopped` (it ran in a terminal) or `lost` (it ran in
///   the background), unless it is launching a new session or runs in a
///   volee host.
pub fn reconcile(registry: &Registry, listing: &Listing, now: Timestamp) -> Vec<Event> {
    let mut events = Vec::new();
    let mut adopted: Vec<AgentId> = Vec::new();
    for session in &listing.sessions {
        if registry.is_ignored(&session.key) {
            continue;
        }
        match registry.agent_for_session(&session.key) {
            Some(agent) => follow(agent, session, now, &mut events),
            None if session.alive => {
                if let Some(waiting) = awaiting_session(registry, listing, session, &adopted) {
                    adopted.push(waiting.id);
                    adopt(waiting, session, now, &mut events);
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

/// A known agent's session is listed: the agent follows its coarse state,
/// and a watched agent its name and its place.
fn follow(agent: &Agent, session: &ListedSession, now: Timestamp, events: &mut Vec<Event>) {
    let is_current = agent
        .current_session()
        .is_some_and(|link| link.key == session.key);
    if !is_current {
        return;
    }
    // The background process that an adoption stopped is still listed for a
    // while: its host, not that entry, says how a hosted agent is.
    if agent.location.host().is_some() && session.background && !session.alive {
        return;
    }
    let watched = agent.watched();
    if watched
        && let Some(name) = &session.name
        && *name != agent.name
    {
        events.push(Event::new(
            now,
            EventKind::AgentRenamed {
                agent: agent.id,
                name: name.clone(),
            },
        ));
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
    let location = if session.background {
        let moved = match agent.location {
            Location::Unknown => true,
            // A hosted Claude runs interactive, so a live background
            // session of a hosted agent means its conversation was resumed
            // in the background after its host ended (an adoption whose
            // program failed, for example).
            Location::Host { .. } => session.alive,
            // The person moved the conversation to the background.
            Location::Terminal | Location::Pane { .. } => watched,
            Location::Background => false,
        };
        moved.then_some(Location::Background)
    } else if watched && session.alive && agent.location == Location::Background {
        // The person resumed a background conversation in a terminal: its
        // live process is there, and a multiplexer may place it in a pane.
        Some(Location::Terminal)
    } else {
        None
    };
    if let Some(location) = location {
        events.push(Event::new(
            now,
            EventKind::LocationChanged {
                agent: agent.id,
                location,
            },
        ));
    }
}

/// The session of an agent volee is launching or resuming, seen before its
/// first hook.
fn adopt(agent: &Agent, session: &ListedSession, now: Timestamp, events: &mut Vec<Event>) {
    let origin = if agent.current_session().is_some() {
        SessionOrigin::Forked
    } else {
        SessionOrigin::Started
    };
    events.push(Event::new(
        now,
        EventKind::SessionLinked {
            agent: agent.id,
            session: session.key.clone(),
            origin,
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

/// Agents whose current session a complete listing no longer contains.
fn vanished(registry: &Registry, listing: &Listing, now: Timestamp, events: &mut Vec<Event>) {
    for agent in registry.agents() {
        if agent.harness != listing.harness {
            continue;
        }
        let Some(current) = agent.current_session() else {
            continue;
        };
        if listing.sessions.iter().any(|s| s.key == current.key) {
            continue;
        }
        if agent.watched() {
            events.extend(gone(agent, now));
            continue;
        }
        // A launching agent waits for a new session (a resume forks one with
        // a new id): its previous session being gone is expected. A hosted
        // agent ends when its host says so.
        if agent.location.host().is_some()
            || matches!(
                agent.state,
                AgentState::Stopped | AgentState::Lost | AgentState::Launching
            )
        {
            continue;
        }
        events.push(ended(agent, now));
    }
}

/// The session of an agent that was alive is no longer listed: it ended in
/// its terminal, or its harness removed it from the background.
fn ended(agent: &Agent, now: Timestamp) -> Event {
    let (state, detail) = if agent.location == Location::Background {
        (AgentState::Lost, "the harness no longer lists this session")
    } else {
        (AgentState::Stopped, "the session ended")
    };
    Event::new(
        now,
        EventKind::StateChanged {
            agent: agent.id,
            state,
            detail: Some(detail.to_owned()),
        },
    )
}

/// What it means, at `now`, that the session of a watched agent
/// ([`Agent::watched`]) is not listed any more (ADR 0027).
///
/// An agent that was alive ended: it becomes `stopped` or `lost`, and stays
/// for [`GONE_GRACE_MS`] so that its end can be seen. One that has been
/// stopped, lost or failed for that long expires: nothing is left to show.
/// For a harness whose listing can prove that a session is gone,
/// [`reconcile`] applies this itself; a caller that decides absence another
/// way calls it.
pub fn gone(agent: &Agent, now: Timestamp) -> Option<Event> {
    if agent.state.is_live() {
        return Some(ended(agent, now));
    }
    (agent.state_since.millis_until(now) >= GONE_GRACE_MS)
        .then(|| Event::new(now, EventKind::AgentExpired { agent: agent.id }))
}

/// The agent volee is launching or resuming in `session`'s directory, if
/// any: it is `launching`, and none of its own sessions is listed alive.
fn awaiting_session<'a>(
    registry: &'a Registry,
    listing: &Listing,
    session: &ListedSession,
    adopted: &[AgentId],
) -> Option<&'a Agent> {
    registry.agents().find(|agent| {
        let has_live_session = agent.current_session().is_some_and(|current| {
            listing
                .sessions
                .iter()
                .any(|listed| listed.key == current.key && listed.alive)
        });
        agent.harness == session.key.harness
            && agent.launched_by != LaunchedBy::Outside
            && agent.state == AgentState::Launching
            && !has_live_session
            && agent.workspace.cwd == session.cwd
            && !adopted.contains(&agent.id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role::{LaunchedBy, Role};
    use crate::workspace::Workspace;

    fn listing(sessions: Vec<ListedSession>) -> Listing {
        Listing {
            harness: HarnessKind::ClaudeCode,
            sessions,
            complete: true,
        }
    }

    fn session(id: &str, state: AgentState) -> ListedSession {
        ListedSession {
            alive: true,
            background: true,
            handle: Some(id.to_owned()),
            ..ListedSession::new(SessionKey::claude(id), "/work/alpha", state)
        }
    }

    fn launching(registry: &mut Registry, agent: u128) -> AgentId {
        let id = AgentId::from_ulid(ulid::Ulid::from_parts(1_000, agent));
        let register = Event::new(
            Timestamp::from_millis(1),
            EventKind::AgentRegistered {
                agent: id,
                name: "worker".into(),
                role: Role::Worker,
                harness: HarnessKind::ClaudeCode,
                launched_by: LaunchedBy::Human,
                parent: None,
                mission: None,
                workspace: Workspace::at("/work/alpha"),
                location: Location::Background,
            },
        );
        let launch = Event::new(
            Timestamp::from_millis(2),
            EventKind::StateChanged {
                agent: id,
                state: AgentState::Launching,
                detail: None,
            },
        );
        registry.apply(&register).unwrap();
        registry.apply(&launch).unwrap();
        id
    }

    fn apply(registry: &mut Registry, events: &[Event]) {
        for event in events {
            registry.apply(event).unwrap();
        }
    }

    #[test]
    fn a_session_nobody_owns_never_becomes_an_agent() {
        let registry = Registry::new();
        let events = reconcile(
            &registry,
            &listing(vec![session("a", AgentState::Working)]),
            Timestamp::from_millis(10),
        );
        assert_eq!(events, []);
    }

    #[test]
    fn the_session_of_a_launching_agent_is_adopted_and_then_followed() {
        let mut registry = Registry::new();
        let id = launching(&mut registry, 1);
        let seen = listing(vec![session("a", AgentState::Working)]);
        let events = reconcile(&registry, &seen, Timestamp::from_millis(10));
        apply(&mut registry, &events);
        let agent = registry.agent(id).unwrap();
        assert_eq!(agent.state, AgentState::Working);
        assert_eq!(
            registry
                .agent_for_session(&SessionKey::claude("a"))
                .unwrap()
                .id,
            id
        );
        let done = listing(vec![session("a", AgentState::Done)]);
        let events = reconcile(&registry, &done, Timestamp::from_millis(20));
        apply(&mut registry, &events);
        assert_eq!(registry.agent(id).unwrap().state, AgentState::Done);
        assert_eq!(reconcile(&registry, &done, Timestamp::from_millis(30)), []);
    }

    #[test]
    fn a_session_that_left_a_complete_listing_ends_its_agent() {
        let mut registry = Registry::new();
        let id = launching(&mut registry, 1);
        let events = reconcile(
            &registry,
            &listing(vec![session("a", AgentState::Working)]),
            Timestamp::from_millis(10),
        );
        apply(&mut registry, &events);
        let events = reconcile(&registry, &listing(vec![]), Timestamp::from_millis(20));
        apply(&mut registry, &events);
        assert_eq!(registry.agent(id).unwrap().state, AgentState::Lost);
    }

    #[test]
    fn a_partial_listing_proves_nothing_gone() {
        let mut registry = Registry::new();
        let id = launching(&mut registry, 1);
        let events = reconcile(
            &registry,
            &listing(vec![session("a", AgentState::Working)]),
            Timestamp::from_millis(10),
        );
        apply(&mut registry, &events);
        let partial = Listing {
            complete: false,
            ..listing(vec![])
        };
        assert_eq!(
            reconcile(&registry, &partial, Timestamp::from_millis(20)),
            []
        );
        assert_eq!(registry.agent(id).unwrap().state, AgentState::Working);
    }
}
