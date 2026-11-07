//! Tidying end to end (ADR 0011): real repositories, real worktrees and
//! delivery evidence read from git, through a real daemon and the fake
//! harness. Nothing is removed without the evidence, and no branch is ever
//! deleted.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::{FakeHarness, git, repository};
use tokio::net::UnixListener;
use ulid::Ulid;
use volee_core::delivery::TidyRefusal;
use volee_core::harness::SessionKey;
use volee_core::reconcile::ListedSession;
use volee_core::role::Role;
use volee_core::state::AgentState;
use volee_core::{AgentId, Event, Registry};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_git::GitCli;
use volee_harness::Harness;
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{
    EventsSince, SinceParams, Spawn, SpawnParams, Spawned, Tidy, TidyOutcome, TidyParams,
};
use volee_proto::paths::Paths;
use volee_proto::rpc;

struct Running {
    socket: PathBuf,
    harness: Arc<FakeHarness>,
    repo: PathBuf,
    _dir: tempfile::TempDir,
}

/// A daemon with a repository named `alpha`, git evidence read every few
/// milliseconds, and a fake harness listed as often.
fn start() -> Running {
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
        harness: Some(Arc::clone(&harness) as Arc<dyn Harness>),
        discovery_interval: Duration::from_millis(10),
        idle_discovery_interval: Duration::from_millis(10),
        evidence: EvidenceSources {
            git: Some(Arc::new(git)),
            settings,
            refresh_interval: Duration::from_millis(20),
            ..EvidenceSources::default()
        },
        volee_bin: Some(PathBuf::from("/opt/volee/bin/volee")),
    };
    tokio::spawn(Daemon::serve(listener, config));
    Running {
        socket,
        harness,
        repo,
        _dir: dir,
    }
}

/// The session the fake harness starts for every launch, as `claude agents`
/// lists it.
fn launched_session(state: AgentState, cwd: &Path) -> ListedSession {
    ListedSession {
        alive: true,
        background: true,
        handle: Some("abcd1234".into()),
        ..ListedSession::new(
            SessionKey::claude("abcd1234-5b60-4cdc-8cf3-000000000001"),
            cwd,
            state,
        )
    }
}

