//! The host process: one program in one terminal, served on a socket
//! (ADR 0019).
//!
//! Threads: one reads the program's output, one writes its input, one waits
//! for it to end, one accepts clients and two serve each client. They all
//! talk to the terminal thread, which owns the [`Screen`] and every
//! decision, so the order of output, snapshots and input is the order in
//! which it handles them.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use rustix::process::{Pid, Signal, kill_process_group};
use volee_term::{Boundary, KeyInput, Screen};

use crate::proto::{self, Event, Exit, Hello, KeyPress, Kind, PROTOCOL, Role, Size, Welcome};
use crate::record::{HostDir, HostRecord, now};

/// Frames queued for one client before it counts as lagging.
const CLIENT_QUEUE: usize = 256;
/// How long a snapshot waits for the end of a synchronized update.
const SYNC_WAIT: Duration = Duration::from_secs(1);
/// How long a stopped program has after `SIGHUP` before `SIGKILL`.
const STOP_GRACE: Duration = Duration::from_secs(5);
/// How long the end of a program waits for its last output.
const EXIT_DRAIN: Duration = Duration::from_millis(500);
/// How long a client may wait for a snapshot it cannot take (it stopped
/// reading) before the host drops it.
const STUCK_AFTER: Duration = Duration::from_secs(30);
/// How long a client has to say hello.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// Input that arrives within this of the previous input goes out in the same
/// write.
const KEY_BURST: Duration = Duration::from_millis(3);
/// The most input gathered into one write.
const WRITE_BATCH: usize = 64 * 1024;
/// How long after a Ctrl+Z the program gets `SIGCONT`. A program that
/// suspends itself on Ctrl+Z (Claude Code does) waits for a shell to resume
/// it; in a host there is none, so the host does, once the program has had
/// time to stop.
const RESUME_AFTER_SUSPEND: Duration = Duration::from_millis(300);

/// Whether keys hold a Ctrl+Z, on which the program may suspend itself: the
/// legacy byte, or the kitty keyboard protocol's sequence.
fn suspends(bytes: &[u8]) -> bool {
    const KITTY: &[u8] = b"\x1b[122;5u";
    bytes.contains(&0x1a) || bytes.windows(KITTY.len()).any(|window| window == KITTY)
}

/// What a host runs, and how.
#[derive(Clone, Debug)]
pub struct HostConfig {
    /// The host's id: the agent's id.
    pub id: String,
    /// Where hosts live.
    pub dir: HostDir,
    /// The program's command line.
    pub command: Vec<String>,
    /// Its working directory.
    pub cwd: PathBuf,
    /// Variables added to the host's own environment.
    pub env: Vec<(String, String)>,
    /// The size before any input owner sets one.
    pub size: Size,
    /// The harness profile (ADR 0022).
    pub profile: String,
    /// The display name of the agent it runs, shown by its clients.
    pub name: Option<String>,
    /// Lines of history kept.
    pub scrollback: usize,
    /// How long the last screen stays after the program ends.
    pub linger: Duration,
}

