//! Acting on agents end to end: previewing, diffing, taking over and stopping
//! them, through a real daemon, real git worktrees and a fake harness that
//! records the handle each action received.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::{FakeHarness, git, repository};
use tokio::net::UnixListener;
use ulid::Ulid;
use volee_core::harness::SessionKey;
use volee_core::reconcile::ListedSession;
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::AgentState;
use volee_core::{AgentId, Event, Registry};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_git::GitCli;
use volee_harness::Harness;
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{
    AgentParams, Attach, AttachCommand, Diff, DiffParams, EventsSince, Preview, SinceParams, Spawn,
    SpawnParams, Spawned, Stop,
};
use volee_proto::paths::Paths;
use volee_proto::rpc;

struct Running {
    socket: PathBuf,
    harness: Arc<FakeHarness>,
    _dir: tempfile::TempDir,
}

/// A daemon with a repository named `alpha`, git, and a fake harness listed
/// every `every`.
fn start(every: Duration) -> Running {
    start_with(every, true)
}

fn start_with(every: Duration, tools: bool) -> Running {
    let dir = tempfile::tempdir().unwrap();
    // Canonical, so that paths compare equal to what git reports.
    let root = dir.path().canonicalize().unwrap();
    let repo = root.join("alpha");
    repository(&repo);
    let paths = Paths::rooted(root.join("volee"));
    std::fs::create_dir_all(&paths.state_dir).unwrap();
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let harness = Arc::new(FakeHarness::default());
    let settings = volee_config::Config::parse(
        &format!("[repos.alpha]\npath = \"{}\"\n", repo.display()),
        Path::new("config.toml"),
        Path::new("/home/nobody"),
    )
    .unwrap();
    let git = GitCli::new()
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(vec![])),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: tools.then(|| Arc::clone(&harness) as Arc<dyn Harness>),
        discovery_interval: every,
        max_discovery_interval: every,
        idle_discovery_interval: every,
        evidence: EvidenceSources {
            git: tools.then(|| Arc::new(git) as Arc<dyn volee_git::Git>),
            settings,
            ..EvidenceSources::default()
        },
        volee_bin: Some(PathBuf::from("/opt/volee/bin/volee")),
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

/// Listing is not under test: list rarely.
const RARELY: Duration = Duration::from_secs(3_600);

/// Listing is under test: list often.
const OFTEN: Duration = Duration::from_millis(10);

async fn spawn_worker(client: &Client, branch: &str) -> Spawned {
    client
        .call::<Spawn>(SpawnParams {
            repo: "alpha".into(),
            branch: branch.into(),
            base: None,
            task: "Add invoices to the billing module.".into(),
            role: Role::Worker,
            parent: None,
            mission: None,
            model: None,
            name: None,
            harness: None,
        })
        .await
        .unwrap()
}

async fn registry(client: &Client) -> Registry {
    let batch = client
        .call::<EventsSince>(SinceParams::default())
        .await
        .unwrap();
    let events: Vec<Event> = batch.events.into_iter().map(|e| e.event).collect();
    Registry::from_events(&events).unwrap()
}

