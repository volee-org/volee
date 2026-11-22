//! The agents view end to end: a real daemon, a fake `claude agents` listing,
//! real repositories and worktrees, and hook events in a real SQLite store.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{FakeHarness, git, repository};
use tokio::net::UnixListener;
use volee_core::delivery::{ChecksState, PrState, PullRequest};
use volee_core::harness::SessionKey;
use volee_core::reconcile::ListedSession;
use volee_core::state::{AgentState, InputRequest};
use volee_daemon::{AgentsViewConfig, Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_git::{
    BaseGuess, BranchHead, DeliveryQuery, DeliveryReport, DiffFormat, DiffOutput, Fingerprint,
    Forge, ForgeStatus, Git, GitCli, GitError, MergeMethod, NewWorktree, RepoInfo, RepoSlug,
    WorktreeInfo,
};
use volee_harness::Harness;
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{AgentCard, AgentsView, ViewAgents, ViewParams};
use volee_proto::paths::Paths;
use volee_store::{NewHookEvent, Store};

/// The real git, counting the two questions only the agents view asks: how
/// a branch was based (the costly, cached part) and what a tree looks like
/// (the cheap part, asked at every reading).
struct CountingGit {
    inner: GitCli,
    base_of: AtomicUsize,
    fingerprints: AtomicUsize,
}

impl Git for CountingGit {
    fn repo_info(&self, dir: &Path) -> Result<Option<RepoInfo>, GitError> {
        self.inner.repo_info(dir)
    }
    fn worktrees(&self, repo: &Path) -> Result<Vec<WorktreeInfo>, GitError> {
        self.inner.worktrees(repo)
    }
    fn default_branch(&self, repo: &Path) -> Result<Option<String>, GitError> {
        self.inner.default_branch(repo)
    }
    fn add_worktree(
        &self,
        repo: &Path,
        path: &Path,
        branch: &str,
        base: &str,
    ) -> Result<NewWorktree, GitError> {
        self.inner.add_worktree(repo, path, branch, base)
    }
    fn remove_worktree(&self, repo: &Path, path: &Path) -> Result<(), GitError> {
        self.inner.remove_worktree(repo, path)
    }
    fn unlock_worktree(&self, repo: &Path, path: &Path) -> Result<(), GitError> {
        self.inner.unlock_worktree(repo, path)
    }
    fn delivery(
        &self,
        worktree: &Path,
        query: &DeliveryQuery<'_>,
    ) -> Result<DeliveryReport, GitError> {
        self.inner.delivery(worktree, query)
    }
    fn diff(
        &self,
        worktree: &Path,
        base: &str,
        format: DiffFormat,
        max_bytes: usize,
    ) -> Result<DiffOutput, GitError> {
        self.inner.diff(worktree, base, format, max_bytes)
    }
    fn merge(&self, repo: &Path, branch: &str, base: &str) -> Result<String, GitError> {
        self.inner.merge(repo, branch, base)
    }
    fn fetch(&self, repo: &Path) -> Result<(), GitError> {
        self.inner.fetch(repo)
    }
    fn origin(&self, repo: &Path) -> Result<Option<RepoSlug>, GitError> {
        self.inner.origin(repo)
    }
    fn base_of(&self, repo: &Path, branch: &str) -> Result<Option<BaseGuess>, GitError> {
        self.base_of.fetch_add(1, Ordering::SeqCst);
        self.inner.base_of(repo, branch)
    }
    fn fingerprint(&self, dir: &Path) -> Result<Option<Fingerprint>, GitError> {
        self.fingerprints.fetch_add(1, Ordering::SeqCst);
        self.inner.fingerprint(dir)
    }
}

/// A forge that answers what the test says and counts the questions.
struct FakeForge {
    usable: bool,
    answer: Mutex<Answer>,
    /// The repository directory of each question.
    calls: Mutex<Vec<PathBuf>>,
}

#[derive(Clone, Copy)]
enum Answer {
    NoPullRequest,
    Pull(PrState),
    Fails,
}

impl FakeForge {
    fn new(usable: bool, answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            usable,
            answer: Mutex::new(answer),
            calls: Mutex::new(Vec::new()),
        })
    }

    /// How many times the agents view asked about `repo`, the repository's
    /// main checkout. (The 0.x delivery evidence asks too, from the
    /// worktree's directory: it is not counted.)
    fn calls_in(&self, repo: &Path) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|dir| dir.as_path() == repo)
            .count()
    }
}