/// Why a host could not run.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The command line is empty.
    #[error("no command to run")]
    NoCommand,
    /// Another host answers on the socket.
    #[error("a host already answers on {0}")]
    Busy(PathBuf),
    /// The pseudo-terminal could not be opened or the program started.
    #[error("cannot start {command}: {reason}")]
    Spawn {
        /// The program.
        command: String,
        /// Why.
        reason: String,
    },
    /// The terminal emulator failed.
    #[error(transparent)]
    Term(#[from] volee_term::Error),
    /// A file or socket operation failed.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Makes this process the leader of a new session, without a controlling
/// terminal, so that closing the terminal that started it sends it nothing.
/// Call it first thing in a host process started by another program (it
/// fails in a process group leader).
pub fn detach_from_terminal() -> io::Result<()> {
    rustix::process::setsid()?;
    Ok(())
}

/// Runs a host until its program has ended and its linger time is over.
/// Returns how the program ended.
pub fn run(config: HostConfig) -> Result<Exit, HostError> {
    let program = config.command.first().ok_or(HostError::NoCommand)?.clone();
    config.dir.ensure()?;
    let socket = config.dir.socket(&config.id);
    if socket.exists() {
        if UnixStream::connect(&socket).is_ok() {
            return Err(HostError::Busy(socket));
        }
        std::fs::remove_file(&socket)?;
    }
    // Bound before the program starts: a socket that cannot be bound (a path
    // too long, a race on the id) must not leave a program running.
    let listener = UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    let size = clamp(config.size);

    let spawn_error = |reason: String| {
        let _ = std::fs::remove_file(&socket);
        HostError::Spawn {
            command: program.clone(),
            reason,
        }
    };
    let pair = native_pty_system()
        .openpty(pty_size(size))
        .map_err(|error| spawn_error(error.to_string()))?;
    let mut command =
        CommandBuilder::from_argv(config.command.iter().map(OsString::from).collect());
    command.cwd(&config.cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("VOLEE_HOST", &config.id);
    for (name, value) in &config.env {
        command.env(name, value);
    }
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| spawn_error(error.to_string()))?;
    drop(pair.slave);
    let child_pid = child.process_id();
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| spawn_error(error.to_string()))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| spawn_error(error.to_string()))?;

    let record = HostRecord {
        id: config.id.clone(),
        protocol: PROTOCOL,
        host_pid: std::process::id(),
        child_pid,
        profile: config.profile.clone(),
        name: config.name.clone(),
        command: config.command.clone(),
        cwd: config.cwd.clone(),
        started_at: now(),
        size,
        exit: None,
    };
    record.write(&config.dir.record(&config.id))?;
    tracing::info!(id = %config.id, ?child_pid, "host started");

    let (tx, rx) = mpsc::channel();
    spawn_reader(reader, tx.clone());
    let input = spawn_writer(writer);
    spawn_waiter(child, tx.clone());
    spawn_acceptor(listener, tx);

    let screen = Screen::new(size.cols, size.rows, config.scrollback)?;
    let mut host = Host {
        config,
        record,
        screen,
        boundary: Boundary::new(),
        master: pair.master,
        input,
        clients: BTreeMap::new(),
        owner: None,
        owner_embedded: false,
        pending_exit: None,
        stop_deadline: None,
        linger_deadline: None,
        resume_deadline: None,
    };
    let exit = host.serve(&rx);
    let _ = std::fs::remove_file(&socket);
    tracing::info!(?exit, "host ended");
    Ok(exit)
}

/// What the other threads tell the terminal thread.
enum Msg {
    Output(Vec<u8>),
    OutputClosed,
    Exited(Exit),
    Connected {
        id: u64,
        hello: Hello,
        tx: SyncSender<Out>,
    },
    Input {
        id: u64,
        bytes: Vec<u8>,
    },
    Key {
        id: u64,
        press: KeyPress,
    },
    Paste {
        id: u64,
        text: String,
    },
    Resize {
        id: u64,
        size: Size,
    },
    Stop,
    Gone {
        id: u64,
    },
}

