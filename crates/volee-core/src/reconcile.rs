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

/// How long a session started elsewhere, outside the background, must have
/// lived before it becomes an agent, in milliseconds. Programs call a
/// harness for one answer and are gone in a few seconds (measured: a
/// menu-bar tool running `claude --print` every five minutes, 2.3 s each
/// time), and the harness lists such a run like any session: it would be a
/// row nobody recognises. A session whose start time is unknown is taken at
/// once.
pub const SETTLE_MS: u64 = 10_000;

/// How long a background session that started beside a live session in a
/// terminal, in the same directory, waits before it becomes an agent of its
/// own, in milliseconds. It may be that session's conversation on its way to
/// the background ([`moved_to_background`]). Measured with Claude Code,
/// three times: the listing taken at the key press shows the session in the
/// terminal and the new one in the background, which has no process yet;
/// for about a second (0.7 s to 1.4 s) listings show the new one alone,
/// still without a process, in a state that says it waits; then it has
/// one. A listing taken in between must not make a second agent of it.
pub const MOVE_SETTLE_MS: u64 = 5_000;

/// How long after it started a background session without a process may
/// still be a conversation that arrives, in milliseconds: longer than the
/// longest wait between two listings, since the arrival was measured on
/// short conversations and a long one may take longer to get its process.
pub const ARRIVAL_MS: u64 = 60_000;

/// Returns the events that bring `registry` in line with `listing`.
///
/// - A listed session that belongs to an agent updates the agent's state when
///   the coarse state differs. When both agree, the registry keeps its richer
///   state, which usually comes from hooks.
/// - An agent in a volee host whose session is listed alive in the background
///   moves back to the background: the conversation left its host. A
///   finished background entry, or no entry at all, changes nothing for it:
///   its host says when it ends.
/// - A live listed session that nobody owns, running in the directory of an
///   agent volee is launching or resuming (state `launching`, and no session
///   of its own listed), is that agent's session: the listing saw it before
///   the session's first hook did. It is a fork when the agent already had a
///   session.
/// - Any other listed session that nobody owns becomes a new `independent`
///   agent, alive or not: a finished background session that the harness
///   still lists is one the person can still open. A live session outside
///   the background waits until it has lived [`SETTLE_MS`], and a background
///   session beside a live session in a terminal, in the same directory,
///   until it has lived [`MOVE_SETTLE_MS`], with or without a process.
///   Sessions of forgotten agents are always skipped.
/// - An agent volee only watches ([`Agent::watched`]) is one session
///   (ADR 0027): it takes the name and the place (background or terminal)
///   the harness gives that session. One that holds several sessions keeps
///   the one listed alive and releases the others, which become agents of
///   their own when they are listed.
/// - When a complete listing no longer contains an agent's current session,
///   a watched agent that was alive becomes `stopped` (it ran in a terminal:
///   the session ended) or `lost` (it ran in the background: the harness
///   removed it), and one whose session has been gone or finished for
///   [`GONE_GRACE_MS`] expires: it leaves the registry. Any other agent
///   becomes `stopped` or `lost` the same way and stays, unless it is
///   launching a new session or runs in a volee host.
pub fn reconcile(
    registry: &Registry,
    listing: &Listing,
    now: Timestamp,
    mut new_agent_id: impl FnMut() -> AgentId,
) -> Vec<Event> {
    let mut events = split(registry, listing, now);
    // The sessions just released are nobody's: the rest reads the registry
    // as it is once they are.
    let released;
    let registry = if events.is_empty() {
        registry
    } else {
        let mut scratch = registry.clone();
        for event in &events {
            // `split` only releases sessions their agent holds.
            let _ = scratch.apply(event);
        }
        released = scratch;
        &released
    };
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
                } else if settled(session, listing, now) {
                    discover(new_agent_id(), session, now, &mut events);
                }
            }
            // Finished, and still listed: the harness keeps it for the
            // person, so volee shows it. One that just started beside a
            // session in a terminal has no process yet, and waits like a
            // live one: it may be that session's conversation.
            None if session.background && settled(session, listing, now) => {
                discover(new_agent_id(), session, now, &mut events);
            }
            None => {}
        }
    }
    if listing.complete {
        vanished(registry, listing, now, &mut events);
    }
    events
}

/// Whether a live session nobody owns has lived long enough to become an
/// agent: a session in a terminal after [`SETTLE_MS`]; a background session
/// at once, unless a live session runs in a terminal in its directory, which
/// it may be continuing: then after [`MOVE_SETTLE_MS`]. A session whose
/// start time is unknown has.
fn settled(session: &ListedSession, listing: &Listing, now: Timestamp) -> bool {
    let lived = |millis: u64| {
        session
            .started_at
            .is_none_or(|started| started.millis_until(now) >= millis)
    };
    if !session.background {
        return lived(SETTLE_MS);
    }
    let beside_a_terminal = listing
        .sessions
        .iter()
        .any(|other| other.alive && !other.background && other.cwd == session.cwd);
    !beside_a_terminal || lived(MOVE_SETTLE_MS)
}

/// Whether `listing` holds a session that will become an agent once it has
/// lived long enough ([`SETTLE_MS`], [`MOVE_SETTLE_MS`]): the caller should
/// list again soon, not wait as if nothing was happening.
pub fn settling(registry: &Registry, listing: &Listing, now: Timestamp) -> bool {
    listing.sessions.iter().any(|session| {
        (session.alive || session.background)
            && !settled(session, listing, now)
            && !registry.is_ignored(&session.key)
            && registry.agent_for_session(&session.key).is_none()
    })
}

/// The events for sessions that continue, under a new id, a session of the
/// previous listing: the same process (same process id and start time)
/// with another session id, the old one gone. Claude Code does this on
/// `/clear`. The agent volee watched under the old id releases it and takes
/// the new one, so that its row stays. Agents volee launched learn it from
/// their hooks.
///
/// `previous` is the listing before `listing`; apply the returned events
/// before calling [`reconcile`].
pub fn continued<'a>(
    registry: &Registry,
    previous: impl IntoIterator<Item = &'a ListedSession>,
    listing: &Listing,
    now: Timestamp,
) -> Vec<Event> {
    let mut events = Vec::new();
    let listed_now = |key: &SessionKey| listing.sessions.iter().any(|session| &session.key == key);
    for before in previous {
        let Some(pid) = before.pid else {
            continue;
        };
        if before.key.harness != listing.harness || listed_now(&before.key) {
            continue;
        }
        let Some(agent) = registry.agent_for_session(&before.key) else {
            continue;
        };
        let current = agent
            .current_session()
            .is_some_and(|link| link.key == before.key);
        if !current || !agent.watched() {
            continue;
        }
        let successor = listing.sessions.iter().find(|session| {
            session.alive
                && session.pid == Some(pid)
                && session.started_at == before.started_at
                && !registry.is_ignored(&session.key)
                && registry.agent_for_session(&session.key).is_none()
        });
        let Some(successor) = successor else {
            continue;
        };
        events.push(Event::new(
            now,
            EventKind::SessionReleased {
                agent: agent.id,
                session: before.key.clone(),
            },
        ));
        events.push(Event::new(
            now,
            EventKind::SessionLinked {
                agent: agent.id,
                session: successor.key.clone(),
                origin: SessionOrigin::Discovered,
            },
        ));
    }
    events
}

