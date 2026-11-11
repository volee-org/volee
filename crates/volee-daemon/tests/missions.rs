//! End-to-end tests of missions: creation, supervision after hooks, closing
//! and aborting, through a real daemon on a temporary socket.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::net::UnixListener;
use tokio::task::JoinHandle;
use ulid::Ulid;
use volee_core::agent::Hold;
use volee_core::delivery::{ChecksState, Delivery, PrState, PullRequest, PushState};
use volee_core::harness::HarnessKind;
use volee_core::message::{MessageKind, Party, Thread};
use volee_core::mission::{Mission, MissionKind, MissionStatus, Verdict};
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::AgentState;
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};
use volee_core::{AgentId, Event, EventKind, MissionId, Registry, ThreadId};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_harness::Harness;
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{
    AbortMission, AbortParams, AgentParams, ApprovalParams, CloseMission, CloseParams,
    CreateMission, CreateMissionParams, EventsSince, Forget, HookIngest, HookParams,
    RequestApproval, SendMessage, SendParams, SinceParams, Stop,
};
use volee_proto::paths::Paths;
use volee_proto::rpc;

const COORDINATOR: u128 = 10;
const BILLING: u128 = 11;
const API: u128 = 12;

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

fn mission_events() -> Vec<Event> {
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
                repos: vec!["billing".into(), "api".into()],
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

fn registered(n: u128, role: Role, name: &str) -> Event {
    at(EventKind::AgentRegistered {
        agent: agent(n),
        name: name.into(),
        role,
        harness: HarnessKind::ClaudeCode,
        launched_by: if role == Role::Coordinator {
            LaunchedBy::Human
        } else {
            LaunchedBy::Agent(agent(COORDINATOR))
        },
        parent: (role != Role::Coordinator).then(|| agent(COORDINATOR)),
        mission: Some(mission_id()),
        workspace: Workspace {
            repo: Some(name.into()),
            branch: Some("feat/invoices".into()),
            ..Workspace::at("/work")
        },
        location: Location::Background,
    })
}

/// A pushed branch with an open pull request: what a change worker must show.
fn shipped(n: u128) -> Event {
    at(EventKind::DeliveryUpdated {
        agent: agent(n),
        delivery: Delivery {
            commits: 2,
            push: PushState::UpToDate,
            // Opened at the branch's tip, the only pull request that counts.
            pull_request: Some(PullRequest {
                number: u64::try_from(n).unwrap(),
                url: format!("https://example.com/pull/{n}"),
                state: PrState::Open,
                checks: ChecksState::Passing,
                head: Some(format!("c0ffee{n}")),
            }),
            tip: Some(format!("c0ffee{n}")),
            ..Delivery::default()
        },
    })
}

/// A mission with its coordinator and two workers. `billing` has shipped,
/// `api` has not.
fn mission_with_workers() -> Vec<Event> {
    let mut events = mission_events();
    events.extend([
        registered(COORDINATOR, Role::Coordinator, "coordinator"),
        registered(BILLING, Role::Worker, "billing"),
        registered(API, Role::Worker, "api"),
        shipped(BILLING),
    ]);
    events
}

struct Running {
    socket: PathBuf,
    _task: JoinHandle<Result<(), volee_daemon::DaemonError>>,
    _dir: tempfile::TempDir,
}

fn start(events: Vec<Event>) -> Running {
    start_with(events, None)
}

fn start_with(events: Vec<Event>, harness: Option<Arc<dyn Harness>>) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(events)),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness,
        discovery_interval: volee_daemon::DEFAULT_DISCOVERY_INTERVAL,
        max_discovery_interval: volee_daemon::DEFAULT_DISCOVERY_INTERVAL,
        idle_discovery_interval: volee_daemon::DEFAULT_DISCOVERY_INTERVAL,
        evidence: EvidenceSources::default(),
        volee_bin: None,
        multiplexers: Vec::new(),
        hosts: false,
        processes: None,
    };
    let task = tokio::spawn(Daemon::serve(listener, config));
    Running {
        socket,
        _task: task,
        _dir: dir,
    }
}