impl Forge for FakeForge {
    fn status(&self) -> ForgeStatus {
        ForgeStatus {
            program: "fake-gh".into(),
            available: self.usable,
            version: Some("1.0.0".into()),
            detail: None,
        }
    }

    fn pull_request(
        &self,
        repo: &Path,
        head: &BranchHead<'_>,
    ) -> Result<Option<PullRequest>, GitError> {
        self.calls.lock().unwrap().push(repo.to_path_buf());
        match *self.answer.lock().unwrap() {
            Answer::NoPullRequest => Ok(None),
            Answer::Pull(state) => Ok(Some(PullRequest {
                number: 7,
                url: "https://github.com/acme/alpha/pull/7".into(),
                state,
                checks: ChecksState::Passing,
                head: Some(head.tip.to_owned()),
            })),
            Answer::Fails => Err(GitError::Failed {
                command: "gh pr list".into(),
                code: Some(1),
                stderr: "boom".into(),
            }),
        }
    }

    fn merge(
        &self,
        _repo: &Path,
        _pull_request: &PullRequest,
        _method: Option<MergeMethod>,
    ) -> Result<MergeMethod, GitError> {
        unreachable!("the agents view never merges")
    }
}

struct Running {
    /// The git the daemon uses, counting what the agents view asks.
    counting: Arc<CountingGit>,
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
    start_with(view, None, false)
}

/// The same, with a forge client and, when `remote`, an `origin` on GitHub.
fn start_with(view: bool, forge: Option<Arc<FakeForge>>, remote: bool) -> Running {
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
    if remote {
        git(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/alpha.git",
            ],
        );
    }
    let paths = Paths::rooted(root.join("volee"));
    std::fs::create_dir_all(&paths.state_dir).unwrap();
    let database = paths.database();
    let store = Store::open(&database).unwrap();
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let harness = Arc::new(FakeHarness::default());
    let counting = Arc::new(CountingGit {
        inner: GitCli::new()
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1"),
        base_of: AtomicUsize::new(0),
        fingerprints: AtomicUsize::new(0),
    });
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
            git: Some(Arc::clone(&counting) as Arc<dyn Git>),
            forge: forge.map(|forge| forge as Arc<dyn Forge>),
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
        counting,
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

#[tokio::test]
async fn the_base_is_the_branch_the_agents_branch_was_created_from_not_the_default() {
    let daemon = start(true);
    // `develop` is two commits ahead of main, and the agent's branch is made
    // from it, then adds one commit of its own.
    git(&daemon.repo, &["branch", "develop"]);
    let develop = daemon.repo.parent().unwrap().join("alpha-develop");
    git(
        &daemon.repo,
        &[
            "worktree",
            "add",
            "--quiet",
            develop.to_str().unwrap(),
            "develop",
        ],
    );
    for file in ["one.rs", "two.rs"] {
        std::fs::write(develop.join(file), "x\n").unwrap();
        git(&develop, &["add", file]);
        git(&develop, &["commit", "--quiet", "-m", file]);
    }
    let sub = daemon.repo.parent().unwrap().join("alpha-sub");
    git(
        &daemon.repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "feat/sub",
            sub.to_str().unwrap(),
            "develop",
        ],
    );
    std::fs::write(sub.join("sub.rs"), "sub\n").unwrap();
    git(&sub, &["add", "sub.rs"]);
    git(&sub, &["commit", "--quiet", "-m", "Add sub"]);
    daemon
        .harness
        .set(vec![session(ID, &sub, AgentState::Idle)]);
    let card = only_card(&daemon).await;
    let repo = card.repo.unwrap();
    assert_eq!(repo.base.as_deref(), Some("develop"));
    assert_eq!(repo.base_source.as_deref(), Some("reflog"));
    let facts = card.git.unwrap();
    assert_eq!(
        facts.ahead, 1,
        "one commit against develop, not three against main"
    );
    assert_eq!(facts.diff.files, 1);
}

