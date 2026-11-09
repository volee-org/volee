//! The daemon keeps workspaces and delivery evidence fresh from git and the
//! forge: real temporary repositories (a bare repository plays `origin`), a
//! fake forge, and a hermetic git environment. Nothing reaches the network
//! or the developer's home directory.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::UnixListener;
use tokio::task::JoinHandle;
use ulid::Ulid;
use volee_config::Fetch;
use volee_core::delivery::{ChecksState, PrState, PullRequest, PushState};
use volee_core::harness::HarnessKind;
use volee_core::message::{Party, Thread};
use volee_core::mission::{Mission, MissionKind, MissionStatus, Verdict};
use volee_core::role::{LaunchedBy, Role};
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};
use volee_core::{AgentId, Event, EventKind, MissionId, Registry, ThreadId};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_git::{BranchHead, Forge, ForgeStatus, GitCli, GitError};
use volee_proto::client::Client;
use volee_proto::methods::{
    EventsSince, HookIngest, HookParams, SinceParams, Status, StatusParams,
};
use volee_proto::paths::Paths;

/// How long the refresher may take under a loaded machine.
const PATIENCE: Duration = Duration::from_secs(90);

const COORDINATOR: u128 = 10;
const WORKER: u128 = 11;

fn agent(n: u128) -> AgentId {
    AgentId::from_ulid(Ulid::from_parts(1_000, n))
}

fn mission_id() -> MissionId {
    MissionId::from_ulid(Ulid::from_parts(4_000, 1))
}

fn thread_id() -> ThreadId {
    ThreadId::from_ulid(Ulid::from_parts(2_000, 1))
}

fn at(kind: EventKind) -> Event {
    Event::new(Timestamp::from_millis(1), kind)
}

/// Temporary repositories, with every git command kept away from the
/// developer's configuration and home directory.
struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        // git prints real paths: `/var` is a link to `/private/var` on macOS.
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("home/.config")).unwrap();
        Self { _dir: dir, root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn env(&self) -> Vec<(&'static str, OsString)> {
        let home = self.root.join("home");
        vec![
            ("GIT_CONFIG_GLOBAL", "/dev/null".into()),
            ("GIT_CONFIG_NOSYSTEM", "1".into()),
            ("HOME", home.clone().into()),
            ("XDG_CONFIG_HOME", home.join(".config").into()),
            ("GIT_CEILING_DIRECTORIES", self.root.clone().into()),
            ("LC_ALL", "C".into()),
        ]
    }

    /// The `GitCli` the daemon uses, in the sandbox's environment.
    fn cli(&self) -> GitCli {
        self.env()
            .into_iter()
            .fold(GitCli::new(), |cli, (key, value)| cli.env(key, value))
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .envs(self.env())
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn configure(&self, repo: &Path) {
        self.git(repo, &["config", "user.name", "Volee Tests"]);
        self.git(repo, &["config", "user.email", "tests@volee.invalid"]);
    }

    fn commit(&self, repo: &Path, file: &str, contents: &str) -> String {
        std::fs::write(repo.join(file), contents).unwrap();
        self.git(repo, &["add", "--", file]);
        self.git(
            repo,
            &["commit", "--quiet", "--message", &format!("Add {file}")],
        );
        self.git(repo, &["rev-parse", "HEAD"])
    }

    /// A clone of a bare `origin` with one commit on `main`, and a worktree
    /// on a new branch `feat/x` from `origin/main`, as volee creates them.
    /// Returns the clone and the worktree.
    fn project(&self) -> (PathBuf, PathBuf) {
        let root = self.root.clone();
        self.git(
            &root,
            &[
                "init",
                "--quiet",
                "--bare",
                "--initial-branch=main",
                "origin.git",
            ],
        );
        self.git(&root, &["init", "--quiet", "--initial-branch=main", "seed"]);
        let seed = self.path("seed");
        self.configure(&seed);
        self.commit(&seed, "README.md", "# Billing\n");
        self.git(&seed, &["remote", "add", "origin", "../origin.git"]);
        self.git(&seed, &["push", "--quiet", "origin", "main"]);
        self.git(&root, &["clone", "--quiet", "origin.git", "billing"]);
        let clone = self.path("billing");
        self.configure(&clone);
        let worktree = self.path("worktrees/feat-x");
        self.git(
            &clone,
            &[
                "worktree",
                "add",
                "--quiet",
                "--no-track",
                "-b",
                "feat/x",
                worktree.to_str().unwrap(),
                "origin/main",
            ],
        );
        (clone, worktree)
    }

    /// A commit someone else pushed to `origin`'s `main` after the project
    /// was cloned: only a fetch brings it to the clone.
    fn pushed_by_a_teammate(&self) -> String {
        let root = self.root.clone();
        self.git(&root, &["clone", "--quiet", "origin.git", "teammate"]);
        let teammate = self.path("teammate");
        self.configure(&teammate);
        let pushed = self.commit(&teammate, "NEWS.md", "News\n");
        self.git(&teammate, &["push", "--quiet", "origin", "main"]);
        pushed
    }
}

/// A forge that answers what the test says, and records what it was asked.
#[derive(Default)]
struct FakeForge {
    pull_request: Mutex<Option<PullRequest>>,
    /// The tips the daemon asked about, with the origin it named.
    asked: Mutex<Vec<(String, Option<String>)>>,
}

impl FakeForge {
    /// From now on, the branch has an open pull request at whatever tip the
    /// daemon asks about, as a forge answers once the branch is pushed.
    fn open(&self, number: u64) {
        *self.pull_request.lock().unwrap() = Some(PullRequest {
            number,
            url: format!("https://example.com/pull/{number}"),
            state: PrState::Open,
            checks: ChecksState::Passing,
            head: None,
        });
    }

    fn asked(&self) -> Vec<(String, Option<String>)> {
        self.asked.lock().unwrap().clone()
    }
}

impl Forge for FakeForge {
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
        self.asked
            .lock()
            .unwrap()
            .push((head.tip.to_owned(), head.origin.map(ToString::to_string)));
        let found = self.pull_request.lock().unwrap().clone();
        Ok(found.map(|pr| PullRequest {
            head: Some(head.tip.to_owned()),
            ..pr
        }))
    }
}