/// What the terminal thread sends a client's writer thread.
enum Out {
    Frame(Kind, Vec<u8>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClientState {
    /// Waiting for a snapshot, since then: the client just connected, the
    /// size changed, or it fell behind.
    Waiting(Instant),
    /// Up to date: it receives the output.
    Live,
}

struct Client {
    tx: SyncSender<Out>,
    state: ClientState,
}

struct Host {
    config: HostConfig,
    record: HostRecord,
    screen: Screen,
    boundary: Boundary,
    master: Box<dyn MasterPty + Send>,
    input: Sender<Vec<u8>>,
    clients: BTreeMap<u64, Client>,
    owner: Option<u64>,
    /// The input owner draws the screen itself and answers no query.
    owner_embedded: bool,
    pending_exit: Option<(Exit, Instant)>,
    stop_deadline: Option<Instant>,
    linger_deadline: Option<Instant>,
    /// When to send `SIGCONT` after a Ctrl+Z.
    resume_deadline: Option<Instant>,
}

impl Host {
    fn serve(&mut self, rx: &Receiver<Msg>) -> Exit {
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(msg) => self.handle(msg),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.send_waiting_snapshots();
            let now = Instant::now();
            if let Some((_, since)) = &self.pending_exit
                && now.duration_since(*since) >= EXIT_DRAIN
            {
                self.finish_exit();
            }
            if self.stop_deadline.is_some_and(|deadline| now >= deadline)
                && self.record.exit.is_none()
            {
                self.signal(Signal::KILL);
                self.stop_deadline = None;
            }
            if self.resume_deadline.is_some_and(|deadline| now >= deadline) {
                self.resume_deadline = None;
                if self.record.exit.is_none() {
                    self.signal(Signal::CONT);
                }
            }
            if self.linger_deadline.is_some_and(|deadline| now >= deadline) {
                break;
            }
        }
        self.record.exit.clone().unwrap_or(Exit {
            code: None,
            signal: None,
            at: now(),
        })
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Output(bytes) => {
                let complete = self.boundary.push(&bytes);
                if !complete.is_empty() {
                    self.output(&complete);
                }
            }
            Msg::OutputClosed => {
                if self.pending_exit.is_some() {
                    self.finish_exit();
                }
            }
            Msg::Exited(exit) => self.pending_exit = Some((exit, Instant::now())),
            Msg::Connected { id, hello, tx } => self.connect(id, &hello, tx),
            Msg::Input { id, bytes } => {
                if self.owner == Some(id) && self.record.exit.is_none() {
                    if suspends(&bytes) {
                        self.resume_soon();
                    }
                    let _ = self.input.send(bytes);
                }
            }
            Msg::Key { id, press } => {
                if self.owner == Some(id) && self.record.exit.is_none() {
                    self.key(&press);
                }
            }
            Msg::Paste { id, text } => {
                if self.owner == Some(id) && self.record.exit.is_none() {
                    match self.screen.encode_paste(&text) {
                        Ok(bytes) => {
                            let _ = self.input.send(bytes);
                        }
                        Err(error) => tracing::warn!(%error, "cannot encode a paste"),
                    }
                }
            }
            Msg::Resize { id, size } => {
                if self.owner == Some(id) {
                    self.resize(size);
                }
            }
            Msg::Stop => {
                if self.record.exit.is_none() {
                    self.signal(Signal::HUP);
                    self.stop_deadline = Some(Instant::now() + STOP_GRACE);
                }
            }
            Msg::Gone { id } => {
                self.clients.remove(&id);
                if self.owner == Some(id) {
                    self.owner = None;
                    self.owner_embedded = false;
                }
            }
        }
    }

    /// Encodes a key the way the program asked and sends it.
    fn key(&mut self, press: &KeyPress) {
        let input = KeyInput {
            key: press.key.clone(),
            shift: press.shift,
            alt: press.alt,
            ctrl: press.ctrl,
        };
        match self.screen.encode_key(&input) {
            Ok(bytes) if !bytes.is_empty() => {
                if press.ctrl && !press.alt && press.key.eq_ignore_ascii_case("z") {
                    self.resume_soon();
                }
                // Named keys and shortcuts only: what people type stays out
                // of the log.
                if press.key.chars().count() > 1 || press.ctrl || press.alt {
                    tracing::debug!(
                        key = %press.key,
                        ctrl = press.ctrl,
                        alt = press.alt,
                        shift = press.shift,
                        kitty = self.screen.keyboard_flags(),
                        bytes = ?String::from_utf8_lossy(&bytes),
                        "key"
                    );
                }
                let _ = self.input.send(bytes);
            }
            Ok(_) => tracing::debug!(key = %press.key, "a key with no encoding"),
            Err(error) => tracing::warn!(%error, "cannot encode a key"),
        }
    }

    /// Schedules the `SIGCONT` that follows a Ctrl+Z.
    fn resume_soon(&mut self) {
        self.resume_deadline = Some(Instant::now() + RESUME_AFTER_SUSPEND);
    }

