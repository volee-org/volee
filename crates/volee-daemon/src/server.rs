//! The socket server: one task per connection, one task per request.

use std::fs::{self, File, TryLockError};
use std::io;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, broadcast, mpsc, watch};
use tokio::task::JoinSet;
use volee_core::AgentId;
use volee_git::Git;
use volee_harness::Harness;
use volee_proto::methods::{
    EVENTS_APPENDED, EVENTS_LAGGED, EventBatch, ForgeStatus, HarnessStatus, Join, Lagged, Method,
    SeqEvent, Subscribe,
};
use volee_proto::paths::Paths;
use volee_proto::rpc::{self, Id, Notification, Outcome, Request, Response, RpcError, Version2};

use crate::clock::Clock;
use crate::error::DaemonError;
use crate::evidence::EvidenceSources;
use crate::handlers::{dispatch, parse, respond};
use crate::log::EventLog;
use crate::state::State;

/// What the daemon needs to run.
pub struct Config {
    /// Where the socket, the lock and the state live.
    pub paths: Paths,
    /// The event log (the SQLite store in production).
    pub log: Box<dyn EventLog>,
    /// The clock.
    pub clock: Arc<dyn Clock>,
    /// What is known about the harnesses on this machine.
    pub harnesses: Vec<HarnessStatus>,
    /// The spool written by hooks while the daemon was down; drained and
    /// replayed at start.
    pub spool: Option<PathBuf>,
    /// The harness the agents view lists, when one is
    /// installed.
    pub harness: Option<Arc<dyn Harness>>,
    /// Where workspaces and delivery evidence are read from:
    /// [`EvidenceSources::default()`] reads nothing.
    pub evidence: EvidenceSources,
    /// The hook server that receives Claude Code's `http` hooks; `None`
    /// starts none.
    pub hook_server: Option<crate::hook_server::HookServerConfig>,
    /// The agents view: `claude agents --json` joined to hooks and git, read
    /// every `poll_interval`; `None` starts none.
    pub agents_view: Option<crate::agents_view::AgentsViewConfig>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("paths", &self.paths)
            .field("clock", &self.clock)
            .field("evidence", &self.evidence)
            .finish_non_exhaustive()
    }
}

/// State shared by every connection.
pub(crate) struct Shared {
    pub(crate) state: Mutex<State>,
    pub(crate) harnesses: Vec<HarnessStatus>,
    pub(crate) harness: Option<Arc<dyn Harness>>,
    pub(crate) git: Option<Arc<dyn Git>>,
    /// The forge client, to merge the pull requests of a mission the person
    /// approved.
    pub(crate) forge_client: Option<Arc<dyn volee_git::Forge>>,
    /// The missions being merged and closed now, so that one answer is
    /// carried out once.
    pub(crate) concluding: std::sync::Mutex<std::collections::HashSet<volee_core::MissionId>>,
    pub(crate) settings: volee_config::Config,
    /// Where fix requests are delivered and confirmed.
    pub(crate) fix: crate::actions::FixSources,
    pub(crate) paths: Paths,
    /// The agents view, when configured and Claude Code is available.
    pub(crate) agents: Option<Arc<crate::agents_view::Agents>>,
    /// The connections that joined for agents of other harnesses (ADR 0014).
    pub(crate) presence: crate::join::Presence,
    /// Requests to read an agent's delivery evidence again now.
    pub(crate) checks: crate::evidence::Checks,
    /// What the evidence refresher learned about the forge, once it checked.
    forge: std::sync::Mutex<Option<ForgeStatus>>,
    shutdown: watch::Sender<bool>,
}

impl Shared {
    pub(crate) fn request_shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    pub(crate) fn forge_status(&self) -> Option<ForgeStatus> {
        // A plain value that is never left half-written: a poisoned lock
        // still holds a valid one.
        self.forge
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn set_forge_status(&self, status: ForgeStatus) {
        *self
            .forge
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(status);
    }
}

/// The volee daemon.
#[derive(Debug)]
pub struct Daemon;

impl Daemon {
    /// Runs the daemon until a client asks it to stop or the process
    /// receives SIGINT or SIGTERM. Takes the single-daemon lock, replays the
    /// log, then listens on the socket with user-only permissions.
    pub async fn run(config: Config) -> Result<(), DaemonError> {
        let paths = config.paths.clone();
        create_private_dir(&paths.state_dir)?;
        let _lock = acquire_lock(&paths.lock_file())?;
        let listener = bind(&paths.socket).await?;
        let result = Self::serve(listener, config).await;
        let _ = fs::remove_file(&paths.socket);
        result
    }

