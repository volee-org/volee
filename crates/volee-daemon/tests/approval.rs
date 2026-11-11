//! A mission from its workers' delivery to its end (ADR 0032), in a real
//! repository without a remote: the work is ready once committed, the
//! coordinator presents its report, the person's "ok merge" is the only
//! thing that lets it merge, the daemon reads the merge from git before it
//! lets the mission close, and closing tidies everything in one gesture.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::{FakeHarness, git, repository};
use tokio::net::UnixListener;
use ulid::Ulid;
use volee_core::harness::{HarnessKind, SessionKey};
use volee_core::message::{MessageKind, Party, Thread};
use volee_core::mission::{Mission, MissionKind, MissionStatus, Verdict};
use volee_core::reconcile::ListedSession;
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::AgentState;
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};
use volee_core::{AgentId, Event, EventKind, MessageId, MissionId, Registry, ThreadId};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_git::GitCli;
use volee_harness::Harness;
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{
    ApprovalParams, CloseMission, CloseParams, EventsSince, HookIngest, HookParams,
    RequestApproval, SendMessage, SendParams, SinceParams, Spawn, SpawnParams,
};
use volee_proto::paths::Paths;
use volee_proto::rpc;

const COORDINATOR: u128 = 10;
const LEAD: u128 = 20;

fn agent(n: u128) -> AgentId {
    AgentId::from_ulid(Ulid::from_parts(1_000, n))
}

fn mission_id() -> MissionId {
    MissionId::from_ulid(Ulid::from_parts(4_000, 1))
}

fn thread_id() -> ThreadId {
    ThreadId::from_ulid(Ulid::from_parts(2_000, 1))
}

struct Running {
    socket: PathBuf,
    harness: Arc<FakeHarness>,
    repo: PathBuf,
    _dir: tempfile::TempDir,
}

