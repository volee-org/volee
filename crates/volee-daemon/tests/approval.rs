//! A mission from its workers' delivery to its end (ADR 0032), in real
//! repositories: the work is ready once committed (and, with a remote,
//! pushed with a pull request), the coordinator presents its report, and
//! the person's "ok merge" is the only thing that merges it. volee makes
//! the merge itself: in the main checkout of a repository without a remote,
//! on the forge for a pull request. It then closes the mission and tidies
//! everything in one gesture; a merge that does not go through changes
//! nothing, opens the mission again and says why.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{FakeHarness, git, launch_handle, repository};
use tokio::net::UnixListener;
use ulid::Ulid;
use volee_core::delivery::{ChecksState, PrState, PullRequest};
use volee_core::harness::{HarnessKind, SessionKey};
use volee_core::message::{Message, MessageKind, Party, Receipt, Thread};
use volee_core::mission::{Mission, MissionKind, MissionStatus, Verdict};
use volee_core::reconcile::ListedSession;
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::AgentState;
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};
use volee_core::{AgentId, Event, EventKind, MessageId, MissionId, Registry, ThreadId};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_git::{BranchHead, Forge, ForgeStatus, GitCli, GitError, MergeMethod};
use volee_harness::Harness;
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{
    ApprovalParams, CloseMission, CloseParams, Empty, EventsSince, HookIngest, HookParams,
    RequestApproval, SendMessage, SendParams, Shutdown, SinceParams, Spawn, SpawnParams,
};
use volee_proto::paths::Paths;
use volee_proto::rpc;

const COORDINATOR: u128 = 10;
const LEAD: u128 = 20;

/// The address of the pull request the fake forge knows.
const PULL_REQUEST: &str = "https://example.com/alpha/pull/7";

fn agent(n: u128) -> AgentId {
    AgentId::from_ulid(Ulid::from_parts(1_000, n))
}

fn mission_id() -> MissionId {
    MissionId::from_ulid(Ulid::from_parts(4_000, 1))
}

fn thread_id() -> ThreadId {
    ThreadId::from_ulid(Ulid::from_parts(2_000, 1))
}

/// A forge with at most one pull request, which the test opens. It merges
/// it when asked, unless the test made it refuse, and records what it was
/// asked to merge.
#[derive(Default)]
struct MergingForge {
    pull_request: Mutex<Option<PullRequest>>,
    refusal: Mutex<Option<String>>,
    /// The forge says yes to a merge and merges nothing.
    idle: Mutex<bool>,
    merges: Mutex<Vec<(String, Option<MergeMethod>)>>,
}

impl MergingForge {
    /// From now on the branch has a pull request in `state` at whatever tip
    /// the daemon asks about, as a forge answers once the branch is pushed.
    fn open(&self, state: PrState) {
        *self.pull_request.lock().unwrap() = Some(PullRequest {
            number: 7,
            url: PULL_REQUEST.into(),
            state,
            checks: ChecksState::Passing,
            head: None,
        });
    }

    /// The person merges the pull request themselves, on the forge.
    fn merged_by_hand(&self) {
        if let Some(pr) = self.pull_request.lock().unwrap().as_mut() {
            pr.state = PrState::Merged;
        }
    }

    /// Makes the forge accept merges without making them.
    fn accept_and_do_nothing(&self) {
        *self.idle.lock().unwrap() = true;
    }

    /// Makes the forge refuse to merge, with these words; `None` lets it
    /// merge again.
    fn refuse(&self, why: Option<&str>) {
        *self.refusal.lock().unwrap() = why.map(str::to_owned);
    }

    /// The merges it was asked for: the pull request's address and the
    /// method.
    fn merges(&self) -> Vec<(String, Option<MergeMethod>)> {
        self.merges.lock().unwrap().clone()
    }
}

impl Forge for MergingForge {
    fn status(&self) -> ForgeStatus {
        ForgeStatus {
            program: "fake-gh".into(),
            available: true,
            version: Some("1.0.0".into()),
            detail: None,
        }
    }

    fn pull_request(
        &self,
        _repo: &Path,
        head: &BranchHead<'_>,
    ) -> Result<Option<PullRequest>, GitError> {
        let found = self.pull_request.lock().unwrap().clone();
        Ok(found.map(|pr| PullRequest {
            head: Some(head.tip.to_owned()),
            ..pr
        }))
    }

    fn merge(
        &self,
        _repo: &Path,
        pull_request: &PullRequest,
        method: Option<MergeMethod>,
    ) -> Result<MergeMethod, GitError> {
        self.merges
            .lock()
            .unwrap()
            .push((pull_request.url.clone(), method));
        if let Some(why) = self.refusal.lock().unwrap().clone() {
            return Err(GitError::Failed {
                command: format!("gh pr merge {}", pull_request.url),
                code: Some(1),
                stderr: why,
            });
        }
        if !*self.idle.lock().unwrap() {
            self.merged_by_hand();
        }
        Ok(method.unwrap_or(MergeMethod::Merge))
    }
}

/// What a test runs in: a repository named `alpha` with one commit on
/// `main`, a fake harness and a fake forge, and the daemon's socket.
struct Running {
    socket: PathBuf,
    harness: Arc<FakeHarness>,
    forge: Arc<MergingForge>,
    repo: PathBuf,
    root: PathBuf,
    /// Whether `alpha` has a remote, and the daemon a forge client.
    remote: bool,
    _dir: tempfile::TempDir,
}