    /// Serves an already bound listener. Used by [`Daemon::run`] and by tests.
    #[allow(clippy::too_many_lines)]
    pub async fn serve(listener: UnixListener, config: Config) -> Result<(), DaemonError> {
        let mut state = State::load(config.log, config.clock)?;
        let caught_up = crate::missions::catch_up(&mut state).len();
        if caught_up > 0 {
            tracing::info!(events = caught_up, "missions caught up");
        }
        // No connection is open yet: agents that joined through MCP are
        // stopped until their server joins again.
        match crate::join::settle_at_start(&mut state) {
            Ok(0) => {}
            Ok(count) => tracing::info!(count, "joined agents wait for their MCP server"),
            Err(error) => tracing::warn!(%error, "cannot stop the joined agents"),
        }
        // What agents that left before this daemon started left behind.
        let swept = crate::files::sweep(&config.paths, state.registry());
        if swept > 0 {
            tracing::info!(count = swept, "removed the files of agents that are gone");
        }
        // Subscribed before anything can commit: no removal is missed.
        let removals = state.subscribe();
        let (shutdown, mut stop) = watch::channel(false);
        // The one set of loaded rules: the finished-work rule, the card's
        // risk, the review plan and the held question delay all ask it.
        let rules = crate::rules::RulesService::new(config.paths.config_dir.clone());
        let agents = crate::agents_view::build(
            config.agents_view.as_ref(),
            config.harness.clone(),
            config.evidence.git.clone(),
            config.evidence.forge.clone(),
            Some(Arc::clone(&rules)),
        );
        let shared = Arc::new(Shared {
            state: Mutex::new(state),
            harnesses: config.harnesses,
            harness: config.harness.clone(),
            git: config.evidence.git.clone(),
            forge_client: config.evidence.forge.clone(),
            concluding: std::sync::Mutex::default(),
            settings: config.evidence.settings.clone(),
            fix: config.evidence.fix.clone(),
            paths: config.paths,
            agents: agents.clone(),
            presence: crate::join::Presence::default(),
            checks: crate::evidence::Checks::default(),
            forge: std::sync::Mutex::new(None),
            shutdown,
        });
        if let Some(dir) = config.spool {
            replay_spool(&shared, dir).await;
        }
        let refresher = crate::evidence::spawn(&shared, config.evidence);
        let following = shared.agents.clone().map(|agents| {
            tokio::spawn(crate::agents_view::follow_listings(
                Arc::clone(&shared),
                agents,
                stop.clone(),
            ))
        });
        let polling = crate::agents_view::spawn_poll(config.agents_view.as_ref(), agents, &stop);
        let reviews = open_reviews(&shared, config.hook_server.as_ref(), &rules);
        let watching_rules = tokio::spawn(Arc::clone(&rules).watch(stop.clone()));
        let reviewing = reviews
            .as_ref()
            .zip(shared.agents.clone())
            .map(|(reviews, agents)| {
                tokio::spawn(crate::review::watch_finished(
                    Arc::clone(reviews),
                    agents,
                    stop.clone(),
                ))
            });
        let (hooks, merging) = start_hook_server(
            config.hook_server,
            shared.agents.clone(),
            reviews,
            Arc::clone(&rules),
            &shared,
            &stop,
        )
        .await?;
        // A mission the person approved while the daemon was stopping, or
        // whose merge it could not finish, is carried out now.
        crate::missions::resume_approved(&shared).await;
        let files = crate::files::spawn(&shared, removals);
        tracing::info!(socket = ?listener.local_addr().ok(), "volee daemon ready");
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        connections.spawn(serve_connection(stream, Arc::clone(&shared)));
                    }
                    Err(error) => tracing::warn!(%error, "accept failed"),
                },
                _ = stop.changed() => break,
                () = terminate() => break,
            }
        }
        tracing::info!("volee daemon stopping");
        connections.abort_all();
        for task in [
            refresher,
            Some(files),
            hooks,
            merging,
            polling,
            following,
            reviewing,
            Some(watching_rules),
        ]
        .into_iter()
        .flatten()
        {
            task.abort();
        }
        Ok(())
    }
}