/// A lead (agent 20) asked for the change mission "Invoicing" in `alpha`;
/// its coordinator (agent 10) rests between two turns.
fn mission(repo: &Path) -> Vec<Event> {
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
                kind: MissionKind::Change,
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

/// A daemon with a repository named `alpha` that has no remote, git
/// evidence read every few milliseconds, and a fake harness listed as often.
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
    let cli = GitCli::new()
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(mission(&repo))),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: Some(Arc::clone(&harness) as Arc<dyn Harness>),
        discovery_interval: Duration::from_millis(10),
        max_discovery_interval: Duration::from_millis(10),
        idle_discovery_interval: Duration::from_millis(10),
        evidence: EvidenceSources {
            git: Some(Arc::new(cli)),
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
    Running {
        socket,
        harness,
        repo,
        _dir: dir,
    }
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

/// The coordinator launches its worker, which commits its work and ends its
/// turn. Returns the worker and its worktree.
async fn delivered_worker(daemon: &Running, client: &Client) -> (AgentId, PathBuf) {
    let spawned = client
        .call::<Spawn>(SpawnParams {
            repo: "alpha".into(),
            branch: "feat/invoices".into(),
            base: None,
            task: "Add invoices to the billing module.".into(),
            role: Role::Worker,
            parent: Some(agent(COORDINATOR)),
            mission: Some(mission_id()),
            model: None,
            name: Some("billing".into()),
            harness: None,
        })
        .await
        .unwrap();
    let worktree = PathBuf::from(&spawned.worktree);
    std::fs::write(worktree.join("invoices.rs"), "pub fn total() {}\n").unwrap();
    git(&worktree, &["add", "invoices.rs"]);
    git(&worktree, &["commit", "--quiet", "-m", "Add invoices"]);
    daemon.harness.set(vec![ListedSession {
        alive: true,
        background: true,
        handle: Some("abcd1234".into()),
        ..ListedSession::new(
            SessionKey::claude("abcd1234-5b60-4cdc-8cf3-000000000001"),
            &worktree,
            AgentState::Done,
        )
    }]);
    (spawned.agent, worktree)
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

fn close() -> CloseParams {
    CloseParams {
        mission: mission_id(),
        report: None,
        force: false,
        by: Some(agent(COORDINATOR)),
    }
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

async fn refused(client: &Client, params: CloseParams) -> String {
    match client.call::<CloseMission>(params).await {
        Err(ClientError::Rpc(error)) => {
            assert_eq!(error.code, rpc::REFUSED, "{}", error.message);
            error.message
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
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

#[tokio::test]
#[allow(clippy::too_many_lines)] // One mission, from its delivery to its end, reads best as one trip.
async fn a_mission_without_a_remote_is_merged_after_ok_merge_then_closed_and_tidied() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let (worker, worktree) = delivered_worker(&daemon, &client).await;

    // Committed work on a clean worktree is all a repository without a
    // remote can ask of a worker: it is delivered, the mission reports.
    let ready = until(&client, |r| {
        r.mission(mission_id())
            .is_some_and(|mission| mission.status == MissionStatus::Reporting)
    })
    .await;
    let assessment = &ready.mission(mission_id()).unwrap().assessments[&worker];
    assert_eq!(assessment.verdict, Verdict::Delivered);
    assert_eq!(assessment.evidence, "1 commit \u{b7} no remote");
    assert!(ready.agent(worker).unwrap().delivery.no_remote());
    let asked_for = told(&ready, Party::Agent(agent(COORDINATOR)));
    assert!(
        asked_for
            .iter()
            .any(|text| text.contains("`request_approval`")),
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

    // Approved, but not merged: the mission does not close.
    answers(&client, asked.question, "ok merge").await;
    let unmerged = refused(&client, close()).await;
    assert!(
        unmerged.contains("merge the work of these workers first") && unmerged.contains("billing"),
        "{unmerged}"
    );
    assert!(worktree.is_dir(), "nothing was tidied");

    // The coordinator merges, in the main checkout, then closes at once:
    // the daemon reads the merge from git before it answers.
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
    client.call::<CloseMission>(close()).await.unwrap();
    let closed = registry(&client).await;
    assert_eq!(
        closed.mission(mission_id()).unwrap().status,
        MissionStatus::Closed
    );
    assert!(
        closed
            .agent(worker)
            .is_none_or(|worker| worker.delivery.merged_into_base),
        "the merge was read before the mission closed"
    );

    // The coordinator's turn ends, then its session: the wind-down tidies
    // the worker and forgets the coordinator.
    for event in ["stop", "session-end"] {
        client
            .call::<HookIngest>(HookParams {
                agent: agent(COORDINATOR),
                event: event.into(),
                payload: serde_json::json!({ "reason": "other" }),
                pid: None,
            })
            .await
            .unwrap();
    }
    let done = until(&client, |r| {
        told(r, Party::Human)
            .iter()
            .any(|text| text.starts_with("Mission \"Invoicing\" is closed."))
    })
    .await;
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
    assert!(
        daemon.repo.join("invoices.rs").is_file(),
        "its work is in the base branch"
    );
    // One sentence says it all, to the person and to the lead.
    let said: Vec<String> = told(&done, Party::Human)
        .into_iter()
        .filter(|text| text.starts_with("Mission \"Invoicing\" is closed."))
        .collect();
    assert_eq!(
        said,
        ["Mission \"Invoicing\" is closed. Tidied: billing (worktrees removed, branches kept)."]
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
            .contains(&("remove-delivered".to_owned(), "abcd1234".to_owned())),
        "{:?}",
        daemon.harness.actions()
    );
}

#[tokio::test]
async fn a_worker_whose_work_did_not_land_is_kept_when_the_person_closes_its_mission() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let (worker, worktree) = delivered_worker(&daemon, &client).await;
    until(&client, |r| {
        r.mission(mission_id())
            .is_some_and(|mission| mission.status == MissionStatus::Reporting)
    })
    .await;
    // The person closes the mission themselves, without a merge: the
    // worker's work has not landed.
    client
        .call::<CloseMission>(CloseParams {
            mission: mission_id(),
            report: None,
            force: false,
            by: None,
        })
        .await
        .unwrap();
    let done = until(&client, |r| {
        told(r, Party::Human)
            .iter()
            .any(|text| text.starts_with("Mission \"Invoicing\" is closed."))
    })
    .await;
    let said = told(&done, Party::Human)
        .into_iter()
        .find(|text| text.starts_with("Mission \"Invoicing\" is closed."))
        .unwrap();
    assert!(
        said.contains("Kept: billing: its work has not landed"),
        "{said}"
    );
    assert!(!said.contains("Tidied"), "{said}");
    assert!(worktree.is_dir(), "its worktree is kept");
    assert!(done.agent(worker).is_some(), "and so is its row");
    assert!(branch_exists(&daemon.repo, "refs/heads/feat/invoices"));
}