/// A lead (agent 20) asked for the mission "Invoicing" in `alpha`; its
/// coordinator (agent 10) rests between two turns.
fn mission(repo: &Path, kind: MissionKind) -> Vec<Event> {
    let at = |kind| Event::new(Timestamp::from_millis(1), kind);
    let member = |n: u128, name: &str, role: Role, of: Option<MissionId>| {
        at(EventKind::AgentRegistered {
            agent: agent(n),
            name: name.into(),
            role,
            harness: HarnessKind::ClaudeCode,
            launched_by: LaunchedBy::Human,
            parent: (role == Role::Coordinator).then(|| agent(LEAD)),
            mission: of,
            workspace: Workspace::at(repo),
            location: Location::Background,
        })
    };
    let rests = |n: u128| {
        at(EventKind::StateChanged {
            agent: agent(n),
            state: AgentState::Idle,
            detail: None,
        })
    };
    vec![
        member(LEAD, "lead", Role::Lead, None),
        rests(LEAD),
        at(EventKind::ThreadOpened {
            thread: Thread {
                id: thread_id(),
                title: "Mission: Invoicing".into(),
                mission: Some(mission_id()),
                participants: vec![Party::Human],
                opened_at: Timestamp::from_millis(1),
            },
        }),
        at(EventKind::MissionCreated {
            mission: Mission {
                id: mission_id(),
                title: "Invoicing".into(),
                kind,
                goal: "Add invoices".into(),
                repos: vec!["alpha".into()],
                requested_by: Some(agent(LEAD)),
                coordinator: None,
                thread: thread_id(),
                status: MissionStatus::Planning,
                report: None,
                created_at: Timestamp::from_millis(1),
                ended_at: None,
                assessments: BTreeMap::new(),
            },
        }),
        member(
            COORDINATOR,
            "coordinator",
            Role::Coordinator,
            Some(mission_id()),
        ),
        rests(COORDINATOR),
    ]
}

/// A daemon with a change mission in a repository that has no remote.
fn start() -> Running {
    start_kind(MissionKind::Change)
}

/// The same, for a mission of `kind`.
fn start_kind(kind: MissionKind) -> Running {
    let mut running = sandbox(false);
    let events = mission(&running.repo, kind);
    serve(&mut running, "volee", events, None);
    running
}

/// A daemon with a change mission in a repository that pushes to a remote
/// and whose pull requests the daemon's forge client merges with `merge`
/// (the repository's `merge` setting).
fn start_with_remote(merge: Option<&str>) -> Running {
    let mut running = sandbox(true);
    let events = mission(&running.repo, MissionKind::Change);
    serve(&mut running, "volee", events, merge);
    running
}

/// The repository `alpha`, and with `remote` a bare repository it pushes
/// its `main` to. No daemon runs yet.
fn sandbox(remote: bool) -> Running {
    let dir = tempfile::tempdir().unwrap();
    // Canonical, so that paths compare equal to what git reports.
    let root = dir.path().canonicalize().unwrap();
    let repo = root.join("alpha");
    repository(&repo);
    if remote {
        git(
            &root,
            &[
                "init",
                "--quiet",
                "--bare",
                "--initial-branch=main",
                "origin.git",
            ],
        );
        let origin = root.join("origin.git");
        git(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(
            &repo,
            &["push", "--quiet", "--set-upstream", "origin", "main"],
        );
    }
    Running {
        socket: PathBuf::new(),
        harness: Arc::new(FakeHarness::default()),
        forge: Arc::new(MergingForge::default()),
        repo,
        root,
        remote,
        _dir: dir,
    }
}

/// Starts a daemon on `events`, with its state in the directory `name`.
/// Git evidence is read when an agent stops and when a request asks for it;
/// the fake harness is listed every few milliseconds.
fn serve(running: &mut Running, name: &str, events: Vec<Event>, merge: Option<&str>) {
    let paths = Paths::rooted(running.root.join(name));
    std::fs::create_dir_all(&paths.state_dir).unwrap();
    running.socket.clone_from(&paths.socket);
    let listener = UnixListener::bind(&running.socket).unwrap();
    let merge = merge.map_or_else(String::new, |method| format!("merge = \"{method}\"\n"));
    let settings = volee_config::Config::parse(
        &format!(
            "[repos.alpha]\npath = \"{}\"\n{merge}",
            running.repo.display()
        ),
        Path::new("config.toml"),
        Path::new("/home/nobody"),
    )
    .unwrap();
    let cli = GitCli::new()
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "volee")
        .env("GIT_AUTHOR_EMAIL", "volee@example.com")
        .env("GIT_COMMITTER_NAME", "volee")
        .env("GIT_COMMITTER_EMAIL", "volee@example.com");
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(events)),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: Some(Arc::clone(&running.harness) as Arc<dyn Harness>),
        discovery_interval: Duration::from_millis(10),
        max_discovery_interval: Duration::from_millis(10),
        idle_discovery_interval: Duration::from_millis(10),
        evidence: EvidenceSources {
            git: Some(Arc::new(cli)),
            forge: running
                .remote
                .then(|| Arc::clone(&running.forge) as Arc<dyn Forge>),
            settings,
            // Long: what the tests see after a merge is what a check asked
            // for at that moment read, not a periodic refresh.
            refresh_interval: Duration::from_secs(3_600),
            ..EvidenceSources::default()
        },
        volee_bin: Some(PathBuf::from("/opt/volee/bin/volee")),
        multiplexers: Vec::new(),
        hosts: false,
        processes: None,
    };
    tokio::spawn(Daemon::serve(listener, config));
}

async fn events(client: &Client) -> Vec<Event> {
    let batch = client
        .call::<EventsSince>(SinceParams::default())
        .await
        .unwrap();
    batch.events.into_iter().map(|e| e.event).collect()
}

async fn registry(client: &Client) -> Registry {
    Registry::from_events(&events(client).await).unwrap()
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
    panic!(
        "the registry never reached the expected state:\n{}",
        described(&registry(client).await)
    );
}

