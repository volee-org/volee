//! Location adapters end to end (ADR 0015): a fake multiplexer finds an
//! interactive session in one of its panes, and a real daemon records where
//! the agent runs, previews the pane and answers its take over with the
//! steps that bring the pane forward.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::FakeHarness;
use tokio::net::UnixListener;
use volee_core::event::EventKind;
use volee_core::harness::SessionKey;
use volee_core::harness::{HarnessKind, SessionOrigin};
use volee_core::reconcile::ListedSession;
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::AgentState;
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};
use volee_core::{AgentId, Event, Registry};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_harness::Harness;
use volee_mux::{Hosted, Located, Multiplexer, MultiplexerStatus, MuxError};
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{
    AgentParams, Attach, EventsSince, PaneFocus, Preview, SinceParams, Status, StatusParams,
};
use volee_proto::paths::Paths;
use volee_proto::rpc;

const SESSION: &str = "e41ffe96-d588-4d61-b6a0-b75bdcebe52c";

/// A multiplexer whose panes the test sets.
#[derive(Debug, Default)]
struct FakeMux {
    /// Its name; `tuios` when empty.
    name: &'static str,
    /// The sessions in its panes, and their addresses.
    panes: Mutex<Vec<(SessionKey, String)>>,
    /// Why it cannot answer, when it cannot.
    down: Mutex<Option<String>>,
    /// What it was asked about, each time.
    asked: Mutex<Vec<Vec<Hosted>>>,
}

impl FakeMux {
    fn named(name: &'static str) -> Self {
        Self {
            name,
            ..Self::default()
        }
    }

    fn put(&self, key: &str, target: &str) {
        *self.panes.lock().unwrap() = vec![(SessionKey::claude(key), target.to_owned())];
    }

    fn clear(&self) {
        self.panes.lock().unwrap().clear();
    }

    fn stop(&self, why: &str) {
        *self.down.lock().unwrap() = Some(why.to_owned());
    }
}

impl Multiplexer for FakeMux {
    fn name(&self) -> &'static str {
        if self.name.is_empty() {
            "tuios"
        } else {
            self.name
        }
    }

    fn status(&self) -> MultiplexerStatus {
        MultiplexerStatus {
            name: self.name().into(),
            available: true,
            version: Some("0.12.1".into()),
            detail: None,
        }
    }

    fn locate(&self, sessions: &[Hosted]) -> Result<Vec<Located>, MuxError> {
        self.asked.lock().unwrap().push(sessions.to_vec());
        if let Some(why) = self.down.lock().unwrap().clone() {
            return Err(MuxError::Unavailable(why));
        }
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
}

/// An interactive session, as Claude Code lists one started by hand: alive,
/// with a process and no handle.
fn interactive(id: &str) -> ListedSession {
    let mut session = ListedSession::new(SessionKey::claude(id), "/work/alpha", AgentState::Idle);
    session.alive = true;
    session.pid = Some(75_441);
    session
}

struct Running {
    socket: PathBuf,
    harness: Arc<FakeHarness>,
    mux: Arc<FakeMux>,
    _dir: tempfile::TempDir,
}

fn start() -> Running {
    start_with(Vec::new(), true)
}

/// A daemon whose log holds `history`, with the fake multiplexer or none.
fn start_with(history: Vec<Event>, uses_mux: bool) -> Running {
    let mux = Arc::new(FakeMux::default());
    let muxes = if uses_mux {
        vec![Arc::clone(&mux)]
    } else {
        Vec::new()
    };
    start_muxes(history, mux, muxes)
}

/// A daemon whose log holds `history` and which uses `muxes`, in order;
/// `mux` is the one the test looks at.
fn start_muxes(history: Vec<Event>, mux: Arc<FakeMux>, muxes: Vec<Arc<FakeMux>>) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let harness = Arc::new(FakeHarness::default());
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(history)),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: Some(Arc::clone(&harness) as Arc<dyn Harness>),
        discovery_interval: Duration::from_millis(10),
        max_discovery_interval: Duration::from_millis(10),
        idle_discovery_interval: Duration::from_millis(10),
        evidence: EvidenceSources::default(),
        volee_bin: None,
        multiplexers: muxes
            .into_iter()
            .map(|mux| mux as Arc<dyn Multiplexer>)
            .collect(),
        hosts: false,
    };
    tokio::spawn(Daemon::serve(listener, config));
    Running {
        socket,
        harness,
        mux,
        _dir: dir,
    }
}

async fn registry(socket: &Path) -> Registry {
    let client = Client::connect(socket).await.unwrap();
    let batch = client
        .call::<EventsSince>(SinceParams::default())
        .await
        .unwrap();
    let events: Vec<Event> = batch.events.into_iter().map(|e| e.event).collect();
    Registry::from_events(&events).unwrap()
}

