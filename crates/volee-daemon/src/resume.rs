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
use volee_core::workspace::Location;
use volee_core::{Agent, AgentId, Event, EventKind, MessageId, Registry};
use volee_harness::SpawnSpec;
use volee_harness::hooks::format_messages_for_agent;

use crate::error::DaemonError;
use crate::launch;
use crate::server::{LaunchGuard, Shared};

/// How long a failed resume waits before the same agent is tried again,
/// unless a new message arrives for it.
const RETRY_AFTER: Duration = Duration::from_secs(600);

/// The first words of a resume prompt. The conversation a resume continues
/// may say that the volee MCP server disconnected (it stopped with the old
/// session), and a model then believes its tools are gone. Measured: a
/// resumed agent wrote that it could not answer, with its tools connected.
const RESUMED: &str = "volee resumed this conversation in a new session to deliver the messages \
below. Your volee MCP tools (send_message, read_messages and the others) are connected in this \
session, even where the conversation says the volee server disconnected.";

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
    // An event can come too early: an agent's session ends (its `SessionEnd`
    // hook) a moment before its host records that the program ended, and a
    // resume started in between finds the host still running. So the agents
    // that wait are also looked at every few seconds.
    let mut sweep =
        tokio::time::interval_at(tokio::time::Instant::now() + SWEEP_EVERY, SWEEP_EVERY);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            received = events.recv() => match received {
                Ok(event) => {
                    for (agent, new_message) in triggered(&event.event.kind) {
                        start(&shared, &book, agent, new_message);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    start_waiting(&shared, &book).await;
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = sweep.tick() => start_waiting(&shared, &book).await,
        }
    }
}

/// How often the agents that wait for a resume are looked at, besides the
/// events that start one.
const SWEEP_EVERY: Duration = Duration::from_secs(5);

