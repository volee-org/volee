//! An agent of another harness, started by hand, found and placed (ADR 0015,
//! ADR 0023): OpenCode in the process table, in a pane of a multiplexer next
//! to a Claude Code session in another pane. A real daemon, a fake process
//! table and a fake multiplexer.
//!
//! Both agents are placed in their panes and stay there round after round:
//! the panes are searched once for every session, whatever its harness. The
//! OpenCode agent is then shown and brought forward as any agent in a pane
//! is; one in a plain terminal is told so.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::FakeHarness;
use tokio::net::UnixListener;
use volee_core::event::EventKind;
use volee_core::harness::{HarnessKind, SessionKey};
use volee_core::reconcile::ListedSession;
use volee_core::state::AgentState;
use volee_core::workspace::Location;
use volee_core::{Agent, Event, Registry};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_harness::{CommandRunner, Harness, Invocation, Output, RunError};
use volee_mux::{Hosted, Located, Multiplexer, MultiplexerStatus, MuxError};
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{AgentParams, Attach, EventsSince, Preview, SinceParams};
use volee_proto::paths::Paths;
use volee_proto::rpc;

const CLAUDE_SESSION: &str = "e41ffe96-d588-4d61-b6a0-b75bdcebe52c";

/// The process table and the directories of its processes, as `ps` and
/// `lsof` print them; the test rewrites the table.
#[derive(Debug)]
struct Table {
    ps: Mutex<String>,
}

impl CommandRunner for Table {
    fn run(&self, invocation: &Invocation) -> Result<Output, RunError> {
        let program = invocation.program.to_string_lossy().into_owned();
        if program.ends_with("ps") {
            return Ok(Output::success(self.ps.lock().unwrap().clone()));
        }
        // Every process of the table runs in a directory named after it.
        let dirs: Vec<String> = self
            .ps
            .lock()
            .unwrap()
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .map(|pid| format!("p{pid}\nfcwd\nn/work/oc-{pid}\n"))
            .collect();
        Ok(Output::success(dirs.concat()))
    }
}

/// OpenCode started by hand, as the process table shows it.
const BY_HAND: &str = "   4242   900 opencode\n   4343   901 opencode\n";

fn opencode(pid: u32) -> SessionKey {
    SessionKey {
        harness: HarnessKind::Other("opencode".into()),
        id: format!("pid-{pid}"),
    }
}

/// A multiplexer whose panes the test sets.
#[derive(Debug, Default)]
struct FakeMux {
    panes: Mutex<Vec<(SessionKey, String)>>,
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
}

struct Running {
    socket: PathBuf,
    table: Arc<Table>,
    _harness: Arc<FakeHarness>,
    _dir: tempfile::TempDir,
}

/// A daemon that lists one Claude Code session started by hand, reads the
/// process table `ps`, and looks in a multiplexer that holds the Claude
/// session in `work:c1` and the OpenCode process 4242 in `work:o1`.
fn start(ps: &str) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let harness = Arc::new(FakeHarness::default());
    let mut claude = ListedSession::new(
        SessionKey::claude(CLAUDE_SESSION),
        "/work/alpha",
        AgentState::Idle,
    );
    claude.alive = true;
    claude.pid = Some(75_441);
    harness.set(vec![claude]);
    let mux = Arc::new(FakeMux::default());
    *mux.panes.lock().unwrap() = vec![
        (SessionKey::claude(CLAUDE_SESSION), "work:c1".to_owned()),
        (opencode(4242), "work:o1".to_owned()),
    ];
    let table = Arc::new(Table {
        ps: Mutex::new(ps.to_owned()),
    });
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
        multiplexers: vec![mux as Arc<dyn Multiplexer>],
        // The process table is read along with the hosts.
        hosts: true,
        processes: Some(Arc::clone(&table) as Arc<dyn CommandRunner>),
    };
    tokio::spawn(Daemon::serve(listener, config));
    Running {
        socket,
        table,
        _harness: harness,
        _dir: dir,
    }
}

async fn events(socket: &Path) -> Vec<Event> {
    let client = Client::connect(socket).await.unwrap();
    let batch = client
        .call::<EventsSince>(SinceParams::default())
        .await
        .unwrap();
    batch.events.into_iter().map(|e| e.event).collect()
}

