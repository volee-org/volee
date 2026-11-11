//! Hooks of an agent in a volee host from other processes (ADR 0026).
//!
//! Claude Code's agents view, opened inside a hosted agent, moves the
//! conversation to a background process that the host's program then shows;
//! its hooks name the hosted agent. That session continues the agent's
//! conversation, and the agent stays in its host while the host runs. The
//! hooks of any other session from another process are not the agent's.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::os::unix::net::UnixListener as StdListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::FakeHarness;
use serde_json::{Value, json};
use tokio::net::UnixListener;
use ulid::Ulid;
use volee_core::harness::{HarnessKind, SessionKey};
use volee_core::reconcile::ListedSession;
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::AgentState;
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};
use volee_core::{Agent, AgentId, Event, EventKind, Registry};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_harness::Harness;
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

/// Launchd: never a descendant of this test, like Claude Code's background
/// supervisor.
const ELSEWHERE: u32 = 1;

struct Running {
    socket: PathBuf,
    harness: Arc<FakeHarness>,
    host: Option<StdListener>,
    _dir: tempfile::TempDir,
}

/// A daemon with one agent in a running volee host `h1`, whose program is
/// this test process; it lists sessions every 10 ms.
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
    let harness = Arc::new(FakeHarness::default());
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(vec![registered])),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: Some(Arc::clone(&harness) as Arc<dyn Harness>),
        discovery_interval: Duration::from_millis(10),
        max_discovery_interval: Duration::from_millis(10),
        idle_discovery_interval: Duration::from_millis(10),
        evidence: EvidenceSources::default(),
        volee_bin: None,
        multiplexers: Vec::new(),
        hosts: true,
        processes: None,
    };
    tokio::spawn(Daemon::serve(listener, config));
    Running {
        socket,
        harness,
        host: Some(host),
        _dir: dir,
    }
}

async fn hook(daemon: &Running, event: &str, payload: Value, pid: u32) {
    let client = Client::connect(&daemon.socket).await.unwrap();
    client
        .call::<HookIngest>(HookParams {
            agent: agent_id(),
            event: event.into(),
            payload,
            pid: Some(pid),
        })
        .await
        .unwrap();
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
    json!({ "hook_event_name": "SessionStart", "session_id": session, "source": source })
}

fn stop(session: &str) -> Value {
    json!({ "hook_event_name": "Stop", "session_id": session, "last_assistant_message": "Done." })
}

fn tool(session: &str) -> Value {
    json!({
        "hook_event_name": "PreToolUse",
        "session_id": session,
        "tool_name": "Bash",
        "tool_input": { "command": "cargo test" },
    })
}

fn listed(id: &str, state: AgentState) -> ListedSession {
    let mut session = ListedSession::new(SessionKey::claude(id), "/work/alpha", state);
    session.alive = true;
    session.background = true;
    session.handle = Some(id.to_owned());
    session
}

fn current(agent: &Agent) -> Option<SessionKey> {
    agent.current_session().map(|link| link.key.clone())
}

#[tokio::test]
async fn a_conversation_moved_to_the_background_stays_the_hosted_agent_s() {
    let mut daemon = start();
    hook(
        &daemon,
        "session-start",
        session_start("own", "startup"),
        program(),
    )
    .await;
    assert_eq!(
        current(&agent(&daemon).await),
        Some(SessionKey::claude("own"))
    );

    // `←` in the hosted Claude: the conversation goes on in a background
    // process, which the host's program shows.
    hook(
        &daemon,
        "session-start",
        session_start("moved", "fork"),
        ELSEWHERE,
    )
    .await;
    daemon
        .harness
        .set(vec![listed("moved", AgentState::Working)]);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let moved = agent(&daemon).await;
    assert_eq!(current(&moved), Some(SessionKey::claude("moved")));
    assert_eq!(
        moved.location,
        Location::Host { id: "h1".into() },
        "it stays in its host"
    );

    // Its hooks are the agent's.
    hook(&daemon, "stop", stop("moved"), ELSEWHERE).await;
    assert_eq!(agent(&daemon).await.state, AgentState::Done);

    // Another session from another process is not the agent: an older one
    // the agents view revived without starting a session.
    hook(&daemon, "pre-tool-use", tool("older"), ELSEWHERE).await;
    hook(
        &daemon,
        "user-prompt-submit",
        json!({ "hook_event_name": "UserPromptSubmit", "session_id": "older" }),
        ELSEWHERE,
    )
    .await;
    let after = agent(&daemon).await;
    assert_eq!(
        after.state,
        AgentState::Done,
        "the other session changed nothing"
    );
    assert_eq!(after.last_activity, None, "nor its activity");

    // Once the host is gone, the conversation that goes on in the background
    // is where the agent is.
    drop(daemon.host.take());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if agent(&daemon).await.location == Location::Background {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the agent never followed its conversation"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn hooks_through_a_shell_of_the_program_are_the_agent_s() {
    let daemon = start();
    hook(
        &daemon,
        "session-start",
        session_start("own", "startup"),
        program(),
    )
    .await;
    let mut shell = std::process::Command::new("sh")
        .args(["-c", "sleep 30"])
        .spawn()
        .unwrap();
    // Even for a session the agent does not know yet, as after `/clear`.
    hook(&daemon, "pre-tool-use", tool("cleared"), shell.id()).await;
    hook(&daemon, "stop", stop("cleared"), shell.id()).await;
    let _ = shell.kill();
    let _ = shell.wait();
    assert_eq!(agent(&daemon).await.state, AgentState::Done);
}

#[tokio::test]
async fn only_the_agent_s_conversation_takes_its_messages() {
    let daemon = start();
    hook(
        &daemon,
        "session-start",
        session_start("own", "startup"),
        program(),
    )
    .await;
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
    let wait = |session: Option<&str>, pid: u32| WaitParams {
        agent: agent_id(),
        timeout_ms: 5_000,
        pid: Some(pid),
        session: session.map(str::to_owned),
    };
    for session in [None, Some("older")] {
        let started = Instant::now();
        let reply = client
            .call::<HookWait>(wait(session, ELSEWHERE))
            .await
            .unwrap();
        assert_eq!(reply.text, None, "{session:?}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "it did not wait"
        );
    }
    let reply = client
        .call::<HookWait>(wait(Some("own"), ELSEWHERE))
        .await
        .unwrap();
    assert!(reply.text.is_some_and(|text| text.contains("keep going")));
}