/// The events for conversations that left a terminal for the harness's
/// background, where they go on under a new session id (ADR 0034).
///
/// Claude Code does this when the person presses the left arrow on a
/// session's empty prompt: a background process takes the conversation
/// under a new session id, and the terminal's own process stays, to show
/// the list of agents. Nothing in a listing links the two sessions.
///
/// `left` are the sessions that stopped being listed **while their process
/// kept running**: the caller looked. A session that ended took its process
/// with it and is not one of them, whatever started beside it. One of them
/// that ran in a terminal, as the session of an agent volee watches
/// ([`Agent::watched`]), is continued by the background session of the same
/// directory that nobody owns, that did not start before it, and that runs
/// or arrives: a conversation that arrives in the background is listed
/// before it has a process, in a state that says it works or waits, where a
/// finished session is done, stopped or failed ([`ARRIVAL_MS`]). It is the
/// one that carries its name when several are there, the only one
/// otherwise, and none when that leaves a doubt (two sessions left that
/// directory, or two arrived under other names). The agent releases the old
/// session and takes the new one, so that its row stays; [`reconcile`] then
/// gives it the name and the place of that session.
///
/// Apply the returned events before calling [`reconcile`].
pub fn moved_to_background<'a>(
    registry: &Registry,
    left: impl IntoIterator<Item = &'a ListedSession>,
    listing: &Listing,
    now: Timestamp,
) -> Vec<Event> {
    let listed_now = |key: &SessionKey| listing.sessions.iter().any(|session| &session.key == key);
    let left: Vec<(&Agent, &ListedSession)> = left
        .into_iter()
        .filter(|before| {
            before.key.harness == listing.harness && !before.background && !listed_now(&before.key)
        })
        .filter_map(|before| {
            let agent = registry.agent_for_session(&before.key)?;
            let current = agent
                .current_session()
                .is_some_and(|link| link.key == before.key);
            (current && agent.watched()).then_some((agent, before))
        })
        .collect();
    let mut taken: Vec<&SessionKey> = Vec::new();
    let mut events = Vec::new();
    for (agent, before) in &left {
        let arrived: Vec<&ListedSession> = listing
            .sessions
            .iter()
            .filter(|session| {
                session.background
                    && (session.alive || arrives(session, now))
                    && session.cwd == before.cwd
                    && !started_before(session, before)
                    && !taken.contains(&&session.key)
                    && !registry.is_ignored(&session.key)
                    && registry.agent_for_session(&session.key).is_none()
            })
            .collect();
        let namesakes: Vec<&ListedSession> = arrived
            .iter()
            .copied()
            .filter(|session| before.name.is_some() && session.name == before.name)
            .collect();
        let alone = left
            .iter()
            .filter(|(_, other)| other.cwd == before.cwd)
            .count()
            == 1;
        let successor = match (namesakes.as_slice(), arrived.as_slice()) {
            ([namesake], _) => *namesake,
            ([], [only]) if alone => *only,
            _ => continue,
        };
        taken.push(&successor.key);
        events.push(Event::new(
            now,
            EventKind::SessionReleased {
                agent: agent.id,
                session: before.key.clone(),
            },
        ));
        events.push(Event::new(
            now,
            EventKind::SessionLinked {
                agent: agent.id,
                session: successor.key.clone(),
                origin: SessionOrigin::Discovered,
            },
        ));
    }
    events
}

/// Whether `session`, which has no process, is a conversation that arrives
/// in the background and not one that finished: its harness says that it
/// works or waits, and it started less than [`ARRIVAL_MS`] ago.
fn arrives(session: &ListedSession, now: Timestamp) -> bool {
    session.state.is_live()
        && session
            .started_at
            .is_some_and(|started| started.millis_until(now) < ARRIVAL_MS)
}

/// Whether `session` started before `other` did, as far as the harness
/// says when they started.
fn started_before(session: &ListedSession, other: &ListedSession) -> bool {
    matches!(
        (session.started_at, other.started_at),
        (Some(started), Some(reference)) if started < reference
    )
}