/// The AI pre-review service, when it is on and there is a database to keep
/// it in (the hook server's).
fn open_reviews(
    shared: &Shared,
    hook_server: Option<&crate::hook_server::HookServerConfig>,
    rules: &Arc<crate::rules::RulesService>,
) -> Option<Arc<crate::review::Reviews>> {
    let settings = shared.settings.review.clone();
    if !settings.enabled {
        return None;
    }
    let database = &hook_server?.database;
    let reviewer = Arc::new(crate::review::ClaudeReviewer::new(
        shared.settings.claude.command.clone(),
        Duration::from_secs(settings.timeout),
    ));
    match crate::review::Reviews::open(settings, Arc::clone(rules), database, reviewer) {
        Ok(reviews) => Some(Arc::new(reviews)),
        Err(error) => {
            tracing::warn!(%error, "the AI pre-review is unavailable");
            None
        }
    }
}

/// Starts the hook server when one is configured: binds first, so that a port
/// already in use stops the daemon with a message instead of failing later.
async fn start_hook_server(
    config: Option<crate::hook_server::HookServerConfig>,
    agents: Option<Arc<crate::agents_view::Agents>>,
    reviews: Option<Arc<crate::review::Reviews>>,
    rules: Arc<crate::rules::RulesService>,
    shared: &Arc<Shared>,
    stop: &watch::Receiver<bool>,
) -> Result<
    (
        Option<tokio::task::JoinHandle<()>>,
        Option<tokio::task::JoinHandle<()>>,
    ),
    DaemonError,
> {
    let Some(hook_config) = config else {
        return Ok((None, None));
    };
    let listener = crate::hook_server::bind(&hook_config).await?;
    let store = volee_store::Store::open(&hook_config.database).map_err(|error| {
        DaemonError::Unavailable(format!("cannot open the hook database: {error}"))
    })?;
    tracing::info!(addr = ?listener.local_addr().ok(), "hook server ready");
    // The web interface shares the listener, when there are agents to show.
    let decisions = crate::decisions::Decisions::new(hook_config.hold);
    let assessments = match crate::assess::Assessments::open(rules, &hook_config.database) {
        Ok(assessments) => Some(Arc::new(assessments)),
        Err(error) => {
            tracing::warn!(%error, "the risk of changes is unavailable");
            None
        }
    };
    let mut web = agents.clone().map(|agents| {
        crate::web::Web::detect(agents, decisions)
            .with_reviews(reviews)
            .with_assessments(assessments.clone())
    });
    // The actions on finished changes: merge, fix, comments, editors.
    let mut merging = None;
    if let (Some(web_ref), Some(assessments), Some(agents)) = (web.as_mut(), assessments, agents) {
        let methods = shared
            .settings
            .repos
            .values()
            .filter_map(|repo| {
                let method = match repo.merge? {
                    volee_config::MergeMethod::Merge => volee_git::MergeMethod::Merge,
                    volee_config::MergeMethod::Squash => volee_git::MergeMethod::Squash,
                    volee_config::MergeMethod::Rebase => volee_git::MergeMethod::Rebase,
                };
                Some((repo.path.clone(), method))
            })
            .collect();
        let parts = crate::actions::Parts {
            git: shared.git.clone(),
            forge: shared.forge_client.clone(),
            methods,
            resumer: Arc::new(crate::actions::fix::ClaudeCli {
                program: shared.settings.claude.command.clone(),
            }),
            timing: {
                let mut timing = crate::actions::fix::Timing::default();
                if let Some(dir) = &shared.fix.socket_dir {
                    timing.socket_dir.clone_from(dir);
                }
                if let Some(within) = shared.fix.confirm_within {
                    timing.confirm_within = within;
                }
                timing
            },
            assessments,
            agents: Arc::clone(&agents),
            projects: shared.fix.projects.clone().or_else(|| web_ref.projects()),
        };
        match crate::actions::Actions::open(parts, &hook_config.database) {
            Ok(actions) => {
                let actions = Arc::new(actions);
                merging = Some(tokio::spawn(crate::actions::watch_auto_merge(
                    Arc::clone(&actions),
                    agents,
                    stop.clone(),
                )));
                *web_ref = web_ref.clone().with_actions(Some(actions));
            }
            Err(error) => tracing::warn!(%error, "the actions on finished changes are unavailable"),
        }
    }
    Ok((
        Some(tokio::spawn(async move {
            if let Err(error) =
                crate::hook_server::serve_with_web(listener, hook_config.token, store, web).await
            {
                tracing::error!(%error, "the hook server stopped");
            }
        })),
        merging,
    ))
}

