//! An agent in a volee host heeds hooks only from its host's program and the
//! program's descendants (ADR 0026): another session that inherited its
//! hooks, such as a background fork Claude Code started, is not the agent.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

use std::os::unix::net::UnixListener as StdListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::net::UnixListener;
use ulid::Ulid;
use volee_core::harness::{HarnessKind, SessionKey};
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::AgentState;
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};
use volee_core::{Agent, AgentId, Event, EventKind, Registry};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_host::{HostRecord, Size};
use volee_proto::client::Client;
use volee_proto::methods::{
    EventsSince, HookIngest, HookParams, HookWait, SendMessage, SendParams, SinceParams, WaitParams,
};
use volee_proto::paths::Paths;

fn agent_id() -> AgentId {
    AgentId::from_ulid(Ulid::from_parts(1_000, 1))
}

/// This test process plays the host's program.
fn program() -> u32 {
    std::process::id()
}

/// Launchd: never a descendant of this test.
const ELSEWHERE: u32 = 1;

struct Running {
    socket: PathBuf,
    _host: StdListener,
    _dir: tempfile::TempDir,
}

/// A daemon with one agent in a running volee host `h1`, whose program is
/// this test process.
fn start() -> Running {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    let hosts = paths.state_dir.join("hosts");
    std::fs::create_dir_all(&hosts).unwrap();
    let host = StdListener::bind(hosts.join("h1.sock")).unwrap();
    HostRecord {
        id: "h1".into(),
        protocol: volee_host::proto::PROTOCOL,
        host_pid: program(),
        child_pid: Some(program()),
        profile: "claude-code".into(),
        name: Some("invoices".into()),
        command: vec!["claude".into()],
        cwd: "/work/alpha".into(),
        started_at: 1,
        size: Size { cols: 80, rows: 24 },
        exit: None,
    }
    .write(&hosts.join("h1.json"))
    .unwrap();
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let registered = Event::new(
        Timestamp::from_millis(1),
        EventKind::AgentRegistered {
            agent: agent_id(),
            name: "invoices".into(),
            role: Role::Worker,
            harness: HarnessKind::ClaudeCode,
            launched_by: LaunchedBy::Human,
            parent: None,
            mission: None,
            workspace: Workspace::at("/work/alpha"),
            location: Location::Host { id: "h1".into() },
        },
    );
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(vec![registered])),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: None,
        discovery_interval: volee_daemon::DEFAULT_DISCOVERY_INTERVAL,
        max_discovery_interval: volee_daemon::DEFAULT_DISCOVERY_INTERVAL,
        idle_discovery_interval: volee_daemon::DEFAULT_DISCOVERY_INTERVAL,
        evidence: EvidenceSources::default(),
        volee_bin: None,
        multiplexers: Vec::new(),
        hosts: true,
    };
    tokio::spawn(Daemon::serve(listener, config));
    Running {
        socket,
        _host: host,
        _dir: dir,
    }
}

async fn hook(daemon: &Running, event: &str, payload: Value, pid: u32) -> Option<String> {
    let client = Client::connect(&daemon.socket).await.unwrap();
    client
        .call::<HookIngest>(HookParams {
            agent: agent_id(),
            event: event.into(),
            payload,
            pid: Some(pid),
        })
        .await
        .unwrap()
        .stdout
}

async fn agent(daemon: &Running) -> Agent {
    let client = Client::connect(&daemon.socket).await.unwrap();
    let batch = client
        .call::<EventsSince>(SinceParams::default())
        .await
        .unwrap();
    let events: Vec<Event> = batch.events.into_iter().map(|e| e.event).collect();
    Registry::from_events(&events)
        .unwrap()
        .agent(agent_id())
        .unwrap()
        .clone()
}

fn session_start(session: &str, source: &str) -> Value {
    json!({
        "hook_event_name": "SessionStart",
        "session_id": session,
        "source": source,
    })
}

#[tokio::test]
async fn a_fork_with_the_agent_s_hooks_does_not_take_its_place() {
    let daemon = start();
    hook(
        &daemon,
        "session-start",
        session_start("own", "startup"),
        program(),
    )
    .await;
    let own = agent(&daemon).await;
    assert!(own.owns_session(&SessionKey::claude("own")));

    // Claude Code's agents view forked a background session with the same
    // settings: its hooks name the hosted agent.
    hook(
        &daemon,
        "session-start",
        session_start("fork", "fork"),
        ELSEWHERE,
    )
    .await;
    hook(
        &daemon,
        "stop",
        json!({ "hook_event_name": "Stop", "last_assistant_message": "Done." }),
        ELSEWHERE,
    )
    .await;
    let after = agent(&daemon).await;
    assert!(!after.owns_session(&SessionKey::claude("fork")));
    assert_eq!(
        after.current_session().map(|link| link.key.clone()),
        Some(SessionKey::claude("own"))
    );
    assert_eq!(
        after.state,
        AgentState::Idle,
        "the fork's stop changed nothing"
    );
    assert_eq!(after.location, Location::Host { id: "h1".into() });

    // A hook run through a shell the program started still counts.
    let mut shell = std::process::Command::new("sh")
        .args(["-c", "sleep 30"])
        .spawn()
        .unwrap();
    hook(
        &daemon,
        "stop",
        json!({ "hook_event_name": "Stop", "last_assistant_message": "Done." }),
        shell.id(),
    )
    .await;
    let _ = shell.kill();
    let _ = shell.wait();
    assert_eq!(agent(&daemon).await.state, AgentState::Done);
}

#[tokio::test]
async fn a_fork_does_not_take_the_agent_s_messages() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    client
        .call::<SendMessage>(SendParams {
            from: volee_core::message::Party::Human,
            to: vec![volee_core::message::Party::Agent(agent_id())],
            kind: volee_core::message::MessageKind::Note,
            body: "keep going".into(),
            refs: vec![],
            in_reply_to: None,
            thread: None,
        })
        .await
        .unwrap();
    let started = Instant::now();
    let reply = client
        .call::<HookWait>(WaitParams {
            agent: agent_id(),
            timeout_ms: 5_000,
            pid: Some(ELSEWHERE),
        })
        .await
        .unwrap();
    assert_eq!(reply.text, None);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "it did not wait"
    );
    let reply = client
        .call::<HookWait>(WaitParams {
            agent: agent_id(),
            timeout_ms: 5_000,
            pid: Some(program()),
        })
        .await
        .unwrap();
    assert!(reply.text.is_some_and(|text| text.contains("keep going")));
}
