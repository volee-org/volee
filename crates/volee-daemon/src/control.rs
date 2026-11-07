//! Acting on agents (ADR 0002): previewing, diffing, taking over and
//! stopping them.
//!
//! The harness acts on a session as it last listed it: the listing gives the
//! handle its commands take. The daemon keeps the last listing; a session it
//! started or resumed and no listing shows yet is reached through the handle
//! the launch printed (the state's expected sessions), so that an agent can
//! be previewed, taken over or stopped right after its launch. Every slow
//! step (the harness, git) runs off the state lock.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::sync::Arc;

use volee_core::harness::SessionKey;
use volee_core::reconcile::{ListedSession, Listing};
use volee_core::role::LaunchedBy;
use volee_core::workspace::Location;
use volee_core::{Agent, AgentId};
use volee_git::DiffFormat;
use volee_harness::{Harness, HarnessError};
use volee_proto::methods::{AgentParams, AttachCommand, DiffParams, DiffText, Empty, PreviewText};

use crate::error::DaemonError;
use crate::launch::{blocking, git, harness};
use crate::server::Shared;

/// The most diff text one request returns, in bytes. Past it, the text is
/// cut at a line break and flagged as truncated.
pub(crate) const MAX_DIFF_BYTES: usize = 512 * 1024;

/// What a person reads before taking an agent over.
pub(crate) const ATTACH_HINT: &str = "Press Ctrl+Z to come back; the agent keeps running.";

/// The sessions of the last harness listing, which the daemon acts on.
#[derive(Debug, Default)]
pub(crate) struct Sessions {
    listed: HashMap<SessionKey, ListedSession>,
}

/// Why the daemon cannot act on an agent's session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoSession {
    /// The agent has no session.
    None,
    /// The session runs in a terminal of its own (for Claude Code, an
    /// interactive session): only that terminal shows and controls it.
    OwnTerminal,
    /// The session exists, but no listing has shown it yet.
    NotListedYet,
}

impl Sessions {
    /// Records a listing, which replaces the previous one.
    pub(crate) fn record(&mut self, listing: &Listing) {
        self.listed = listing
            .sessions
            .iter()
            .map(|session| (session.key.clone(), session.clone()))
            .collect();
    }

    /// The session to act on for `agent`: its current session as the last
    /// listing showed it, else the session the daemon started or resumed for
    /// it, known by the handle the launch printed (`expected`).
    pub(crate) fn session_for(
        &self,
        agent: &Agent,
        expected: Option<&str>,
    ) -> Result<ListedSession, NoSession> {
        let current = agent.current_session().map(|link| &link.key);
        if let Some(listed) = current.and_then(|key| self.listed.get(key)) {
            return if listed.handle.is_some() {
                Ok(listed.clone())
            } else {
                Err(NoSession::OwnTerminal)
            };
        }
        if let Some(handle) = expected {
            let key = current.cloned().unwrap_or_else(|| SessionKey {
                harness: agent.harness.clone(),
                id: handle.to_owned(),
            });
            return Ok(ListedSession {
                alive: true,
                background: true,
                handle: Some(handle.to_owned()),
                ..ListedSession::new(key, agent.workspace.cwd.clone(), agent.state.clone())
            });
        }
        match (current, &agent.location) {
            (None, _) => Err(NoSession::None),
            (Some(_), Location::Terminal | Location::Pane { .. }) => Err(NoSession::OwnTerminal),
            (Some(_), _) => Err(NoSession::NotListedYet),
        }
    }
}

/// What an action on an agent's session needs.
struct Target {
    harness: Arc<dyn Harness>,
    name: String,
    launched_by: LaunchedBy,
    session: ListedSession,
}

/// The harness and the session to act on for `agent`, or a refusal that
/// says what to do instead.
async fn target(shared: &Arc<Shared>, agent: AgentId) -> Result<Target, DaemonError> {
    let harness = harness(shared)?;
    let state = shared.state.lock().await;
    let record = state
        .registry()
        .agent(agent)
        .ok_or_else(|| DaemonError::NotFound(format!("unknown agent {agent}")))?;
    if record.harness != harness.kind() {
        return Err(DaemonError::Unavailable(format!(
            "{} runs on {}, which this daemon cannot drive",
            record.name, record.harness
        )));
    }
    let session = state
        .sessions()
        .session_for(record, state.expected_handle(agent))
        .map_err(|reason| refusal(&record.name, reason))?;
    Ok(Target {
        harness,
        name: record.name.clone(),
        launched_by: record.launched_by,
        session,
    })
}

/// Why nothing can be done with the session of the agent `name`.
fn refusal(name: &str, reason: NoSession) -> DaemonError {
    match reason {
        NoSession::None => DaemonError::Refused(format!("{name} has no session")),
        NoSession::OwnTerminal => own_terminal(name),
        NoSession::NotListedYet => DaemonError::Unavailable(format!(
            "volee has not listed the session of {name} yet: try again in a moment"
        )),
    }
}

fn own_terminal(name: &str) -> DaemonError {
    DaemonError::Refused(format!(
        "{name} runs in a terminal of its own: switch to that terminal to see or control it"
    ))
}