/// A watched agent is one session (ADR 0027). One that holds several, as
/// the agents an earlier volee discovered can, keeps the one the harness
/// lists alive (failing that the last one it lists, failing that its last
/// one) and releases the others.
fn split(registry: &Registry, listing: &Listing, now: Timestamp) -> Vec<Event> {
    let mut events = Vec::new();
    for agent in registry.agents() {
        if agent.harness != listing.harness || !agent.watched() || agent.sessions.len() < 2 {
            continue;
        }
        let listed = |key: &SessionKey| listing.sessions.iter().find(|session| &session.key == key);
        let newest_first = || agent.sessions.iter().rev();
        let keep = newest_first()
            .find(|link| listed(&link.key).is_some_and(|session| session.alive))
            .or_else(|| newest_first().find(|link| listed(&link.key).is_some()))
            .or_else(|| agent.sessions.last());
        let Some(keep) = keep else {
            continue;
        };
        for link in &agent.sessions {
            if link.key != keep.key {
                events.push(Event::new(
                    now,
                    EventKind::SessionReleased {
                        agent: agent.id,
                        session: link.key.clone(),
                    },
                ));
            }
        }
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

/// Where the agents of some sessions run, after a terminal multiplexer looked
/// for those sessions in its panes (ADR 0015).
///
/// `asked` are the sessions `multiplexer` was asked about, and `found` the
/// pane address (`<session>:<pane>`) of those it found. An agent whose current
/// session was found moves to that pane; one that was in a pane of this
/// multiplexer and is no longer found goes back to a plain terminal. Other
/// agents, background ones included, keep their location.
pub fn relocate(
    registry: &Registry,
    multiplexer: &str,
    asked: &[SessionKey],
    found: &[(SessionKey, String)],
    now: Timestamp,
) -> Vec<Event> {
    let mut events = Vec::new();
    for key in asked {
        let Some(agent) = registry.agent_for_session(key) else {
            continue;
        };
        let current = agent.current_session().is_some_and(|link| &link.key == key);
        // Background agents and agents in a volee host run in no pane.
        if !current || matches!(agent.location, Location::Background | Location::Host { .. }) {
            continue;
        }
        let location = match found.iter().find(|(session, _)| session == key) {
            Some((_, target)) => Location::Pane {
                multiplexer: multiplexer.to_owned(),
                target: target.clone(),
            },
            None => match &agent.location {
                Location::Pane {
                    multiplexer: own, ..
                } if own == multiplexer => Location::Terminal,
                _ => continue,
            },
        };
        if agent.location != location {
            events.push(Event::new(
                now,
                EventKind::LocationChanged {
                    agent: agent.id,
                    location,
                },
            ));
        }
    }
    events
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
            name: Some(format!("session {id}")),
            alive,
            background: true,
            handle: Some(id.to_owned()),
            ..ListedSession::new(SessionKey::claude(id), "/work/alpha", state)
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

    /// Reconciles, applies the events and returns them. New agents get ids
    /// that carry the time, so that two calls never give the same one.
    fn sync(registry: &mut Registry, listing: &Listing, millis: i64) -> Vec<Event> {
        let mut next = 0;
        let ids = move || {
            next += 1;
            AgentId::from_ulid(Ulid::from_parts(millis.unsigned_abs(), next))
        };
        let events = reconcile(registry, listing, Timestamp::from_millis(millis), ids);
        apply_all(registry, &events);
        events
    }

    /// An interactive session, as Claude Code lists one run by hand.
    fn interactive(id: &str) -> ListedSession {
        ListedSession {
            alive: true,
            pid: Some(4_242),
            ..ListedSession::new(SessionKey::claude(id), "/work/alpha", AgentState::Idle)
        }
    }

    #[test]
    fn agents_follow_their_session_into_a_pane_and_out_again() {
        let mut registry = Registry::new();
        sync(
            &mut registry,
            &listing(vec![
                interactive("in-pane"),
                interactive("elsewhere"),
                listed("bg", AgentState::Working, true),
            ]),
            10,
        );
        let location = |registry: &Registry, id: &str| {
            registry
                .agent_for_session(&SessionKey::claude(id))
                .unwrap()
                .location
                .clone()
        };
        assert_eq!(location(&registry, "in-pane"), Location::Terminal);
        let asked = [
            SessionKey::claude("in-pane"),
            SessionKey::claude("elsewhere"),
            SessionKey::claude("bg"),
        ];
        let found = [
            (SessionKey::claude("in-pane"), "work:c434".to_owned()),
            (SessionKey::claude("bg"), "work:9a".to_owned()),
        ];
        let events = relocate(
            &registry,
            "tuios",
            &asked,
            &found,
            Timestamp::from_millis(20),
        );
        apply_all(&mut registry, &events);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(
            location(&registry, "in-pane"),
            Location::Pane {
                multiplexer: "tuios".into(),
                target: "work:c434".into(),
            }
        );
        // A session outside every pane, and a background one, stay as they
        // were.
        assert_eq!(location(&registry, "elsewhere"), Location::Terminal);
        assert_eq!(location(&registry, "bg"), Location::Background);

        // Found again in the same pane: nothing to record. Another
        // multiplexer that does not find it leaves it alone.
        let at = Timestamp::from_millis(30);
        let none: [Event; 0] = [];
        assert_eq!(relocate(&registry, "tuios", &asked, &found, at), none);
        assert_eq!(relocate(&registry, "tmux", &asked, &[], at), none);
        // tuios no longer finds it: back to a plain terminal.
        let events = relocate(&registry, "tuios", &asked, &[], Timestamp::from_millis(40));
        apply_all(&mut registry, &events);
        assert_eq!(location(&registry, "in-pane"), Location::Terminal);
    }

    #[test]
    fn a_hosted_agent_resumed_in_the_background_moves_back_there() {
        let mut registry = Registry::new();
        sync(
            &mut registry,
            &listing(vec![listed("bg", AgentState::Idle, true)]),
            10,
        );
        let agent = registry
            .agent_for_session(&SessionKey::claude("bg"))
            .unwrap()
            .id;
        let host = Location::Host {
            id: "ag-host".into(),
        };
        let stopped = [
            Event::new(
                Timestamp::from_millis(20),
                EventKind::LocationChanged {
                    agent,
                    location: host.clone(),
                },
            ),
            Event::new(
                Timestamp::from_millis(20),
                EventKind::StateChanged {
                    agent,
                    state: AgentState::Stopped,
                    detail: Some("its program ended".into()),
                },
            ),
        ];
        apply_all(&mut registry, &stopped);
        // Its own session in the host is interactive: it stays there.
        let in_host = ListedSession {
            background: false,
            ..listed("bg", AgentState::Idle, true)
        };
        sync(&mut registry, &listing(vec![in_host]), 30);
        assert_eq!(registry.agent(agent).unwrap().location, host);
        // A finished background entry does not move it either.
        sync(
            &mut registry,
            &listing(vec![listed("bg", AgentState::Stopped, false)]),
            40,
        );
        assert_eq!(registry.agent(agent).unwrap().location, host);
        // Resumed in the background: it lives there again.
        sync(
            &mut registry,
            &listing(vec![listed("bg", AgentState::Working, true)]),
            50,
        );
        let moved = registry.agent(agent).unwrap();
        assert_eq!(moved.location, Location::Background);
        assert_eq!(moved.state, AgentState::Working);
    }

    #[test]
    fn a_hosted_agent_ignores_the_background_process_its_adoption_stopped() {
        let mut registry = Registry::new();
        sync(
            &mut registry,
            &listing(vec![listed("bg", AgentState::Idle, true)]),
            10,
        );
        let agent = registry
            .agent_for_session(&SessionKey::claude("bg"))
            .unwrap()
            .id;
        let adopted = [
            Event::new(
                Timestamp::from_millis(20),
                EventKind::LocationChanged {
                    agent,
                    location: Location::Host { id: "h".into() },
                },
            ),
            Event::new(
                Timestamp::from_millis(20),
                EventKind::StateChanged {
                    agent,
                    state: AgentState::Idle,
                    detail: Some("adopted into a volee host".into()),
                },
            ),
        ];
        apply_all(&mut registry, &adopted);
        let none: Vec<Event> = Vec::new();
        // The stopped background process, then no entry at all: the host
        // decides, the listing changes nothing.
        let stopped = listing(vec![listed("bg", AgentState::Stopped, false)]);
        assert_eq!(sync(&mut registry, &stopped, 30), none);
        assert_eq!(sync(&mut registry, &listing(vec![]), 40), none);
        assert_eq!(registry.agent(agent).unwrap().state, AgentState::Idle);
    }

    #[test]
    fn a_new_listed_session_has_no_optional_details() {
        let session = ListedSession::new(SessionKey::claude("s1"), "/work", AgentState::Idle);
        assert_eq!(session.name, None);
        assert!(!session.alive && !session.background);
        assert_eq!(session.handle, None);
        assert_eq!(session.pid, None);
        assert_eq!(session.started_at, None);
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
    fn a_finished_background_session_still_listed_becomes_an_agent() {
        let mut registry = Registry::new();
        let finished = listing(vec![listed("old00000", AgentState::Stopped, false)]);
        sync(&mut registry, &finished, 10);
        let agent = registry
            .agent_for_session(&SessionKey::claude("old00000"))
            .unwrap();
        assert_eq!(agent.state, AgentState::Stopped);
        assert_eq!(agent.location, Location::Background);
        assert_eq!(agent.name, "session old00000");
        assert!(agent.watched());
        // It stays as long as its harness lists it, however long ago it
        // ended.
        let much_later = 10 + 100 * i64::try_from(GONE_GRACE_MS).unwrap();
        assert_eq!(sync(&mut registry, &finished, much_later), []);
    }

    #[test]
    fn a_finished_session_outside_the_background_is_history() {
        let registry = Registry::new();
        let mut session = listed("old00000", AgentState::Stopped, false);
        session.background = false;
        let events = reconcile(
            &registry,
            &listing(vec![session]),
            Timestamp::from_millis(10),
            ids(),
        );
        assert_eq!(events, []);
    }

    /// The agent that owns a session.
    fn owner<'a>(registry: &'a Registry, id: &str) -> &'a Agent {
        registry
            .agent_for_session(&SessionKey::claude(id))
            .unwrap_or_else(|| panic!("nobody owns session {id}"))
    }

    #[test]
    fn a_watched_agent_holding_several_sessions_keeps_the_live_one() {
        // As an earlier volee left it: one agent, three sessions of the
        // same conversation, the last linked being a stopped copy.
        let mut registry = Registry::new();
        sync(
            &mut registry,
            &listing(vec![listed("live", AgentState::Working, true)]),
            1,
        );
        let agent = owner(&registry, "live").id;
        for session in ["gone", "copy"] {
            registry
                .apply(&Event::new(
                    Timestamp::from_millis(2),
                    EventKind::SessionLinked {
                        agent,
                        session: SessionKey::claude(session),
                        origin: SessionOrigin::Forked,
                    },
                ))
                .unwrap();
        }
        registry
            .apply(&Event::new(
                Timestamp::from_millis(3),
                EventKind::StateChanged {
                    agent,
                    state: AgentState::Stopped,
                    detail: None,
                },
            ))
            .unwrap();
        let both = listing(vec![
            listed("live", AgentState::Working, true),
            listed("copy", AgentState::Stopped, false),
        ]);
        sync(&mut registry, &both, 4);
        let kept = registry.agent(agent).unwrap();
        assert_eq!(kept.sessions.len(), 1, "{:?}", kept.sessions);
        assert_eq!(
            kept.current_session().unwrap().key,
            SessionKey::claude("live")
        );
        assert_eq!(
            kept.state,
            AgentState::Working,
            "it follows its live session"
        );
        // The copy is a row of its own, under the name its harness gives
        // it; the session that is not listed is nobody's.
        let copy = owner(&registry, "copy");
        assert_ne!(copy.id, agent);
        assert_eq!(copy.name, "session copy");
        assert_eq!(copy.state, AgentState::Stopped);
        assert!(
            registry
                .agent_for_session(&SessionKey::claude("gone"))
                .is_none()
        );
        assert_eq!(registry.agents().count(), 2);
        // Settled: the same listing changes nothing more.
        assert_eq!(sync(&mut registry, &both, 5), []);
    }

    #[test]
    fn a_watched_agent_with_no_live_session_keeps_the_last_one_listed() {
        let mut registry = Registry::new();
        sync(
            &mut registry,
            &listing(vec![listed("first", AgentState::Stopped, false)]),
            1,
        );
        let agent = owner(&registry, "first").id;
        registry
            .apply(&Event::new(
                Timestamp::from_millis(2),
                EventKind::SessionLinked {
                    agent,
                    session: SessionKey::claude("unlisted"),
                    origin: SessionOrigin::Forked,
                },
            ))
            .unwrap();
        sync(
            &mut registry,
            &listing(vec![listed("first", AgentState::Stopped, false)]),
            3,
        );
        let kept = registry.agent(agent).unwrap();
        assert_eq!(kept.sessions.len(), 1);
        assert!(kept.owns_session(&SessionKey::claude("first")));
    }

    #[test]
    fn a_watched_agent_takes_the_name_its_harness_gives_its_session() {
        let mut registry = Registry::new();
        sync(
            &mut registry,
            &listing(vec![listed("aaaa1111", AgentState::Idle, true)]),
            1,
        );
        let agent = owner(&registry, "aaaa1111").id;
        let mut renamed = listed("aaaa1111", AgentState::Idle, true);
        renamed.name = Some("fix the login flow".into());
        let events = sync(&mut registry, &listing(vec![renamed.clone()]), 2);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(registry.agent(agent).unwrap().name, "fix the login flow");
        // A listing that gives no name takes none away.
        renamed.name = None;
        assert_eq!(sync(&mut registry, &listing(vec![renamed]), 3), []);
        assert_eq!(registry.agent(agent).unwrap().name, "fix the login flow");
    }

    #[test]
    fn an_agent_volee_launched_keeps_the_name_volee_gave_it() {
        let mut registry = Registry::new();
        let agent = launched(&mut registry, 1, "/work/alpha");
        sync(
            &mut registry,
            &listing(vec![listed("s-1", AgentState::Working, true)]),
            5,
        );
        assert!(
            registry
                .agent(agent)
                .unwrap()
                .owns_session(&SessionKey::claude("s-1"))
        );
        assert_eq!(registry.agent(agent).unwrap().name, "worker 1");
    }

    #[test]
    fn a_watched_agent_is_where_its_harness_says_its_session_runs() {
        let mut registry = Registry::new();
        sync(
            &mut registry,
            &listing(vec![listed("conv", AgentState::Stopped, false)]),
            1,
        );
        let agent = owner(&registry, "conv").id;
        let location = |registry: &Registry| registry.agent(agent).unwrap().location.clone();
        assert_eq!(location(&registry), Location::Background);
        // The person resumes the conversation in a terminal: same session,
        // listed as an interactive one.
        let resumed = ListedSession {
            name: Some("session conv".into()),
            ..interactive("conv")
        };
        sync(&mut registry, &listing(vec![resumed.clone()]), 2);
        assert_eq!(location(&registry), Location::Terminal);
        assert_eq!(registry.agent(agent).unwrap().state, AgentState::Idle);
        // A multiplexer places it in a pane; the listing leaves it there.
        let found = [(SessionKey::claude("conv"), "work:1".to_owned())];
        let moved = relocate(
            &registry,
            "tuios",
            &[SessionKey::claude("conv")],
            &found,
            Timestamp::from_millis(3),
        );
        apply_all(&mut registry, &moved);
        assert_eq!(sync(&mut registry, &listing(vec![resumed]), 4), []);
        assert!(matches!(location(&registry), Location::Pane { .. }));
        // Back in the background: the pane no longer holds it.
        sync(
            &mut registry,
            &listing(vec![listed("conv", AgentState::Working, true)]),
            5,
        );
        assert_eq!(location(&registry), Location::Background);
    }

    #[test]
    fn a_watched_agent_expires_once_its_session_has_been_gone_for_the_grace() {
        let grace = i64::try_from(GONE_GRACE_MS).unwrap();
        let mut registry = Registry::new();
        sync(&mut registry, &listing(vec![interactive("term")]), 1_000);
        let agent = owner(&registry, "term").id;
        // Its session ended: it shows as stopped, for a while.
        sync(&mut registry, &listing(vec![]), 2_000);
        let ended = registry.agent(agent).unwrap();
        assert_eq!(ended.state, AgentState::Stopped);
        assert_eq!(ended.detail.as_deref(), Some("the session ended"));
        assert_eq!(sync(&mut registry, &listing(vec![]), 2_000 + grace - 1), []);
        // Then it leaves the registry.
        let events = sync(&mut registry, &listing(vec![]), 2_000 + grace);
        assert_eq!(
            events,
            [Event::new(
                Timestamp::from_millis(2_000 + grace),
                EventKind::AgentExpired { agent }
            )]
        );
        assert!(registry.agent(agent).is_none());
        assert_eq!(sync(&mut registry, &listing(vec![]), 3_000 + grace), []);
    }

    #[test]
    fn a_session_back_within_the_grace_keeps_its_agent() {
        let mut registry = Registry::new();
        sync(&mut registry, &listing(vec![interactive("term")]), 1_000);
        let agent = owner(&registry, "term").id;
        sync(&mut registry, &listing(vec![]), 2_000);
        sync(&mut registry, &listing(vec![interactive("term")]), 3_000);
        let back = registry.agent(agent).unwrap();
        assert_eq!(back.state, AgentState::Idle);
        let much_later = 3_000 + 10 * i64::try_from(GONE_GRACE_MS).unwrap();
        assert_eq!(
            sync(
                &mut registry,
                &listing(vec![interactive("term")]),
                much_later
            ),
            []
        );
    }

    #[test]
    fn a_finished_session_removed_from_the_listing_expires_at_once() {
        let grace = i64::try_from(GONE_GRACE_MS).unwrap();
        let mut registry = Registry::new();
        sync(
            &mut registry,
            &listing(vec![
                listed("done", AgentState::Stopped, false),
                listed("failed", AgentState::Failed, false),
            ]),
            1_000,
        );
        // Long after they ended, the person removes them (`claude rm`).
        let events = sync(&mut registry, &listing(vec![]), 1_000 + 5 * grace);
        assert_eq!(events.len(), 2, "{events:?}");
        assert!(
            events
                .iter()
                .all(|event| matches!(event.kind, EventKind::AgentExpired { .. }))
        );
        assert_eq!(registry.agents().count(), 0);
    }

    #[test]
    fn a_running_background_session_that_is_removed_is_lost_then_expires() {
        let grace = i64::try_from(GONE_GRACE_MS).unwrap();
        let mut registry = Registry::new();
        sync(
            &mut registry,
            &listing(vec![listed("bg", AgentState::Working, true)]),
            1_000,
        );
        let agent = owner(&registry, "bg").id;
        sync(&mut registry, &listing(vec![]), 2_000);
        assert_eq!(registry.agent(agent).unwrap().state, AgentState::Lost);
        sync(&mut registry, &listing(vec![]), 2_000 + grace);
        assert!(registry.agent(agent).is_none());
    }

    #[test]
    fn an_expired_session_comes_back_when_it_is_listed_again() {
        let grace = i64::try_from(GONE_GRACE_MS).unwrap();
        let mut registry = Registry::new();
        sync(&mut registry, &listing(vec![interactive("term")]), 1_000);
        let first = owner(&registry, "term").id;
        sync(&mut registry, &listing(vec![]), 2_000);
        sync(&mut registry, &listing(vec![]), 2_000 + grace);
        assert!(registry.agent(first).is_none());
        // The same conversation, resumed under the same session id.
        sync(
            &mut registry,
            &listing(vec![interactive("term")]),
            9_000 + grace,
        );
        let back = owner(&registry, "term");
        assert_ne!(back.id, first);
        assert_eq!(back.state, AgentState::Idle);
    }

    /// An interactive session with its process and its start time.
    fn process(id: &str, pid: u32, started: i64) -> ListedSession {
        ListedSession {
            pid: Some(pid),
            started_at: Some(Timestamp::from_millis(started)),
            name: Some("alpha-f7".into()),
            ..interactive(id)
        }
    }

    /// A conversation as Claude Code lists it once it runs in the
    /// background: a handle, another process, and when it got there.
    fn in_background(id: &str, name: &str, started: i64) -> ListedSession {
        ListedSession {
            name: Some(name.to_owned()),
            pid: Some(9_000),
            started_at: Some(Timestamp::from_millis(started)),
            ..listed(id, AgentState::Idle, true)
        }
    }

    #[test]
    fn a_conversation_moved_to_the_background_keeps_its_agent() {
        // The left arrow on the empty prompt of a session in a terminal:
        // another process, another session id, and a name Claude Code made
        // from the conversation.
        let mut registry = Registry::new();
        let before = listing(vec![process("first", 100, 1_000)]);
        sync(&mut registry, &before, 60_000);
        let agent = owner(&registry, "first").id;
        assert_eq!(registry.agent(agent).unwrap().location, Location::Terminal);
        let after = listing(vec![in_background("second", "Fix the invoices", 61_000)]);
        let at = Timestamp::from_millis(62_000);
        let moved = moved_to_background(&registry, &before.sessions, &after, at);
        assert_eq!(moved.len(), 2, "{moved:?}");
        apply_all(&mut registry, &moved);
        sync(&mut registry, &after, 62_000);
        assert_eq!(registry.agents().count(), 1, "the same row, and no other");
        let kept = registry.agent(agent).unwrap();
        assert_eq!(kept.sessions.len(), 1);
        assert_eq!(
            kept.current_session().unwrap().key,
            SessionKey::claude("second")
        );
        assert_eq!(kept.location, Location::Background);
        assert_eq!(kept.name, "Fix the invoices");
        assert_eq!(kept.state, AgentState::Idle);
        let first = SessionKey::claude("first");
        assert!(registry.agent_for_session(&first).is_none());
    }

    /// The same conversation during its first second in the background:
    /// listed, without a process yet, and waiting for the person, as Claude
    /// Code says of it.
    fn arriving(id: &str, name: &str, started: i64) -> ListedSession {
        ListedSession {
            alive: false,
            pid: None,
            state: AgentState::NeedsInput {
                request: InputRequest::Other {
                    detail: "blocked".into(),
                },
            },
            ..in_background(id, name, started)
        }
    }

    #[test]
    fn a_conversation_is_followed_before_it_has_a_process_in_the_background() {
        // What Claude Code lists during the second that follows the key:
        // both sessions, then the new one alone, neither time with a process.
        let mut registry = Registry::new();
        let before = listing(vec![process("first", 100, 1_000)]);
        sync(&mut registry, &before, 60_000);
        let agent = owner(&registry, "first").id;
        let both = listing(vec![
            process("first", 100, 1_000),
            arriving("second", "alpha-f7", 61_000),
        ]);
        assert_eq!(sync(&mut registry, &both, 61_100), []);
        assert!(settling(&registry, &both, Timestamp::from_millis(61_100)));
        let alone = listing(vec![arriving("second", "alpha-f7", 61_000)]);
        let at = Timestamp::from_millis(61_900);
        let moved = moved_to_background(&registry, &both.sessions, &alone, at);
        assert_eq!(moved.len(), 2, "{moved:?}");
        apply_all(&mut registry, &moved);
        sync(&mut registry, &alone, 61_900);
        assert_eq!(registry.agents().count(), 1, "the same row, and no other");
        let kept = registry.agent(agent).unwrap();
        assert_eq!(
            kept.current_session().unwrap().key,
            SessionKey::claude("second")
        );
        assert_eq!(kept.location, Location::Background);
        assert!(kept.state.is_live(), "it did not end: {:?}", kept.state);
        // Once it has a process, its state is all that changes.
        let running = listing(vec![in_background("second", "alpha-f7", 61_000)]);
        sync(&mut registry, &running, 63_900);
        assert_eq!(registry.agents().count(), 1);
        assert_eq!(registry.agent(agent).unwrap().state, AgentState::Idle);
    }

    #[test]
    fn a_finished_background_session_is_no_conversation_on_its_way() {
        // No process either, but Claude Code says that it stopped: a
        // session that ended and that it still lists.
        let mut registry = Registry::new();
        let before = listing(vec![process("first", 100, 1_000)]);
        sync(&mut registry, &before, 60_000);
        let finished = ListedSession {
            state: AgentState::Stopped,
            ..arriving("old", "an older task", 30_000)
        };
        let after = listing(vec![finished.clone()]);
        let at = Timestamp::from_millis(62_000);
        let none: Vec<Event> = Vec::new();
        assert_eq!(
            moved_to_background(&registry, &before.sessions, &after, at),
            none
        );
        // Nor is one said to wait, without a process, a minute after it
        // started: whatever it was, it is not arriving.
        let arrival = i64::try_from(ARRIVAL_MS).unwrap();
        let stale = listing(vec![arriving("stale", "a task", 2_000)]);
        let late = Timestamp::from_millis(2_000 + arrival);
        assert_eq!(
            moved_to_background(&registry, &before.sessions, &stale, late),
            none
        );
        // A long conversation may take that long to get its process: just
        // before, it still is.
        let soon = Timestamp::from_millis(2_000 + arrival - 1);
        assert_eq!(
            moved_to_background(&registry, &before.sessions, &stale, soon).len(),
            2
        );
        // Beside the session in the terminal, it is a row of its own at once.
        let both = listing(vec![process("first", 100, 1_000), finished]);
        assert!(!settling(&registry, &both, at));
        sync(&mut registry, &both, 62_000);
        assert_eq!(registry.agents().count(), 2);
        assert_eq!(owner(&registry, "old").state, AgentState::Stopped);
    }

    #[test]
    fn a_listing_that_shows_both_sessions_makes_no_second_agent() {
        // Claude Code lists the background session a moment before it stops
        // listing the one in the terminal.
        let mut registry = Registry::new();
        let before = listing(vec![process("first", 100, 1_000)]);
        sync(&mut registry, &before, 60_000);
        let agent = owner(&registry, "first").id;
        let both = listing(vec![
            process("first", 100, 1_000),
            in_background("second", "alpha-f7", 61_000),
        ]);
        assert_eq!(sync(&mut registry, &both, 61_200), []);
        assert!(settling(&registry, &both, Timestamp::from_millis(61_200)));
        assert_eq!(registry.agents().count(), 1);
        // The next listing shows the background session alone: the agent's.
        let after = listing(vec![in_background("second", "alpha-f7", 61_000)]);
        let at = Timestamp::from_millis(63_200);
        let moved = moved_to_background(&registry, &both.sessions, &after, at);
        apply_all(&mut registry, &moved);
        sync(&mut registry, &after, 63_200);
        assert_eq!(registry.agents().count(), 1);
        assert_eq!(owner(&registry, "second").id, agent);
    }

    #[test]
    fn a_background_session_beside_a_terminal_becomes_an_agent_after_a_moment() {
        // Someone starts a background session where a terminal session
        // runs, and that one stays: two conversations, two agents.
        let wait = i64::try_from(MOVE_SETTLE_MS).unwrap();
        let mut registry = Registry::new();
        let both = listing(vec![
            process("first", 100, 1_000),
            in_background("second", "another task", 61_000),
        ]);
        sync(&mut registry, &both, 60_000);
        assert_eq!(registry.agents().count(), 1);
        assert_eq!(sync(&mut registry, &both, 61_000 + wait - 1), []);
        sync(&mut registry, &both, 61_000 + wait);
        assert_eq!(registry.agents().count(), 2);
        assert!(!settling(
            &registry,
            &both,
            Timestamp::from_millis(61_000 + wait)
        ));
    }

    #[test]
    fn a_background_session_waits_for_no_terminal_of_another_directory() {
        let mut registry = Registry::new();
        let elsewhere = ListedSession {
            cwd: "/work/beta".into(),
            ..process("first", 100, 1_000)
        };
        let ended = ListedSession {
            alive: false,
            ..process("ended", 101, 1_000)
        };
        let both = listing(vec![
            elsewhere,
            ended,
            in_background("second", "a task", 61_000),
        ]);
        sync(&mut registry, &both, 61_100);
        assert_eq!(owner(&registry, "second").location, Location::Background);
    }

    #[test]
    fn a_session_that_left_is_continued_only_without_a_doubt() {
        let mut registry = Registry::new();
        let before = listing(vec![process("first", 100, 1_000)]);
        sync(&mut registry, &before, 60_000);
        let at = Timestamp::from_millis(62_000);
        let none: Vec<Event> = Vec::new();
        let moved = |registry: &Registry, after: &Listing| {
            moved_to_background(registry, &before.sessions, after, at)
        };
        // Two sessions arrived under other names: which one is it?
        let two = listing(vec![
            in_background("second", "a task", 61_000),
            in_background("third", "another task", 61_500),
        ]);
        assert_eq!(moved(&registry, &two), none);
        // One of them carries its name: that one.
        let namesake = listing(vec![
            in_background("second", "a task", 61_000),
            in_background("third", "alpha-f7", 61_500),
        ]);
        let events = moved(&registry, &namesake);
        assert!(
            matches!(
                events.last().map(|event| &event.kind),
                Some(EventKind::SessionLinked { session, .. }) if session.id == "third"
            ),
            "{events:?}"
        );
        // A session of another directory, one that started before the
        // session left behind, one that ended, one in a terminal: no.
        for other in [
            ListedSession {
                cwd: "/work/beta".into(),
                ..in_background("second", "a task", 61_000)
            },
            in_background("second", "a task", 500),
            ListedSession {
                alive: false,
                state: AgentState::Stopped,
                ..in_background("second", "a task", 61_000)
            },
            process("second", 101, 61_000),
        ] {
            assert_eq!(moved(&registry, &listing(vec![other])), none);
        }
        // The session still listed did not leave.
        let still = listing(vec![
            process("first", 100, 1_000),
            in_background("second", "a task", 61_000),
        ]);
        assert_eq!(moved(&registry, &still), none);
        // A session that already has an owner is not taken from it.
        let after = listing(vec![in_background("second", "a task", 61_000)]);
        let mut owned = registry.clone();
        sync(&mut owned, &after, 70_000);
        assert_eq!(moved(&owned, &after), none);
        // A forgotten session is nobody's to take.
        let mut forgetful = registry.clone();
        let stranger = AgentId::from_ulid(Ulid::from_parts(5, 5));
        forgetful
            .apply(&Event::new(
                at,
                EventKind::AgentRegistered {
                    agent: stranger,
                    name: "forgotten".into(),
                    role: Role::Independent,
                    harness: HarnessKind::ClaudeCode,
                    launched_by: LaunchedBy::Outside,
                    parent: None,
                    mission: None,
                    workspace: Workspace::at("/work/alpha"),
                    location: Location::Background,
                },
            ))
            .unwrap();
        forgetful
            .apply(&Event::new(
                at,
                EventKind::SessionLinked {
                    agent: stranger,
                    session: SessionKey::claude("second"),
                    origin: SessionOrigin::Discovered,
                },
            ))
            .unwrap();
        forgetful
            .apply(&Event::new(
                at,
                EventKind::AgentForgotten { agent: stranger },
            ))
            .unwrap();
        assert_eq!(moved(&forgetful, &after), none);
    }

    #[test]
    fn two_sessions_that_left_one_directory_are_told_apart_by_name_only() {
        let mut registry = Registry::new();
        let other = ListedSession {
            name: Some("alpha-0b".into()),
            ..process("other", 200, 2_000)
        };
        let before = listing(vec![process("first", 100, 1_000), other]);
        sync(&mut registry, &before, 60_000);
        let at = Timestamp::from_millis(62_000);
        // One arrival under a new name: whose is it?
        let unnamed = listing(vec![in_background("second", "a task", 61_000)]);
        let none: Vec<Event> = Vec::new();
        assert_eq!(
            moved_to_background(&registry, &before.sessions, &unnamed, at),
            none
        );
        // Both arrive under the names they had: each agent keeps its own.
        let named = listing(vec![
            in_background("second", "alpha-0b", 61_000),
            in_background("third", "alpha-f7", 61_200),
        ]);
        let events = moved_to_background(&registry, &before.sessions, &named, at);
        let mut followed = registry.clone();
        apply_all(&mut followed, &events);
        assert_eq!(owner(&followed, "third").name, "alpha-f7");
        assert_eq!(owner(&followed, "second").name, "alpha-0b");
        assert_eq!(followed.agents().count(), 2);
        // Two sessions under one name left, and one arrived under it: one
        // agent takes it, and the other is not given the same session.
        let mut twins = Registry::new();
        let before = listing(vec![
            process("first", 100, 1_000),
            process("other", 200, 2_000),
        ]);
        sync(&mut twins, &before, 60_000);
        let one = listing(vec![in_background("second", "alpha-f7", 61_000)]);
        let events = moved_to_background(&twins, &before.sessions, &one, at);
        assert_eq!(events.len(), 2, "{events:?}");
        apply_all(&mut twins, &events);
        assert_eq!(twins.agents().count(), 2);
    }

    #[test]
    fn only_a_watched_agent_in_a_terminal_is_moved_by_a_listing() {
        // An agent volee launched learns it from its hooks.
        let mut registry = Registry::new();
        let agent = launched(&mut registry, 1, "/work/alpha");
        let before = listing(vec![process("mine", 5, 1_000)]);
        sync(&mut registry, &before, 1_500);
        assert!(
            registry
                .agent(agent)
                .unwrap()
                .owns_session(&SessionKey::claude("mine"))
        );
        let after = listing(vec![in_background("next", "a task", 1_800)]);
        let at = Timestamp::from_millis(2_000);
        let none: Vec<Event> = Vec::new();
        assert_eq!(
            moved_to_background(&registry, &before.sessions, &after, at),
            none
        );
        // A background session that is no longer listed was removed: the
        // one that starts next is another conversation.
        let mut registry = Registry::new();
        let before = listing(vec![in_background("first", "a task", 1_000)]);
        sync(&mut registry, &before, 60_000);
        let after = listing(vec![in_background("second", "a task", 61_000)]);
        assert_eq!(
            moved_to_background(&registry, &before.sessions, &after, at),
            none
        );
    }

    #[test]
    fn a_session_started_elsewhere_waits_until_it_has_settled() {
        let settle = i64::try_from(SETTLE_MS).unwrap();
        let mut registry = Registry::new();
        // A program's one-off call: listed at two seconds, gone at four.
        let young = listing(vec![process("blip", 77, 1_000)]);
        assert!(settling(&registry, &young, Timestamp::from_millis(3_000)));
        assert_eq!(sync(&mut registry, &young, 3_000), []);
        assert_eq!(sync(&mut registry, &listing(vec![]), 5_000), []);
        assert_eq!(registry.agents().count(), 0, "it never was a row");

        // A session that lasts becomes an agent once it has settled.
        let lasting = listing(vec![process("real", 78, 10_000)]);
        assert_eq!(sync(&mut registry, &lasting, 10_000 + settle - 1), []);
        sync(&mut registry, &lasting, 10_000 + settle);
        assert_eq!(owner(&registry, "real").state, AgentState::Idle);
        assert!(!settling(
            &registry,
            &lasting,
            Timestamp::from_millis(10_000 + settle)
        ));
    }

    #[test]
    fn a_background_session_or_one_without_a_start_time_is_taken_at_once() {
        let mut registry = Registry::new();
        let mut background = listed("bg", AgentState::Working, true);
        background.started_at = Some(Timestamp::from_millis(1_000));
        // In another directory: beside a terminal session of its own
        // directory, a background session waits a moment.
        let no_start = ListedSession {
            cwd: "/work/beta".into(),
            ..interactive("no-start")
        };
        let both = listing(vec![background, no_start]);
        assert!(!settling(&registry, &both, Timestamp::from_millis(1_001)));
        sync(&mut registry, &both, 1_001);
        assert_eq!(registry.agents().count(), 2);
    }

    #[test]
    fn a_launch_takes_its_session_before_it_has_settled() {
        let mut registry = Registry::new();
        let agent = launched(&mut registry, 1, "/work/alpha");
        sync(
            &mut registry,
            &listing(vec![process("mine", 5, 1_000)]),
            1_500,
        );
        let record = registry.agent(agent).unwrap();
        assert!(record.owns_session(&SessionKey::claude("mine")));
    }

    #[test]
    fn a_session_that_changes_its_id_in_place_keeps_its_agent() {
        // `/clear`: same process, same name, same start, another session id.
        let mut registry = Registry::new();
        let before = listing(vec![process("first", 29_159, 1_000)]);
        sync(&mut registry, &before, 60_000);
        let agent = owner(&registry, "first").id;
        let after = listing(vec![process("second", 29_159, 1_000)]);
        let at = Timestamp::from_millis(61_000);
        let moved = continued(&registry, &before.sessions, &after, at);
        assert_eq!(moved.len(), 2, "{moved:?}");
        apply_all(&mut registry, &moved);
        assert_eq!(
            sync(&mut registry, &after, 61_000),
            [],
            "nothing left to do"
        );
        let kept = registry.agent(agent).unwrap();
        assert_eq!(kept.sessions.len(), 1);
        assert_eq!(
            kept.current_session().unwrap().key,
            SessionKey::claude("second")
        );
        assert_eq!(kept.state, AgentState::Idle);
        assert_eq!(registry.agents().count(), 1);
        let first = SessionKey::claude("first");
        assert!(registry.agent_for_session(&first).is_none());
    }

    #[test]
    fn another_process_never_continues_a_session() {
        let mut registry = Registry::new();
        let before = listing(vec![process("first", 100, 1_000)]);
        sync(&mut registry, &before, 60_000);
        let at = Timestamp::from_millis(61_000);
        let none: Vec<Event> = Vec::new();
        // Another process id, or the same one reused by a process started
        // later: someone else.
        for other in [
            process("second", 101, 1_000),
            process("second", 100, 50_000),
        ] {
            let after = listing(vec![other]);
            assert_eq!(continued(&registry, &before.sessions, &after, at), none);
        }
        // The old session still listed next to the new one: two sessions.
        let both = listing(vec![
            process("first", 100, 1_000),
            process("second", 100, 1_000),
        ]);
        assert_eq!(continued(&registry, &before.sessions, &both, at), none);
        // A session that already has an owner is not taken from it.
        let mut owned = registry.clone();
        sync(&mut owned, &both, 70_000);
        let taken = listing(vec![process("second", 100, 1_000)]);
        assert_eq!(continued(&owned, &before.sessions, &taken, at), none);
    }

    #[test]
    fn an_agent_volee_launched_is_not_moved_by_a_listing() {
        // Its hooks say when its session changes.
        let mut registry = Registry::new();
        let agent = launched(&mut registry, 1, "/work/alpha");
        let before = listing(vec![process("mine", 5, 1_000)]);
        sync(&mut registry, &before, 1_500);
        let record = registry.agent(agent).unwrap();
        assert!(record.owns_session(&SessionKey::claude("mine")));
        let after = listing(vec![process("next", 5, 1_000)]);
        let none: Vec<Event> = Vec::new();
        let at = Timestamp::from_millis(2_000);
        assert_eq!(continued(&registry, &before.sessions, &after, at), none);
    }

    #[test]
    fn an_agent_volee_launched_never_expires() {
        let grace = i64::try_from(GONE_GRACE_MS).unwrap();
        let mut registry = Registry::new();
        let agent = launched(&mut registry, 1, "/work/alpha");
        sync(
            &mut registry,
            &listing(vec![listed("s-1", AgentState::Working, true)]),
            5,
        );
        sync(&mut registry, &listing(vec![]), 1_000);
        assert_eq!(registry.agent(agent).unwrap().state, AgentState::Lost);
        assert_eq!(
            sync(&mut registry, &listing(vec![]), 1_000 + 10 * grace),
            []
        );
        assert!(registry.agent(agent).is_some());
    }

    #[test]
    fn an_incomplete_listing_never_expires_an_agent() {
        let grace = i64::try_from(GONE_GRACE_MS).unwrap();
        let mut registry = Registry::new();
        sync(
            &mut registry,
            &listing(vec![listed("done", AgentState::Stopped, false)]),
            1_000,
        );
        let partial = Listing {
            complete: false,
            ..listing(vec![])
        };
        assert_eq!(sync(&mut registry, &partial, 1_000 + 5 * grace), []);
        assert_eq!(registry.agents().count(), 1);
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
        // An agent volee launched keeps the sessions of its conversation.
        let mut registry = Registry::new();
        let agent = launched(&mut registry, 1, "/work/alpha");
        for kind in [
            EventKind::SessionLinked {
                agent,
                session: SessionKey::claude("old11111"),
                origin: SessionOrigin::Started,
            },
            EventKind::StateChanged {
                agent,
                state: AgentState::Idle,
                detail: None,
            },
            EventKind::SessionLinked {
                agent,
                session: SessionKey::claude("new22222"),
                origin: SessionOrigin::Forked,
            },
        ] {
            registry
                .apply(&Event::new(Timestamp::from_millis(2), kind))
                .unwrap();
        }
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

    #[test]
    fn an_agent_launching_a_new_session_is_not_lost() {
        // A resume: the old session is gone and the fork is not listed yet.
        let mut registry = Registry::new();
        let agent = launched(&mut registry, 1, "/work/alpha");
        for kind in [
            EventKind::SessionLinked {
                agent,
                session: SessionKey::claude("old"),
                origin: SessionOrigin::Started,
            },
            EventKind::StateChanged {
                agent,
                state: AgentState::Launching,
                detail: None,
            },
        ] {
            registry
                .apply(&Event::new(Timestamp::from_millis(2), kind))
                .unwrap();
        }
        let events = reconcile(
            &registry,
            &listing(vec![]),
            Timestamp::from_millis(5),
            ids(),
        );
        assert_eq!(events, vec![]);
    }

    #[test]
    fn a_resumed_agent_adopts_its_fork_in_its_directory() {
        let mut registry = Registry::new();
        let agent = launched(&mut registry, 1, "/work/alpha");
        for kind in [
            EventKind::SessionLinked {
                agent,
                session: SessionKey::claude("old"),
                origin: SessionOrigin::Started,
            },
            EventKind::StateChanged {
                agent,
                state: AgentState::Launching,
                detail: None,
            },
        ] {
            registry
                .apply(&Event::new(Timestamp::from_millis(2), kind))
                .unwrap();
        }
        let events = reconcile(
            &registry,
            &listing(vec![listed("fork", AgentState::Working, true)]),
            Timestamp::from_millis(5),
            ids(),
        );
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            EventKind::SessionLinked { agent: linked, origin: SessionOrigin::Forked, .. }
                if *linked == agent
        )));
        assert!(
            events
                .iter()
                .all(|event| !matches!(event.kind, EventKind::AgentRegistered { .. }))
        );
    }

    #[test]
    fn an_agent_whose_session_is_listed_adopts_nothing_more() {
        let mut registry = Registry::new();
        let agent = launched(&mut registry, 1, "/work/alpha");
        registry
            .apply(&Event::new(
                Timestamp::from_millis(2),
                EventKind::SessionLinked {
                    agent,
                    session: SessionKey::claude("mine"),
                    origin: SessionOrigin::Started,
                },
            ))
            .unwrap();
        // Still launching (no hook yet), its session listed, and a stranger
        // starts in the same directory: the stranger is someone else.
        let events = reconcile(
            &registry,
            &listing(vec![
                listed("mine", AgentState::Working, true),
                listed("stranger", AgentState::Working, true),
            ]),
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