    /// Feeds output to the screen, answers queries when no person's terminal
    /// does, and passes the output on to clients that are up to date.
    fn output(&mut self, bytes: &[u8]) {
        self.screen.feed(bytes);
        let replies = self.screen.take_replies();
        // An owner waiting for its snapshot does not see this output, so its
        // terminal cannot answer: the host does. An embedded owner never
        // answers.
        let owner_answers = !self.owner_embedded
            && self.owner.is_some_and(|id| {
                self.clients
                    .get(&id)
                    .is_some_and(|client| client.state == ClientState::Live)
            });
        if !owner_answers && !replies.is_empty() && self.record.exit.is_none() {
            tracing::debug!(replies = ?String::from_utf8_lossy(&replies), "answered the program's queries");
            let _ = self.input.send(replies);
        }
        let signals = self.screen.take_signals();
        self.broadcast(Kind::Output, bytes);
        if signals.title_changed
            && let Some(title) = self.screen.title()
        {
            self.broadcast_event(&Event::Title { title });
        }
        if signals.bell {
            self.broadcast_event(&Event::Bell);
        }
    }

    fn connect(&mut self, id: u64, hello: &Hello, tx: SyncSender<Out>) {
        tracing::info!(id, client = %hello.client, role = ?hello.role, "client connected");
        let wants_input = hello.role == Role::Control && self.record.exit.is_none();
        if wants_input {
            if let Some(previous) = self.owner.take() {
                self.send_event_to(previous, &Event::InputTaken);
                // Dropping its sender closes its connection.
                self.clients.remove(&previous);
            }
            self.owner = Some(id);
            self.owner_embedded = hello.embedded;
        }
        let welcome = Welcome {
            protocol: PROTOCOL,
            id: self.config.id.clone(),
            command: self.config.command.clone(),
            cwd: self.config.cwd.clone(),
            child_pid: self.record.child_pid,
            size: self.record.size,
            scrollback: self.config.scrollback,
            title: self.screen.title(),
            exit: self.record.exit.clone(),
            input: wants_input,
            keys: true,
        };
        let Ok(payload) = serde_json::to_vec(&welcome) else {
            return;
        };
        if tx.try_send(Out::Frame(Kind::Welcome, payload)).is_err() {
            return;
        }
        self.clients.insert(
            id,
            Client {
                tx,
                state: ClientState::Waiting(Instant::now()),
            },
        );
        if wants_input && clamp(hello.size) != self.record.size {
            self.resize(hello.size);
        }
        self.send_waiting_snapshots();
    }