/// The tip of `feat/invoices`, which the agents view remembers.
fn branch_tip(daemon: &Running) -> String {
    git(&daemon.repo, &["rev-parse", "feat/invoices"])
}

fn merge_into_main(daemon: &Running) {
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
}

#[tokio::test]
async fn what_the_view_saw_of_a_branch_survives_a_restart_and_proves_a_later_merge() {
    // A previous run saw the branch with its commit, and wrote it down.
    let daemon = start(true);
    commit_in_worktree(&daemon);
    let tip = branch_tip(&daemon);
    daemon
        .store
        .set_unmerged_tip(
            &daemon.repo.to_string_lossy(),
            "feat/invoices",
            Some(&tip),
            1,
        )
        .unwrap();
    // The daemon was down while the work landed.
    merge_into_main(&daemon);
    daemon
        .harness
        .set(vec![session(ID, &daemon.worktree, AgentState::Idle)]);
    let facts = only_card(&daemon).await.git.unwrap();
    assert!(facts.merged, "{facts:?}");
    assert_eq!(facts.merge_source.as_deref(), Some("history"));
}

#[tokio::test]
async fn without_that_memory_a_merge_made_in_the_dark_is_not_recognized() {
    let daemon = start(true);
    commit_in_worktree(&daemon);
    merge_into_main(&daemon);
    daemon
        .harness
        .set(vec![session(ID, &daemon.worktree, AgentState::Idle)]);
    let facts = only_card(&daemon).await.git.unwrap();
    assert!(!facts.merged, "nothing was ever seen of that branch");
    assert_eq!(facts.ahead, 0);
}

#[tokio::test]
async fn each_reading_writes_down_the_tip_it_saw() {
    let daemon = start(true);
    commit_in_worktree(&daemon);
    daemon
        .harness
        .set(vec![session(ID, &daemon.worktree, AgentState::Idle)]);
    only_card(&daemon).await;
    let repo = daemon.repo.to_string_lossy().into_owned();
    assert_eq!(
        daemon.store.unmerged_tip(&repo, "feat/invoices").unwrap(),
        Some(branch_tip(&daemon))
    );
}

fn forge_ready(answer: Answer) -> (Running, Arc<FakeForge>) {
    let forge = FakeForge::new(true, answer);
    let daemon = start_with(true, Some(Arc::clone(&forge)), true);
    commit_in_worktree(&daemon);
    daemon
        .harness
        .set(vec![session(ID, &daemon.worktree, AgentState::Idle)]);
    hooks(
        &daemon.store,
        ID,
        &[("UserPromptSubmit", 200), ("Stop", 300)],
    );
    (daemon, forge)
}

#[tokio::test]
async fn a_squash_merge_is_seen_through_the_pull_request_and_the_answer_is_cached() {
    let (daemon, forge) = forge_ready(Answer::Pull(PrState::Merged));
    // The work landed on main as a different commit: a squash.
    std::fs::write(daemon.repo.join("squashed.rs"), "squashed\n").unwrap();
    git(&daemon.repo, &["add", "squashed.rs"]);
    git(
        &daemon.repo,
        &["commit", "--quiet", "-m", "Squash of feat/invoices (#7)"],
    );
    let card = only_card(&daemon).await;
    let facts = card.git.unwrap();
    assert!(facts.merged, "{facts:?}");
    assert_eq!(facts.merge_source.as_deref(), Some("pull-request"));
    assert_eq!(facts.pull_request.unwrap().number, 7);
    assert!(!card.finished_work, "the work is in main");
    assert_eq!(forge.calls_in(&daemon.repo), 1);
    // Merged for good: asked once, never again.
    only_card(&daemon).await;
    only_card(&daemon).await;
    assert_eq!(forge.calls_in(&daemon.repo), 1, "a merged answer is cached");
}