async fn connect(daemon: &Running) -> Client {
    Client::connect(&daemon.socket).await.unwrap()
}

async fn registry(client: &Client) -> Registry {
    let batch = client
        .call::<EventsSince>(SinceParams::default())
        .await
        .unwrap();
    let events: Vec<Event> = batch.events.into_iter().map(|e| e.event).collect();
    Registry::from_events(&events).unwrap()
}

/// A worker's turn ends: the hook a Claude Code agent sends when it stops.
async fn stops(client: &Client, n: u128) {
    client
        .call::<HookIngest>(HookParams {
            agent: agent(n),
            event: "stop".into(),
            payload: serde_json::json!({ "last_assistant_message": "Done." }),
            pid: None,
        })
        .await
        .unwrap();
}

fn rpc_error(error: ClientError) -> rpc::RpcError {
    match error {
        ClientError::Rpc(error) => error,
        other => panic!("expected an RPC error, got {other:?}"),
    }
}

fn texts(registry: &Registry, party: Party) -> Vec<String> {
    let mut texts: Vec<String> = registry
        .thread_messages(thread_id())
        .into_iter()
        .filter(|message| message.receipts.iter().any(|r| r.to == party))
        .map(|message| message.body.clone())
        .collect();
    texts.sort();
    texts
}

#[tokio::test]
async fn a_mission_starts_in_planning_with_its_own_thread() {
    // A mission needs a harness for its coordinator. This daemon has a fake
    // one but no git, so the coordinator does not start (the launch tests
    // cover that); the mission itself is what is under test here.
    let daemon = start_with(vec![], Some(Arc::new(common::FakeHarness::default())));
    let client = connect(&daemon).await;
    let created = client
        .call::<CreateMission>(CreateMissionParams {
            title: "  Invoicing  ".into(),
            kind: MissionKind::Change,
            goal: "Add invoices to billing and expose them in the API.".into(),
            repos: vec!["billing".into(), "api".into()],
            requested_by: None,
        })
        .await
        .unwrap();
    let registry = registry(&client).await;
    let mission = registry.mission(created.mission).unwrap();
    assert_eq!(mission.title, "Invoicing");
    assert_eq!(mission.status, MissionStatus::Planning);
    assert_eq!(mission.thread, created.thread);
    let thread = registry.thread(created.thread).unwrap();
    assert_eq!(thread.mission, Some(created.mission));
    assert_eq!(thread.participants, [Party::Human]);
}

#[tokio::test]
async fn without_claude_code_a_mission_is_refused_and_nothing_is_recorded() {
    let daemon = start(vec![]);
    let client = connect(&daemon).await;
    let error = rpc_error(
        client
            .call::<CreateMission>(CreateMissionParams {
                title: "Invoicing".into(),
                kind: MissionKind::Change,
                goal: "Add invoices to billing.".into(),
                repos: vec!["billing".into()],
                requested_by: None,
            })
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code, rpc::UNAVAILABLE, "{}", error.message);
    assert!(
        error.message.contains("needs Claude Code")
            && error.message.contains("volee daemon status"),
        "{}",
        error.message
    );
    let registry = registry(&client).await;
    assert_eq!(registry.missions().count(), 0);
    assert_eq!(registry.threads().count(), 0, "no thread either");
}

#[tokio::test]
async fn a_mission_needs_a_title_a_goal_and_a_repository() {
    let daemon = start(vec![]);
    let client = connect(&daemon).await;
    let valid = CreateMissionParams {
        title: "Invoicing".into(),
        kind: MissionKind::Analysis,
        goal: "Find where invoices are computed.".into(),
        repos: vec!["billing".into()],
        requested_by: None,
    };
    for params in [
        CreateMissionParams {
            title: " ".into(),
            ..valid.clone()
        },
        CreateMissionParams {
            goal: String::new(),
            ..valid.clone()
        },
        CreateMissionParams {
            repos: vec![],
            ..valid.clone()
        },
        CreateMissionParams {
            requested_by: Some(agent(99)),
            ..valid
        },
    ] {
        let error = rpc_error(client.call::<CreateMission>(params).await.unwrap_err());
        assert!(
            [rpc::INVALID_PARAMS, rpc::NOT_FOUND].contains(&error.code),
            "unexpected error {error:?}"
        );
    }
    assert_eq!(registry(&client).await.missions().count(), 0);
}

