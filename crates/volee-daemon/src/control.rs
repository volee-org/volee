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
use volee_core::message::{Message, MessageKind, Party, Receipt};
use volee_core::reconcile::{ListedSession, Listing};
use volee_core::workspace::Location;
use volee_core::{Agent, AgentId, Event, EventKind};
use volee_git::DiffFormat;
use volee_harness::{Harness, HarnessError};
use volee_mux::{Multiplexer, MuxError};
use volee_proto::methods::{
    AgentParams, AttachCommand, DiffParams, DiffText, Empty, PaneFocus, PreviewText,
};

use crate::error::DaemonError;
use crate::launch::{blocking, git, harness};
use crate::server::Shared;
use crate::state::State;

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
    /// Records a listing, which replaces the previous one of its harness.
    pub(crate) fn record(&mut self, listing: &Listing) {
        self.listed.retain(|key, _| key.harness != listing.harness);
        self.listed.extend(
            listing
                .sessions
                .iter()
                .map(|session| (session.key.clone(), session.clone())),
        );
    }

    /// A session as the last listing showed it.
    pub(crate) fn listed(&self, key: &SessionKey) -> Option<&ListedSession> {
        self.listed.get(key)
    }

    /// Every session of the last listings.
    pub(crate) fn all(&self) -> impl Iterator<Item = &ListedSession> {
        self.listed.values()
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
        // A launch in a host is no background session: its expected handle
        // only keeps it from looking stalled until its first hook.
        if let Some(handle) = expected.filter(|_| agent.location.host().is_none()) {
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

/// The multiplexer pane an agent runs in, when volee found it in one
/// (ADR 0015).
struct Pane {
    adapter: Arc<dyn Multiplexer>,
    target: String,
    name: String,
}

/// The pane `agent` runs in, if any of the daemon's multiplexers found it.
/// An unknown agent has none: the harness's path says what is wrong.
async fn pane(shared: &Arc<Shared>, agent: AgentId) -> Option<Pane> {
    let state = shared.state.lock().await;
    let record = state.registry().agent(agent)?;
    let Location::Pane {
        multiplexer,
        target,
    } = &record.location
    else {
        return None;
    };
    shared.panes.adapter(multiplexer).map(|adapter| Pane {
        adapter,
        target: target.clone(),
        name: record.name.clone(),
    })
}

/// The volee host an agent runs in (ADR 0019).
struct Hosted {
    socket: std::path::PathBuf,
    id: String,
    name: String,
}

/// The host `agent` runs in, if it runs in one.
async fn hosted(shared: &Arc<Shared>, agent: AgentId) -> Option<Hosted> {
    let state = shared.state.lock().await;
    let record = state.registry().agent(agent)?;
    let id = record.location.host()?.to_owned();
    Some(Hosted {
        socket: crate::launch::hosts_dir(shared).join(format!("{id}.sock")),
        id,
        name: record.name.clone(),
    })
}

/// How a client of the daemon names itself to hosts.
const HOST_CLIENT: &str = concat!("volee daemon ", env!("CARGO_PKG_VERSION"));

/// What a person reads before taking over an agent in a host.
const HOST_ATTACH_HINT: &str = "Press Ctrl+Q to come back; the agent keeps running.";

/// A background session volee does not host: shown, never driven (ADR 0023).
fn background_refusal(name: &str) -> DaemonError {
    DaemonError::Refused(format!(
        "{name} runs as a Claude Code background session, which volee only shows: press A to \
         adopt it into a volee host, then take it over"
    ))
}

/// A multiplexer failure about the pane of the agent `name`, said for
/// people.
fn pane_error(name: &str, adapter: &dyn Multiplexer, error: &MuxError) -> DaemonError {
    let multiplexer = adapter.name();
    DaemonError::Unavailable(format!(
        "cannot reach the {multiplexer} pane of {name}: {error}"
    ))
}

/// `agents.preview`: the agent's recent terminal output, raw, with its
/// terminal escape sequences, for a terminal emulator to render. For an
/// agent in a multiplexer pane, what the pane shows; for one that never had a
/// session, the end of its setup log when it has one (ADR 0017), which shows
/// a setup under way or why it failed.
pub(crate) async fn preview(
    shared: &Arc<Shared>,
    params: AgentParams,
) -> Result<PreviewText, DaemonError> {
    // An agent in a host: the host's exact screen (ADR 0019).
    if let Some(Hosted { socket, name, .. }) = hosted(shared, params.agent).await {
        let read = blocking(move || {
            volee_host::snapshot(&socket, HOST_CLIENT, std::time::Duration::from_secs(3))
        })
        .await?;
        return match read {
            Ok((_, screen)) => Ok(PreviewText {
                screen: String::from_utf8_lossy(&screen).into_owned(),
            }),
            Err(_) => match setup_screen(shared, params.agent).await {
                Some(screen) => Ok(PreviewText { screen }),
                None => Err(DaemonError::Unavailable(format!(
                    "{name} is not running: its volee host is gone"
                ))),
            },
        };
    }
    if let Some(Pane {
        adapter,
        target,
        name,
    }) = pane(shared, params.agent).await
    {
        let reader = Arc::clone(&adapter);
        let screen = blocking(move || reader.capture(&target))
            .await?
            .map_err(|error| pane_error(&name, adapter.as_ref(), &error))?;
        return Ok(PreviewText { screen });
    }
    let Target {
        harness,
        name,
        session,
        ..
    } = match target(shared, params.agent).await {
        Ok(target) => target,
        Err(error) => {
            return match setup_screen(shared, params.agent).await {
                Some(screen) => Ok(PreviewText { screen }),
                None => Err(error),
            };
        }
    };
    let screen = blocking(move || harness.logs(&session))
        .await?
        .map_err(|error| harness_error(&name, error))?;
    Ok(PreviewText {
        screen: String::from_utf8_lossy(&screen).into_owned(),
    })
}

/// How much of a setup log a preview shows, from its end.
const SETUP_PREVIEW_BYTES: u64 = 64 * 1024;

/// The end of the setup log of an agent that never had a session, as
/// terminal output under a line that says what it is; `None` without one.
async fn setup_screen(shared: &Arc<Shared>, agent: AgentId) -> Option<String> {
    let never_ran = shared
        .state
        .lock()
        .await
        .registry()
        .agent(agent)
        .is_some_and(|record| record.sessions.is_empty());
    if !never_ran {
        return None;
    }
    let log = shared.paths.agent_dir(agent).join("setup.log");
    let text = blocking(move || read_tail(&log, SETUP_PREVIEW_BYTES))
        .await
        .ok()??;
    let mut screen =
        String::from("volee set up this worktree; no session has started. setup.log:\r\n\r\n");
    for line in text.lines() {
        screen.push_str(line);
        screen.push_str("\r\n");
    }
    Some(screen)
}

/// The last `bytes` of the file at `path`, from a line start; `None` when it
/// cannot be read.
fn read_tail(path: &std::path::Path, bytes: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw).ok()?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    Some(if start > 0 {
        // Drop the partial first line.
        text.split_once('\n')
            .map_or(text.clone(), |(_, rest)| rest.to_owned())
    } else {
        text
    })
}

/// `agents.attach`: the command that takes the agent over in the client's
/// terminal, or, for an agent in a multiplexer pane, the steps that bring
/// that pane forward, which the client runs itself.
pub(crate) async fn attach(
    shared: &Arc<Shared>,
    params: AgentParams,
) -> Result<AttachCommand, DaemonError> {
    // An agent in a host: `volee host attach`, which the client runs in its
    // own terminal; Ctrl+Q comes back (ADR 0021).
    if let Some(Hosted { id, name, .. }) = hosted(shared, params.agent).await {
        if !crate::launch::host_runs(shared, &id) {
            return Err(DaemonError::Refused(format!(
                "{name} is not running: send it a message to resume it in a new volee host"
            )));
        }
        let volee = shared.volee_bin.clone().ok_or_else(|| {
            DaemonError::Unavailable(format!("this daemon cannot take {name} over"))
        })?;
        return Ok(AttachCommand {
            program: volee.display().to_string(),
            args: vec![
                "host".to_owned(),
                "--dir".to_owned(),
                crate::launch::hosts_dir(shared).display().to_string(),
                "attach".to_owned(),
                id,
            ],
            cwd: None,
            env_remove: Vec::new(),
            hint: HOST_ATTACH_HINT.to_owned(),
            focus: None,
        });
    }
    {
        let state = shared.state.lock().await;
        // A daemon that hosts its agents only shows background sessions.
        if let Some(record) = state.registry().agent(params.agent)
            && record.location == Location::Background
            && shared.hosts
        {
            return Err(background_refusal(&record.name));
        }
    }
    if let Some(Pane {
        adapter,
        target,
        name,
    }) = pane(shared, params.agent).await
    {
        let steps = adapter
            .focus_steps(&target)
            .map_err(|error| pane_error(&name, adapter.as_ref(), &error))?;
        let session = volee_mux::split_target(&target)
            .map_or(target.as_str(), |(session, _)| session)
            .to_owned();
        return Ok(AttachCommand {
            program: String::new(),
            args: Vec::new(),
            cwd: None,
            env_remove: Vec::new(),
            hint: String::new(),
            focus: Some(PaneFocus {
                multiplexer: adapter.name().to_owned(),
                session,
                steps,
                attach: adapter.attach_command(&target).unwrap_or_default(),
            }),
        });
    }
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
        focus: None,
    })
}