/// The mission, its verdicts, its agents and its thread, for a wait that
/// timed out.
fn described(registry: &Registry) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    if let Some(mission) = registry.mission(mission_id()) {
        let _ = writeln!(out, "mission: {:?}", mission.status);
        for (worker, assessment) in &mission.assessments {
            let _ = writeln!(
                out,
                "  verdict on {worker}: {:?} ({})",
                assessment.verdict, assessment.evidence
            );
        }
    }
    for agent in registry.agents() {
        let _ = writeln!(
            out,
            "agent {} {} ({:?}): {:?} since {:?}, worked {}, sessions {}, delivery `{}` checked \
             {:?}",
            agent.id,
            agent.name,
            agent.role,
            agent.state,
            agent.state_since,
            agent.worked,
            agent.sessions.len(),
            agent.delivery.summary(),
            agent.delivery.checked_at
        );
    }
    for message in registry.thread_messages(thread_id()) {
        let _ = writeln!(out, "  {:?}: {}", message.from, message.body);
    }
    out
}

async fn until_status(client: &Client, status: MissionStatus) -> Registry {
    until(client, |r| {
        r.mission(mission_id())
            .is_some_and(|mission| mission.status == status)
    })
    .await
}

/// The coordinator launches a worker named `name` on `branch`.
async fn launched(client: &Client, name: &str, branch: &str) -> (AgentId, PathBuf) {
    let spawned = client
        .call::<Spawn>(SpawnParams {
            repo: "alpha".into(),
            branch: branch.into(),
            base: None,
            task: "Add invoices to the billing module.".into(),
            role: Role::Worker,
            parent: Some(agent(COORDINATOR)),
            mission: Some(mission_id()),
            model: None,
            name: Some(name.into()),
            harness: None,
        })
        .await
        .unwrap();
    (spawned.agent, PathBuf::from(&spawned.worktree))
}

/// The worker in `worktree` commits `file`.
fn commits(worktree: &Path, file: &str, content: &str) {
    std::fs::write(worktree.join(file), content).unwrap();
    git(worktree, &["add", file]);
    git(worktree, &["commit", "--quiet", "-m", "Add invoices"]);
}

/// The session of the worker of the `n`th launch (from one), in `worktree`,
/// its turn ended, as the harness lists it: under the handle the harness gave
/// that launch.
fn ended(worktree: &Path, n: usize) -> ListedSession {
    let handle = launch_handle(n);
    ListedSession {
        alive: true,
        background: true,
        handle: Some(handle.clone()),
        ..ListedSession::new(
            SessionKey::claude(format!("{handle}-5b60-4cdc-8cf3-000000000001")),
            worktree,
            AgentState::Done,
        )
    }
}

/// The coordinator launches its worker, which commits its work (and, with
/// a remote, pushes it and opens a draft pull request, as its instructions
/// say) and ends its turn. Returns the worker and its worktree.
async fn delivered_worker(daemon: &Running, client: &Client) -> (AgentId, PathBuf) {
    let (worker, worktree) = launched(client, "billing", "feat/invoices").await;
    commits(&worktree, "invoices.rs", "pub fn total() {}\n");
    if daemon.remote {
        git(
            &worktree,
            &[
                "push",
                "--quiet",
                "--set-upstream",
                "origin",
                "feat/invoices",
            ],
        );
        daemon.forge.open(PrState::Draft);
    }
    daemon.harness.set(vec![ended(&worktree, 1)]);
    (worker, worktree)
}

fn present() -> ApprovalParams {
    ApprovalParams {
        mission: mission_id(),
        by: agent(COORDINATOR),
        done: "Invoices in the billing module.".into(),
        proof: "branch feat/invoices, one commit, tests green.".into(),
        left: "nothing".into(),
    }
}

/// The mission's worker delivered and the coordinator presented its
/// report: the question the person is asked.
async fn presented(client: &Client) -> MessageId {
    until_status(client, MissionStatus::Reporting).await;
    client
        .call::<RequestApproval>(present())
        .await
        .unwrap()
        .question
}

async fn answers(client: &Client, question: MessageId, words: &str) {
    client
        .call::<SendMessage>(SendParams {
            from: Party::Human,
            to: vec![Party::Agent(agent(COORDINATOR))],
            kind: MessageKind::Answer,
            body: words.into(),
            refs: vec![],
            in_reply_to: Some(question),
            options: Vec::new(),
            thread: None,
        })
        .await
        .unwrap();
}

/// The person closes the mission themselves.
fn close_by_hand(force: bool) -> CloseParams {
    CloseParams {
        mission: mission_id(),
        report: None,
        force,
        by: None,
    }
}

/// A hook of agent `n`, as Claude Code sends it.
async fn hook(client: &Client, n: u128, event: &str) {
    client
        .call::<HookIngest>(HookParams {
            agent: agent(n),
            event: event.into(),
            payload: serde_json::json!({ "reason": "other" }),
            pid: None,
        })
        .await
        .unwrap();
}