fn registered(n: u128, role: Role, cwd: &Path, mission: Option<MissionId>) -> Event {
    registered_by(n, role, cwd, mission, LaunchedBy::Human)
}

fn registered_by(
    n: u128,
    role: Role,
    cwd: &Path,
    mission: Option<MissionId>,
    launched_by: LaunchedBy,
) -> Event {
    at(EventKind::AgentRegistered {
        agent: agent(n),
        name: format!("agent-{n}"),
        role,
        harness: HarnessKind::ClaudeCode,
        launched_by,
        parent: None,
        mission,
        workspace: Workspace::at(cwd),
        location: Location::Background,
    })
}

fn change_mission() -> Vec<Event> {
    vec![
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
                kind: MissionKind::Change,
                goal: "Add invoices".into(),
                repos: vec!["billing".into()],
                requested_by: None,
                coordinator: None,
                thread: thread_id(),
                status: MissionStatus::Planning,
                report: None,
                created_at: Timestamp::from_millis(1),
                ended_at: None,
                assessments: BTreeMap::new(),
            },
        }),
    ]
}

struct Running {
    socket: PathBuf,
    _task: JoinHandle<Result<(), volee_daemon::DaemonError>>,
    _dir: tempfile::TempDir,
}

fn start(events: Vec<Event>, clock: Arc<ManualClock>, evidence: EvidenceSources) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(events)),
        clock,
        harnesses: vec![],
        spool: None,
        harness: None,
        discovery_interval: volee_daemon::DEFAULT_DISCOVERY_INTERVAL,
        max_discovery_interval: volee_daemon::DEFAULT_DISCOVERY_INTERVAL,
        idle_discovery_interval: volee_daemon::DEFAULT_DISCOVERY_INTERVAL,
        evidence,
        volee_bin: None,
        multiplexers: Vec::new(),
        hosts: false,
    };
    Running {
        socket,
        _task: tokio::spawn(Daemon::serve(listener, config)),
        _dir: dir,
    }
}

async fn events(client: &Client) -> Vec<Event> {
    client
        .call::<EventsSince>(SinceParams::default())
        .await
        .unwrap()
        .events
        .into_iter()
        .map(|seq| seq.event)
        .collect()
}