/// `agents.stop`: stops the agent's session; the conversation is kept, and
/// volee does not resume the agent on its own until the human writes to it.
/// An agent that is not running is only held. volee stops only the agents it
/// launched (ADR 0011).
pub(crate) async fn stop(shared: &Arc<Shared>, params: AgentParams) -> Result<Empty, DaemonError> {
    // An agent in a host: its host ends the program like a closed terminal.
    if let Some(Hosted {
        socket, name, id, ..
    }) = hosted(shared, params.agent).await
    {
        hold(shared, params.agent).await?;
        // Whatever the state says: a host that runs is stopped.
        if crate::launch::host_runs(shared, &id) {
            blocking(move || volee_host::stop(&socket, HOST_CLIENT))
                .await?
                .map_err(|error| {
                    DaemonError::Unavailable(format!("cannot reach the host of {name}: {error}"))
                })?;
        }
        return Ok(Empty {});
    }
    harness(shared)?;
    let (name, managed, live) = {
        let state = shared.state.lock().await;
        let record = state
            .registry()
            .agent(params.agent)
            .ok_or_else(|| DaemonError::NotFound(format!("unknown agent {}", params.agent)))?;
        (
            record.name.clone(),
            record.managed(),
            record.state.is_live(),
        )
    };
    // A running agent's session decides first: it says where to act on a
    // session that volee cannot drive.
    let running = if live {
        Some(target(shared, params.agent).await?)
    } else {
        None
    };
    if !managed {
        return Err(DaemonError::Refused(format!(
            "{name} was not launched by volee, which stops only the agents it launched or \
             adopted: stop it where it was started, or adopt it first"
        )));
    }
    // Recorded before the session ends: the end must find the hold, or a
    // message waiting for the agent would resume it at once.
    hold(shared, params.agent).await?;
    if let Some(Target {
        harness,
        name,
        session,
        ..
    }) = running
        && session.alive
    {
        blocking(move || harness.stop(&session))
            .await?
            .map_err(|error| harness_error(&name, error))?;
    }
    Ok(Empty {})
}