/// Starts a resume for every agent that needs one.
async fn start_waiting(shared: &Arc<Shared>, book: &Arc<Mutex<Book>>) {
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
        start(shared, book, agent, false);
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

/// Whether volee should resume `agent` now: volee drives it (it launched or
/// adopted it), its session ended, messages wait for it, and nothing holds
/// it (the human stopped it, or its mission is over).
fn needs_resume(registry: &Registry, agent: &Agent) -> bool {
    (agent.harness == HarnessKind::ClaudeCode || agent.harness == volee_harness::opencode::kind())
        && agent.managed()
        && agent.hold.is_none()
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
    let opencode = {
        let state = shared.state.lock().await;
        let registry = state.registry();
        let Some(record) = registry.agent(agent) else {
            return Ok(None);
        };
        // Every agent that stops comes here, and most have nothing waiting:
        // those claim nothing and get no file written.
        if !needs_resume(registry, record) {
            return Ok(None);
        }
        record.harness == volee_harness::opencode::kind()
    };
    // A conversation whose host still runs, or that another path is
    // launching, gets no second host: its hooks deliver the messages.
    let Ok(_claimed) = claim(shared, agent).await else {
        return Ok(None);
    };
    if opencode {
        return resume_opencode(shared, agent).await;
    }
    let harness = shared
        .harness
        .clone()
        .ok_or_else(|| DaemonError::Unavailable("Claude Code is not available".into()))?;
    let (settings_file, mcp_config_file) = launch::write_agent_files(shared, &harness, agent)?;
    // The check and the move to Launching share one lock, so that a stop or
    // the end of a mission cannot slip in between.
    let (
        Plan {
            session,
            spec,
            shown,
        },
        previous,
    ) = {
        let mut state = shared.state.lock().await;
        let registry = state.registry();
        let Some(record) = registry.agent(agent) else {
            return Ok(None);
        };
        if !needs_resume(registry, record) {
            return Ok(None);
        }
        let previous = record.state.clone();
        let Some(plan) = plan(shared, registry, record, settings_file, mcp_config_file) else {
            return Ok(None);
        };
        let shown = plan.shown.len();
        // Launching from now on, before the harness even starts: discovery
        // then adopts the fork if it lists it before its first hook, and
        // never marks the agent lost in between.
        let now = state.now();
        state.commit(vec![Event::new(
            now,
            EventKind::StateChanged {
                agent,
                state: AgentState::Launching,
                detail: Some(format!(
                    "resuming to deliver {shown} message{}",
                    if shown == 1 { "" } else { "s" }
                )),
            },
        )])?;
        (plan, previous)
    };
    let location = launch::location_of(spec.host.as_ref());
    let resumed = {
        let harness = Arc::clone(&harness);
        launch::blocking(move || harness.resume(&session, &spec)).await?
    };
    let mut state = shared.state.lock().await;
    let now = state.now();
    let resumed = match resumed {
        Ok(resumed) => resumed,
        Err(error) => {
            state.commit(vec![Event::new(
                now,
                EventKind::StateChanged {
                    agent,
                    state: previous,
                    detail: Some(format!("resume failed: {error}")),
                },
            )])?;
            return Err(error.into());
        }
    };
    state.expect_session(resumed.handle, agent, resumed.origin);
    let mut events = vec![Event::new(
        now,
        EventKind::LocationChanged { agent, location },
    )];
    events.extend(shown.iter().map(|message| {
        Event::new(
            now,
            EventKind::MessageDelivered {
                message: *message,
                to: Party::Agent(agent),
            },
        )
    }));
    state.commit(events)?;
    Ok(Some(shown.len()))
}

/// Keeps a second host away from `agent`'s conversation: refused while its
/// host still runs, or while another launch or resume of it is under way.
/// The guard keeps the others out until it drops.
async fn claim(shared: &Arc<Shared>, agent: AgentId) -> Result<LaunchGuard, DaemonError> {
    let (name, host) = {
        let state = shared.state.lock().await;
        let record = state
            .registry()
            .agent(agent)
            .ok_or_else(|| DaemonError::NotFound(format!("unknown agent {agent}")))?;
        (
            record.name.clone(),
            record.location.host().map(str::to_owned),
        )
    };
    if let Some(host) = host
        && launch::host_runs(shared, &host)
    {
        return Err(DaemonError::Refused(format!(
            "{name} still runs in its volee host: press i in volee to type into it"
        )));
    }
    shared
        .try_begin_launch(agent)
        .ok_or_else(|| DaemonError::Refused(format!("{name} is already being launched or resumed")))
}

/// Why a conversation moves into a new host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rehost {
    /// The person adopted an agent volee did not host (ADR 0023).
    Adopt,
    /// The machine restarted and ended the agent's host while it worked
    /// (ADR 0024).
    AfterReboot,
    /// The same, for an agent that waited at its prompt: it comes back
    /// waiting, without a prompt.
    AfterRebootIdle,
}

impl Rehost {
    /// The prompt the resumed session starts with: none when the agent waits
    /// at its prompt.
    fn prompt(self) -> &'static str {
        match self {
            Self::Adopt | Self::AfterRebootIdle => "",
            Self::AfterReboot => RESTARTED,
        }
    }

    fn state(self) -> AgentState {
        match self {
            Self::Adopt | Self::AfterRebootIdle => AgentState::Idle,
            Self::AfterReboot => AgentState::Launching,
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::Adopt => "adopted into a volee host",
            Self::AfterReboot => "resumed after the machine restarted",
            Self::AfterRebootIdle => "back after the machine restarted",
        }
    }
}

/// What a resumed session reads after a reboot ended its host (ADR 0024).
const RESTARTED: &str = "The machine restarted and ended this session, maybe in the middle of a \
     command or an answer. volee resumed the conversation in a new volee host: check where you \
     were, then continue.";

/// A host that started less than this ago and is gone again counts as a
/// crash, not a reboot: its agent is not resumed again (ADR 0024).
const CRASH_WINDOW_SECS: u64 = 600;

