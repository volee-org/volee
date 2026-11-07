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
use volee_proto::methods::{EventsSince, SinceParams, Subscribe, SubscribeParams};
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

/// A wait no test outlasts: a listing after it never happens in a test.
const HOUR: Duration = Duration::from_secs(3_600);

/// A daemon that lists every 10 ms, watched or not.
fn start() -> Running {
    start_with(Duration::from_millis(10), Duration::from_millis(10))
}

/// A daemon that lists every `watched` while an interactive client is
/// subscribed, and every `idle` otherwise.
fn start_with(watched: Duration, idle: Duration) -> Running {
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
        discovery_interval: watched,
        idle_discovery_interval: idle,
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

/// Waits until `ready` holds, for at most ten seconds.
async fn eventually(what: &str, ready: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Subscribes to the daemon's events, as the interface does
/// (`interactive`) or as an agent's MCP server does. The subscription lasts
/// as long as the returned client.
async fn subscribe(socket: &Path, interactive: bool) -> Client {
    let client = Client::connect(socket).await.unwrap();
    client
        .call::<Subscribe>(SubscribeParams {
            after: None,
            interactive,
        })
        .await
        .unwrap();
    client
}

/// Waits until discovery stops listing (no listing for 200 ms), for at most
/// ten seconds.
async fn quiet(harness: &FakeHarness) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let before = harness.listings();
        tokio::time::sleep(Duration::from_millis(200)).await;
        if harness.listings() == before {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "discovery kept listing at the watched cadence"
        );
    }
}

#[tokio::test]
async fn without_an_interactive_client_discovery_waits_the_idle_interval() {
    let daemon = start_with(Duration::from_millis(10), HOUR);
    eventually("the daemon lists once when it starts", || {
        daemon.harness.listings() == 1
    })
    .await;
    // An agent's MCP server subscribes too, but no person watches.
    let _mcp = subscribe(&daemon.socket, false).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        daemon.harness.listings(),
        1,
        "nothing more before the idle wait ends"
    );
}

#[tokio::test]
async fn an_interactive_client_gets_a_listing_at_once() {
    // Both waits last an hour: only the new subscriber can cause a listing.
    let daemon = start_with(HOUR, HOUR);
    eventually("the daemon lists once when it starts", || {
        daemon.harness.listings() == 1
    })
    .await;
    daemon.harness.set(vec![session(
        "dddd4444-0000",
        "release",
        AgentState::Working,
        true,
    )]);
    let _interface = subscribe(&daemon.socket, true).await;
    let registry = until(&daemon.socket, |r| r.agents().count() == 1).await;
    assert_eq!(registry.agents().next().unwrap().name, "release");
    assert_eq!(daemon.harness.listings(), 2, "one listing, at once");
}

#[tokio::test]
async fn discovery_is_fast_while_someone_watches_and_idle_once_they_leave() {
    let daemon = start_with(Duration::from_millis(10), HOUR);
    eventually("the daemon lists once when it starts", || {
        daemon.harness.listings() == 1
    })
    .await;
    let interface = subscribe(&daemon.socket, true).await;
    eventually(
        "discovery lists every 10 ms while the interface is open",
        || daemon.harness.listings() >= 6,
    )
    .await;
    drop(interface);
    quiet(&daemon.harness).await;

    // Someone opens the interface again: the fast cadence comes back.
    let watched_from = daemon.harness.listings();
    let _again = subscribe(&daemon.socket, true).await;
    eventually("discovery lists often again", || {
        daemon.harness.listings() >= watched_from + 5
    })
    .await;
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