/// Waits until `check` finds what it looks for in the daemon's events.
async fn eventually<T>(
    client: &Client,
    what: &str,
    check: impl Fn(&Registry, &[Event]) -> Option<T>,
) -> T {
    let started = Instant::now();
    loop {
        let events = events(client).await;
        let registry = Registry::from_events(&events).unwrap();
        if let Some(found) = check(&registry, &events) {
            return found;
        }
        assert!(started.elapsed() < PATIENCE, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn settings(clone: &Path) -> volee_config::Config {
    let text = format!(
        "[repos.billing]\npath = \"{}\"\nbase = \"main\"\n",
        clone.display()
    );
    volee_config::Config::parse(&text, Path::new("config.toml"), Path::new("/nowhere")).unwrap()
}

/// Whether the daemon fetched the repository of one agent, launched by
/// `launched_by`, before its first evidence, with `[git] fetch` set to
/// `fetch`. The fetch, when allowed, runs before the evidence is read.
async fn fetched_before_the_first_check(fetch: Fetch, launched_by: LaunchedBy) -> bool {
    let sandbox = Sandbox::new();
    let (clone, worktree) = sandbox.project();
    let pushed = sandbox.pushed_by_a_teammate();
    let mut settings = settings(&clone);
    settings.git.fetch = fetch;
    let evidence = EvidenceSources {
        settings,
        ..EvidenceSources::git(Arc::new(sandbox.cli()))
    };
    let log = vec![registered_by(1, Role::Lead, &worktree, None, launched_by)];
    let daemon = start(log, Arc::new(ManualClock::at(10_000)), evidence);
    let client = Client::connect(&daemon.socket).await.unwrap();
    eventually(&client, "a first check", |registry, _| {
        registry.agent(agent(1))?.delivery.checked_at
    })
    .await;
    sandbox.git(&clone, &["rev-parse", "origin/main"]) == pushed
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_repositories_the_person_allows_are_fetched() {
    // By default, the repositories of agents volee launched, and only those.
    assert!(fetched_before_the_first_check(Fetch::Launched, LaunchedBy::Human).await);
    assert!(!fetched_before_the_first_check(Fetch::Launched, LaunchedBy::Outside).await);
    assert!(fetched_before_the_first_check(Fetch::All, LaunchedBy::Outside).await);
    assert!(!fetched_before_the_first_check(Fetch::Never, LaunchedBy::Human).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_registered_in_a_worktree_gets_its_workspace_and_evidence() {
    let sandbox = Sandbox::new();
    let (clone, worktree) = sandbox.project();
    let start_commit = sandbox.git(&clone, &["rev-parse", "origin/main"]);
    let evidence = EvidenceSources {
        settings: settings(&clone),
        ..EvidenceSources::git(Arc::new(sandbox.cli()))
    };
    let daemon = start(
        vec![registered(1, Role::Lead, &worktree, None)],
        Arc::new(ManualClock::at(10_000)),
        evidence,
    );
    let client = Client::connect(&daemon.socket).await.unwrap();

    let workspace = eventually(&client, "the workspace", |registry, _| {
        let workspace = &registry.agent(agent(1))?.workspace;
        workspace.base.is_some().then(|| workspace.clone())
    })
    .await;
    assert_eq!(
        workspace,
        Workspace {
            cwd: worktree.clone(),
            // The name from the configuration, which is also the folder's.
            repo: Some("billing".into()),
            repo_root: Some(clone.clone()),
            worktree: Some(worktree.clone()),
            branch: Some("feat/x".into()),
            base: Some("main".into()),
            start_commit: None,
        }
    );

    let delivery = eventually(&client, "the evidence", |registry, _| {
        let delivery = &registry.agent(agent(1))?.delivery;
        delivery.checked_at.map(|_| delivery.clone())
    })
    .await;
    assert_eq!(delivery.commits, 0);
    assert_eq!(delivery.tip.as_deref(), Some(start_commit.as_str()));
    assert_eq!(delivery.push, PushState::NoUpstream);
    assert_eq!(delivery.pull_request, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_worker_is_judged_only_on_evidence_checked_after_it_stopped() {
    let sandbox = Sandbox::new();
    let (clone, worktree) = sandbox.project();
    let forge = Arc::new(FakeForge::default());
    let clock = Arc::new(ManualClock::at(10_000));
    let evidence = EvidenceSources {
        forge: Some(forge.clone()),
        settings: settings(&clone),
        ..EvidenceSources::git(Arc::new(sandbox.cli()))
    };
    let mut log = change_mission();
    log.push(registered(
        COORDINATOR,
        Role::Coordinator,
        &clone,
        Some(mission_id()),
    ));
    log.push(registered(
        WORKER,
        Role::Worker,
        &worktree,
        Some(mission_id()),
    ));
    let daemon = start(log, clock.clone(), evidence);
    let client = Client::connect(&daemon.socket).await.unwrap();

    // The first check, while the worker has done nothing yet.
    eventually(&client, "a first check", |registry, _| {
        registry.agent(agent(WORKER))?.delivery.checked_at
    })
    .await;
    let status = client
        .call::<Status>(StatusParams::default())
        .await
        .unwrap();
    assert_eq!(
        status.forge.map(|forge| forge.program).as_deref(),
        Some("fake-gh")
    );

    // The worker commits, pushes and opens a pull request, then stops.
    sandbox.commit(&worktree, "invoice.rs", "pub struct Invoice;\n");
    sandbox.git(
        &worktree,
        &["push", "--quiet", "--set-upstream", "origin", "feat/x"],
    );
    forge.open(7);
    clock.advance(1_000);
    client
        .call::<HookIngest>(HookParams {
            agent: agent(WORKER),
            event: "stop".into(),
            payload: serde_json::json!({ "last_assistant_message": "Done." }),
        })
        .await
        .unwrap();

    let verdict = eventually(&client, "a verdict", |registry, _| {
        let mission = registry.mission(mission_id())?;
        mission.assessments.get(&agent(WORKER)).map(|a| a.verdict)
    })
    .await;
    assert_eq!(verdict, Verdict::Delivered);

    let events = events(&client).await;
    // The coordinator never heard "not delivered" before "delivered".
    assert!(!events.iter().any(|event| matches!(
        &event.kind,
        EventKind::WorkerAssessed {
            verdict: Verdict::NotDelivered,
            ..
        }
    )));
    let registry = Registry::from_events(&events).unwrap();
    let delivery = &registry.agent(agent(WORKER)).unwrap().delivery;
    assert_eq!(delivery.commits, 1);
    assert_eq!(delivery.push, PushState::UpToDate);
    assert_eq!(delivery.pull_request.as_ref().map(|pr| pr.number), Some(7));
    // The forge was asked about the branch's tip; the sandbox's `origin` is
    // a directory, not a repository on a forge, so no origin was named.
    let tip = delivery.tip.clone().unwrap();
    assert_eq!(
        delivery.pull_request.as_ref().unwrap().head,
        Some(tip.clone())
    );
    assert!(forge.asked().contains(&(tip, None)), "{:?}", forge.asked());
    assert!(delivery.checked_at >= Some(Timestamp::from_millis(11_000)));
    assert_eq!(
        registry.mission(mission_id()).unwrap().status,
        MissionStatus::Reporting
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_agents_are_checked_again_on_a_timer() {
    let sandbox = Sandbox::new();
    let (_clone, worktree) = sandbox.project();
    let evidence = EvidenceSources {
        refresh_interval: Duration::from_millis(200),
        ..EvidenceSources::git(Arc::new(sandbox.cli()))
    };
    let daemon = start(
        vec![registered(1, Role::Lead, &worktree, None)],
        Arc::new(ManualClock::at(10_000)),
        evidence,
    );
    let client = Client::connect(&daemon.socket).await.unwrap();
    eventually(&client, "a first check", |registry, _| {
        registry.agent(agent(1))?.delivery.checked_at
    })
    .await;

    // No event says anything: the timer finds the new commit. The timer may
    // run while the commit is being made; evidence must never mix the tree
    // before the commit with the branch after it.
    let tip = sandbox.commit(&worktree, "invoice.rs", "pub struct Invoice;\n");
    let delivery = eventually(&client, "the new commit", |registry, _| {
        let delivery = &registry.agent(agent(1))?.delivery;
        (delivery.commits == 1).then(|| delivery.clone())
    })
    .await;
    assert_eq!(delivery.tip, Some(tip));
    assert!(!delivery.dirty);
}