/// Resumes, in new hosts, the agents a reboot interrupted: their host is gone
/// and recorded no end, and nothing holds them (ADR 0024). Runs once, when
/// the daemon starts.
pub(crate) async fn resume_interrupted(shared: Arc<Shared>) {
    if !shared.hosts || !shared.settings.agents.resume_after_reboot {
        return;
    }
    let candidates: Vec<(AgentId, String, String, Rehost, bool)> = {
        let state = shared.state.lock().await;
        state
            .registry()
            .agents()
            .filter(|agent| {
                (agent.harness == HarnessKind::ClaudeCode
                    || agent.harness == volee_harness::opencode::kind())
                    && agent.hold.is_none()
                    && agent.current_session().is_some()
                    // Finished work stays finished.
                    && !matches!(agent.state, AgentState::Done | AgentState::Failed)
            })
            .filter_map(|agent| {
                let host = agent.location.host()?.to_owned();
                // An agent that waited at its prompt comes back waiting.
                let why = if agent.state == AgentState::Idle {
                    Rehost::AfterRebootIdle
                } else {
                    Rehost::AfterReboot
                };
                let resumed_before = agent.detail.as_deref() == Some(Rehost::AfterReboot.detail())
                    || agent.detail.as_deref() == Some(Rehost::AfterRebootIdle.detail());
                Some((agent.id, host, agent.name.clone(), why, resumed_before))
            })
            .collect()
    };
    let dir = launch::hosts_dir(&shared);
    for (agent, host, name, why, resumed_before) in candidates {
        let socket = dir.join(format!("{host}.sock"));
        let record = dir.join(format!("{host}.json"));
        let verdict =
            tokio::task::spawn_blocking(move || interrupted(&socket, &record, resumed_before))
                .await;
        match verdict {
            Ok(Interrupted::No) | Err(_) => {}
            Ok(Interrupted::Crashed) => {
                tracing::warn!(%agent, "its host ended right after it started: not resumed");
                let mut state = shared.state.lock().await;
                let now = state.now();
                let _ = state.commit(vec![Event::new(
                    now,
                    EventKind::StateChanged {
                        agent,
                        state: AgentState::Failed,
                        detail: Some(
                            "its volee host ended again within ten minutes of a resume after a \
                             reboot: write to it to resume it"
                                .into(),
                        ),
                    },
                )]);
            }
            Ok(Interrupted::Yes) => match rehost(&shared, agent, why).await {
                Ok(()) => tracing::info!(%agent, "resumed after a reboot"),
                Err(error) => tracing::warn!(%agent, %name, %error, "cannot resume after a reboot"),
            },
        }
    }
}

/// What a host left when the daemon started.
#[derive(Debug, PartialEq, Eq)]
enum Interrupted {
    /// It runs, or its program ended and said so.
    No,
    /// It is gone without an end: the machine or the host stopped.
    Yes,
    /// Gone without an end again, less than ten minutes after a resume
    /// after a reboot started it.
    Crashed,
}