/// Replays hook events recorded while the daemon was down. Records that no
/// longer apply (for example an agent forgotten since) are skipped.
async fn replay_spool(shared: &Arc<Shared>, dir: PathBuf) {
    let drained = match volee_store::Spool::new(dir).drain() {
        Ok(drained) => drained,
        Err(error) => {
            tracing::warn!(%error, "cannot drain the hook spool");
            return;
        }
    };
    let total = drained.records.len();
    let mut replayed = 0_usize;
    for record in drained.records {
        let Ok(params) = serde_json::from_value(record) else {
            continue;
        };
        if crate::handlers::replay_hook(shared, params).await.is_ok() {
            replayed += 1;
        }
    }
    if total > 0 {
        tracing::info!(total, replayed, "replayed spooled hook events");
    }
}

async fn terminate() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut interrupt), Ok(mut term)) = (
            signal(SignalKind::interrupt()),
            signal(SignalKind::terminate()),
        ) else {
            std::future::pending::<()>().await;
            return;
        };
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = term.recv() => {}
        }
    }
}

fn create_private_dir(dir: &Path) -> Result<(), DaemonError> {
    fs::create_dir_all(dir).map_err(|source| DaemonError::Setup {
        action: "cannot create",
        path: dir.to_owned(),
        source,
    })?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|source| {
        DaemonError::Setup {
            action: "cannot restrict permissions of",
            path: dir.to_owned(),
            source,
        }
    })
}

fn acquire_lock(path: &Path) -> Result<File, DaemonError> {
    let file = File::create(path).map_err(|source| DaemonError::Setup {
        action: "cannot create the lock",
        path: path.to_owned(),
        source,
    })?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(DaemonError::AlreadyRunning(path.to_owned())),
        Err(TryLockError::Error(source)) => Err(DaemonError::Setup {
            action: "cannot lock",
            path: path.to_owned(),
            source,
        }),
    }
}

/// Binds the socket, replacing a stale socket file left by a crash, and
/// restricts it to the current user.
///
/// Only a socket is ever replaced: a `VOLEE_SOCKET` that names another kind
/// of file by mistake stops the daemon instead of deleting the file.
///
/// The socket is created with the process's umask, then restricted to
/// 0600. The umask is not narrowed around `bind`: it is process-wide, so
/// changing it would race with every other thread that creates files, and
/// setting it needs `unsafe`, which the workspace forbids. The directory
/// closes that gap instead: the default places are private (the state
/// directory, made 0700 before `bind`, or `$XDG_RUNTIME_DIR`, 0700 by
/// specification), so no other user can reach the socket before its mode
/// is restricted. A directory that lets others in gets a warning.
async fn bind(path: &Path) -> Result<UnixListener, DaemonError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            if UnixStream::connect(path).await.is_ok() {
                return Err(DaemonError::AlreadyRunning(path.to_owned()));
            }
            fs::remove_file(path).map_err(|source| DaemonError::Setup {
                action: "cannot remove the stale socket",
                path: path.to_owned(),
                source,
            })?;
        }
        Ok(_) => {
            return Err(DaemonError::Setup {
                action: "will not replace",
                path: path.to_owned(),
                source: io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "it is not a socket; remove it, or set VOLEE_SOCKET to another path",
                ),
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(DaemonError::Setup {
                action: "cannot inspect",
                path: path.to_owned(),
                source,
            });
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| DaemonError::Setup {
            action: "cannot create",
            path: parent.to_owned(),
            source,
        })?;
        warn_if_others_can_enter(parent);
    }
    let listener = UnixListener::bind(path).map_err(|source| DaemonError::Setup {
        action: "cannot listen on",
        path: path.to_owned(),
        source,
    })?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| {
        DaemonError::Setup {
            action: "cannot restrict permissions of",
            path: path.to_owned(),
            source,
        }
    })?;
    Ok(listener)
}

/// Warns when users other than this one can enter `dir`, the socket's
/// directory: between `bind` and the chmod that follows, they could
/// connect.
fn warn_if_others_can_enter(dir: &Path) {
    if let Ok(metadata) = fs::metadata(dir)
        && metadata.permissions().mode() & 0o011 != 0
    {
        tracing::warn!(
            directory = %dir.display(),
            "other users can enter the directory of the socket: set VOLEE_SOCKET to a path \
             in a directory only you can enter"
        );
    }
}

