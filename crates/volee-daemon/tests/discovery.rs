//! Discovery end to end: a fake harness lists sessions, and a real daemon
//! turns them into agents and follows their state.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::FakeHarness;
use tokio::net::UnixListener;
use volee_core::harness::SessionKey;
use volee_core::reconcile::ListedSession;
use volee_core::state::AgentState;
use volee_core::{Event, Registry};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_harness::Harness;
use volee_proto::client::Client;
use volee_proto::methods::{EventsSince, SinceParams};
use volee_proto::paths::Paths;

fn session(id: &str, name: &str, state: AgentState, alive: bool) -> ListedSession {
    let mut session = ListedSession::new(SessionKey::claude(id), "/work/alpha", state);
    session.name = Some(name.to_owned());
    session.alive = alive;
    session.background = true;
    session.handle = Some(id[..8].to_owned());
    session
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
        evidence: EvidenceSources::default(),
        volee_bin: None,
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

/// Waits until the registry satisfies `ready`, for at most ten seconds.
async fn until(socket: &Path, ready: impl Fn(&Registry) -> bool) -> Registry {
    for _ in 0..1_000 {
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