/// A harness failure about the agent `name`, said for people.
fn harness_error(name: &str, error: HarnessError) -> DaemonError {
    match error {
        HarnessError::UnknownSession { .. } => DaemonError::NotFound(format!(
            "the harness no longer knows the session of {name}: it was removed"
        )),
        HarnessError::NotControllable { .. } => own_terminal(name),
        error => DaemonError::Harness(error),
    }
}

/// `agents.preview`: the agent's recent terminal output, raw, with its
/// terminal escape sequences, for a terminal emulator to render.
pub(crate) async fn preview(
    shared: &Arc<Shared>,
    params: AgentParams,
) -> Result<PreviewText, DaemonError> {
    let Target {
        harness,
        name,
        session,
        ..
    } = target(shared, params.agent).await?;
    let screen = blocking(move || harness.logs(&session))
        .await?
        .map_err(|error| harness_error(&name, error))?;
    Ok(PreviewText {
        screen: String::from_utf8_lossy(&screen).into_owned(),
    })
}

/// `agents.attach`: the command that takes the agent over in the client's
/// terminal.
pub(crate) async fn attach(
    shared: &Arc<Shared>,
    params: AgentParams,
) -> Result<AttachCommand, DaemonError> {
    let Target {
        harness,
        name,
        session,
        ..
    } = target(shared, params.agent).await?;
    let command = harness
        .attach_command(&session)
        .map_err(|error| harness_error(&name, error))?;
    let text = |value: &OsStr| value.to_string_lossy().into_owned();
    Ok(AttachCommand {
        program: text(command.get_program()),
        args: command.get_args().map(text).collect(),
        cwd: command
            .get_current_dir()
            .map(|dir| dir.display().to_string()),
        env_remove: command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| text(key))
            .collect(),
        hint: ATTACH_HINT.to_owned(),
    })
}

/// `agents.stop`: stops the agent's session; the conversation is kept.
/// volee stops only the agents it launched (ADR 0011).
pub(crate) async fn stop(shared: &Arc<Shared>, params: AgentParams) -> Result<Empty, DaemonError> {
    let Target {
        harness,
        name,
        launched_by,
        session,
    } = target(shared, params.agent).await?;
    if launched_by == LaunchedBy::Outside {
        return Err(DaemonError::Refused(format!(
            "{name} was not launched by volee, which stops only the agents it launched: stop it \
             where it was started"
        )));
    }
    if !session.alive {
        return Err(DaemonError::Refused(format!("{name} is not running")));
    }
    blocking(move || harness.stop(&session))
        .await?
        .map_err(|error| harness_error(&name, error))?;
    Ok(Empty {})
}

