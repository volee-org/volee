//! Opening agents end to end (ADR 0027): a real daemon gives a terminal on
//! each session through the session's own door, here a small local program
//! in place of `claude attach` or `tuios pane-view`. Nothing is stopped,
//! forked or resumed to get it.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::FakeHarness;
use tokio::net::UnixListener;
use ulid::Ulid;
use volee_core::harness::SessionKey;
use volee_core::reconcile::ListedSession;
use volee_core::state::AgentState;
use volee_core::workspace::Location;
use volee_core::{AgentId, Event, Registry};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_harness::Harness;
use volee_host::{Connection, Size, Watch};
use volee_mux::{Hosted, Located, Multiplexer, MultiplexerStatus, MuxError};
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{EventsSince, Open, OpenParams, Opened, SinceParams};
use volee_proto::paths::Paths;
use volee_proto::rpc;

const SIZE: Size = Size { cols: 70, rows: 14 };

/// A multiplexer with one pane, which it shows alone or not.
#[derive(Debug, Default)]
struct FakeMux {
    panes: Mutex<Vec<(SessionKey, String)>>,
    /// The command that shows a pane alone; none, as for tmux.
    view: Option<Vec<String>>,
}

impl Multiplexer for FakeMux {
    fn name(&self) -> &'static str {
        "tuios"
    }

    fn status(&self) -> MultiplexerStatus {
        MultiplexerStatus {
            name: "tuios".into(),
            available: true,
            version: Some("0.12.1".into()),
            detail: None,
        }
    }

    fn locate(&self, sessions: &[Hosted]) -> Result<Vec<Located>, MuxError> {
        let panes = self.panes.lock().unwrap();
        Ok(sessions
            .iter()
            .filter_map(|session| {
                panes
                    .iter()
                    .find(|(key, _)| *key == session.key)
                    .map(|(key, target)| Located {
                        key: key.clone(),
                        target: target.clone(),
                    })
            })
            .collect())
    }

    fn capture(&self, target: &str) -> Result<String, MuxError> {
        Ok(format!("\x1b[1;1H> screen of {target}"))
    }

    fn focus_steps(&self, target: &str) -> Result<Vec<Vec<String>>, MuxError> {
        Ok(vec![vec!["tuios".into(), "focus".into(), target.into()]])
    }

    fn view_command(&self, target: &str) -> Option<Vec<String>> {
        let mut command = self.view.clone()?;
        command.push(target.to_owned());
        Some(command)
    }
}

struct Running {
    socket: PathBuf,
    harness: Arc<FakeHarness>,
    dir: tempfile::TempDir,
}

/// A daemon with a fake harness listed every 10 ms and `mux`.
fn start(mux: FakeMux) -> Running {
    // A short path: a view's socket lives under it.
    let dir = tempfile::Builder::new()
        .prefix("vo")
        .tempdir_in("/tmp")
        .unwrap();
    let paths = Paths::rooted(dir.path());
    std::fs::create_dir_all(&paths.state_dir).unwrap();
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let harness = Arc::new(FakeHarness::default());
    // The door of a session: a program that says which one it opened, then
    // repeats what is typed. Never the real Claude Code.
    harness.set_door(&["sh", "-c", "echo \"door of $0 in $(pwd)\"; exec cat"]);
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(vec![])),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: Some(Arc::clone(&harness) as Arc<dyn Harness>),
        discovery_interval: Duration::from_millis(10),
        max_discovery_interval: Duration::from_millis(10),
        idle_discovery_interval: Duration::from_millis(10),
        evidence: EvidenceSources::default(),
        volee_bin: None,
        multiplexers: vec![Arc::new(mux) as Arc<dyn Multiplexer>],
        hosts: false,
        processes: None,
    };
    tokio::spawn(Daemon::serve(listener, config));
    Running {
        socket,
        harness,
        dir,
    }
}

async fn registry(client: &Client) -> Registry {
    let batch = client
        .call::<EventsSince>(SinceParams::default())
        .await
        .unwrap();
    let events: Vec<Event> = batch.events.into_iter().map(|e| e.event).collect();
    Registry::from_events(&events).unwrap()
}