#[tokio::test]
async fn the_coordinator_is_linked_and_the_mission_runs_once_workers_exist() {
    let daemon = start(mission_with_workers());
    let client = connect(&daemon).await;
    let registry = registry(&client).await;
    let mission = registry.mission(mission_id()).unwrap();
    assert_eq!(mission.coordinator, Some(agent(COORDINATOR)));
    assert_eq!(mission.status, MissionStatus::Running);
    // Workers that are still launching have no verdict yet.
    assert!(mission.assessments.is_empty());
}

#[tokio::test]
async fn the_coordinator_hears_about_each_worker_and_is_asked_for_a_report() {
    let daemon = start(mission_with_workers());
    let client = connect(&daemon).await;

    stops(&client, BILLING).await;
    let after_billing = registry(&client).await;
    let mission = after_billing.mission(mission_id()).unwrap();
    assert_eq!(
        mission.assessments[&agent(BILLING)].verdict,
        Verdict::Delivered
    );
    assert_eq!(mission.status, MissionStatus::Running);
    let notices = texts(&after_billing, Party::Agent(agent(COORDINATOR)));
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].starts_with("Worker billing"), "{notices:?}");
    assert_eq!(after_billing.pending_for(agent(COORDINATOR)).len(), 1);

    stops(&client, API).await;
    let after_api = registry(&client).await;
    let mission = after_api.mission(mission_id()).unwrap();
    assert_eq!(
        mission.assessments[&agent(API)].verdict,
        Verdict::NotDelivered
    );
    let notices = texts(&after_api, Party::Agent(agent(COORDINATOR)));
    assert!(
        notices
            .iter()
            .any(|text| text.contains("api") && text.contains("no commits")),
        "{notices:?}"
    );
    // One worker has not delivered: no report is asked for yet.
    assert_eq!(mission.status, MissionStatus::Running);
}

#[tokio::test]
async fn every_worker_delivered_moves_the_mission_to_reporting() {
    let mut events = mission_with_workers();
    events.push(shipped(API));
    let daemon = start(events);
    let client = connect(&daemon).await;

    stops(&client, BILLING).await;
    stops(&client, API).await;

    let registry = registry(&client).await;
    let mission = registry.mission(mission_id()).unwrap();
    assert_eq!(mission.status, MissionStatus::Reporting);
    let to_human = texts(&registry, Party::Human);
    assert!(
        to_human
            .iter()
            .any(|text| text.starts_with("All 2 workers")),
        "{to_human:?}"
    );
    // Two worker notices and the request for the report.
    assert_eq!(registry.pending_for(agent(COORDINATOR)).len(), 3);

    // Stopping again changes nothing: verdicts and notices are sent once.
    stops(&client, API).await;
    let again = self::registry(&client).await;
    assert_eq!(
        again.thread_messages(thread_id()).len(),
        registry.thread_messages(thread_id()).len()
    );
    assert_eq!(
        again.mission(mission_id()).unwrap().assessments,
        mission.assessments
    );
}

