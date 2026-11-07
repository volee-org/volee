//! The socket server: one task per connection, one task per request.

use std::fs::{self, File, TryLockError};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, broadcast, mpsc, watch};
use tokio::task::JoinSet;
use volee_git::Git;
use volee_harness::Harness;
use volee_proto::methods::{
    EVENTS_APPENDED, EVENTS_LAGGED, EventBatch, ForgeStatus, HarnessStatus, Lagged, Method,
    SeqEvent, Subscribe,
};
use volee_proto::paths::Paths;
use volee_proto::rpc::{self, Id, Notification, Outcome, Request, Response, RpcError, Version2};

use crate::clock::Clock;
use crate::discovery::Cadence;
use crate::error::DaemonError;
use crate::evidence::EvidenceSources;
use crate::handlers::{dispatch, parse};
use crate::log::EventLog;
use crate::state::State;
use crate::watchers::Watchers;

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
    /// The harness whose sessions the daemon discovers, when one is
    /// installed.
    pub harness: Option<Arc<dyn Harness>>,
    /// How often the harness is listed while at least one interactive
    /// client (the interface) is subscribed.
    pub discovery_interval: Duration,
    /// How often the harness is listed while no interactive client is
    /// subscribed. Agents volee launched report through their hooks either
    /// way; only sessions started elsewhere show up later.
    pub idle_discovery_interval: Duration,
    /// Where workspaces and delivery evidence are read from:
    /// [`EvidenceSources::default()`] reads nothing.
    pub evidence: EvidenceSources,
    /// The volee program, which the hooks and MCP servers of launched agents
    /// run. Without it, the daemon cannot launch agents.
    pub volee_bin: Option<PathBuf>,
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
    pub(crate) settings: volee_config::Config,
    pub(crate) volee_bin: Option<PathBuf>,
    pub(crate) paths: Paths,
    /// The interactive subscriptions open now, which set discovery's pace.
    pub(crate) watchers: Watchers,
    /// Agents being launched for each mission, counted against its limit.
    pub(crate) slots: crate::launch::Slots,
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
    pub async fn serve(listener: UnixListener, config: Config) -> Result<(), DaemonError> {
        let mut state = State::load(config.log, config.clock)?;
        let caught_up = crate::missions::catch_up(&mut state).len();
        if caught_up > 0 {
            tracing::info!(events = caught_up, "missions caught up");
        }
        let (shutdown, mut stop) = watch::channel(false);
        let shared = Arc::new(Shared {
            state: Mutex::new(state),
            harnesses: config.harnesses,
            harness: config.harness.clone(),
            git: config.evidence.git.clone(),
            settings: config.evidence.settings.clone(),
            volee_bin: config.volee_bin,
            paths: config.paths,
            watchers: Watchers::new(),
            slots: crate::launch::Slots::default(),
            forge: std::sync::Mutex::new(None),
            shutdown,
        });
        if let Some(dir) = config.spool {
            replay_spool(&shared, dir).await;
        }
        if let Some(harness) = config.harness {
            let cadence = Cadence {
                watched: config.discovery_interval,
                idle: config.idle_discovery_interval,
            };
            tokio::spawn(crate::discovery::run(
                Arc::clone(&shared),
                harness,
                cadence,
                stop.clone(),
            ));
        }
        let refresher = crate::evidence::spawn(&shared, config.evidence);
        let resumer = crate::resume::spawn(&shared);
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
        for task in [refresher, resumer].into_iter().flatten() {
            task.abort();
        }
        Ok(())
    }
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
async fn bind(path: &Path) -> Result<UnixListener, DaemonError> {
    if path.exists() {
        if UnixStream::connect(path).await.is_ok() {
            return Err(DaemonError::AlreadyRunning(path.to_owned()));
        }
        fs::remove_file(path).map_err(|source| DaemonError::Setup {
            action: "cannot remove the stale socket",
            path: path.to_owned(),
            source,
        })?;
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| DaemonError::Setup {
            action: "cannot create",
            path: parent.to_owned(),
            source,
        })?;
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
    let watcher = params.interactive.then(|| shared.watchers.watch());
    let batch = EventBatch { events, last_seq };
    let value = serde_json::to_value(&batch)
        .map_err(|error| RpcError::new(rpc::INTERNAL_ERROR, error.to_string()));
    send(outgoing, &response(request.id, value));
    let floor = last_seq.unwrap_or(0);
    let outgoing = outgoing.clone();
    tasks.spawn(async move {
        let _watcher = watcher;
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