/// Waits until the registry satisfies `ready`, for at most ten seconds.
async fn until(client: &Client, ready: impl Fn(&Registry) -> bool) -> Registry {
    for _ in 0..1_000 {
        let registry = registry(client).await;
        if ready(&registry) {
            return registry;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the registry never reached the expected state");
}

fn rpc_error<T: std::fmt::Debug>(result: Result<T, ClientError>) -> rpc::RpcError {
    match result {
        Err(ClientError::Rpc(error)) => error,
        other => panic!("expected an RPC error, got {other:?}"),
    }
}

/// A background session, as `claude agents` lists it.
fn background(id: &str, alive: bool) -> ListedSession {
    ListedSession {
        name: Some("invoices".to_owned()),
        alive,
        background: true,
        handle: Some(id[..8].to_owned()),
        ..ListedSession::new(
            SessionKey::claude(id),
            "/work/alpha",
            if alive {
                AgentState::Working
            } else {
                AgentState::Stopped
            },
        )
    }
}

/// An interactive session, as Claude Code lists one a person runs.
fn interactive(id: &str) -> ListedSession {
    ListedSession {
        name: Some("by hand".to_owned()),
        alive: true,
        pid: Some(4_242),
        ..ListedSession::new(SessionKey::claude(id), "/work/alpha", AgentState::Idle)
    }
}

fn open(agent: AgentId) -> OpenParams {
    OpenParams {
        agent,
        cols: SIZE.cols,
        rows: SIZE.rows,
    }
}

/// Reads a view's screen until it shows `text`, for at most ten seconds.
fn sees(socket: &Path, text: &str) -> String {
    let mut watch = Watch::start(socket, "test").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        watch.pump();
        let screen = watch.text().unwrap_or_default();
        if screen.contains(text) {
            return screen;
        }
        assert!(
            Instant::now() < deadline,
            "never saw {text:?} in {screen:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Ends a view's program so that the test leaves no process behind.
fn close(socket: &Path) {
    let _ = volee_host::stop(socket, "test");
}

const BACKGROUND: &str = "0a1b2c3d-0000-4000-8000-000000000001";
const BY_HAND: &str = "0a1b2c3d-0000-4000-8000-000000000002";

#[tokio::test(flavor = "multi_thread")]
async fn a_background_session_opens_through_its_own_door() {
    let daemon = start(FakeMux::default());
    daemon.harness.set(vec![background(BACKGROUND, true)]);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let registry = until(&client, |r| r.agents().count() == 1).await;
    let agent = registry.agents().next().unwrap().id;

    let opened = client.call::<Open>(open(agent)).await.unwrap();
    assert!(opened.view, "a view, not the agent's own host");
    let socket = opened.socket.clone().unwrap();
    assert!(
        socket.to_string_lossy().contains("/views/view-"),
        "{}",
        socket.display()
    );

    // The view runs the harness's own command for that session, at the size
    // of the panel, and takes what is typed.
    let typed = tokio::task::spawn_blocking(move || {
        let owner = Connection::open_embedded(&socket, SIZE, "test").unwrap();
        assert!(owner.welcome.input, "the client holds the input");
        assert_eq!(owner.welcome.size, SIZE);
        sees(&socket, "door of 0a1b2c3d");
        owner.sender().unwrap().input(b"hello\r").unwrap();
        let screen = sees(&socket, "hello");
        (owner, socket, screen)
    })
    .await
    .unwrap();
    let (owner, socket, _) = typed;

    // Opened again while it runs: the same view.
    let again = client.call::<Open>(open(agent)).await.unwrap();
    assert_eq!(again, opened);

    // Nothing was stopped, resumed or adopted: the session is where it was.
    assert_eq!(
        daemon.harness.actions(),
        [
            ("attach".to_owned(), "0a1b2c3d".to_owned()),
            ("attach".to_owned(), "0a1b2c3d".to_owned()),
        ]
    );
    let registry = registry_now(&client).await;
    let record = registry.agent(agent).unwrap();
    assert_eq!(record.location, Location::Background);
    assert_eq!(record.sessions.len(), 1);
    assert!(record.watched());

    drop(owner);
    close(&socket);
}

async fn registry_now(client: &Client) -> Registry {
    registry(client).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_finished_background_session_opens_too() {
    let daemon = start(FakeMux::default());
    daemon.harness.set(vec![background(BACKGROUND, false)]);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let registry = until(&client, |r| r.agents().count() == 1).await;
    let agent = registry.agents().next().unwrap().id;
    let opened = client.call::<Open>(open(agent)).await.unwrap();
    let socket = opened.socket.unwrap();
    let shown = socket.clone();
    tokio::task::spawn_blocking(move || sees(&shown, "door of 0a1b2c3d"))
        .await
        .unwrap();
    close(&socket);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_door_runs_in_its_session_s_directory_never_in_the_root() {
    let daemon = start(FakeMux::default());
    // A session in a directory that exists: its door runs there.
    let work = daemon.dir.path().canonicalize().unwrap().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let mut here = background(BACKGROUND, true);
    here.cwd.clone_from(&work);
    // One whose directory is gone (a worktree removed since): the daemon's
    // own directory, not `/`.
    let mut gone = background("1b2c3d4e-0000-4000-8000-000000000009", true);
    gone.name = Some("gone".to_owned());
    gone.cwd = work.join("removed");
    daemon.harness.set(vec![here, gone]);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let registry = until(&client, |r| r.agents().count() == 2).await;
    let agent = |name: &str| registry.agents().find(|a| a.name == name).unwrap().id;

    let opened = client.call::<Open>(open(agent("invoices"))).await.unwrap();
    let socket = opened.socket.unwrap();
    let (shown, expected) = (socket.clone(), format!("in {}", work.display()));
    tokio::task::spawn_blocking(move || sees(&shown, &expected))
        .await
        .unwrap();
    close(&socket);

    let opened = client.call::<Open>(open(agent("gone"))).await.unwrap();
    let socket = opened.socket.unwrap();
    let shown = socket.clone();
    let screen = tokio::task::spawn_blocking(move || sees(&shown, "door of 1b2c3d4e in "))
        .await
        .unwrap();
    assert!(screen.contains("/state"), "{screen}");
    assert!(
        !screen.contains("in /\n") && !screen.contains("in / "),
        "{screen}"
    );
    close(&socket);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_in_a_terminal_volee_cannot_reach_says_the_way_in() {
    let daemon = start(FakeMux::default());
    daemon.harness.set(vec![interactive(BY_HAND)]);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let registry = until(&client, |r| r.agents().count() == 1).await;
    let agent = registry.agents().next().unwrap().id;
    let refused = rpc_error(client.call::<Open>(open(agent)).await);
    assert_eq!(refused.code, rpc::REFUSED);
    assert!(
        refused.message.contains("switch to that terminal")
            && refused.message.contains("left arrow"),
        "{}",
        refused.message
    );
    assert_eq!(daemon.harness.actions(), [], "nothing was run");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pane_opens_through_its_multiplexer() {
    let mux = FakeMux {
        panes: Mutex::new(vec![(SessionKey::claude(BY_HAND), "work:3f2a".to_owned())]),
        view: Some(
            ["sh", "-c", "echo \"pane $0\"; exec cat"]
                .map(str::to_owned)
                .to_vec(),
        ),
    };
    let daemon = start(mux);
    daemon.harness.set(vec![interactive(BY_HAND)]);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let registry = until(&client, |r| {
        r.agents()
            .any(|agent| matches!(agent.location, Location::Pane { .. }))
    })
    .await;
    let agent = registry.agents().next().unwrap().id;
    let opened = client.call::<Open>(open(agent)).await.unwrap();
    assert!(opened.view);
    let socket = opened.socket.unwrap();
    let shown = socket.clone();
    tokio::task::spawn_blocking(move || sees(&shown, "pane work:3f2a"))
        .await
        .unwrap();
    assert_eq!(
        daemon.harness.actions(),
        [],
        "the harness has no part in it"
    );
    close(&socket);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pane_its_multiplexer_cannot_show_alone_is_left_to_the_client() {
    let mux = FakeMux {
        panes: Mutex::new(vec![(SessionKey::claude(BY_HAND), "work:%1".to_owned())]),
        view: None,
    };
    let daemon = start(mux);
    daemon.harness.set(vec![interactive(BY_HAND)]);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let registry = until(&client, |r| {
        r.agents()
            .any(|agent| matches!(agent.location, Location::Pane { .. }))
    })
    .await;
    let agent = registry.agents().next().unwrap().id;
    assert_eq!(
        client.call::<Open>(open(agent)).await.unwrap(),
        Opened {
            socket: None,
            view: false
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_agent_is_not_found() {
    let daemon = start(FakeMux::default());
    let client = Client::connect(&daemon.socket).await.unwrap();
    let agent = AgentId::from_ulid(Ulid::from_parts(9_999, 9));
    assert_eq!(
        rpc_error(client.call::<Open>(open(agent)).await).code,
        rpc::NOT_FOUND
    );
}