/// Waits until the session's agent runs `at`, for at most ten seconds.
async fn located(socket: &Path, at: &Location) -> Registry {
    for _ in 0..1_000 {
        let registry = registry(socket).await;
        if registry
            .agent_for_session(&SessionKey::claude(SESSION))
            .is_some_and(|agent| agent.location == *at)
        {
            return registry;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the agent never ran in {at:?}");
}

fn pane(target: &str) -> Location {
    Location::Pane {
        multiplexer: "tuios".into(),
        target: target.into(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_in_a_pane_is_shown_and_brought_forward() {
    let daemon = start();
    daemon.mux.put(SESSION, "work:c434b5b6");
    daemon.harness.set(vec![interactive(SESSION)]);
    let found = located(&daemon.socket, &pane("work:c434b5b6")).await;
    let agent = found
        .agent_for_session(&SessionKey::claude(SESSION))
        .unwrap()
        .id;
    let asked = daemon.mux.asked.lock().unwrap()[0].clone();
    assert_eq!(
        asked,
        [Hosted {
            key: SessionKey::claude(SESSION),
            pid: Some(75_441),
        }]
    );

    let client = Client::connect(&daemon.socket).await.unwrap();
    let preview = client.call::<Preview>(AgentParams { agent }).await.unwrap();
    assert_eq!(preview.screen, "\x1b[1;1H> screen of work:c434b5b6");
    let attach = client.call::<Attach>(AgentParams { agent }).await.unwrap();
    assert_eq!(attach.program, "", "nothing takes the terminal over");
    assert_eq!(
        attach.focus,
        Some(PaneFocus {
            multiplexer: "tuios".into(),
            session: "work".into(),
            steps: vec![vec!["tuios".into(), "focus".into(), "work:c434b5b6".into()]],
            attach: Vec::new(),
        })
    );
    let status = client
        .call::<Status>(StatusParams {
            protocol: volee_proto::PROTOCOL_VERSION,
        })
        .await
        .unwrap();
    assert_eq!(status.multiplexers.len(), 1);
    assert!(status.multiplexers[0].available);

    // tuios stops answering: the agent keeps its pane, and the status says
    // why.
    daemon.mux.stop("The TUIOS daemon is not running");
    let rounds = daemon.mux.asked.lock().unwrap().len();
    for _ in 0..1_000 {
        if daemon.mux.asked.lock().unwrap().len() > rounds + 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let after = registry(&daemon.socket).await;
    assert_eq!(after.agent(agent).unwrap().location, pane("work:c434b5b6"));
    let status = client
        .call::<Status>(StatusParams {
            protocol: volee_proto::PROTOCOL_VERSION,
        })
        .await
        .unwrap();
    assert!(!status.multiplexers[0].available);
    assert_eq!(
        status.multiplexers[0].detail.as_deref(),
        Some("The TUIOS daemon is not running")
    );

    // It answers again without the session: a plain terminal again, which
    // volee cannot show.
    *daemon.mux.down.lock().unwrap() = None;
    daemon.mux.clear();
    located(&daemon.socket, &Location::Terminal).await;
    let refused = client.call::<Preview>(AgentParams { agent }).await;
    assert!(
        matches!(&refused, Err(ClientError::Rpc(error)) if error.code == rpc::REFUSED),
        "{refused:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_whose_session_ended_leaves_its_pane() {
    let daemon = start();
    daemon.mux.put(SESSION, "work:c434b5b6");
    daemon.harness.set(vec![interactive(SESSION)]);
    located(&daemon.socket, &pane("work:c434b5b6")).await;
    let mut ended = interactive(SESSION);
    ended.alive = false;
    ended.pid = None;
    daemon.harness.set(vec![ended]);
    located(&daemon.socket, &Location::Terminal).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pane_of_a_multiplexer_no_longer_used_is_forgotten() {
    // An earlier run found the session in a tuios pane; this one runs
    // without tuios, so it can neither show nor reach that pane.
    let agent = AgentId::from_ulid(ulid::Ulid::from_parts(1, 1));
    let at = Timestamp::from_millis(5_000);
    let history = vec![
        Event::new(
            at,
            EventKind::AgentRegistered {
                agent,
                name: "developer-a1".into(),
                role: Role::Independent,
                harness: HarnessKind::ClaudeCode,
                launched_by: LaunchedBy::Outside,
                parent: None,
                mission: None,
                workspace: Workspace::at("/work/alpha"),
                location: Location::Terminal,
            },
        ),
        Event::new(
            at,
            EventKind::SessionLinked {
                agent,
                session: SessionKey::claude(SESSION),
                origin: SessionOrigin::Discovered,
            },
        ),
        Event::new(
            at,
            EventKind::LocationChanged {
                agent,
                location: pane("work:c434b5b6"),
            },
        ),
    ];
    let daemon = start_with(history, false);
    daemon.harness.set(vec![interactive(SESSION)]);
    located(&daemon.socket, &Location::Terminal).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_multiplexer_that_finds_a_session_keeps_it() {
    // A session in tmux, inside a tuios pane whose variables it inherited:
    // both multiplexers find it.
    let tmux = Arc::new(FakeMux::named("tmux"));
    let tuios = Arc::new(FakeMux::named("tuios"));
    tmux.put(SESSION, "work:%1");
    tuios.put(SESSION, "work:c434b5b6");
    let daemon = start_muxes(
        Vec::new(),
        Arc::clone(&tmux),
        vec![Arc::clone(&tmux), Arc::clone(&tuios)],
    );
    daemon.harness.set(vec![interactive(SESSION)]);
    let in_tmux = Location::Pane {
        multiplexer: "tmux".into(),
        target: "work:%1".into(),
    };
    located(&daemon.socket, &in_tmux).await;
    // Let a few more rounds pass: the agent stays, moved once.
    for _ in 0..1_000 {
        if tmux.asked.lock().unwrap().len() >= 6 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let registry = located(&daemon.socket, &in_tmux).await;
    assert!(
        registry
            .agent_for_session(&SessionKey::claude(SESSION))
            .is_some()
    );
    let client = Client::connect(&daemon.socket).await.unwrap();
    let moves = client
        .call::<EventsSince>(SinceParams::default())
        .await
        .unwrap()
        .events
        .into_iter()
        .filter(|event| matches!(event.event.kind, EventKind::LocationChanged { .. }))
        .count();
    assert_eq!(moves, 1);
    // tuios was never offered the session tmux had found.
    let offered = tuios
        .asked
        .lock()
        .unwrap()
        .iter()
        .flatten()
        .any(|session| session.key == SessionKey::claude(SESSION));
    assert!(!offered);
}
