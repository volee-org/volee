//! The agents view end to end: a real daemon, a fake `claude agents` listing,
//! real repositories and worktrees, and hook events in a real SQLite store.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::{FakeHarness, git, repository};
use tokio::net::UnixListener;
use volee_core::harness::SessionKey;
use volee_core::reconcile::ListedSession;
use volee_core::state::{AgentState, InputRequest};
use volee_daemon::{AgentsViewConfig, Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_git::GitCli;
use volee_harness::Harness;
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{AgentCard, AgentsView, ViewAgents, ViewParams};
use volee_proto::paths::Paths;
use volee_store::{NewHookEvent, Store};

struct Running {
    socket: PathBuf,
    harness: Arc<FakeHarness>,
    store: Store,
    repo: PathBuf,
    worktree: PathBuf,
    _dir: tempfile::TempDir,
}

/// A daemon with the agents view on, a repository `alpha` with a worktree on
/// `feat/invoices`, and a fake listing the test sets.
fn start(view: bool) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let repo = root.join("alpha");
    repository(&repo);
    let worktree = root.join("alpha-feat");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "feat/invoices",
            worktree.to_str().unwrap(),
            "main",
        ],
    );
    let paths = Paths::rooted(root.join("volee"));
    std::fs::create_dir_all(&paths.state_dir).unwrap();
    let database = paths.database();
    let store = Store::open(&database).unwrap();
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let harness = Arc::new(FakeHarness::default());
    let git_cli = GitCli::new()
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(vec![])),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: Some(Arc::clone(&harness) as Arc<dyn Harness>),
        discovery_interval: Duration::from_secs(3_600),
        max_discovery_interval: Duration::from_secs(3_600),
        idle_discovery_interval: Duration::from_secs(3_600),
        evidence: EvidenceSources {
            git: Some(Arc::new(git_cli)),
            refresh_interval: Duration::from_secs(3_600),
            ..EvidenceSources::default()
        },
        processes: None,
        hook_server: None,
        agents_view: view.then(|| AgentsViewConfig {
            poll_interval: Duration::from_secs(3_600),
            database,
        }),
    };
    tokio::spawn(Daemon::serve(listener, config));
    Running {
        socket,
        harness,
        store,
        repo,
        worktree,
        _dir: dir,
    }
}

fn session(id: &str, cwd: &Path, state: AgentState) -> ListedSession {
    ListedSession {
        alive: true,
        pid: Some(4242),
        name: Some("billing".into()),
        ..ListedSession::new(SessionKey::claude(id), cwd, state)
    }
}