#[tokio::test]
async fn an_open_pull_request_is_shown_and_does_not_make_the_work_merged() {
    let (daemon, forge) = forge_ready(Answer::Pull(PrState::Open));
    let card = only_card(&daemon).await;
    let facts = card.git.unwrap();
    assert!(!facts.merged);
    assert_eq!(facts.pull_request.unwrap().state, PrState::Open);
    assert_eq!(facts.merge_unknown, None);
    assert!(card.finished_work);
    only_card(&daemon).await;
    assert_eq!(forge.calls_in(&daemon.repo), 1, "cached for a few minutes");
}

#[tokio::test]
async fn a_branch_without_pull_request_is_not_merged_and_not_unknown() {
    let (daemon, _forge) = forge_ready(Answer::NoPullRequest);
    let facts = only_card(&daemon).await.git.unwrap();
    assert!(!facts.merged && facts.pull_request.is_none());
    assert_eq!(
        facts.merge_unknown, None,
        "the forge was asked and said none"
    );
}

#[tokio::test]
async fn when_the_forge_cannot_be_asked_the_merge_is_unknown_never_guessed() {
    // No forge client at all.
    let daemon = start_with(true, None, true);
    commit_in_worktree(&daemon);
    daemon
        .harness
        .set(vec![session(ID, &daemon.worktree, AgentState::Idle)]);
    let facts = only_card(&daemon).await.git.unwrap();
    assert!(!facts.merged);
    assert!(
        facts
            .merge_unknown
            .as_deref()
            .unwrap()
            .contains("no forge client"),
        "{facts:?}"
    );

    // gh missing or signed out.
    let forge = FakeForge::new(false, Answer::NoPullRequest);
    let daemon = start_with(true, Some(Arc::clone(&forge)), true);
    commit_in_worktree(&daemon);
    daemon
        .harness
        .set(vec![session(ID, &daemon.worktree, AgentState::Idle)]);
    let facts = only_card(&daemon).await.git.unwrap();
    assert!(
        facts
            .merge_unknown
            .as_deref()
            .unwrap()
            .contains("gh is missing")
    );
    assert_eq!(forge.calls_in(&daemon.repo), 0);

    // No GitHub remote.
    let forge = FakeForge::new(true, Answer::NoPullRequest);
    let daemon = start_with(true, Some(Arc::clone(&forge)), false);
    commit_in_worktree(&daemon);
    daemon
        .harness
        .set(vec![session(ID, &daemon.worktree, AgentState::Idle)]);
    let facts = only_card(&daemon).await.git.unwrap();
    assert!(
        facts
            .merge_unknown
            .as_deref()
            .unwrap()
            .contains("no GitHub remote")
    );
    assert_eq!(forge.calls_in(&daemon.repo), 0);
}

#[tokio::test]
async fn a_forge_that_fails_is_unknown_and_is_not_asked_again_at_once() {
    let (daemon, forge) = forge_ready(Answer::Fails);
    let facts = only_card(&daemon).await.git.unwrap();
    assert!(!facts.merged);
    assert!(
        facts
            .merge_unknown
            .as_deref()
            .unwrap()
            .contains("could not be asked"),
        "{facts:?}"
    );
    only_card(&daemon).await;
    assert_eq!(
        forge.calls_in(&daemon.repo),
        1,
        "a failure is cached for a minute"
    );
}

fn costly_readings(daemon: &Running) -> usize {
    daemon.counting.base_of.load(Ordering::SeqCst)
}

fn cheap_readings(daemon: &Running) -> usize {
    daemon.counting.fingerprints.load(Ordering::SeqCst)
}