async fn spawn_worker(client: &Client) -> Spawned {
    client
        .call::<Spawn>(SpawnParams {
            repo: "alpha".into(),
            branch: "feat/invoices".into(),
            base: None,
            task: "Add invoices to the billing module.".into(),
            role: Role::Worker,
            parent: None,
            mission: None,
            model: None,
            name: None,
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

/// Waits until the registry satisfies `ready`, for at most thirty seconds:
/// git reads the evidence on blocking threads, which a busy machine slows.
async fn until(client: &Client, ready: impl Fn(&Registry) -> bool) -> Registry {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while tokio::time::Instant::now() < deadline {
        let registry = registry(client).await;
        if ready(&registry) {
            return registry;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the registry never reached the expected state");
}

async fn tidy(client: &Client, agent: AgentId, dry_run: bool) -> TidyOutcome {
    client
        .call::<Tidy>(TidyParams { agent, dry_run })
        .await
        .unwrap()
}

fn reasons(outcome: TidyOutcome) -> Vec<TidyRefusal> {
    match outcome {
        TidyOutcome::Kept { reasons } => reasons,
        other => panic!("expected the worktree to be kept, got {other:?}"),
    }
}

/// A worker that committed on its branch and finished its turn, whose
/// evidence volee has read: one commit not merged yet.
async fn finished_worker(daemon: &Running, client: &Client) -> (AgentId, PathBuf) {
    let spawned = spawn_worker(client).await;
    let worktree = PathBuf::from(&spawned.worktree);
    std::fs::write(worktree.join("invoices.rs"), "pub fn total() {}\n").unwrap();
    git(&worktree, &["add", "invoices.rs"]);
    git(&worktree, &["commit", "--quiet", "-m", "Add invoices"]);
    daemon
        .harness
        .set(vec![launched_session(AgentState::Done, &worktree)]);
    until(client, |r| {
        r.agent(spawned.agent).is_some_and(|agent| {
            agent.state == AgentState::Done
                && agent.delivery.commits == 1
                && agent.delivery.checked_at.is_some()
        })
    })
    .await;
    (spawned.agent, worktree)
}

/// Merges the worker's branch into `main` and waits for the evidence to say
/// so.
async fn merge(daemon: &Running, client: &Client, agent: AgentId) {
    git(
        &daemon.repo,
        &[
            "merge",
            "--quiet",
            "--no-ff",
            "-m",
            "Merge feat/invoices",
            "feat/invoices",
        ],
    );
    until(client, |r| {
        r.agent(agent)
            .is_some_and(|agent| agent.delivery.merged_into_base)
    })
    .await;
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    std::process::Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", branch])
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap()
        .status
        .success()
}

fn locked(repo: &Path, worktree: &Path) -> bool {
    let list = git(repo, &["worktree", "list", "--porcelain"]);
    let entry = format!("worktree {}", worktree.display());
    list.split("\n\n")
        .find(|block| block.lines().next() == Some(entry.as_str()))
        .is_some_and(|block| block.lines().any(|line| line.starts_with("locked")))
}

#[tokio::test]
async fn a_merged_branch_is_tidied_and_its_branch_kept() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let (agent, worktree) = finished_worker(&daemon, &client).await;
    merge(&daemon, &client, agent).await;

    // A dry run says what would happen and touches nothing.
    let dry = tidy(&client, agent, true).await;
    assert_eq!(
        dry,
        TidyOutcome::WouldRemove {
            worktree: worktree.display().to_string(),
            branch: Some("feat/invoices".into()),
        }
    );
    assert!(worktree.is_dir());
    assert_eq!(daemon.harness.actions(), []);

    let removed = tidy(&client, agent, false).await;
    assert_eq!(
        removed,
        TidyOutcome::Removed {
            worktree: worktree.display().to_string(),
            branch: Some("feat/invoices".into()),
        }
    );
    assert!(!worktree.exists(), "the worktree is gone");
    assert!(
        branch_exists(&daemon.repo, "refs/heads/feat/invoices"),
        "the branch is kept"
    );
    assert_eq!(
        daemon.harness.actions(),
        [("remove-delivered".to_owned(), "abcd1234".to_owned())]
    );
    let registry = registry(&client).await;
    assert!(
        registry.agent(agent).is_none(),
        "the agent leaves the views"
    );
    assert!(registry.is_ignored(&SessionKey::claude("abcd1234-5b60-4cdc-8cf3-000000000001")));
}

#[tokio::test]
async fn an_unmerged_branch_is_kept_with_its_reason() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let (agent, worktree) = finished_worker(&daemon, &client).await;
    assert_eq!(
        reasons(tidy(&client, agent, false).await),
        [TidyRefusal::NotDelivered]
    );
    assert!(worktree.is_dir());
    assert_eq!(daemon.harness.actions(), [], "the session is left alone");
    assert!(registry(&client).await.agent(agent).is_some());
}

#[tokio::test]
async fn uncommitted_changes_keep_the_worktree() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let (agent, worktree) = finished_worker(&daemon, &client).await;
    merge(&daemon, &client, agent).await;
    std::fs::write(worktree.join("notes.txt"), "not committed\n").unwrap();
    until(&client, |r| {
        r.agent(agent).is_some_and(|a| a.delivery.dirty)
    })
    .await;
    let kept = reasons(tidy(&client, agent, false).await);
    assert_eq!(kept, [TidyRefusal::UncommittedChanges]);
    assert!(worktree.join("notes.txt").is_file());
    assert_eq!(daemon.harness.actions(), []);
}

#[tokio::test]
async fn a_running_agent_is_kept() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let spawned = spawn_worker(&client).await;
    let worktree = PathBuf::from(&spawned.worktree);
    daemon
        .harness
        .set(vec![launched_session(AgentState::Working, &worktree)]);
    until(&client, |r| {
        r.agent(spawned.agent)
            .is_some_and(|agent| agent.state == AgentState::Working)
    })
    .await;
    let kept = reasons(tidy(&client, spawned.agent, false).await);
    assert!(kept.contains(&TidyRefusal::Busy), "{kept:?}");
    assert!(worktree.is_dir());
    assert_eq!(daemon.harness.actions(), []);
}

#[tokio::test]
async fn a_locked_worktree_is_unlocked_only_when_the_evidence_allows() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let (agent, worktree) = finished_worker(&daemon, &client).await;
    // Claude Code locks the worktrees its sessions use, and keeps a session
    // whose worktree it did not create.
    git(
        &daemon.repo,
        &[
            "worktree",
            "lock",
            "--reason",
            "claude session abcd1234",
            &worktree.display().to_string(),
        ],
    );
    daemon.harness.keep_on_remove(true);

    assert_eq!(
        reasons(tidy(&client, agent, false).await),
        [TidyRefusal::NotDelivered]
    );
    assert!(locked(&daemon.repo, &worktree), "refused: the lock stays");

    merge(&daemon, &client, agent).await;
    assert!(matches!(
        tidy(&client, agent, false).await,
        TidyOutcome::Removed { .. }
    ));
    assert!(!worktree.exists());
    assert!(branch_exists(&daemon.repo, "refs/heads/feat/invoices"));
    // Asked once with the evidence, and once more after the worktree went.
    assert_eq!(
        daemon.harness.actions(),
        [
            ("remove-delivered".to_owned(), "abcd1234".to_owned()),
            ("remove-delivered".to_owned(), "abcd1234".to_owned()),
        ]
    );
}

#[tokio::test]
async fn an_agent_volee_did_not_launch_is_never_tidied() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let found = ListedSession {
        name: Some("by hand".into()),
        alive: true,
        background: true,
        handle: Some("feed5678".into()),
        ..ListedSession::new(
            SessionKey::claude("feed5678-0000-4000-8000-000000000002"),
            &daemon.repo,
            AgentState::Done,
        )
    };
    daemon.harness.set(vec![found]);
    let registry = until(&client, |r| r.agents().count() == 1).await;
    let agent = registry.agents().next().unwrap().id;
    let kept = reasons(tidy(&client, agent, false).await);
    assert!(kept.contains(&TidyRefusal::NotLaunchedByVolee), "{kept:?}");
    assert!(kept.contains(&TidyRefusal::NotAVoleeWorktree), "{kept:?}");
    assert!(daemon.repo.is_dir());
}

#[tokio::test]
async fn unknown_agents_are_not_found() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let error = client
        .call::<Tidy>(TidyParams {
            agent: AgentId::from_ulid(Ulid::from_parts(9_999, 9)),
            dry_run: true,
        })
        .await
        .unwrap_err();
    match error {
        ClientError::Rpc(error) => assert_eq!(error.code, rpc::NOT_FOUND),
        other => panic!("expected an RPC error, got {other:?}"),
    }
}