/// Records that the human stopped `agent`, and tells the coordinator of its
/// running mission so that it does not wait for it.
async fn hold(shared: &Arc<Shared>, agent: AgentId) -> Result<(), DaemonError> {
    let mut state = shared.state.lock().await;
    let now = state.now();
    let mut events = vec![Event::new(now, EventKind::StopRequested { agent })];
    if let Some(message) = stop_notice(&state, agent) {
        events.push(Event::new(now, EventKind::MessagePosted { message }));
    }
    state.commit(events)?;
    Ok(())
}

/// The note for the coordinator when the human stops one of its workers;
/// none when the agent was already held or its mission is over.
fn stop_notice(state: &State, agent: AgentId) -> Option<Message> {
    let registry = state.registry();
    let record = registry.agent(agent)?;
    let mission = registry.mission(record.mission?)?;
    let coordinator = mission.coordinator.filter(|id| *id != agent)?;
    if record.hold.is_some() || mission.status.is_over() {
        return None;
    }
    Some(Message {
        id: state.new_message_id(),
        thread: mission.thread,
        from: Party::System,
        kind: MessageKind::Note,
        body: format!(
            "The human stopped {}. volee will not resume it until the human writes to it, so \
             messages you send it wait: plan without it, or ask the human.",
            record.name
        ),
        refs: Vec::new(),
        in_reply_to: None,
        at: state.now(),
        receipts: vec![Receipt {
            to: Party::Agent(coordinator),
            delivered_at: None,
            read_at: None,
        }],
    })
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
    use volee_core::role::{LaunchedBy, Role};
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
