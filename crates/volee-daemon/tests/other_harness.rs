//! An agent of another harness, started by hand, found and placed (ADR 0015,
//! ADR 0023): OpenCode in the process table, in a pane of a multiplexer next
//! to a Claude Code session in another pane. A real daemon, a fake process
//! table and a fake multiplexer.
//!
//! And an agent of a harness volee has no listing for, which the multiplexer
//! recognizes in one of its panes (ADR 0030): it is an agent in that pane,
//! for as long as the multiplexer names it.
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
use volee_mux::{Hosted, Located, Multiplexer, MultiplexerStatus, MuxError, PaneAgent};
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

/// A multiplexer whose panes the test sets, and the agents it recognizes
/// in them.
#[derive(Debug, Default)]
struct FakeMux {
    panes: Mutex<Vec<(SessionKey, String)>>,
    agents: Mutex<Vec<PaneAgent>>,
    /// How it answers when asked which panes run an agent.
    telling: Mutex<Telling>,
    /// The harnesses it was last told the daemon finds by itself.
    known: Mutex<Vec<String>>,
}

/// How a multiplexer answers when asked which of its panes run an agent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Telling {
    /// It names them.
    #[default]
    Names,
    /// It fails, this time: a timeout, an answer nobody can read.
    Fails,
    /// Its server does not run: it has no pane at all.
    NoServer,
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

    fn agents(&self, known: &[&str]) -> Result<Vec<PaneAgent>, MuxError> {
        *self.known.lock().unwrap() = known.iter().map(|name| (*name).to_owned()).collect();
        match *self.telling.lock().unwrap() {
            Telling::Names => Ok(self.agents.lock().unwrap().clone()),
            Telling::Fails => Err(MuxError::Unexpected {
                command: "tuios call list-agents".into(),
            }),
            Telling::NoServer => Err(MuxError::Unavailable("no server runs".into())),
        }
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
    mux: Arc<FakeMux>,
    clock: Arc<ManualClock>,
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
    let clock = Arc::new(ManualClock::at(10_000));
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(vec![])),
        clock: Arc::clone(&clock) as Arc<dyn volee_daemon::Clock>,
        harnesses: vec![],
        spool: None,
        harness: Some(Arc::clone(&harness) as Arc<dyn Harness>),
        discovery_interval: Duration::from_millis(10),
        max_discovery_interval: Duration::from_millis(10),
        idle_discovery_interval: Duration::from_millis(10),
        evidence: EvidenceSources::default(),
        volee_bin: None,
        multiplexers: vec![Arc::clone(&mux) as Arc<dyn Multiplexer>],
        // The process table is read along with the hosts.
        hosts: true,
        processes: Some(Arc::clone(&table) as Arc<dyn CommandRunner>),
    };
    tokio::spawn(Daemon::serve(listener, config));
    Running {
        socket,
        table,
        mux,
        clock,
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

/// The agent of a harness volee has no listing for, keyed by its process.
fn in_a_pane(harness: &str, pid: u32) -> SessionKey {
    SessionKey {
        harness: HarnessKind::Other(harness.into()),
        id: format!("pid-{pid}"),
    }
}

/// What a multiplexer says of a pane that runs an agent.
fn recognized(harness: &str, pid: u32, cwd: &str, target: &str) -> PaneAgent {
    PaneAgent {
        harness: harness.into(),
        pid,
        cwd: cwd.into(),
        target: target.into(),
    }
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

#[tokio::test(flavor = "multi_thread")]
async fn a_pane_that_runs_a_harness_volee_cannot_list_is_an_agent_in_that_pane() {
    let daemon = start("");
    let state_of =
        |registry: &Registry, key: &SessionKey| at(registry, key).map(|agent| agent.state.clone());
    // The multiplexer recognizes Codex in one of its panes, and Gemini in
    // another: neither has a listing volee reads.
    let codex = in_a_pane("codex", 5151);
    let gemini = in_a_pane("gemini-cli", 6262);
    *daemon.mux.agents.lock().unwrap() = vec![
        recognized("codex", 5151, "/work/api", "work:x1"),
        recognized("gemini-cli", 6262, "/work/docs", "work:g1"),
    ];
    daemon.mux.panes.lock().unwrap().extend([
        (codex.clone(), "work:x1".to_owned()),
        (gemini.clone(), "work:g1".to_owned()),
    ]);

    let registry = until(&daemon.socket, |registry| {
        at(registry, &codex).is_some_and(|agent| agent.location == pane("work:x1"))
            && at(registry, &gemini).is_some_and(|agent| agent.location == pane("work:g1"))
    })
    .await;
    let agent = at(&registry, &codex).unwrap().clone();
    // An agent of the harness the multiplexer names, in the pane's
    // directory, which volee only watches.
    assert_eq!(agent.harness, HarnessKind::Other("codex".into()));
    assert_eq!(agent.name, "api");
    assert_eq!(agent.workspace.cwd, Path::new("/work/api"));
    assert_eq!(agent.state, AgentState::Idle);
    assert!(agent.watched() && !agent.joined() && !agent.generic());
    // The harnesses volee finds by itself are not asked of the multiplexer.
    assert_eq!(
        *daemon.mux.known.lock().unwrap(),
        ["claude-code", "opencode"]
    );

    // It is shown and brought forward as any agent in a pane.
    let client = Client::connect(&daemon.socket).await.unwrap();
    let preview = client
        .call::<Preview>(AgentParams { agent: agent.id })
        .await
        .unwrap();
    assert_eq!(preview.screen, "\x1b[1;1H> screen of work:x1");
    assert!(preview.exact);
    let attach = client
        .call::<Attach>(AgentParams { agent: agent.id })
        .await
        .unwrap();
    assert_eq!(
        attach.focus.map(|focus| focus.steps),
        Some(vec![vec![
            "tuios".to_owned(),
            "focus".to_owned(),
            "work:x1".to_owned()
        ]])
    );

    // A multiplexer that fails to tell, this time, says nothing of the
    // agents it named before: they stay as they are.
    *daemon.mux.telling.lock().unwrap() = Telling::Fails;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let registry = Registry::from_events(&events(&daemon.socket).await).unwrap();
    assert_eq!(state_of(&registry, &codex), Some(AgentState::Idle));
    assert_eq!(state_of(&registry, &gemini), Some(AgentState::Idle));

    // It answers again. In the Codex pane another process is now in the
    // foreground: the person quit the program and started it again. The
    // agent is the process, not the pane: the first one has ended, and the
    // second is a new agent in the same pane.
    let again = in_a_pane("codex", 7373);
    *daemon.mux.agents.lock().unwrap() = vec![
        recognized("codex", 7373, "/work/api", "work:x1"),
        recognized("gemini-cli", 6262, "/work/docs", "work:g1"),
    ];
    {
        let mut panes = daemon.mux.panes.lock().unwrap();
        panes.retain(|(key, _)| *key != codex);
        panes.push((again.clone(), "work:x1".to_owned()));
    }
    *daemon.mux.telling.lock().unwrap() = Telling::Names;
    let registry = until(&daemon.socket, |registry| {
        registry
            .agent(agent.id)
            .is_some_and(|agent| agent.state == AgentState::Stopped)
            && at(registry, &again).is_some_and(|agent| agent.location == pane("work:x1"))
    })
    .await;
    assert_eq!(
        registry.agent(agent.id).unwrap().detail.as_deref(),
        Some("its process ended")
    );
    assert_ne!(at(&registry, &again).unwrap().id, agent.id);
    assert_eq!(state_of(&registry, &gemini), Some(AgentState::Idle));

    // A minute later the one that ended has left the list.
    daemon.clock.advance(61_000);
    until(&daemon.socket, |registry| {
        registry.agent(agent.id).is_none()
    })
    .await;

    // A multiplexer whose server no longer runs has no pane, and so no
    // agent: the ones it named have ended.
    *daemon.mux.telling.lock().unwrap() = Telling::NoServer;
    until(&daemon.socket, |registry| {
        state_of(registry, &again) == Some(AgentState::Stopped)
            && state_of(registry, &gemini) == Some(AgentState::Stopped)
    })
    .await;
}
