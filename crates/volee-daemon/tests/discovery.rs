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
use volee_proto::methods::{
    EventsSince, RefreshDiscovery, RefreshParams, SinceParams, Subscribe, SubscribeParams,
};
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
/// subscribed (changes or not), and every `idle` otherwise.
fn start_with(watched: Duration, idle: Duration) -> Running {
    start_cadence(watched, watched, idle)
}

/// A daemon whose watched wait is `shortest` after a change and doubles up
/// to `longest` while nothing changes; nobody watching, it waits an hour.
fn start_backing_off(shortest: Duration, longest: Duration) -> Running {
    start_cadence(shortest, longest, HOUR)
}

fn start_cadence(watched: Duration, longest: Duration, idle: Duration) -> Running {
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
        max_discovery_interval: longest,
        idle_discovery_interval: idle,
        evidence: EvidenceSources::default(),
        volee_bin: None,
        multiplexers: Vec::new(),
        hosts: false,
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

/// Asks the daemon for a listing now, as `volee ls` does.
async fn refresh(socket: &Path, timeout_ms: Option<u64>) -> bool {
    let client = Client::connect(socket).await.unwrap();
    client
        .call::<RefreshDiscovery>(RefreshParams { timeout_ms })
        .await
        .unwrap()
        .listed
}

#[tokio::test]
async fn a_refresh_lists_at_once_and_answers_once_the_listing_is_applied() {
    // Both waits last an hour: only the request can cause a listing.
    let daemon = start_with(HOUR, HOUR);
    eventually("the daemon lists once when it starts", || {
        daemon.harness.listings() == 1
    })
    .await;
    daemon.harness.set(vec![session(
        "eeee5555-0000",
        "hotfix",
        AgentState::Working,
        true,
    )]);
    assert!(refresh(&daemon.socket, None).await);
    // Read once, right after the answer: the session is already an agent.
    let registry = registry(&daemon.socket).await;
    assert_eq!(registry.agents().next().unwrap().name, "hotfix");
    assert_eq!(daemon.harness.listings(), 2);
}

#[tokio::test]
async fn concurrent_refreshes_share_a_listing() {
    let daemon = start_with(HOUR, HOUR);
    eventually("the daemon lists once when it starts", || {
        daemon.harness.listings() == 1
    })
    .await;
    // Slow listings: every request arrives while the first one runs.
    daemon.harness.slow_listings(Duration::from_millis(300));
    let requests: Vec<_> = (0..5)
        .map(|_| {
            let socket = daemon.socket.clone();
            tokio::spawn(async move { refresh(&socket, None).await })
        })
        .collect();
    for request in requests {
        assert!(request.await.unwrap());
    }
    // The listing at start, the one the requests caused, and at most one
    // more for requests that came in once it had started.
    assert!(
        daemon.harness.listings() <= 3,
        "{}",
        daemon.harness.listings()
    );
}

#[tokio::test]
async fn a_refresh_gives_up_after_its_timeout_and_the_listing_goes_on() {
    let daemon = start_with(HOUR, HOUR);
    eventually("the daemon lists once when it starts", || {
        daemon.harness.listings() == 1
    })
    .await;
    daemon.harness.slow_listings(Duration::from_millis(500));
    let asked = tokio::time::Instant::now();
    assert!(!refresh(&daemon.socket, Some(50)).await);
    assert!(asked.elapsed() < Duration::from_millis(450));
    eventually("the listing completes anyway", || {
        daemon.harness.listings() == 2
    })
    .await;
}

#[tokio::test]
async fn a_failed_listing_is_not_a_fresh_one() {
    let daemon = start_with(HOUR, HOUR);
    eventually("the daemon lists once when it starts", || {
        daemon.harness.listings() == 1
    })
    .await;
    daemon.harness.fail(true);
    assert!(!refresh(&daemon.socket, None).await);
    assert_eq!(daemon.harness.listings(), 2, "it was tried");
}

#[tokio::test]
async fn without_a_harness_a_refresh_answers_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(vec![])),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: None,
        discovery_interval: HOUR,
        max_discovery_interval: HOUR,
        idle_discovery_interval: HOUR,
        evidence: EvidenceSources::default(),
        volee_bin: None,
        multiplexers: Vec::new(),
        hosts: false,
    };
    tokio::spawn(Daemon::serve(listener, config));
    let asked = tokio::time::Instant::now();
    assert!(!refresh(&socket, Some(5_000)).await);
    assert!(asked.elapsed() < Duration::from_secs(4), "no waiting");
}

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

/// How many listings happen during `window`, from now.
async fn listings_during(harness: &FakeHarness, window: Duration) -> usize {
    let before = harness.listings();
    tokio::time::sleep(window).await;
    harness.listings() - before
}

#[tokio::test]
async fn while_nothing_changes_the_watched_wait_grows_to_its_cap() {
    // 10 ms after a change, doubling up to 200 ms while nothing changes.
    let daemon = start_backing_off(ms(10), ms(200));
    eventually("the daemon lists once when it starts", || {
        daemon.harness.listings() == 1
    })
    .await;
    let _interface = subscribe(&daemon.socket, true).await;
    // At a fixed 10 ms, a second and a half holds about 150 listings;
    // backing off (10, 20, 40, 80, 160, then 200 ms) holds about a dozen.
    let early = listings_during(&daemon.harness, ms(1_500)).await;
    assert!(early < 40, "{early} listings in 1.5 s");
    // Capped, not doubling for ever: at 200 ms, two seconds hold about ten
    // listings; uncapped (320, 640, 1,280 ms...), one at most.
    let capped = listings_during(&daemon.harness, ms(2_000)).await;
    assert!(capped >= 4, "{capped} listings in 2 s");
}

#[tokio::test]
async fn a_change_brings_the_short_wait_back() {
    let daemon = start_backing_off(ms(10), ms(1_000));
    eventually("the daemon lists once when it starts", || {
        daemon.harness.listings() == 1
    })
    .await;
    let _interface = subscribe(&daemon.socket, true).await;
    // Nothing changes for a second and a half: the wait reaches a second.
    tokio::time::sleep(ms(1_500)).await;
    daemon.harness.set(vec![session(
        "ffff6666-0000",
        "rebase",
        AgentState::Working,
        true,
    )]);
    until(&daemon.socket, |r| r.agents().count() == 1).await;
    // After the listing that found it: 10, 20, 40 ms again, where a backed
    // off discovery would wait a second.
    let after = listings_during(&daemon.harness, ms(500)).await;
    assert!(after >= 3, "{after} listings in 0.5 s after a change");
}

#[tokio::test]
async fn a_new_watcher_and_a_refresh_bring_the_short_wait_back() {
    let daemon = start_backing_off(ms(10), ms(1_000));
    eventually("the daemon lists once when it starts", || {
        daemon.harness.listings() == 1
    })
    .await;
    let _interface = subscribe(&daemon.socket, true).await;
    tokio::time::sleep(ms(1_500)).await;

    // A second interface opens: a listing at once, then the short wait.
    let _second = subscribe(&daemon.socket, true).await;
    let after = listings_during(&daemon.harness, ms(500)).await;
    assert!(after >= 3, "{after} listings in 0.5 s after a new watcher");

    // Backed off again, then `volee ls` asks for a listing.
    tokio::time::sleep(ms(1_500)).await;
    assert!(refresh(&daemon.socket, None).await);
    let after = listings_during(&daemon.harness, ms(500)).await;
    assert!(after >= 3, "{after} listings in 0.5 s after a refresh");
}