/// Waits until `ready` holds of the registry, for at most ten seconds.
async fn until(socket: &Path, ready: impl Fn(&Registry) -> bool) -> Registry {
    for _ in 0..1_000 {
        let registry = Registry::from_events(&events(socket).await).unwrap();
        if ready(&registry) {
            return registry;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the registry never got there");
}

fn pane(target: &str) -> Location {
    Location::Pane {
        multiplexer: "tuios".into(),
        target: target.into(),
    }
}

fn at<'a>(registry: &'a Registry, key: &SessionKey) -> Option<&'a Agent> {
    registry.agent_for_session(key)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_of_another_harness_is_placed_in_its_pane_and_stays_there() {
    let daemon = start(BY_HAND);
    let claude = SessionKey::claude(CLAUDE_SESSION);
    let registry = until(&daemon.socket, |registry| {
        at(registry, &opencode(4242)).is_some_and(|agent| agent.location == pane("work:o1"))
            && at(registry, &claude).is_some_and(|agent| agent.location == pane("work:c1"))
            && at(registry, &opencode(4343)).is_some()
    })
    .await;
    let in_pane = at(&registry, &opencode(4242)).unwrap().clone();
    assert_eq!(in_pane.harness, HarnessKind::Other("opencode".into()));
    // Found by volee, known by a session: watched, not joined through MCP.
    assert!(in_pane.watched() && !in_pane.joined());
    // The one no pane holds runs in a terminal volee cannot reach.
    let elsewhere = at(&registry, &opencode(4343)).unwrap().clone();
    assert_eq!(elsewhere.location, Location::Terminal);

    // Thirty rounds later, each agent has been placed once: two searches,
    // one per harness, would move each other's agents out of their panes
    // and back at every round.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let moves = |events: &[Event], agent: &Agent| {
        events
            .iter()
            .filter(|event| {
                matches!(&event.kind, EventKind::LocationChanged { agent: moved, .. } if *moved == agent.id)
            })
            .count()
    };
    let log = events(&daemon.socket).await;
    let registry = Registry::from_events(&log).unwrap();
    let claude_agent = at(&registry, &claude).unwrap().clone();
    assert_eq!(moves(&log, &in_pane), 1, "the OpenCode agent moved once");
    assert_eq!(
        moves(&log, &claude_agent),
        1,
        "the Claude Code agent moved once"
    );
    assert_eq!(claude_agent.location, pane("work:c1"));
    assert_eq!(
        at(&registry, &opencode(4242)).unwrap().location,
        pane("work:o1")
    );

    // In its pane, it is shown and brought forward as any agent in a pane.
    let client = Client::connect(&daemon.socket).await.unwrap();
    let preview = client
        .call::<Preview>(AgentParams { agent: in_pane.id })
        .await
        .unwrap();
    assert_eq!(preview.screen, "\x1b[1;1H> screen of work:o1");
    assert!(preview.exact);
    let attach = client
        .call::<Attach>(AgentParams { agent: in_pane.id })
        .await
        .unwrap();
    assert_eq!(attach.program, "", "nothing takes the terminal over");
    assert_eq!(
        attach.focus.map(|focus| focus.steps),
        Some(vec![vec![
            "tuios".to_owned(),
            "focus".to_owned(),
            "work:o1".to_owned()
        ]])
    );

    // In a plain terminal, volee says where it is, not that it cannot be
    // driven.
    let refused = client
        .call::<Preview>(AgentParams {
            agent: elsewhere.id,
        })
        .await;
    let Err(ClientError::Rpc(error)) = refused else {
        panic!("the preview of an agent in a plain terminal is refused: {refused:?}");
    };
    assert_eq!(error.code, rpc::REFUSED);
    assert!(
        error
            .message
            .contains("runs in a terminal volee cannot reach"),
        "{}",
        error.message
    );

    // Its process ends: it stops, and its pane is no longer its own.
    *daemon.table.ps.lock().unwrap() = "   4343   901 opencode\n".to_owned();
    let registry = until(&daemon.socket, |registry| {
        registry
            .agent(in_pane.id)
            .is_none_or(|agent| !agent.state.is_live() && agent.location == Location::Terminal)
    })
    .await;
    assert!(
        registry
            .agent(elsewhere.id)
            .is_some_and(|agent| agent.state.is_live())
    );
}