    /// Sends a snapshot to every client waiting for one, unless the program
    /// is in the middle of a synchronized update that started less than a
    /// second ago. Each snapshot follows the current size, so that a client
    /// that missed a resize while waiting builds its screen at the right
    /// size. A client that has waited too long is dropped.
    fn send_waiting_snapshots(&mut self) {
        let syncing = self.screen.in_synchronized_update();
        let mut eligible = Vec::new();
        let mut stuck = Vec::new();
        for (id, client) in &self.clients {
            if let ClientState::Waiting(since) = client.state {
                if since.elapsed() >= STUCK_AFTER {
                    stuck.push(*id);
                } else if !syncing || since.elapsed() >= SYNC_WAIT {
                    eligible.push(*id);
                }
            }
        }
        for id in stuck {
            tracing::warn!(id, "dropping a client that stopped reading");
            self.clients.remove(&id);
            if self.owner == Some(id) {
                self.owner = None;
            }
        }
        if eligible.is_empty() {
            return;
        }
        let snapshot = match self.screen.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::warn!(%error, "snapshot failed");
                return;
            }
        };
        let Ok(size) = serde_json::to_vec(&Event::Resized {
            size: self.record.size,
        }) else {
            return;
        };
        for id in eligible {
            let Some(client) = self.clients.get_mut(&id) else {
                continue;
            };
            let sent = client
                .tx
                .try_send(Out::Frame(Kind::Event, size.clone()))
                .and_then(|()| {
                    client
                        .tx
                        .try_send(Out::Frame(Kind::Snapshot, snapshot.clone()))
                });
            match sent {
                Ok(()) => client.state = ClientState::Live,
                Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => {
                    self.clients.remove(&id);
                }
            }
        }
    }

    fn broadcast(&mut self, kind: Kind, payload: &[u8]) {
        let mut gone = Vec::new();
        for (id, client) in &mut self.clients {
            if client.state != ClientState::Live {
                continue;
            }
            match client.tx.try_send(Out::Frame(kind, payload.to_vec())) {
                Ok(()) => {}
                // It fell behind: it gets a fresh snapshot once it drains.
                Err(TrySendError::Full(_)) => client.state = ClientState::Waiting(Instant::now()),
                Err(TrySendError::Disconnected(_)) => gone.push(*id),
            }
        }
        for id in gone {
            self.clients.remove(&id);
        }
    }

    /// Sends an event to every client, including the ones waiting for a
    /// snapshot: an event is state (a title, an exit), not part of the
    /// stream a snapshot replaces.
    fn broadcast_event(&mut self, event: &Event) {
        let Ok(payload) = serde_json::to_vec(event) else {
            return;
        };
        let mut gone = Vec::new();
        for (id, client) in &mut self.clients {
            match client.tx.try_send(Out::Frame(Kind::Event, payload.clone())) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    if client.state == ClientState::Live {
                        client.state = ClientState::Waiting(Instant::now());
                    }
                }
                Err(TrySendError::Disconnected(_)) => gone.push(*id),
            }
        }
        for id in gone {
            self.clients.remove(&id);
        }
    }

    fn send_event_to(&mut self, id: u64, event: &Event) {
        if let (Some(client), Ok(payload)) = (self.clients.get(&id), serde_json::to_vec(event)) {
            let _ = client.tx.try_send(Out::Frame(Kind::Event, payload));
        }
    }

    /// The input owner's size becomes the program's size. Watchers are told
    /// and get a fresh snapshot.
    fn resize(&mut self, size: Size) {
        let size = clamp(size);
        if size.cols == 0 || size.rows == 0 || size == self.record.size {
            return;
        }
        if let Err(error) = self.master.resize(pty_size(size)) {
            tracing::warn!(%error, "resize failed");
            return;
        }
        if let Err(error) = self.screen.resize(size.cols, size.rows) {
            tracing::warn!(%error, "screen resize failed");
        }
        // An in-band size report is the owner's terminal's to send.
        let _ = self.screen.take_replies();
        self.record.size = size;
        self.write_record();
        self.broadcast_event(&Event::Resized { size });
        let owner = self.owner;
        for (id, client) in &mut self.clients {
            if Some(*id) != owner {
                client.state = ClientState::Waiting(Instant::now());
            }
        }
    }

    fn finish_exit(&mut self) {
        let Some((exit, _)) = self.pending_exit.take() else {
            return;
        };
        tracing::info!(?exit, "program ended");
        self.record.exit = Some(exit.clone());
        self.write_record();
        self.broadcast_event(&Event::Exited { exit });
        self.owner = None;
        self.linger_deadline = Some(Instant::now() + self.config.linger);
    }

    fn signal(&self, signal: Signal) {
        let group = self
            .record
            .child_pid
            .and_then(|pid| i32::try_from(pid).ok())
            .and_then(Pid::from_raw);
        if let Some(group) = group
            && let Err(error) = kill_process_group(group, signal)
        {
            tracing::warn!(%error, ?signal, "signal failed");
        }
    }

    fn write_record(&self) {
        if let Err(error) = self.record.write(&self.config.dir.record(&self.config.id)) {
            tracing::warn!(%error, "cannot write the host record");
        }
    }
}

/// The largest terminal a host emulates: a size from a client is trusted only
/// this far.
const MAX_SIZE: u16 = 1000;

fn clamp(size: Size) -> Size {
    Size {
        cols: size.cols.min(MAX_SIZE),
        rows: size.rows.min(MAX_SIZE),
    }
}

fn pty_size(size: Size) -> PtySize {
    PtySize {
        rows: size.rows.max(1),
        cols: size.cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn spawn_reader(mut reader: Box<dyn Read + Send>, tx: Sender<Msg>) {
    thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(Msg::Output(buf[..n].to_vec())).is_err() {
                        return;
                    }
                }
            }
        }
        let _ = tx.send(Msg::OutputClosed);
    });
}