fn interrupted(
    socket: &std::path::Path,
    record: &std::path::Path,
    resumed_before: bool,
) -> Interrupted {
    if std::os::unix::net::UnixStream::connect(socket).is_ok() {
        return Interrupted::No;
    }
    let Ok(record) = volee_host::HostRecord::read(record) else {
        return Interrupted::No;
    };
    if record.exit.is_some() {
        return Interrupted::No;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    if resumed_before && now.saturating_sub(record.started_at) < CRASH_WINDOW_SECS {
        Interrupted::Crashed
    } else {
        Interrupted::Yes
    }
}

/// What an adoption needs, read under the lock.
struct Adoption {
    session: volee_core::harness::SessionKey,
    listed: Option<volee_core::reconcile::ListedSession>,
    location: Location,
    name: String,
    root: std::path::PathBuf,
    worktree: Option<String>,
    model: Option<String>,
    instructions: Option<String>,
    cwd: std::path::PathBuf,
}

/// Reads what adopting `agent` needs, or why it cannot be adopted.
async fn adoption(
    shared: &Arc<Shared>,
    agent: AgentId,
    why: Rehost,
) -> Result<Adoption, DaemonError> {
    let state = shared.state.lock().await;
    let record = state
        .registry()
        .agent(agent)
        .ok_or_else(|| DaemonError::NotFound(format!("unknown agent {agent}")))?;
    if record.harness != HarnessKind::ClaudeCode
        && record.harness != volee_harness::opencode::kind()
    {
        return Err(DaemonError::Refused(format!(
            "{} runs in {}, which volee cannot resume yet",
            record.name, record.harness
        )));
    }
    if why == Rehost::Adopt && record.location.host().is_some() && record.state.is_live() {
        return Err(DaemonError::Refused(format!(
            "{} already runs in a volee host: press i in volee to type into it",
            record.name
        )));
    }
    let session = record
        .current_session()
        .map(|link| link.key.clone())
        .ok_or_else(|| {
            DaemonError::Refused(format!("{} has no conversation to resume", record.name))
        })?;
    let main = record
        .workspace
        .repo_root
        .clone()
        .unwrap_or_else(|| record.workspace.cwd.clone());
    // `root` is where the resumed program runs: the worktree when it exists.
    let (root, worktree) = launch::resume_place(&main, record.workspace.worktree.as_deref());
    let model = record.model.clone().or_else(|| {
        shared
            .settings
            .model_for(record.workspace.repo.as_deref())
            .map(str::to_owned)
    });
    Ok(Adoption {
        listed: state.sessions().listed(&session).cloned(),
        session,
        location: record.location.clone(),
        name: record.name.clone(),
        root,
        worktree,
        model,
        cwd: record.workspace.cwd.clone(),
        instructions: (record.launched_by != LaunchedBy::Outside)
            .then(|| launch::instructions_for(state.registry(), record)),
    })
}

/// Moves an agent's conversation into a host (ADR 0023): stops its
/// background session if it runs, then resumes the conversation in a new
/// host, at its prompt, with volee's hooks. An agent that runs in someone's
/// terminal must be quit there first, so that two programs never write the
/// same conversation.
pub(crate) async fn adopt(shared: &Arc<Shared>, agent: AgentId) -> Result<(), DaemonError> {
    rehost(shared, agent, Rehost::Adopt).await
}

/// Resumes `agent`'s conversation in a new host, for `why`.
async fn rehost(shared: &Arc<Shared>, agent: AgentId, why: Rehost) -> Result<(), DaemonError> {
    let _claimed = claim(shared, agent).await?;
    let Adoption {
        session,
        listed,
        location,
        name,
        root,
        worktree,
        model,
        instructions,
        cwd,
    } = adoption(shared, agent, why).await?;
    let alive = listed.as_ref().is_some_and(|session| session.alive);
    if alive && !matches!(location, Location::Background) {
        return Err(DaemonError::Refused(format!(
            "{name} runs in a terminal: quit it there first, then adopt it"
        )));
    }
    if session.harness == volee_harness::opencode::kind() {
        let prompt = Some(why.prompt()).filter(|prompt| !prompt.is_empty());
        return rehost_opencode(
            shared,
            agent,
            &session.id,
            &name,
            &cwd,
            prompt,
            why.state(),
            why.detail(),
        )
        .await;
    }
    let harness = shared
        .harness
        .clone()
        .ok_or_else(|| DaemonError::Unavailable("Claude Code is not available".into()))?;
    if alive && let Some(listed) = listed {
        let harness = Arc::clone(&harness);
        launch::blocking(move || harness.stop(&listed)).await??;
    }
    let (settings_file, mcp_config_file) = launch::write_agent_files(shared, &harness, agent)?;
    let host = launch::host_launch(shared, agent, &name)
        .ok_or_else(|| DaemonError::Unavailable("this daemon cannot start volee hosts".into()))?;
    let location = launch::location_of(Some(&host));
    let spec = SpawnSpec {
        cwd: root,
        name,
        model,
        prompt: why.prompt().to_owned(),
        settings_file,
        mcp_config_file,
        permission_mode: launch::permission_mode(shared),
        allowed_tools: Vec::new(),
        disallowed_tools: Vec::new(),
        append_system_prompt: instructions,
        worktree,
        host: Some(host),
    };
    let resumed = {
        let harness = Arc::clone(&harness);
        launch::blocking(move || harness.resume(&session, &spec)).await??
    };
    let mut state = shared.state.lock().await;
    let now = state.now();
    state.expect_session(resumed.handle, agent, resumed.origin);
    state.commit(vec![
        Event::new(now, EventKind::LocationChanged { agent, location }),
        Event::new(
            now,
            EventKind::StateChanged {
                agent,
                state: why.state(),
                detail: Some(why.detail().into()),
            },
        ),
    ])?;
    Ok(())
}

/// Resumes an OpenCode session in a new host, in `cwd`: `opencode --session
/// <id> [--prompt <prompt>]` (ADR 0022).
#[allow(clippy::too_many_arguments)] // Each is one fact of the resume.
async fn rehost_opencode(
    shared: &Arc<Shared>,
    agent: AgentId,
    session: &str,
    name: &str,
    cwd: &std::path::Path,
    prompt: Option<&str>,
    state_after: AgentState,
    detail: &str,
) -> Result<(), DaemonError> {
    use volee_harness::opencode;
    if session.starts_with("pid-") {
        return Err(DaemonError::Refused(format!(
            "{name} was started without a session id (opencode --session), so volee cannot \
             resume its conversation"
        )));
    }
    let host = launch::host_launch(shared, agent, name)
        .ok_or_else(|| DaemonError::Unavailable("this daemon cannot start volee hosts".into()))?;
    let location = launch::location_of(Some(&host));
    let args = opencode::args(
        session,
        prompt,
        launch::permission_mode(shared) == volee_harness::PermissionMode::Auto,
    );
    let cwd = cwd.to_path_buf();
    launch::blocking(move || {
        opencode::launch_in_host(
            &volee_harness::SystemRunner,
            &host,
            &cwd,
            args,
            Duration::from_secs(60),
        )
    })
    .await??;
    let mut state = shared.state.lock().await;
    let now = state.now();
    state.commit(vec![
        Event::new(now, EventKind::LocationChanged { agent, location }),
        Event::new(
            now,
            EventKind::StateChanged {
                agent,
                state: state_after,
                detail: Some(detail.to_owned()),
            },
        ),
    ])?;
    Ok(())
}

/// Delivers the pending messages of a stopped OpenCode agent by resuming
/// its session in a new host with them as its prompt.
async fn resume_opencode(
    shared: &Arc<Shared>,
    agent: AgentId,
) -> Result<Option<usize>, DaemonError> {
    let (session, name, cwd, prompt, shown) = {
        let mut state = shared.state.lock().await;
        let registry = state.registry();
        let Some(record) = registry.agent(agent) else {
            return Ok(None);
        };
        if !needs_resume(registry, record) {
            return Ok(None);
        }
        let Some(link) = record.current_session() else {
            return Ok(None);
        };
        let pending = registry.pending_for(agent);
        let rendered = format_messages_for_agent(pending.iter().copied(), |id| {
            registry.name_of(id).map(str::to_owned)
        });
        let shown: Vec<MessageId> = pending
            .iter()
            .take(rendered.shown)
            .map(|message| message.id)
            .collect();
        let found = (
            link.key.id.clone(),
            record.name.clone(),
            record.workspace.cwd.clone(),
            format!("{RESUMED}\n\n{}", rendered.text),
            shown,
        );
        let now = state.now();
        state.commit(vec![Event::new(
            now,
            EventKind::StateChanged {
                agent,
                state: AgentState::Launching,
                detail: Some("resuming to deliver messages".into()),
            },
        )])?;
        found
    };
    rehost_opencode(
        shared,
        agent,
        &session,
        &name,
        &cwd,
        Some(&prompt),
        AgentState::Working,
        "resumed to deliver messages",
    )
    .await?;
    let mut state = shared.state.lock().await;
    let now = state.now();
    let delivered: Vec<Event> = shown
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
    state.commit(delivered)?;
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
    let main = agent.workspace.repo_root.clone()?;
    let (root, worktree) = launch::resume_place(&main, agent.workspace.worktree.as_deref());
    let pending = registry.pending_for(agent.id);
    let rendered = format_messages_for_agent(pending.iter().copied(), |id| {
        registry.name_of(id).map(str::to_owned)
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
            prompt: format!("{RESUMED}\n\n{}", rendered.text),
            settings_file,
            mcp_config_file,
            permission_mode: launch::permission_mode(shared),
            allowed_tools: Vec::new(),
            disallowed_tools,
            // An adopted agent keeps its own instructions, as at its adoption.
            append_system_prompt: (agent.launched_by != LaunchedBy::Outside)
                .then(|| launch::instructions_for(registry, agent)),
            worktree,
            host: launch::host_launch(shared, agent.id, &agent.name),
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
                options: Vec::new(),
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
