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
    AbortMission, AbortParams, AgentParams, CloseMission, CloseParams, CreateMission,
    CreateMissionParams, EventsSince, Forget, HookIngest, HookParams, SendMessage, SendParams,
    SinceParams, Stop,
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

async fn coordinator_reports(client: &Client, body: &str) -> volee_core::MessageId {
    client
        .call::<SendMessage>(SendParams {
            from: Party::Agent(agent(COORDINATOR)),
            to: vec![Party::Human],
            kind: MessageKind::Report,
            body: body.into(),
            refs: vec![],
            in_reply_to: None,
            thread: None,
        })
        .await
        .unwrap()
        .message
}

#[tokio::test]
async fn the_coordinator_closes_with_its_latest_report() {
    let mut events = mission_with_workers();
    events.push(shipped(API));
    let daemon = start(events);
    let client = connect(&daemon).await;
    reporting(&client).await;

    let refused = rpc_error(
        client
            .call::<CloseMission>(close_by(COORDINATOR, false))
            .await
            .unwrap_err(),
    );
    assert_eq!(refused.code, rpc::REFUSED);
    assert!(refused.message.contains("report"), "{}", refused.message);

    coordinator_reports(&client, "First draft.").await;
    let last = coordinator_reports(&client, "Invoices are in billing#11 and api#12.").await;
    client
        .call::<CloseMission>(close_by(COORDINATOR, false))
        .await
        .unwrap();
    let registry = registry(&client).await;
    let mission = registry.mission(mission_id()).unwrap();
    assert_eq!(mission.status, MissionStatus::Closed);
    assert_eq!(mission.report, Some(last));
}

#[tokio::test]
async fn only_the_coordinator_closes_and_never_by_force() {
    let mut events = mission_with_workers();
    events.push(shipped(API));
    let daemon = start(events);
    let client = connect(&daemon).await;
    reporting(&client).await;
    coordinator_reports(&client, "Done.").await;

    for params in [close_by(BILLING, false), close_by(COORDINATOR, true)] {
        let error = rpc_error(client.call::<CloseMission>(params).await.unwrap_err());
        assert_eq!(error.code, rpc::REFUSED, "{}", error.message);
    }
    let registry = registry(&client).await;
    assert_eq!(
        registry.mission(mission_id()).unwrap().status,
        MissionStatus::Reporting
    );
}