/// `agents.diff`: the agent's changes against the merge base with the branch
/// it started from, as statistics or as a patch, at most
/// [`MAX_DIFF_BYTES`].
pub(crate) async fn diff(
    shared: &Arc<Shared>,
    params: DiffParams,
) -> Result<DiffText, DaemonError> {
    let git = git(shared)?;
    let (name, dir, base) = {
        let state = shared.state.lock().await;
        let agent = state
            .registry()
            .agent(params.agent)
            .ok_or_else(|| DaemonError::NotFound(format!("unknown agent {}", params.agent)))?;
        let workspace = &agent.workspace;
        (
            agent.name.clone(),
            workspace
                .worktree
                .clone()
                .unwrap_or_else(|| workspace.cwd.clone()),
            workspace.base.clone(),
        )
    };
    if !dir.is_dir() {
        return Err(DaemonError::Refused(format!(
            "the directory of {name} is gone: {}",
            dir.display()
        )));
    }
    let info = {
        let (git, dir) = (Arc::clone(&git), dir.clone());
        blocking(move || git.repo_info(&dir)).await??
    }
    .ok_or_else(|| DaemonError::Refused(format!("{name} does not work in a git repository")))?;
    let base = if let Some(base) = base {
        base
    } else {
        let (git, root) = (Arc::clone(&git), info.main_checkout);
        blocking(move || git.default_branch(&root))
            .await??
            .ok_or_else(|| {
                DaemonError::Refused(format!(
                    "volee does not know which branch {name} started from, and its repository \
                     has no main, master or develop branch"
                ))
            })?
    };
    let format = if params.full {
        DiffFormat::Patch
    } else {
        DiffFormat::Stat
    };
    let output = blocking(move || git.diff(&dir, &base, format, MAX_DIFF_BYTES)).await??;
    Ok(DiffText {
        text: output.text,
        truncated: output.truncated,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use ulid::Ulid;
    use volee_core::harness::{HarnessKind, SessionOrigin};
    use volee_core::role::Role;
    use volee_core::state::AgentState;
    use volee_core::time::Timestamp;
    use volee_core::workspace::Workspace;
    use volee_core::{Event, EventKind, Registry};

    use super::*;

    const KEY: &str = "7d3c1f0a-2b4e-4c6d-9e8f-0a1b2c3d4e5f";

    fn agent_id() -> AgentId {
        AgentId::from_ulid(Ulid::from_parts(1_000, 1))
    }

    /// A registry with one agent, launched by volee or found running, linked
    /// to `session` when given.
    fn registry(launched_by: LaunchedBy, location: Location, session: Option<&str>) -> Registry {
        let at = |kind| Event::new(Timestamp::from_millis(1), kind);
        let mut events = vec![at(EventKind::AgentRegistered {
            agent: agent_id(),
            name: "alpha".into(),
            role: Role::Worker,
            harness: HarnessKind::ClaudeCode,
            launched_by,
            parent: None,
            mission: None,
            workspace: Workspace::at("/work/alpha"),
            location,
        })];
        if let Some(id) = session {
            events.push(at(EventKind::SessionLinked {
                agent: agent_id(),
                session: SessionKey::claude(id),
                origin: SessionOrigin::Started,
            }));
        }
        Registry::from_events(&events).unwrap()
    }

    fn listed(id: &str, handle: Option<&str>) -> ListedSession {
        ListedSession {
            alive: true,
            background: handle.is_some(),
            handle: handle.map(str::to_owned),
            ..ListedSession::new(SessionKey::claude(id), "/work/alpha", AgentState::Working)
        }
    }

    fn listing(sessions: Vec<ListedSession>) -> Listing {
        Listing {
            harness: HarnessKind::ClaudeCode,
            sessions,
            complete: true,
        }
    }

    fn agent(registry: &Registry) -> &Agent {
        registry.agent(agent_id()).unwrap()
    }

    #[test]
    fn a_listed_session_is_acted_on_as_listed() {
        let registry = registry(LaunchedBy::Human, Location::Background, Some(KEY));
        let mut sessions = Sessions::default();
        sessions.record(&listing(vec![listed(KEY, Some("7d3c1f0a"))]));
        let session = sessions.session_for(agent(&registry), None).unwrap();
        assert_eq!(session.handle.as_deref(), Some("7d3c1f0a"));
        assert_eq!(session.key, SessionKey::claude(KEY));
    }

    #[test]
    fn the_listing_wins_over_the_launch_handle() {
        let registry = registry(LaunchedBy::Human, Location::Background, Some(KEY));
        let mut sessions = Sessions::default();
        sessions.record(&listing(vec![listed(KEY, Some("7d3c1f0a"))]));
        let session = sessions
            .session_for(agent(&registry), Some("feed5678"))
            .unwrap();
        assert_eq!(session.handle.as_deref(), Some("7d3c1f0a"));
    }

    #[test]
    fn a_launched_session_is_acted_on_before_any_listing() {
        let registry = registry(LaunchedBy::Human, Location::Background, None);
        let sessions = Sessions::default();
        assert_eq!(
            sessions.session_for(agent(&registry), None),
            Err(NoSession::None)
        );
        let session = sessions
            .session_for(agent(&registry), Some("7d3c1f0a"))
            .unwrap();
        assert_eq!(session.handle.as_deref(), Some("7d3c1f0a"));
        assert!(session.alive && session.background);
        assert_eq!(session.cwd, Path::new("/work/alpha"));
    }

    #[test]
    fn a_launched_session_with_a_known_key_keeps_it() {
        let registry = registry(LaunchedBy::Human, Location::Background, Some(KEY));
        let session = Sessions::default()
            .session_for(agent(&registry), Some("7d3c1f0a"))
            .unwrap();
        assert_eq!(session.key, SessionKey::claude(KEY));
    }

    #[test]
    fn a_session_gone_from_the_listing_is_not_acted_on_from_memory() {
        let registry = registry(LaunchedBy::Human, Location::Background, Some(KEY));
        let mut sessions = Sessions::default();
        sessions.record(&listing(vec![listed(KEY, Some("7d3c1f0a"))]));
        sessions.record(&listing(vec![]));
        assert_eq!(
            sessions.session_for(agent(&registry), None),
            Err(NoSession::NotListedYet)
        );
    }

    #[test]
    fn an_interactive_session_lives_in_its_own_terminal() {
        let registry = registry(LaunchedBy::Outside, Location::Terminal, Some(KEY));
        let mut sessions = Sessions::default();
        sessions.record(&listing(vec![listed(KEY, None)]));
        assert_eq!(
            sessions.session_for(agent(&registry), None),
            Err(NoSession::OwnTerminal)
        );
        sessions.record(&listing(vec![]));
        assert_eq!(
            sessions.session_for(agent(&registry), None),
            Err(NoSession::OwnTerminal)
        );
    }

    #[test]
    fn an_older_listed_session_is_never_used_for_the_current_one() {
        let mut registry = registry(LaunchedBy::Human, Location::Background, Some(KEY));
        registry
            .apply(&Event::new(
                Timestamp::from_millis(2),
                EventKind::SessionLinked {
                    agent: agent_id(),
                    session: SessionKey::claude("c4e5a6b7-8901-4cde-8f01-23456789abcd"),
                    origin: SessionOrigin::Forked,
                },
            ))
            .unwrap();
        let mut sessions = Sessions::default();
        sessions.record(&listing(vec![listed(KEY, Some("7d3c1f0a"))]));
        assert_eq!(
            sessions.session_for(agent(&registry), None),
            Err(NoSession::NotListedYet)
        );
    }
}