/// Stores hook events for `id`, one per `(name, at)`.
fn hooks(store: &Store, id: &str, events: &[(&str, i64)]) {
    for (name, at) in events {
        store
            .append_hook_event(&NewHookEvent {
                received_at: *at,
                event: (*name).to_owned(),
                session_id: Some(id.to_owned()),
                cwd: None,
                payload: format!(r#"{{"hook_event_name":"{name}","session_id":"{id}"}}"#),
            })
            .unwrap();
    }
}

async fn view(daemon: &Running) -> AgentsView {
    let client = Client::connect(&daemon.socket).await.unwrap();
    client
        .call::<ViewAgents>(ViewParams {
            max_age_ms: Some(0),
        })
        .await
        .unwrap()
}

async fn only_card(daemon: &Running) -> AgentCard {
    let mut view = view(daemon).await;
    assert_eq!(view.agents.len(), 1, "{view:?}");
    view.agents.remove(0)
}

fn commit_in_worktree(daemon: &Running) {
    std::fs::write(daemon.worktree.join("invoices.rs"), "pub fn total() {}\n").unwrap();
    git(&daemon.worktree, &["add", "invoices.rs"]);
    git(
        &daemon.worktree,
        &["commit", "--quiet", "-m", "Add invoices"],
    );
}

const ID: &str = "11111111-2222-4333-8444-555555555555";

#[tokio::test]
async fn an_agent_that_stopped_after_committing_is_finished_work_with_its_diff_summary() {
    let daemon = start(true);
    commit_in_worktree(&daemon);
    hooks(
        &daemon.store,
        ID,
        &[
            ("SessionStart", 100),
            ("UserPromptSubmit", 200),
            ("PreToolUse", 300),
            ("PostToolUse", 400),
            ("Stop", 500),
        ],
    );
    daemon
        .harness
        .set(vec![session(ID, &daemon.worktree, AgentState::Idle)]);
    let card = only_card(&daemon).await;
    assert_eq!(card.session_id, ID);
    assert_eq!(card.name.as_deref(), Some("billing"));
    assert_eq!(card.pid, Some(4242));
    let repo = card.repo.unwrap();
    assert_eq!(repo.name, "alpha");
    assert_eq!(repo.root, daemon.repo);
    assert_eq!(repo.worktree.as_ref(), Some(&daemon.worktree));
    assert_eq!(repo.branch.as_deref(), Some("feat/invoices"));
    assert_eq!(repo.base.as_deref(), Some("main"));
    let facts = card.git.unwrap();
    assert_eq!((facts.ahead, facts.unique, facts.behind), (1, 1, 0));
    assert!(!facts.dirty && !facts.merged);
    assert_eq!(
        (
            facts.diff.files,
            facts.diff.insertions,
            facts.diff.deletions
        ),
        (1, 1, 0)
    );
    assert_eq!(card.hooks.events, 5);
    assert_eq!(card.hooks.last_event.as_deref(), Some("Stop"));
    assert_eq!(card.hooks.activity.last_activity_at, Some(400));
    assert_eq!(card.hooks.activity.last_stop_at, Some(500));
    assert!(card.finished_work);
}

#[tokio::test]
async fn an_agent_that_acted_again_after_its_stop_is_not_finished() {
    let daemon = start(true);
    commit_in_worktree(&daemon);
    hooks(
        &daemon.store,
        ID,
        &[
            ("UserPromptSubmit", 200),
            ("Stop", 300),
            ("UserPromptSubmit", 400),
        ],
    );
    daemon
        .harness
        .set(vec![session(ID, &daemon.worktree, AgentState::Idle)]);
    assert!(!only_card(&daemon).await.finished_work);
}

#[tokio::test]
async fn a_stopped_agent_without_commits_or_with_a_merged_branch_is_not_finished() {
    let daemon = start(true);
    hooks(
        &daemon.store,
        ID,
        &[("UserPromptSubmit", 200), ("Stop", 300)],
    );
    daemon
        .harness
        .set(vec![session(ID, &daemon.worktree, AgentState::Idle)]);
    let card = only_card(&daemon).await;
    assert_eq!(card.git.unwrap().ahead, 0);
    assert!(!card.finished_work, "no commits of its own");

    // It commits, the daemon sees it, then the work lands in main.
    commit_in_worktree(&daemon);
    assert!(only_card(&daemon).await.finished_work);
    git(
        &daemon.repo,
        &[
            "merge",
            "--quiet",
            "--no-ff",
            "-m",
            "Merge",
            "feat/invoices",
        ],
    );
    let card = only_card(&daemon).await;
    let facts = card.git.unwrap();
    assert!(
        facts.merged,
        "the merge is recognized from the memory of the last reading"
    );
    assert!(!card.finished_work, "nothing left to look at");
}

#[tokio::test]
async fn a_working_agent_is_not_finished_and_a_waiting_one_says_what_it_waits_for() {
    let daemon = start(true);
    commit_in_worktree(&daemon);
    hooks(
        &daemon.store,
        ID,
        &[("UserPromptSubmit", 200), ("Stop", 300)],
    );
    daemon
        .harness
        .set(vec![session(ID, &daemon.worktree, AgentState::Working)]);
    assert!(!only_card(&daemon).await.finished_work);
    daemon.harness.set(vec![session(
        ID,
        &daemon.worktree,
        AgentState::NeedsInput {
            request: InputRequest::Permission { tool: None },
        },
    )]);
    let card = only_card(&daemon).await;
    assert_eq!(card.waiting_for.as_deref(), Some("a permission"));
    assert!(!card.finished_work);
}

#[tokio::test]
async fn a_session_outside_any_repository_still_gets_a_card() {
    let daemon = start(true);
    let elsewhere = tempfile::tempdir().unwrap();
    daemon.harness.set(vec![session(
        ID,
        &elsewhere.path().canonicalize().unwrap(),
        AgentState::Idle,
    )]);
    let card = only_card(&daemon).await;
    assert!(card.repo.is_none() && card.git.is_none() && card.git_error.is_none());
    assert_eq!(card.hooks.events, 0);
}

#[tokio::test]
async fn a_failing_listing_keeps_the_last_agents_and_says_why() {
    let daemon = start(true);
    daemon
        .harness
        .set(vec![session(ID, &daemon.worktree, AgentState::Idle)]);
    assert_eq!(view(&daemon).await.agents.len(), 1);
    daemon.harness.fail(true);
    let degraded = view(&daemon).await;
    assert!(degraded.error.is_some(), "{degraded:?}");
    assert_eq!(degraded.agents.len(), 1, "the last reading stays");
    daemon.harness.fail(false);
    let healed = view(&daemon).await;
    assert_eq!(healed.error, None);
    assert_eq!(healed.agents.len(), 1);
    assert!(healed.updated_at.is_some());
}

#[tokio::test]
async fn a_view_that_is_not_configured_says_so() {
    let daemon = start(false);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let error = client
        .call::<ViewAgents>(ViewParams::default())
        .await
        .unwrap_err();
    match error {
        ClientError::Rpc(error) => {
            assert!(error.message.contains("agents view"), "{}", error.message);
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// Measured on Claude Code 2.1.330: the subagent that writes the suggested
/// next prompt stops a few seconds after the turn's `Stop`.
#[tokio::test]
async fn a_subagent_that_stops_after_the_stop_does_not_undo_finished_work() {
    let daemon = start(true);
    commit_in_worktree(&daemon);
    hooks(
        &daemon.store,
        ID,
        &[
            ("UserPromptSubmit", 200),
            ("PostToolUse", 300),
            ("Stop", 400),
            ("SubagentStop", 404),
        ],
    );
    daemon
        .harness
        .set(vec![session(ID, &daemon.worktree, AgentState::Idle)]);
    let card = only_card(&daemon).await;
    assert_eq!(card.hooks.last_event.as_deref(), Some("SubagentStop"));
    assert!(card.finished_work);
}