#[tokio::test]
async fn a_mission_closes_with_its_report_once_every_worker_delivered() {
    let mut events = mission_with_workers();
    events.push(shipped(API));
    let daemon = start(events);
    let client = connect(&daemon).await;

    let early = client
        .call::<CloseMission>(CloseParams {
            mission: mission_id(),
            report: None,
            force: false,
            by: None,
        })
        .await
        .unwrap_err();
    let early = rpc_error(early);
    assert_eq!(early.code, rpc::REFUSED);
    assert!(early.message.contains("billing, api"), "{}", early.message);

    stops(&client, BILLING).await;
    stops(&client, API).await;
    let report = client
        .call::<SendMessage>(SendParams {
            from: Party::Agent(agent(COORDINATOR)),
            to: vec![Party::Human],
            kind: MessageKind::Report,
            body: "Invoices are in billing#11 and api#12.".into(),
            refs: vec![],
            in_reply_to: None,
            options: Vec::new(),
            thread: None,
        })
        .await
        .unwrap();
    assert_eq!(
        report.thread,
        thread_id(),
        "a mission agent writes in its thread"
    );

    client
        .call::<CloseMission>(CloseParams {
            mission: mission_id(),
            report: Some(report.message),
            force: false,
            by: None,
        })
        .await
        .unwrap();
    let registry = registry(&client).await;
    let mission = registry.mission(mission_id()).unwrap();
    assert_eq!(mission.status, MissionStatus::Closed);
    assert_eq!(mission.report, Some(report.message));
    assert_eq!(mission.ended_at, Some(Timestamp::from_millis(10_000)));

    let twice = client
        .call::<CloseMission>(CloseParams {
            mission: mission_id(),
            report: None,
            force: false,
            by: None,
        })
        .await
        .unwrap_err();
    assert_eq!(rpc_error(twice).code, rpc::REFUSED);
}

#[tokio::test]
async fn the_report_must_come_from_the_mission_thread() {
    let mut events = mission_with_workers();
    events.extend([shipped(API), registered(99, Role::Lead, "outsider")]);
    // The outsider is not part of the mission.
    if let Some(EventKind::AgentRegistered { mission, .. }) =
        events.last_mut().map(|event| &mut event.kind)
    {
        *mission = None;
    }
    let daemon = start(events);
    let client = connect(&daemon).await;
    stops(&client, BILLING).await;
    stops(&client, API).await;
    let elsewhere = client
        .call::<SendMessage>(SendParams {
            from: Party::Agent(agent(99)),
            to: vec![Party::Human],
            kind: MessageKind::Report,
            body: "Unrelated.".into(),
            refs: vec![],
            in_reply_to: None,
            options: Vec::new(),
            thread: None,
        })
        .await
        .unwrap();
    let error = client
        .call::<CloseMission>(CloseParams {
            mission: mission_id(),
            report: Some(elsewhere.message),
            force: false,
            by: None,
        })
        .await
        .unwrap_err();
    assert_eq!(rpc_error(error).code, rpc::REFUSED);
}

#[tokio::test]
async fn the_human_can_force_a_mission_closed() {
    let daemon = start(mission_with_workers());
    let client = connect(&daemon).await;
    client
        .call::<CloseMission>(CloseParams {
            mission: mission_id(),
            report: None,
            force: true,
            by: None,
        })
        .await
        .unwrap();
    let registry = registry(&client).await;
    assert_eq!(
        registry.mission(mission_id()).unwrap().status,
        MissionStatus::Closed
    );
}

#[tokio::test]
async fn aborting_tells_every_live_agent_and_holds_them_all() {
    let mut events = mission_with_workers();
    // `api`'s session already ended: nothing to tell it, nothing to resume.
    events.push(at(EventKind::StateChanged {
        agent: agent(API),
        state: AgentState::Stopped,
        detail: None,
    }));
    let daemon = start(events);
    let client = connect(&daemon).await;
    client
        .call::<AbortMission>(AbortParams {
            mission: mission_id(),
            reason: Some("wrong approach".into()),
        })
        .await
        .unwrap();
    let registry_now = registry(&client).await;
    assert_eq!(
        registry_now.mission(mission_id()).unwrap().status,
        MissionStatus::Aborted
    );
    // The coordinator also heard that `api` stopped without delivering.
    for n in [COORDINATOR, BILLING] {
        let aborts = registry_now
            .pending_for(agent(n))
            .into_iter()
            .filter(|m| m.kind == MessageKind::Alert && m.body.contains("wrong approach"))
            .count();
        assert_eq!(aborts, 1, "agent {n}");
    }
    assert!(registry_now.pending_for(agent(API)).is_empty(), "stopped");
    for n in [COORDINATOR, BILLING, API] {
        assert!(
            matches!(
                registry_now.agent(agent(n)).unwrap().hold,
                Some(Hold::MissionOver { .. })
            ),
            "agent {n} is never resumed on its own"
        );
    }
    assert!(
        texts(&registry_now, Party::Human)
            .iter()
            .any(|text| text.contains("aborted"))
    );

    // A worker stopping after the abort gets no verdict.
    stops(&client, BILLING).await;
    let after = registry(&client).await;
    assert!(
        !after
            .mission(mission_id())
            .unwrap()
            .assessments
            .contains_key(&agent(BILLING))
    );

    let again = client
        .call::<AbortMission>(AbortParams {
            mission: mission_id(),
            reason: None,
        })
        .await
        .unwrap_err();
    assert_eq!(rpc_error(again).code, rpc::REFUSED);
}

