//! What the daemon says it read of an agent's screen (ADR 0028).
//!
//! `agents.preview` gives terminal output and says what it is: the screen
//! itself or the replay of a session's output. Here, the screen of an agent
//! in a volee host, which comes with the size of the host's terminal: a
//! snapshot only redraws as it is in a terminal of that size. The other
//! kinds are checked where their agents are: a multiplexer's capture in
//! `panes.rs`, a replay in `control.rs`, a setup log in `launch.rs`.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::FakeHarness;
use tokio::net::UnixListener;
use ulid::Ulid;
use volee_core::harness::HarnessKind;
use volee_core::role::{LaunchedBy, Role};
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};
use volee_core::{AgentId, Event, EventKind};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_harness::Harness;
use volee_host::{HostConfig, HostDir, Size};
use volee_proto::client::Client;
use volee_proto::methods::{AgentParams, Preview, ScreenSize};
use volee_proto::paths::Paths;

/// Taller than the 50 rows a reader gives a screen whose size it has to
/// tell from what the screen writes.
const SIZE: Size = Size {
    cols: 120,
    rows: 60,
};

#[tokio::test]
async fn the_screen_of_an_agent_in_a_host_comes_with_the_size_of_its_terminal() {
    // A short path: the host's socket lives under it.
    let dir = tempfile::Builder::new()
        .prefix("vo")
        .tempdir_in("/tmp")
        .unwrap();
    let paths = Paths::rooted(dir.path());
    let hosts = HostDir::new(paths.state_dir.join("hosts"));
    std::fs::create_dir_all(paths.state_dir.join("hosts")).unwrap();

    // The agent's host, run by this test: its program writes on the first
    // and on the last row of its terminal, then waits.
    let program = "printf '\\033[1;1Hfirst row\\033[60;1Hlast row\\033[30;1H'; exec cat";
    let host = std::thread::spawn({
        let config = HostConfig {
            id: "h1".into(),
            dir: hosts.clone(),
            command: ["sh", "-c", program].map(str::to_owned).to_vec(),
            cwd: dir.path().to_path_buf(),
            env: Vec::new(),
            env_remove: Vec::new(),
            size: SIZE,
            profile: "generic".into(),
            name: Some("invoices".into()),
            scrollback: 100,
            linger: Duration::from_millis(100),
            idle_exit: None,
        };
        move || volee_host::run(config)
    });
    let socket = hosts.socket("h1");
    let drawn = || {
        volee_host::snapshot(&socket, "test", Duration::from_secs(3))
            .is_ok_and(|(_, screen)| String::from_utf8_lossy(&screen).contains("last row"))
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !drawn() {
        assert!(Instant::now() < deadline, "the host never drew its screen");
        std::thread::sleep(Duration::from_millis(20));
    }

    let agent = AgentId::from_ulid(Ulid::from_parts(1_000, 1));
    let registered = Event::new(
        Timestamp::from_millis(1),
        EventKind::AgentRegistered {
            agent,
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
    let daemon_socket = paths.socket.clone();
    let listener = UnixListener::bind(&daemon_socket).unwrap();
    let harness = Arc::new(FakeHarness::default());
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(vec![registered])),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: Some(harness as Arc<dyn Harness>),
        discovery_interval: Duration::from_millis(10),
        max_discovery_interval: Duration::from_millis(10),
        idle_discovery_interval: Duration::from_millis(10),
        evidence: EvidenceSources::default(),
        volee_bin: None,
        multiplexers: Vec::new(),
        hosts: true,
    };
    tokio::spawn(Daemon::serve(listener, config));

    let client = Client::connect(&daemon_socket).await.unwrap();
    let preview = client.call::<Preview>(AgentParams { agent }).await.unwrap();
    assert!(
        preview.exact,
        "a host's snapshot is the agent's screen itself"
    );
    assert_eq!(
        preview.size,
        Some(ScreenSize {
            cols: 120,
            rows: 60
        })
    );
    assert!(
        preview.screen.contains("first row") && preview.screen.contains("last row"),
        "{:?}",
        preview.screen
    );

    volee_host::stop(&socket, "test").unwrap();
    host.join().unwrap().unwrap();
}