/// The worker tells its coordinator and the person that it is done, as the
/// `report` tool does.
async fn reports(client: &Client, worker: AgentId) {
    client
        .call::<SendMessage>(SendParams {
            from: Party::Agent(worker),
            to: vec![Party::Human, Party::Agent(agent(COORDINATOR))],
            kind: MessageKind::Report,
            body: "**Done.** Invoices are on feat/invoices.".into(),
            refs: vec![],
            in_reply_to: None,
            options: Vec::new(),
            thread: None,
        })
        .await
        .unwrap();
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

/// What volee told `party` in the mission's thread.
fn told(registry: &Registry, party: Party) -> Vec<String> {
    registry
        .thread_messages(thread_id())
        .into_iter()
        .filter(|message| message.from == Party::System && message.is_for(party))
        .map(|message| message.body.clone())
        .collect()
}

/// The sentence that ends a mission, once volee said it to the person.
fn closing(registry: &Registry) -> Option<String> {
    told(registry, Party::Human)
        .into_iter()
        .find(|text| text.starts_with("Mission \"Invoicing\" is closed."))
}

/// What volee said of merges that did not go through.
fn failures(registry: &Registry) -> Vec<&Message> {
    registry
        .thread_messages(thread_id())
        .into_iter()
        .filter(|message| message.from == Party::System && message.body.contains("could not merge"))
        .collect()
}

/// The person's "ok merge" met a merge that cannot go through: the one
/// alert that says so, once the mission is open again.
async fn refused_merge(client: &Client) -> (Registry, String) {
    let reopened = until(client, |r| !failures(r).is_empty()).await;
    let [alert] = &failures(&reopened)[..] else {
        panic!("one alert: {:?}", failures(&reopened));
    };
    assert_eq!(alert.kind, MessageKind::Alert);
    // The person, the coordinator, and the lead that asked for the mission.
    assert_eq!(
        alert.recipients().collect::<Vec<_>>(),
        [
            Party::Human,
            Party::Agent(agent(COORDINATOR)),
            Party::Agent(agent(LEAD))
        ]
    );
    assert!(
        alert
            .body
            .starts_with("volee could not merge all the work of \"Invoicing\". "),
        "{}",
        alert.body
    );
    assert!(
        alert
            .body
            .contains("What was merged stays merged, and nothing was forced."),
        "{}",
        alert.body
    );
    assert_eq!(
        reopened.mission(mission_id()).unwrap().status,
        MissionStatus::Reporting,
        "the mission is open again"
    );
    let body = alert.body.clone();
    (reopened, body)
}

/// The mission closed: the coordinator's session ends, and volee says what
/// it tidied.
async fn wound_down(client: &Client) -> Registry {
    until_status(client, MissionStatus::Closed).await;
    hook(client, COORDINATOR, "session-end").await;
    until(client, |r| closing(r).is_some()).await
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One mission, from its delivery to its end, reads best as one trip.
async fn a_mission_without_a_remote_is_merged_by_volee_after_ok_merge_then_closed_and_tidied() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let (worker, worktree) = delivered_worker(&daemon, &client).await;

    // Committed work on a clean worktree is all a repository without a
    // remote can ask of a worker: it is delivered, the mission reports.
    let ready = until_status(&client, MissionStatus::Reporting).await;
    let assessment = &ready.mission(mission_id()).unwrap().assessments[&worker];
    assert_eq!(assessment.verdict, Verdict::Delivered);
    assert_eq!(assessment.evidence, "1 commit \u{b7} no remote");
    assert!(ready.agent(worker).unwrap().delivery.no_remote());
    let asked_for = told(&ready, Party::Agent(agent(COORDINATOR)));
    assert!(
        asked_for
            .iter()
            .any(|text| text.contains("`request_approval`")
                && text.contains("You merge nothing and close nothing: volee does")),
        "{asked_for:?}"
    );

    // The coordinator presents its report: to the person, who decides, and
    // to the lead that asked for the mission, as news.
    let asked = client.call::<RequestApproval>(present()).await.unwrap();
    let waiting = registry(&client).await;
    let question = waiting.message(asked.question).unwrap();
    assert_eq!(
        question.recipients().collect::<Vec<_>>(),
        [Party::Human, Party::Agent(agent(LEAD))]
    );
    assert_eq!(question.options, ["ok merge", "no"]);
    assert_eq!(
        waiting.mission(mission_id()).unwrap().status,
        MissionStatus::AwaitingApproval
    );

    // Until the person answers, nothing is merged, however long it takes;
    // and no agent closes the mission in their place.
    let start = git(&daemon.repo, &["rev-parse", "HEAD"]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(git(&daemon.repo, &["rev-parse", "HEAD"]), start);
    assert!(!daemon.repo.join("invoices.rs").exists());
    let by_the_coordinator = CloseParams {
        by: Some(agent(COORDINATOR)),
        ..close_by_hand(false)
    };
    match client.call::<CloseMission>(by_the_coordinator).await {
        Err(ClientError::Rpc(error)) => {
            assert_eq!(error.code, rpc::REFUSED, "{}", error.message);
            assert!(
                error.message.contains("an agent does not close a mission"),
                "{}",
                error.message
            );
        }
        other => panic!("expected a refusal, got {other:?}"),
    }

    // A file the person left in the main checkout, which git does not
    // track, is not in the way and is not touched.
    std::fs::write(daemon.repo.join("notes.txt"), "mine\n").unwrap();
    // Their answer wakes the coordinator, which starts a turn.
    hook(&client, COORDINATOR, "user-prompt-submit").await;
    answers(&client, asked.question, "ok merge").await;

    // volee merges: a merge commit on `main`, in the main checkout, that
    // keeps the worker's commit; then it closes the mission.
    let closed = until_status(&client, MissionStatus::Closed).await;
    assert_eq!(
        git(&daemon.repo, &["log", "-1", "--format=%s"]),
        "Merge branch 'feat/invoices'"
    );
    let parents = git(&daemon.repo, &["rev-list", "--parents", "-n", "1", "HEAD"]);
    let parents: Vec<&str> = parents.split_whitespace().collect();
    assert_eq!(parents.len(), 3, "a merge commit: {parents:?}");
    assert_eq!(parents[1], start, "on top of what main was");
    assert_eq!(
        parents[2],
        git(&daemon.repo, &["rev-parse", "feat/invoices"]),
        "of the worker's branch"
    );
    assert_eq!(git(&daemon.repo, &["branch", "--show-current"]), "main");
    assert_eq!(
        git(&daemon.repo, &["status", "--porcelain"]),
        "?? notes.txt"
    );
    assert_eq!(
        std::fs::read_to_string(daemon.repo.join("invoices.rs")).unwrap(),
        "pub fn total() {}\n",
        "the work is in the base branch"
    );
    assert_eq!(
        closed.mission(mission_id()).unwrap().report,
        Some(asked.question),
        "the report the person approved"
    );
    // The coordinator was told that the merge is not its own to make.
    let heard = told(&closed, Party::Agent(agent(COORDINATOR)));
    assert!(
        heard.iter().any(|text| text.contains("The human approved")
            && text.contains("volee merges every worker's work itself")),
        "{heard:?}"
    );
    assert_eq!(failures(&closed), Vec::<&Message>::new());

    // The turn the answer woke is the coordinator's own: while it lasts,
    // the coordinator is not stopped and the last word is not said.
    until(&client, |r| r.agent(worker).is_none()).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    let during = registry(&client).await;
    assert_eq!(
        during.agent(agent(COORDINATOR)).map(|agent| &agent.state),
        Some(&AgentState::Working)
    );
    assert_eq!(
        closing(&during),
        None,
        "said before the coordinator's turn ended"
    );
    // Its turn ends, then its session: it is forgotten.
    hook(&client, COORDINATOR, "stop").await;
    hook(&client, COORDINATOR, "session-end").await;
    let done = until(&client, |r| closing(r).is_some()).await;
    assert!(done.agent(worker).is_none(), "the worker left the list");
    assert!(
        done.agent(agent(COORDINATOR)).is_none(),
        "the coordinator left the list"
    );
    assert!(done.agent(agent(LEAD)).is_some(), "the lead stays");
    assert!(!worktree.exists(), "the worker's worktree is gone");
    assert!(
        branch_exists(&daemon.repo, "refs/heads/feat/invoices"),
        "its branch is kept"
    );
    // One sentence says it all, to the person and to the lead.
    let said: Vec<String> = told(&done, Party::Human)
        .into_iter()
        .filter(|text| text.starts_with("Mission \"Invoicing\" is closed."))
        .collect();
    assert_eq!(
        said,
        [
            "Mission \"Invoicing\" is closed. Merged: billing (feat/invoices into main). Tidied: \
         billing (worktrees removed, branches kept)."
        ]
    );
    assert_eq!(
        told(&done, Party::Agent(agent(LEAD)))
            .iter()
            .filter(|text| text.starts_with("Mission \"Invoicing\" is closed."))
            .count(),
        1
    );
    // The harness was asked to delete the worker's session with the
    // evidence that its work landed, and nothing else.
    assert!(
        daemon
            .harness
            .actions()
            .contains(&("remove-delivered".to_owned(), launch_handle(1))),
        "{:?}",
        daemon.harness.actions()
    );
}

#[tokio::test]
async fn a_main_checkout_with_uncommitted_changes_is_left_alone_until_the_person_cleaned_it() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let (worker, worktree) = delivered_worker(&daemon, &client).await;
    let asked = presented(&client).await;
    // The person is in the middle of something in the main checkout.
    std::fs::write(daemon.repo.join("README.md"), "alpha, by hand\n").unwrap();
    let start = git(&daemon.repo, &["rev-parse", "HEAD"]);
    answers(&client, asked, "ok merge").await;

    let (reopened, alert) = refused_merge(&client).await;
    let why = format!(
        "billing: cannot merge `feat/invoices` in `{}`: it has uncommitted changes (1 file): \
         commit or stash them first",
        daemon.repo.display()
    );
    assert!(alert.contains(&why), "{alert}");
    // Nothing moved: not the branch, not their change, not the worker.
    assert_eq!(git(&daemon.repo, &["rev-parse", "HEAD"]), start);
    assert_eq!(
        std::fs::read_to_string(daemon.repo.join("README.md")).unwrap(),
        "alpha, by hand\n"
    );
    assert!(!daemon.repo.join("invoices.rs").exists());
    assert!(worktree.is_dir());
    assert!(reopened.agent(worker).is_some());
    assert_eq!(closing(&reopened), None);

    // They commit their change. The answer they gave approved a merge that
    // did not happen: the coordinator presents its report again, and they
    // approve again.
    git(&daemon.repo, &["commit", "--quiet", "-am", "Edit by hand"]);
    let again = client.call::<RequestApproval>(present()).await.unwrap();
    assert_ne!(again.question, asked);
    answers(&client, again.question, "ok merge").await;
    let done = wound_down(&client).await;
    assert_eq!(
        closing(&done).unwrap(),
        "Mission \"Invoicing\" is closed. Merged: billing (feat/invoices into main). Tidied: \
         billing (worktrees removed, branches kept)."
    );
    assert!(daemon.repo.join("invoices.rs").is_file());
    assert_eq!(
        std::fs::read_to_string(daemon.repo.join("README.md")).unwrap(),
        "alpha, by hand\n"
    );
    assert_eq!(failures(&done).len(), 1, "said once");
}

#[tokio::test]
async fn a_main_checkout_on_another_branch_is_not_merged_into() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let (_, worktree) = delivered_worker(&daemon, &client).await;
    let asked = presented(&client).await;
    git(&daemon.repo, &["switch", "--quiet", "-c", "wip"]);
    let main = git(&daemon.repo, &["rev-parse", "main"]);
    answers(&client, asked, "ok merge").await;

    let (_, alert) = refused_merge(&client).await;
    assert!(
        alert.contains("it has `wip` checked out, not `main`: check `main` out there first"),
        "{alert}"
    );
    // volee did not switch branches for them, and merged nowhere.
    assert_eq!(git(&daemon.repo, &["branch", "--show-current"]), "wip");
    assert_eq!(git(&daemon.repo, &["rev-parse", "main"]), main);
    assert_eq!(git(&daemon.repo, &["rev-parse", "wip"]), main);
    assert!(worktree.is_dir());
}