/// Waits for the program to end and says how.
fn spawn_waiter(mut child: Box<dyn portable_pty::Child + Send + Sync>, tx: Sender<Msg>) {
    thread::spawn(move || {
        let exit = match child.wait() {
            Ok(status) => Exit {
                code: status.signal().is_none().then(|| status.exit_code()),
                signal: status.signal().map(str::to_owned),
                at: now(),
            },
            Err(error) => Exit {
                code: None,
                signal: Some(error.to_string()),
                at: now(),
            },
        };
        let _ = tx.send(Msg::Exited(exit));
    });
}

fn spawn_writer(mut writer: Box<dyn Write + Send>) -> Sender<Vec<u8>> {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        while let Ok(mut bytes) = rx.recv() {
            // Keys that arrive together go out in one write, as a terminal
            // sends a burst (text a tool types, a paste without bracketed
            // paste): programs such as Claude Code tell a paste from typing
            // by that, and a burst cut into single keys made Claude Code take
            // the Enter that followed as part of the text.
            while bytes.len() < WRITE_BATCH {
                match rx.recv_timeout(KEY_BURST) {
                    Ok(more) => bytes.extend(more),
                    Err(_) => break,
                }
            }
            if writer
                .write_all(&bytes)
                .and_then(|()| writer.flush())
                .is_err()
            {
                break;
            }
        }
    });
    tx
}

fn spawn_acceptor(listener: UnixListener, tx: Sender<Msg>) {
    thread::spawn(move || {
        for (id, stream) in (1u64..).zip(listener.incoming()) {
            match stream {
                Ok(stream) => {
                    let tx = tx.clone();
                    thread::spawn(move || serve_client(id, &stream, &tx));
                }
                Err(error) => tracing::warn!(%error, "accept failed"),
            }
        }
    });
}

/// Reads a client's frames and passes them to the terminal thread; a second
/// thread writes what the terminal thread queues for it.
fn serve_client(id: u64, stream: &UnixStream, tx: &Sender<Msg>) {
    let hello = (|| -> io::Result<Option<Hello>> {
        stream.set_read_timeout(Some(HELLO_TIMEOUT))?;
        let Some(frame) = proto::read_frame(stream)? else {
            return Ok(None);
        };
        if frame.kind != Kind::Hello {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "expected hello"));
        }
        stream.set_read_timeout(None)?;
        proto::parse(&frame).map(Some)
    })();
    let hello = match hello {
        Ok(Some(hello)) => hello,
        // A probe connects only to learn that the host answers, and leaves at
        // once; macOS then refuses the read timeout with EINVAL.
        Ok(None) => {
            tracing::debug!(id, "client left before its hello");
            return;
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::InvalidInput
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::BrokenPipe
            ) =>
        {
            tracing::debug!(id, %error, "client left before its hello");
            return;
        }
        Err(error) => {
            tracing::warn!(id, %error, "client refused");
            return;
        }
    };
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    let (out_tx, out_rx) = mpsc::sync_channel::<Out>(CLIENT_QUEUE);
    thread::spawn(move || {
        for Out::Frame(kind, payload) in out_rx {
            if proto::write_frame(&write_half, kind, &payload).is_err() {
                break;
            }
        }
        // The terminal thread dropped this client, or it went away.
        let _ = write_half.shutdown(std::net::Shutdown::Both);
    });
    if tx
        .send(Msg::Connected {
            id,
            hello,
            tx: out_tx,
        })
        .is_err()
    {
        return;
    }
    while let Ok(Some(frame)) = proto::read_frame(stream) {
        let msg = match frame.kind {
            Kind::Input => Msg::Input {
                id,
                bytes: frame.payload,
            },
            Kind::Key => match proto::parse::<KeyPress>(&frame) {
                Ok(press) => Msg::Key { id, press },
                Err(_) => continue,
            },
            Kind::Paste => match String::from_utf8(frame.payload) {
                Ok(text) => Msg::Paste { id, text },
                Err(_) => continue,
            },
            Kind::Resize => match proto::parse::<Size>(&frame) {
                Ok(size) => Msg::Resize { id, size },
                Err(_) => continue,
            },
            Kind::Stop => Msg::Stop,
            _ => continue,
        };
        if tx.send(msg).is_err() {
            return;
        }
    }
    let _ = tx.send(Msg::Gone { id });
}