async fn serve_connection(stream: UnixStream, shared: Arc<Shared>) {
    let (read, mut write) = stream.into_split();
    let (outgoing, mut lines_out) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(line) = lines_out.recv().await {
            if write.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    });
    // Requests of a connection die with it: a hook that disconnects must not
    // keep a waiter alive and "deliver" a message nobody reads.
    let mut tasks = JoinSet::new();
    // Agents of other harnesses that joined on this connection: alive until
    // it closes (ADR 0014).
    let mut joined = Vec::new();
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let request = match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(error) => {
                send(
                    &outgoing,
                    &response(
                        None,
                        Err(RpcError::new(rpc::PARSE_ERROR, error.to_string())),
                    ),
                );
                continue;
            }
        };
        if request.method == Subscribe::NAME {
            subscribe(&shared, request, &outgoing, &mut tasks).await;
            continue;
        }
        if request.method == Join::NAME {
            joined.extend(join(&shared, request, &outgoing).await);
            continue;
        }
        let shared = Arc::clone(&shared);
        let outgoing = outgoing.clone();
        tasks.spawn(async move {
            let result = dispatch(&shared, &request.method, request.params).await;
            if let Some(id) = request.id {
                send(&outgoing, &response(Some(id), result));
            }
        });
    }
    tasks.abort_all();
    drop(outgoing);
    let _ = writer.await;
    crate::join::left(&shared, &joined).await;
}

/// Joins an agent of another harness for this connection and answers;
/// returns the agent, which stays alive until the connection closes.
async fn join(
    shared: &Arc<Shared>,
    request: Request,
    outgoing: &mpsc::UnboundedSender<String>,
) -> Option<AgentId> {
    let joined = match parse::<Join>(request.params) {
        Ok(params) => crate::join::join(shared, params).await,
        Err(error) => {
            send(outgoing, &response(request.id, Err(error)));
            return None;
        }
    };
    let agent = joined.as_ref().ok().map(|joined| joined.agent);
    send(outgoing, &response(request.id, respond(joined)));
    agent
}

/// Answers with the backlog, then forwards every new event. The backlog and
/// the receiver are taken under the same lock, so nothing is missed. An
/// interactive subscription counts as a watcher for as long as it forwards
/// events (discovery then lists at once, and often).
async fn subscribe(
    shared: &Arc<Shared>,
    request: Request,
    outgoing: &mpsc::UnboundedSender<String>,
    tasks: &mut JoinSet<()>,
) {
    let params = match parse::<Subscribe>(request.params) {
        Ok(params) => params,
        Err(error) => {
            send(outgoing, &response(request.id, Err(error)));
            return;
        }
    };
    let snapshot = {
        let state = shared.state.lock().await;
        state
            .since(params.after)
            .and_then(|events| Ok((events, state.last_seq()?, state.subscribe())))
    };
    let (events, last_seq, mut receiver) = match snapshot {
        Ok(snapshot) => snapshot,
        Err(error) => {
            send(outgoing, &response(request.id, Err(error.into())));
            return;
        }
    };
    // The guard lives in the forwarding task: it is dropped when the
    // subscription ends, or when its connection closes and aborts the task.
    let batch = EventBatch { events, last_seq };
    let value = serde_json::to_value(&batch)
        .map_err(|error| RpcError::new(rpc::INTERNAL_ERROR, error.to_string()));
    send(outgoing, &response(request.id, value));
    let floor = last_seq.unwrap_or(0);
    let outgoing = outgoing.clone();
    tasks.spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) if event.seq > floor => {
                    if !send(&outgoing, &notification(&event)) {
                        break;
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "a subscriber lagged behind and was dropped");
                    let lagged = Notification {
                        jsonrpc: Version2,
                        method: EVENTS_LAGGED.to_owned(),
                        params: serde_json::to_value(Lagged { skipped }).unwrap_or_default(),
                    };
                    if let Ok(line) = serde_json::to_string(&lagged) {
                        let _ = outgoing.send(line + "\n");
                    }
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn response(id: Option<Id>, result: Result<Value, RpcError>) -> Response {
    Response {
        jsonrpc: Version2,
        id,
        outcome: match result {
            Ok(value) => Outcome::Result(value),
            Err(error) => Outcome::Error(error),
        },
    }
}

fn notification(event: &SeqEvent) -> Notification {
    Notification {
        jsonrpc: Version2,
        method: EVENTS_APPENDED.to_owned(),
        params: serde_json::to_value(event).unwrap_or(Value::Null),
    }
}

/// Queues one JSON line; returns `false` when the connection is gone.
fn send<T: serde::Serialize>(outgoing: &mpsc::UnboundedSender<String>, message: &T) -> bool {
    match serde_json::to_string(message) {
        Ok(mut line) => {
            line.push('\n');
            outgoing.send(line).is_ok()
        }
        Err(error) => {
            tracing::error!(%error, "cannot serialize a response");
            true
        }
    }
}