#[tokio::test]
async fn a_conflict_abandons_the_merge_and_leaves_the_main_checkout_as_it_was() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let (_, worktree) = delivered_worker(&daemon, &client).await;
    let asked = presented(&client).await;
    // Meanwhile `main` got a file of the same name, written differently.
    commits(&daemon.repo, "invoices.rs", "pub fn total() -> u32 { 0 }\n");
    let start = git(&daemon.repo, &["rev-parse", "HEAD"]);
    answers(&client, asked, "ok merge").await;

    let (_, alert) = refused_merge(&client).await;
    assert!(
        alert.contains(
            "git could not make the merge and it was abandoned, the checkout is as it was"
        ),
        "{alert}"
    );
    // No merge is left half done.
    assert_eq!(git(&daemon.repo, &["rev-parse", "HEAD"]), start);
    assert_eq!(git(&daemon.repo, &["status", "--porcelain"]), "");
    assert!(!daemon.repo.join(".git/MERGE_HEAD").exists());
    assert_eq!(
        std::fs::read_to_string(daemon.repo.join("invoices.rs")).unwrap(),
        "pub fn total() -> u32 { 0 }\n"
    );
    assert!(worktree.is_dir());
}

#[tokio::test]
async fn what_was_merged_stays_merged_when_another_merge_fails() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    // Two workers add the same file, each its own way: whichever is merged
    // second conflicts with the first.
    let (billing, billing_tree) = launched(&client, "billing", "feat/invoices").await;
    let (taxes, taxes_tree) = launched(&client, "taxes", "feat/taxes").await;
    commits(&billing_tree, "invoices.rs", "// by billing\n");
    commits(&taxes_tree, "invoices.rs", "// by taxes\n");
    daemon
        .harness
        .set(vec![ended(&billing_tree, 1), ended(&taxes_tree, 2)]);
    let asked = presented(&client).await;
    answers(&client, asked, "ok merge").await;

    let (reopened, alert) = refused_merge(&client).await;
    let landed = |n: AgentId| reopened.agent(n).unwrap().delivery.merged_into_base;
    let (merged, left) = match (landed(billing), landed(taxes)) {
        (true, false) => ("billing", "taxes"),
        (false, true) => ("taxes", "billing"),
        other => panic!("one merge and one failure, got {other:?}: {alert}"),
    };
    // The alert names the one that failed, and only that one.
    assert!(alert.contains(&format!("{left}: cannot merge")), "{alert}");
    assert!(!alert.contains(&format!("{merged}: ")), "{alert}");
    assert_eq!(
        std::fs::read_to_string(daemon.repo.join("invoices.rs")).unwrap(),
        format!("// by {merged}\n")
    );
    assert_eq!(git(&daemon.repo, &["status", "--porcelain"]), "");

    // The person ends the mission as it is: the worker whose work landed is
    // tidied, the other is kept with its work.
    client
        .call::<CloseMission>(close_by_hand(true))
        .await
        .unwrap();
    let done = wound_down(&client).await;
    let said = closing(&done).unwrap();
    let branch = if merged == "billing" {
        "feat/invoices"
    } else {
        "feat/taxes"
    };
    assert!(
        said.contains(&format!(
            "Merged: {merged} ({branch} into main). Tidied: {merged} (worktrees removed, \
             branches kept)."
        )),
        "{said}"
    );
    assert!(
        said.contains(&format!("Kept: {left}: its work has not landed")),
        "{said}"
    );
    let kept_tree = if left == "taxes" {
        &taxes_tree
    } else {
        &billing_tree
    };
    assert!(kept_tree.is_dir());
}