#[tokio::test]
async fn an_agent_that_did_not_move_costs_one_cheap_reading_not_a_full_one() {
    let daemon = start(true);
    commit_in_worktree(&daemon);
    daemon
        .harness
        .set(vec![session(ID, &daemon.worktree, AgentState::Idle)]);
    let first = only_card(&daemon).await;
    assert_eq!(costly_readings(&daemon), 1);
    for _ in 0..5 {
        let again = only_card(&daemon).await;
        assert_eq!(again.git, first.git, "the cached facts are the same");
        assert_eq!(again.repo, first.repo);
    }
    assert_eq!(
        costly_readings(&daemon),
        1,
        "nothing moved: nothing read again"
    );
    // One cheap reading for each of the six requests (the poller's own first
    // reading may add one if it came after the listing was set).
    assert!(
        (6..=7).contains(&cheap_readings(&daemon)),
        "{}",
        cheap_readings(&daemon)
    );
}

#[tokio::test]
async fn a_commit_or_an_uncommitted_change_makes_the_facts_read_again() {
    let daemon = start(true);
    daemon
        .harness
        .set(vec![session(ID, &daemon.worktree, AgentState::Idle)]);
    assert_eq!(only_card(&daemon).await.git.unwrap().ahead, 0);
    assert_eq!(costly_readings(&daemon), 1);

    commit_in_worktree(&daemon);
    let after_commit = only_card(&daemon).await.git.unwrap();
    assert_eq!(after_commit.ahead, 1, "the new commit is seen at once");
    assert_eq!(costly_readings(&daemon), 2);

    std::fs::write(daemon.worktree.join("notes.txt"), "not added\n").unwrap();
    let dirty = only_card(&daemon).await.git.unwrap();
    assert!(dirty.dirty, "an untracked file is seen at once");
    assert_eq!(costly_readings(&daemon), 3);

    std::fs::remove_file(daemon.worktree.join("notes.txt")).unwrap();
    assert!(!only_card(&daemon).await.git.unwrap().dirty);
    assert_eq!(costly_readings(&daemon), 4);
}

#[tokio::test]
async fn a_session_outside_any_repository_starts_no_git_command() {
    let daemon = start(true);
    let elsewhere = tempfile::tempdir().unwrap();
    daemon.harness.set(vec![session(
        ID,
        &elsewhere.path().canonicalize().unwrap(),
        AgentState::Idle,
    )]);
    let card = only_card(&daemon).await;
    assert!(card.repo.is_none() && card.git.is_none() && card.git_error.is_none());
    assert_eq!(cheap_readings(&daemon), 0, "not even the cheap one");
    assert_eq!(costly_readings(&daemon), 0);
}

#[tokio::test]
async fn a_session_in_a_subdirectory_of_a_repository_gets_its_facts() {
    let daemon = start(true);
    commit_in_worktree(&daemon);
    let deep = daemon.worktree.join("deep/er");
    std::fs::create_dir_all(&deep).unwrap();
    daemon
        .harness
        .set(vec![session(ID, &deep, AgentState::Idle)]);
    let card = only_card(&daemon).await;
    assert_eq!(card.repo.unwrap().branch.as_deref(), Some("feat/invoices"));
    assert_eq!(card.git.unwrap().ahead, 1);
}

#[tokio::test]
async fn a_base_that_moves_is_seen_at_once_though_the_worktree_did_not_move() {
    let daemon = start(true);
    commit_in_worktree(&daemon);
    daemon
        .harness
        .set(vec![session(ID, &daemon.worktree, AgentState::Idle)]);
    assert_eq!(only_card(&daemon).await.git.unwrap().behind, 0);
    assert_eq!(costly_readings(&daemon), 1);
    // Someone commits on main, in the main checkout: the agent's worktree,
    // index and branch are untouched.
    std::fs::write(daemon.repo.join("theirs.rs"), "theirs\n").unwrap();
    git(&daemon.repo, &["add", "theirs.rs"]);
    git(&daemon.repo, &["commit", "--quiet", "-m", "Add theirs"]);
    assert_eq!(only_card(&daemon).await.git.unwrap().behind, 1);
    assert_eq!(costly_readings(&daemon), 2);
}
