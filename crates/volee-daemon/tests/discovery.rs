//! Discovery end to end: a fake harness lists sessions, and a real daemon
//! turns them into agents and follows their state.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::net::UnixListener;
use volee_core::harness::{HarnessKind, SessionKey};
use volee_core::reconcile::{ListedSession, Listing};
use volee_core::state::AgentState;
use volee_core::{AgentId, Event, Registry};
use volee_daemon::{Config, Daemon, ManualClock, MemoryLog};
use volee_harness::{Capabilities, Harness, HarnessError, SpawnSpec, Spawned, Version};
use volee_proto::client::Client;
use volee_proto::methods::{EventsSince, SinceParams};
use volee_proto::paths::Paths;

/// A harness whose listing the test sets. Listing fails while `failing`.
#[derive(Debug, Default)]
struct FakeHarness {
    sessions: Mutex<Vec<ListedSession>>,
    failing: Mutex<bool>,
}

impl FakeHarness {
    fn set(&self, sessions: Vec<ListedSession>) {
        *self.sessions.lock().unwrap() = sessions;
    }

    fn fail(&self, failing: bool) {
        *self.failing.lock().unwrap() = failing;
    }
}

fn unsupported(what: &str) -> HarnessError {
    HarnessError::CommandFailed {
        command: what.to_owned(),
        code: None,
        message: "not supported by the fake harness".to_owned(),
    }
}

impl Harness for FakeHarness {
    fn kind(&self) -> HarnessKind {
        HarnessKind::ClaudeCode
    }

    fn capabilities(&self) -> Result<Capabilities, HarnessError> {
        Ok(Capabilities {
            version: Version::new(2, 1, 319),
            list_sessions: true,
            background_sessions: true,
        })
    }

    fn list(&self) -> Result<Listing, HarnessError> {
        if *self.failing.lock().unwrap() {
            return Err(unsupported("agents"));
        }
        Ok(Listing {
            harness: HarnessKind::ClaudeCode,
            sessions: self.sessions.lock().unwrap().clone(),
            complete: true,
        })
    }

    fn spawn(&self, _: &SpawnSpec) -> Result<Spawned, HarnessError> {
        Err(unsupported("spawn"))
    }

    fn resume(&self, _: &SessionKey, _: &SpawnSpec) -> Result<Spawned, HarnessError> {
        Err(unsupported("resume"))
    }

    fn stop(&self, _: &SessionKey) -> Result<(), HarnessError> {
        Err(unsupported("stop"))
    }

    fn remove(&self, _: &SessionKey) -> Result<(), HarnessError> {
        Err(unsupported("remove"))
    }

    fn logs(&self, _: &SessionKey) -> Result<Vec<u8>, HarnessError> {
        Err(unsupported("logs"))
    }

    fn attach_command(&self, _: &SessionKey) -> Result<Command, HarnessError> {
        Err(unsupported("attach"))
    }

    fn agent_settings(&self, _: &Path, _: AgentId, _: Duration) -> Result<Value, HarnessError> {
        Err(unsupported("settings"))
    }

    fn mcp_config(&self, _: &Path, _: AgentId) -> Result<Value, HarnessError> {
        Err(unsupported("mcp"))
    }
}

fn session(id: &str, name: &str, state: AgentState, alive: bool) -> ListedSession {
    ListedSession {
        key: SessionKey::claude(id),
        name: Some(name.to_owned()),
        cwd: PathBuf::from("/work/alpha"),
        state,
        alive,
        background: true,
    }
}

struct Running {
    socket: PathBuf,
    harness: Arc<FakeHarness>,
    _dir: tempfile::TempDir,
}

fn start() -> Running {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let harness = Arc::new(FakeHarness::default());
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(vec![])),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: Some(Arc::clone(&harness) as Arc<dyn Harness>),
        discovery_interval: Duration::from_millis(10),
    };
    tokio::spawn(Daemon::serve(listener, config));
    Running {
        socket,
        harness,
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

/// Waits until the registry satisfies `ready`, for at most two seconds.
async fn until(socket: &Path, ready: impl Fn(&Registry) -> bool) -> Registry {
    for _ in 0..200 {
        let registry = registry(socket).await;
        if ready(&registry) {
            return registry;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the registry never reached the expected state");
}

#[tokio::test]
async fn listed_sessions_become_agents_that_follow_their_state() {
    let daemon = start();
    daemon.harness.set(vec![
        session("aaaa1111-0000", "invoices", AgentState::Working, true),
        session("bbbb2222-0000", "docs", AgentState::Done, false),
    ]);
    let registry = until(&daemon.socket, |r| r.agents().count() == 1).await;
    // A finished session nobody owns is history, not an agent.
    assert!(registry.agents().all(|agent| agent.name != "docs"));
    let invoices = registry
        .agents()
        .find(|agent| agent.name == "invoices")
        .unwrap();
    assert_eq!(invoices.state, AgentState::Working);
    assert_eq!(
        registry
            .agent_for_session(&SessionKey::claude("aaaa1111-0000"))
            .map(|agent| agent.id),
        Some(invoices.id)
    );

    // The session finishes its work: the agent follows.
    daemon.harness.set(vec![
        session("aaaa1111-0000", "invoices", AgentState::Idle, true),
        session("bbbb2222-0000", "docs", AgentState::Done, false),
    ]);
    let registry = until(&daemon.socket, |r| {
        r.agents()
            .any(|agent| agent.name == "invoices" && agent.state == AgentState::Idle)
    })
    .await;
    assert_eq!(registry.agents().count(), 1, "no duplicate agents");
}

#[tokio::test]
async fn a_failing_listing_changes_nothing_and_discovery_resumes() {
    let daemon = start();
    daemon.harness.fail(true);
    daemon.harness.set(vec![session(
        "cccc3333-0000",
        "audit",
        AgentState::Working,
        true,
    )]);
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(registry(&daemon.socket).await.agents().count(), 0);

    daemon.harness.fail(false);
    let registry = until(&daemon.socket, |r| r.agents().count() == 1).await;
    assert_eq!(registry.agents().next().unwrap().name, "audit");
}

#[tokio::test]
async fn a_session_that_disappears_is_not_marked_done() {
    let daemon = start();
    daemon.harness.set(vec![session(
        "dddd4444-0000",
        "refactor",
        AgentState::Working,
        true,
    )]);
    until(&daemon.socket, |r| r.agents().count() == 1).await;

    // A complete listing without the session: it ended without a word.
    daemon.harness.set(vec![]);
    let registry = until(&daemon.socket, |r| {
        r.agents().all(|agent| agent.state != AgentState::Working)
    })
    .await;
    let agent = registry.agents().next().unwrap();
    assert_ne!(agent.state, AgentState::Done, "gone is not done");
}