#[tokio::test]
async fn a_pull_request_is_merged_on_the_forge_with_the_repository_s_method() {
    let daemon = start_with_remote(Some("squash"));
    let client = Client::connect(&daemon.socket).await.unwrap();
    let (worker, worktree) = delivered_worker(&daemon, &client).await;
    let ready = until_status(&client, MissionStatus::Reporting).await;
    assert_eq!(
        ready.mission(mission_id()).unwrap().assessments[&worker].evidence,
        "1 commit \u{b7} pushed \u{b7} PR #7 draft, checks passing"
    );
    let asked = client.call::<RequestApproval>(present()).await.unwrap();
    assert_eq!(daemon.forge.merges(), [], "nothing before the answer");

    // The forge refuses first: its own rules still apply.
    daemon.forge.refuse(Some(
        "Pull request is not mergeable: the base branch policy prohibits the merge.",
    ));
    answers(&client, asked.question, "ok merge").await;
    let (_, alert) = refused_merge(&client).await;
    assert!(
        alert.contains(&format!(
            "billing: `gh pr merge {PULL_REQUEST}` failed with exit code 1: Pull request is \
             not mergeable: the base branch policy prohibits the merge."
        )),
        "{alert}"
    );
    let squash = (PULL_REQUEST.to_owned(), Some(MergeMethod::Squash));
    assert_eq!(daemon.forge.merges(), std::slice::from_ref(&squash));
    assert!(worktree.is_dir());

    // What blocked is settled; the report is presented and approved again.
    daemon.forge.refuse(None);
    let again = client.call::<RequestApproval>(present()).await.unwrap();
    answers(&client, again.question, "ok merge").await;
    let done = wound_down(&client).await;
    assert_eq!(daemon.forge.merges(), [squash.clone(), squash]);
    assert_eq!(
        closing(&done).unwrap(),
        "Mission \"Invoicing\" is closed. Merged: billing (feat/invoices into main, PR #7). \
         Tidied: billing (worktrees removed, branches kept)."
    );
    assert!(done.agent(worker).is_none());
    assert!(!worktree.exists());
    assert!(branch_exists(&daemon.repo, "refs/heads/feat/invoices"));
    // The merge is the forge's: volee merged nothing in the main checkout.
    assert!(!daemon.repo.join("invoices.rs").exists());
    assert_eq!(git(&daemon.repo, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn without_a_setting_the_forge_chooses_among_the_methods_the_repository_allows() {
    let daemon = start_with_remote(None);
    let client = Client::connect(&daemon.socket).await.unwrap();
    delivered_worker(&daemon, &client).await;
    let asked = presented(&client).await;
    answers(&client, asked, "ok merge").await;
    wound_down(&client).await;
    assert_eq!(daemon.forge.merges(), [(PULL_REQUEST.to_owned(), None)]);
}

#[tokio::test]
async fn a_pull_request_the_person_merged_themselves_is_not_merged_again() {
    let daemon = start_with_remote(None);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let (worker, _) = delivered_worker(&daemon, &client).await;
    let asked = presented(&client).await;
    // They read the report, merge on the forge, then answer. Nothing has
    // read the forge since the worker stopped.
    daemon.forge.merged_by_hand();
    assert!(
        !registry(&client)
            .await
            .agent(worker)
            .unwrap()
            .delivery
            .is_delivered()
    );
    answers(&client, asked, "ok merge").await;
    let done = wound_down(&client).await;
    // volee read the forge before it merged anything: there was nothing
    // left to merge.
    assert_eq!(daemon.forge.merges(), []);
    assert_eq!(failures(&done), Vec::<&Message>::new());
    assert_eq!(
        closing(&done).unwrap(),
        "Mission \"Invoicing\" is closed. Merged: billing (feat/invoices into main, PR #7). \
         Tidied: billing (worktrees removed, branches kept)."
    );
}

#[tokio::test]
async fn a_merge_the_evidence_does_not_show_is_not_a_merge() {
    let daemon = start_with_remote(None);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let (worker, worktree) = delivered_worker(&daemon, &client).await;
    let asked = presented(&client).await;
    // The forge answers that it merged, and the pull request stays open.
    daemon.forge.accept_and_do_nothing();
    answers(&client, asked, "ok merge").await;
    let (reopened, alert) = refused_merge(&client).await;
    assert_eq!(daemon.forge.merges().len(), 1);
    assert!(
        alert.contains("billing: volee sees no merge of its branch"),
        "{alert}"
    );
    // Not closed, and nothing tidied, on the forge's word alone.
    assert!(reopened.agent(worker).is_some());
    assert!(worktree.is_dir());
    assert_eq!(closing(&reopened), None);
}

#[tokio::test]
async fn an_approved_mission_the_daemon_did_not_finish_is_merged_when_it_starts_again() {
    let mut daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let (worker, worktree) = delivered_worker(&daemon, &client).await;
    let asked = presented(&client).await;
    // The daemon stops as the person answers: their answer and what it
    // decides are recorded, and nothing was merged.
    let mut log = events(&client).await;
    client.call::<Shutdown>(Empty {}).await.unwrap();
    drop(client);
    let at = Timestamp::from_millis(10_000);
    log.extend([
        Event::new(
            at,
            EventKind::MessagePosted {
                message: Message {
                    id: MessageId::from_ulid(Ulid::from_parts(9_000, 1)),
                    thread: thread_id(),
                    from: Party::Human,
                    kind: MessageKind::Answer,
                    body: "ok merge".into(),
                    refs: Vec::new(),
                    in_reply_to: Some(asked),
                    options: Vec::new(),
                    at,
                    receipts: vec![Receipt {
                        to: Party::Agent(agent(COORDINATOR)),
                        delivered_at: None,
                        read_at: None,
                    }],
                },
            },
        ),
        Event::new(
            at,
            EventKind::MissionUpdated {
                mission: mission_id(),
                status: MissionStatus::Approved,
                coordinator: None,
                report: None,
            },
        ),
    ]);
    assert!(!daemon.repo.join("invoices.rs").exists());

    serve(&mut daemon, "volee-again", log, None);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let done = wound_down(&client).await;
    assert!(daemon.repo.join("invoices.rs").is_file());
    assert_eq!(
        git(&daemon.repo, &["log", "-1", "--format=%s"]),
        "Merge branch 'feat/invoices'"
    );
    assert_eq!(
        closing(&done).unwrap(),
        "Mission \"Invoicing\" is closed. Merged: billing (feat/invoices into main). Tidied: \
         billing (worktrees removed, branches kept)."
    );
    assert!(done.agent(worker).is_none());
    assert!(!worktree.exists());
}

#[tokio::test]
async fn a_worker_whose_work_did_not_land_is_kept_when_the_person_closes_its_mission() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let (worker, worktree) = delivered_worker(&daemon, &client).await;
    until_status(&client, MissionStatus::Reporting).await;
    // It said that it is done: in a change mission, that proves nothing
    // about where its commits are.
    reports(&client, worker).await;
    // The person closes the mission themselves, without a merge: the
    // worker's work has not landed.
    client
        .call::<CloseMission>(close_by_hand(false))
        .await
        .unwrap();
    let done = until(&client, |r| closing(r).is_some()).await;
    let said = closing(&done).unwrap();
    assert!(
        said.contains("Kept: billing: its work has not landed"),
        "{said}"
    );
    assert!(!said.contains("Tidied"), "{said}");
    assert!(!said.contains("Merged"), "{said}");
    assert!(worktree.is_dir(), "its worktree is kept");
    assert!(done.agent(worker).is_some(), "and so is its row");
    assert!(branch_exists(&daemon.repo, "refs/heads/feat/invoices"));
    // Closing by hand merges nothing.
    assert!(!daemon.repo.join("invoices.rs").exists());
}

#[tokio::test]
async fn the_person_who_merged_and_closes_themselves_gets_the_same_tidying() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let (worker, worktree) = delivered_worker(&daemon, &client).await;
    until_status(&client, MissionStatus::Reporting).await;
    // No report presented, no coordinator involved: the person merges the
    // branch and closes the mission. Nothing has read the merge yet.
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
    assert!(
        !registry(&client)
            .await
            .agent(worker)
            .unwrap()
            .delivery
            .merged_into_base
    );
    client
        .call::<CloseMission>(close_by_hand(false))
        .await
        .unwrap();
    // The coordinator rested: it is stopped at once, without a turn to wait
    // for; here its session ends when the test says so.
    hook(&client, COORDINATOR, "session-end").await;
    let done = until(&client, |r| closing(r).is_some()).await;
    assert!(done.agent(worker).is_none(), "the worker left the list");
    assert!(
        done.agent(agent(COORDINATOR)).is_none(),
        "the coordinator left the list"
    );
    assert!(!worktree.exists(), "its worktree is gone");
    assert!(branch_exists(&daemon.repo, "refs/heads/feat/invoices"));
    assert_eq!(
        closing(&done).unwrap(),
        "Mission \"Invoicing\" is closed. Merged: billing (feat/invoices into main). Tidied: \
         billing (worktrees removed, branches kept)."
    );
}

#[tokio::test]
async fn a_coordinator_in_a_turn_is_stopped_at_once_when_the_person_closes_by_hand() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    delivered_worker(&daemon, &client).await;
    until_status(&client, MissionStatus::Reporting).await;
    hook(&client, COORDINATOR, "user-prompt-submit").await;
    client
        .call::<CloseMission>(close_by_hand(false))
        .await
        .unwrap();
    // Its turn is not waited for: the person decided.
    let done = tokio::time::timeout(Duration::from_secs(10), async {
        until(&client, |r| closing(r).is_some()).await
    })
    .await
    .expect("the wind-down does not wait for the coordinator's turn");
    assert!(closing(&done).is_some());
}