#[tokio::test]
async fn stopping_a_worker_tells_the_coordinator_once() {
    let mut events = mission_with_workers();
    events.push(at(EventKind::StateChanged {
        agent: agent(API),
        state: AgentState::Stopped,
        detail: None,
    }));
    let daemon = start_with(events, Some(Arc::new(common::FakeHarness::default())));
    let client = connect(&daemon).await;
    for _ in 0..2 {
        client
            .call::<Stop>(AgentParams { agent: agent(API) })
            .await
            .unwrap();
    }
    let registry_now = registry(&client).await;
    let notes: Vec<_> = registry_now
        .pending_for(agent(COORDINATOR))
        .into_iter()
        .filter(|m| m.kind == MessageKind::Note && m.body.contains("The human stopped api"))
        .collect();
    assert_eq!(notes.len(), 1, "told once");
    assert_eq!(notes[0].thread, thread_id(), "in the mission's thread");
    assert!(matches!(
        registry_now.agent(agent(API)).unwrap().hold,
        Some(Hold::StoppedByHuman { .. })
    ));
}

#[tokio::test]
async fn an_agent_of_an_open_mission_is_not_forgotten() {
    let mut events = mission_with_workers();
    events.push(at(EventKind::StateChanged {
        agent: agent(API),
        state: AgentState::Stopped,
        detail: None,
    }));
    let daemon = start(events);
    let client = connect(&daemon).await;
    let error = client
        .call::<Forget>(AgentParams { agent: agent(API) })
        .await
        .unwrap_err();
    let error = rpc_error(error);
    assert_eq!(error.code, rpc::REFUSED);
    assert!(error.message.contains("open mission"), "{}", error.message);
}

#[tokio::test]
async fn unknown_missions_are_reported() {
    let daemon = start(vec![]);
    let client = connect(&daemon).await;
    let error = client
        .call::<AbortMission>(AbortParams {
            mission: mission_id(),
            reason: None,
        })
        .await
        .unwrap_err();
    assert_eq!(rpc_error(error).code, rpc::NOT_FOUND);
}

/// Every worker shipped and stopped: the mission waits for its report.
async fn reporting(client: &Client) {
    stops(client, BILLING).await;
    stops(client, API).await;
}

fn close_by(by: u128, force: bool) -> CloseParams {
    CloseParams {
        mission: mission_id(),
        report: None,
        force,
        by: Some(agent(by)),
    }
}

/// A merged pull request at the branch's tip, as the forge says it.
fn merged(n: u128) -> Event {
    let mut event = shipped(n);
    if let EventKind::DeliveryUpdated { delivery, .. } = &mut event.kind {
        delivery.commits = 0;
        if let Some(pr) = &mut delivery.pull_request {
            pr.state = PrState::Merged;
        }
    }
    event
}

/// The coordinator presents its report.
fn present(by: u128) -> ApprovalParams {
    ApprovalParams {
        mission: mission_id(),
        by: agent(by),
        done: "Invoices in billing and api.".into(),
        proof: "billing#11 and api#12, tests green.".into(),
        left: String::new(),
    }
}