/// Waits until the registry satisfies `ready`, for at most two seconds.
async fn until(client: &Client, ready: impl Fn(&Registry) -> bool) -> Registry {
    for _ in 0..200 {
        let registry = registry(client).await;
        if ready(&registry) {
            return registry;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the registry never reached the expected state");
}

fn rpc_error<T: std::fmt::Debug>(result: Result<T, ClientError>) -> rpc::RpcError {
    match result {
        Err(ClientError::Rpc(error)) => error,
        other => panic!("expected an RPC error, got {other:?}"),
    }
}

/// A background session found running, as `claude agents` lists it.
fn background(id: &str, name: &str) -> ListedSession {
    ListedSession {
        name: Some(name.to_owned()),
        alive: true,
        background: true,
        handle: Some(id[..8].to_owned()),
        ..ListedSession::new(
            SessionKey::claude(id),
            "/work/elsewhere",
            AgentState::Working,
        )
    }
}

#[tokio::test]
async fn a_launched_agent_can_be_previewed_taken_over_and_stopped_at_once() {
    let daemon = start(RARELY);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let spawned = spawn_worker(&client, "feat/invoices").await;
    let agent = AgentParams {
        agent: spawned.agent,
    };

    let preview = client.call::<Preview>(agent).await.unwrap();
    assert!(
        preview.screen.contains("screen of abcd1234"),
        "{:?}",
        preview.screen
    );
    // What the harness gives is the replay of the session's output, not
    // its screen, and nothing tells the size of its terminal.
    assert!(!preview.exact);
    assert_eq!(preview.size, None);
    assert!(
        preview.screen.contains("\u{1b}[32m"),
        "kept raw for the TUI"
    );

    let attach = client.call::<Attach>(agent).await.unwrap();
    assert_eq!(
        attach,
        AttachCommand {
            program: "claude".into(),
            args: vec!["attach".into(), "abcd1234".into()],
            cwd: None,
            env_remove: vec!["CLAUDECODE".into()],
            hint: "Press Ctrl+Z to come back; the agent keeps running.".into(),
            focus: None,
        }
    );

    client.call::<Stop>(agent).await.unwrap();
    assert_eq!(
        daemon.harness.actions(),
        [
            ("logs".to_owned(), "abcd1234".to_owned()),
            ("attach".to_owned(), "abcd1234".to_owned()),
            ("stop".to_owned(), "abcd1234".to_owned()),
        ]
    );
}

#[tokio::test]
async fn a_launched_session_listed_before_its_first_hook_joins_its_agent() {
    let daemon = start(OFTEN);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let spawned = spawn_worker(&client, "feat/invoices").await;
    let key = SessionKey::claude("abcd1234-5b60-4cdc-8cf3-000000000001");
    daemon
        .harness
        .set(vec![background(&key.id, "alpha-feat-invoices")]);
    let registry = until(&client, |r| r.agent_for_session(&key).is_some()).await;
    assert_eq!(
        registry.agent_for_session(&key).map(|agent| agent.id),
        Some(spawned.agent),
        "the session is the launched agent's, not a new independent agent"
    );
    assert_eq!(registry.agents().count(), 1);
}

#[tokio::test]
async fn a_listed_session_is_acted_on_with_its_listed_id() {
    let daemon = start(OFTEN);
    let client = Client::connect(&daemon.socket).await.unwrap();
    daemon.harness.set(vec![background(
        "feed5678-0000-4000-8000-000000000002",
        "found running",
    )]);
    let registry = until(&client, |r| r.agents().count() == 1).await;
    let found = registry.agents().next().unwrap();
    assert_eq!(found.launched_by, LaunchedBy::Outside);
    let agent = AgentParams { agent: found.id };

    let preview = client.call::<Preview>(agent).await.unwrap();
    assert!(preview.screen.contains("screen of feed5678"));
    assert!(!preview.exact, "a replay");

    // volee stops only the agents it launched (ADR 0011).
    let refused = rpc_error(client.call::<Stop>(agent).await);
    assert_eq!(refused.code, rpc::REFUSED);
    assert!(
        refused.message.contains("not launched by volee"),
        "{}",
        refused.message
    );
    assert_eq!(
        daemon.harness.actions(),
        [("logs".to_owned(), "feed5678".to_owned())]
    );
}

#[tokio::test]
async fn an_interactive_session_is_used_from_its_own_terminal() {
    let daemon = start(OFTEN);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let interactive = ListedSession {
        name: Some("by hand".into()),
        alive: true,
        ..ListedSession::new(
            SessionKey::claude("0a1b2c3d-0000-4000-8000-000000000003"),
            "/work/elsewhere",
            AgentState::Working,
        )
    };
    daemon.harness.set(vec![interactive]);
    let registry = until(&client, |r| r.agents().count() == 1).await;
    let agent = AgentParams {
        agent: registry.agents().next().unwrap().id,
    };
    for refused in [
        rpc_error(client.call::<Preview>(agent).await),
        rpc_error(client.call::<Attach>(agent).await),
        rpc_error(client.call::<Stop>(agent).await),
    ] {
        assert_eq!(refused.code, rpc::REFUSED);
        assert!(
            refused.message.contains("switch to that terminal"),
            "{}",
            refused.message
        );
    }
    assert_eq!(daemon.harness.actions(), []);
}

#[tokio::test]
async fn a_session_the_harness_forgot_is_not_found() {
    let daemon = start(RARELY);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let spawned = spawn_worker(&client, "feat/invoices").await;
    daemon.harness.forget_sessions();
    let error = rpc_error(
        client
            .call::<Preview>(AgentParams {
                agent: spawned.agent,
            })
            .await,
    );
    assert_eq!(error.code, rpc::NOT_FOUND);
    assert!(
        error.message.contains("no longer knows"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn unknown_agents_are_not_found() {
    let daemon = start(RARELY);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let agent = AgentParams {
        agent: AgentId::from_ulid(Ulid::from_parts(9_999, 9)),
    };
    assert_eq!(
        rpc_error(client.call::<Preview>(agent).await).code,
        rpc::NOT_FOUND
    );
    let diff = DiffParams {
        agent: agent.agent,
        full: false,
    };
    assert_eq!(
        rpc_error(client.call::<Diff>(diff).await).code,
        rpc::NOT_FOUND
    );
}

#[tokio::test]
async fn without_the_tools_actions_are_unavailable() {
    let daemon = start_with(RARELY, false);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let agent = AgentParams {
        agent: AgentId::from_ulid(Ulid::from_parts(9_999, 9)),
    };
    for error in [
        rpc_error(client.call::<Preview>(agent).await),
        rpc_error(client.call::<Attach>(agent).await),
        rpc_error(client.call::<Stop>(agent).await),
        rpc_error(
            client
                .call::<Diff>(DiffParams {
                    agent: agent.agent,
                    full: true,
                })
                .await,
        ),
    ] {
        assert_eq!(error.code, rpc::UNAVAILABLE, "{}", error.message);
    }
}

#[tokio::test]
async fn the_diff_shows_the_work_against_the_base_branch() {
    let daemon = start(RARELY);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let spawned = spawn_worker(&client, "feat/invoices").await;
    let worktree = PathBuf::from(&spawned.worktree);
    std::fs::write(worktree.join("README.md"), "alpha\ninvoices\n").unwrap();
    git(&worktree, &["commit", "--quiet", "-am", "Mention invoices"]);
    std::fs::write(worktree.join("invoices.rs"), "pub fn total() {}\n").unwrap();

    let stat = client
        .call::<Diff>(DiffParams {
            agent: spawned.agent,
            full: false,
        })
        .await
        .unwrap();
    assert!(!stat.truncated);
    assert!(stat.text.contains("README.md"), "{}", stat.text);
    assert!(stat.text.contains("invoices.rs"), "{}", stat.text);

    let patch = client
        .call::<Diff>(DiffParams {
            agent: spawned.agent,
            full: true,
        })
        .await
        .unwrap();
    assert!(!patch.truncated);
    assert!(patch.text.contains("+invoices"), "{}", patch.text);
    assert!(patch.text.contains("+pub fn total() {}"), "{}", patch.text);
}

#[tokio::test]
async fn the_diff_of_an_agent_outside_git_is_refused() {
    let daemon = start(OFTEN);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let outside = tempfile::tempdir().unwrap();
    let mut session = background("feed5678-0000-4000-8000-000000000004", "loose");
    session.cwd = outside.path().canonicalize().unwrap();
    daemon.harness.set(vec![session]);
    let registry = until(&client, |r| r.agents().count() == 1).await;
    let error = rpc_error(
        client
            .call::<Diff>(DiffParams {
                agent: registry.agents().next().unwrap().id,
                full: false,
            })
            .await,
    );
    assert_eq!(error.code, rpc::REFUSED);
    assert!(
        error.message.contains("does not work in a git repository"),
        "{}",
        error.message
    );
}