#[tokio::test]
async fn an_analysis_mission_is_closed_by_volee_on_ok_close_and_tidied_on_its_reports() {
    let daemon = start_kind(MissionKind::Analysis);
    let client = Client::connect(&daemon.socket).await.unwrap();
    // The worker looks, changes nothing, and reports.
    let (worker, worktree) = launched(&client, "audit", "audit/invoices").await;
    reports(&client, worker).await;
    daemon.harness.set(vec![ended(&worktree, 1)]);
    until_status(&client, MissionStatus::Reporting).await;

    let asked = client.call::<RequestApproval>(present()).await.unwrap();
    let waiting = registry(&client).await;
    let question = waiting.message(asked.question).unwrap();
    assert_eq!(question.options, ["ok close", "no"]);
    assert!(
        question
            .body
            .starts_with("Mission \"Invoicing\" is ready to close."),
        "{}",
        question.body
    );
    // The approval of a change mission is not this mission's.
    answers(&client, asked.question, "ok merge").await;
    assert_eq!(
        registry(&client)
            .await
            .mission(mission_id())
            .unwrap()
            .status,
        MissionStatus::Reporting
    );
    let again = client.call::<RequestApproval>(present()).await.unwrap();
    let start = git(&daemon.repo, &["rev-parse", "HEAD"]);
    answers(&client, again.question, "ok close").await;
    // Nothing to merge: volee closes.
    let done = wound_down(&client).await;
    assert_eq!(git(&daemon.repo, &["rev-parse", "HEAD"]), start);
    let heard = told(&done, Party::Agent(agent(COORDINATOR)));
    assert!(
        heard
            .iter()
            .any(|text| text.contains("volee closes the mission and stops you")),
        "{heard:?}"
    );
    // Its report is what an analysis worker delivers: its worktree goes.
    assert!(done.agent(worker).is_none(), "the worker left the list");
    assert!(!worktree.exists(), "its worktree is gone");
    assert!(branch_exists(&daemon.repo, "refs/heads/audit/invoices"));
    assert_eq!(
        closing(&done).unwrap(),
        "Mission \"Invoicing\" is closed. Tidied: audit (worktrees removed, branches kept)."
    );
}