/// The person answers a question in their own name.
async fn answers(client: &Client, question: volee_core::MessageId, words: &str) {
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

async fn status(client: &Client) -> MissionStatus {
    registry(client).await.mission(mission_id()).unwrap().status
}

async fn refusal<M: volee_proto::methods::Method>(client: &Client, params: M::Params) -> String {
    let Err(error) = client.call::<M>(params).await else {
        panic!("the daemon accepted what it must refuse");
    };
    let error = rpc_error(error);
    assert_eq!(error.code, rpc::REFUSED, "{}", error.message);
    error.message
}

/// Waits until the registry satisfies `ready`, for at most ten seconds:
/// what follows the person's approval runs after the request that carried
/// their answer returned.
async fn until(client: &Client, ready: impl Fn(&Registry) -> bool) -> Registry {
    for _ in 0..500 {
        let registry = registry(client).await;
        if ready(&registry) {
            return registry;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the registry never reached the expected state");
}

#[tokio::test]
async fn the_report_is_presented_as_a_question_that_offers_ok_merge_and_no() {
    let mut events = mission_with_workers();
    events.push(shipped(API));
    let daemon = start(events);
    let client = connect(&daemon).await;

    // Before every worker delivered there is nothing to approve.
    let early = refusal::<RequestApproval>(&client, present(COORDINATOR)).await;
    assert!(early.contains("not ready to be approved"), "{early}");
    assert!(early.contains("billing, api"), "{early}");

    reporting(&client).await;
    let not_mine = refusal::<RequestApproval>(&client, present(BILLING)).await;
    assert!(not_mine.contains("only the coordinator"), "{not_mine}");

    let asked = client
        .call::<RequestApproval>(present(COORDINATOR))
        .await
        .unwrap();
    let registry = registry(&client).await;
    let mission = registry.mission(mission_id()).unwrap();
    assert_eq!(mission.status, MissionStatus::AwaitingApproval);
    assert_eq!(mission.report, Some(asked.question));
    let question = registry.message(asked.question).unwrap();
    assert_eq!(question.kind, MessageKind::Question);
    assert_eq!(question.from, Party::Agent(agent(COORDINATOR)));
    assert_eq!(question.thread, thread_id());
    assert_eq!(question.options, ["ok merge", "no"]);
    assert_eq!(question.recipients().collect::<Vec<_>>(), [Party::Human]);
    assert_eq!(
        question.body,
        "Mission \"Invoicing\" is ready to merge.\nDone: Invoices in billing and api.\nProof: \
         billing#11 and api#12, tests green.\nLeft: nothing"
    );
    // It waits on top of the person's messages.
    assert_eq!(registry.unanswered_questions(), [question]);

    // Asking twice before the person answered: refused.
    let twice = refusal::<RequestApproval>(&client, present(COORDINATOR)).await;
    assert!(
        twice.contains("has not answered your report yet"),
        "{twice}"
    );
    assert_eq!(status(&client).await, MissionStatus::AwaitingApproval);
}

#[tokio::test]
async fn a_report_says_what_is_done_and_gives_the_proof_on_single_lines() {
    let mut events = mission_with_workers();
    events.push(shipped(API));
    let daemon = start(events);
    let client = connect(&daemon).await;
    reporting(&client).await;

    let mut empty = present(COORDINATOR);
    empty.proof = "  ".into();
    let error = rpc_error(client.call::<RequestApproval>(empty).await.unwrap_err());
    assert_eq!(error.code, rpc::INVALID_PARAMS, "{}", error.message);

    let mut long = present(COORDINATOR);
    long.done = "Invoices\nin billing\n\nand\u{7} api.".into();
    long.left = "x".repeat(400);
    let asked = client.call::<RequestApproval>(long).await.unwrap();
    let registry = registry(&client).await;
    let body = &registry.message(asked.question).unwrap().body;
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 4, "{body}");
    assert_eq!(lines[1], "Done: Invoices in billing and api.");
    assert_eq!(lines[3].chars().count(), "Left: ".len() + 300, "{body}");
    assert!(lines[3].ends_with('\u{2026}'), "{body}");
}

#[tokio::test]
async fn an_agent_never_closes_a_mission() {
    let mut events = mission_with_workers();
    events.extend([merged(BILLING), merged(API)]);
    let daemon = start(events);
    let client = connect(&daemon).await;
    reporting(&client).await;
    // Neither its coordinator nor a worker, by force or not: even with
    // every pull request merged, a mission ends on the person's word.
    for params in [
        close_by(COORDINATOR, false),
        close_by(COORDINATOR, true),
        close_by(BILLING, false),
    ] {
        let refused = refusal::<CloseMission>(&client, params).await;
        assert!(
            refused.contains("an agent does not close a mission")
                && refused.contains("`request_approval`"),
            "{refused}"
        );
    }
    assert_eq!(status(&client).await, MissionStatus::Reporting);
    // Nor while the person reads its report.
    client
        .call::<RequestApproval>(present(COORDINATOR))
        .await
        .unwrap();
    let refused = refusal::<CloseMission>(&client, close_by(COORDINATOR, false)).await;
    assert!(
        refused.contains("an agent does not close a mission"),
        "{refused}"
    );
    assert_eq!(status(&client).await, MissionStatus::AwaitingApproval);
}

#[tokio::test]
async fn no_keeps_the_mission_open_and_the_coordinator_is_told() {
    let mut events = mission_with_workers();
    events.push(shipped(API));
    let daemon = start(events);
    let client = connect(&daemon).await;
    reporting(&client).await;
    let asked = client
        .call::<RequestApproval>(present(COORDINATOR))
        .await
        .unwrap();
    answers(&client, asked.question, "no").await;

    let registry = registry(&client).await;
    assert_eq!(
        registry.mission(mission_id()).unwrap().status,
        MissionStatus::Reporting
    );
    let told = texts(&registry, Party::Agent(agent(COORDINATOR)));
    assert!(
        told.iter()
            .any(|text| text.contains("did not approve") && text.contains("Nothing is merged")),
        "{told:?}"
    );

    // A second report is a new question; an answer to the first one, late,
    // approves nothing.
    let again = client
        .call::<RequestApproval>(present(COORDINATOR))
        .await
        .unwrap();
    assert_ne!(again.question, asked.question);
    answers(&client, asked.question, "ok merge").await;
    assert_eq!(status(&client).await, MissionStatus::AwaitingApproval);
    // Nothing was attempted: nobody hears of a merge.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let late = crate::registry(&client).await;
    assert_eq!(
        late.mission(mission_id()).unwrap().status,
        MissionStatus::AwaitingApproval
    );
    assert!(
        !texts(&late, Party::Human)
            .iter()
            .any(|text| text.contains("could not merge")),
        "{:?}",
        texts(&late, Party::Human)
    );
}

#[tokio::test]
async fn an_approval_volee_cannot_carry_out_opens_the_mission_again_and_says_why() {
    // Two open pull requests, and a daemon that has no forge client to
    // merge them with.
    let mut events = mission_with_workers();
    events.push(shipped(API));
    let daemon = start(events);
    let client = connect(&daemon).await;
    reporting(&client).await;
    let asked = client
        .call::<RequestApproval>(present(COORDINATOR))
        .await
        .unwrap();
    // Their spelling of the approval does not matter.
    answers(&client, asked.question, "OK merge").await;

    let failed = |registry: &Registry| {
        registry
            .thread_messages(thread_id())
            .into_iter()
            .filter(|message| message.body.contains("could not merge"))
            .cloned()
            .collect::<Vec<_>>()
    };
    let reopened = until(&client, |registry| !failed(registry).is_empty()).await;
    // The coordinator was told that the person approved, and that the
    // merge is not its own to make.
    let told = texts(&reopened, Party::Agent(agent(COORDINATOR)));
    assert!(
        told.iter().any(|text| text.contains("The human approved")
            && text.contains("volee merges every worker's work itself")
            && text.contains("merge nothing")),
        "{told:?}"
    );
    // Why, worker by worker, to the person and to the coordinator; once.
    let [alert] = &failed(&reopened)[..] else {
        panic!("one alert: {:?}", failed(&reopened));
    };
    assert_eq!(alert.kind, MessageKind::Alert);
    assert_eq!(alert.from, Party::System);
    assert_eq!(
        alert.recipients().collect::<Vec<_>>(),
        [Party::Human, Party::Agent(agent(COORDINATOR))]
    );
    assert!(
        alert
            .body
            .starts_with("volee could not merge all the work of \"Invoicing\". "),
        "{}",
        alert.body
    );
    for why in [
        "billing: volee has no forge client to merge https://example.com/pull/11 with",
        "api: volee has no forge client to merge https://example.com/pull/12 with",
        "What was merged stays merged, and nothing was forced.",
        "presents its report again with `request_approval`",
    ] {
        assert!(alert.body.contains(why), "{why}: {}", alert.body);
    }
    // The mission is open again, and waits for a new report: the answer
    // that was given approved a merge that did not happen.
    assert_eq!(
        reopened.mission(mission_id()).unwrap().status,
        MissionStatus::Reporting
    );
    let again = client
        .call::<RequestApproval>(present(COORDINATOR))
        .await
        .unwrap();
    assert_ne!(again.question, asked.question);
    assert_eq!(status(&client).await, MissionStatus::AwaitingApproval);
    assert_eq!(failed(&registry(&client).await).len(), 1, "said once");
}

#[tokio::test]
async fn a_mission_whose_work_is_already_merged_closes_on_the_person_s_approval() {
    // The person merged both pull requests on the forge themselves, then
    // answered the report.
    let mut events = mission_with_workers();
    events.extend([merged(BILLING), merged(API)]);
    let daemon = start(events);
    let client = connect(&daemon).await;
    reporting(&client).await;
    let asked = client
        .call::<RequestApproval>(present(COORDINATOR))
        .await
        .unwrap();
    // Until they answer, nothing ends.
    assert_eq!(status(&client).await, MissionStatus::AwaitingApproval);
    answers(&client, asked.question, "ok merge").await;
    let closed = until(&client, |registry| {
        registry.mission(mission_id()).unwrap().status == MissionStatus::Closed
    })
    .await;
    let mission = closed.mission(mission_id()).unwrap();
    assert_eq!(
        mission.report,
        Some(asked.question),
        "the report the person approved"
    );
    assert!(
        !texts(&closed, Party::Human)
            .iter()
            .any(|text| text.contains("could not merge")),
        "{:?}",
        texts(&closed, Party::Human)
    );
    // Presenting again is refused: the mission is over.
    refusal::<RequestApproval>(&client, present(COORDINATOR)).await;
}

#[tokio::test]
async fn the_person_closes_a_mission_whose_report_waits_and_is_told_what_was_kept() {
    let mut events = mission_with_workers();
    events.push(shipped(API));
    let daemon = start(events);
    let client = connect(&daemon).await;
    reporting(&client).await;
    client
        .call::<RequestApproval>(present(COORDINATOR))
        .await
        .unwrap();
    // No answer, no force: closing is the person's own decision.
    client
        .call::<CloseMission>(CloseParams {
            mission: mission_id(),
            report: None,
            force: false,
            by: None,
        })
        .await
        .unwrap();
    assert_eq!(status(&client).await, MissionStatus::Closed);

    // The wind-down says what it kept: here nothing can be tidied (no
    // worktree of volee's, no evidence read from git) or stopped (no
    // harness), and each is kept with its reason.
    let done = until(&client, |registry| {
        texts(registry, Party::Human)
            .iter()
            .any(|text| text.starts_with("Mission \"Invoicing\" is closed."))
    })
    .await;
    let said = texts(&done, Party::Human)
        .into_iter()
        .find(|text| text.starts_with("Mission \"Invoicing\" is closed."))
        .unwrap();
    assert!(said.contains("Kept: billing: "), "{said}");
    assert!(said.contains("api: "), "{said}");
    assert!(
        said.contains("coordinator: it still runs; stop it with `volee stop coordinator`"),
        "{said}"
    );
    assert!(!said.contains("Tidied"), "{said}");
    for n in [COORDINATOR, BILLING, API] {
        assert!(done.agent(agent(n)).is_some(), "agent {n} is kept");
    }
}