#[tokio::test]
async fn a_worker_that_just_started_is_not_judged_before_it_took_its_task() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let (worker, worktree) = launched(&client, "billing", "feat/invoices").await;
    let listed = |state: AgentState| {
        daemon.harness.set(vec![ListedSession {
            state,
            ..ended(&worktree, 1)
        }]);
    };
    // Its session starts: it rests at its prompt for a moment, before its
    // task is submitted. The evidence of that moment is read.
    listed(AgentState::Idle);
    let started = until(&client, |r| {
        r.agent(worker).is_some_and(|agent| {
            agent.state == AgentState::Idle
                && agent
                    .delivery
                    .checked_at
                    .is_some_and(|checked| checked >= agent.state_since)
        })
    })
    .await;
    // Nothing is said of it: it has not worked yet.
    assert!(
        started
            .mission(mission_id())
            .unwrap()
            .assessments
            .is_empty(),
        "{:?}",
        started.mission(mission_id()).unwrap().assessments
    );
    assert_eq!(
        told(&started, Party::Agent(agent(COORDINATOR))),
        Vec::<String>::new()
    );

    // It takes its task, works, and ends its turn without a commit: now it
    // did stop without delivering, and the coordinator hears it.
    listed(AgentState::Working);
    until(&client, |r| {
        r.agent(worker)
            .is_some_and(|agent| agent.state == AgentState::Working)
    })
    .await;
    listed(AgentState::Idle);
    let stopped = until(&client, |r| {
        r.mission(mission_id())
            .is_some_and(|mission| mission.assessments.contains_key(&worker))
    })
    .await;
    assert_eq!(
        stopped.mission(mission_id()).unwrap().assessments[&worker].verdict,
        Verdict::NotDelivered
    );
    let heard = told(&stopped, Party::Agent(agent(COORDINATOR)));
    assert!(
        heard
            .iter()
            .any(|text| text.contains("stopped without delivering: no commits")),
        "{heard:?}"
    );
    assert!(worktree.is_dir());
}
