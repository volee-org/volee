//! Agents of other harnesses that join through volee's MCP server
//! (ADR 0014), against a real daemon: identity, liveness, missions,
//! activity.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UnixListener;
use ulid::Ulid;
use volee_core::harness::HarnessKind;
use volee_core::message::{MessageKind, Party, Thread};
use volee_core::mission::{Mission, MissionKind, MissionStatus};
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::AgentState;
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};
use volee_core::{AgentId, Event, EventKind, MissionId, Registry, ThreadId};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{
    ActivityParams, EventsSince, Join, JoinParams, Joined, RecordActivity, SendMessage, SendParams,
    SinceParams,
};
use volee_proto::paths::Paths;
use volee_proto::rpc;

const COORDINATOR: u128 = 10;
const WORKER: u128 = 11;

fn agent(n: u128) -> AgentId {
    AgentId::from_ulid(Ulid::from_parts(1_000, n))
}

fn mission_id() -> MissionId {
    MissionId::from_ulid(Ulid::from_parts(4_000, 1))
}

fn at(kind: EventKind) -> Event {
    Event::new(Timestamp::from_millis(1), kind)
}

/// A running mission with a coordinator and a worker.
fn mission(status: MissionStatus) -> Vec<Event> {
    let thread = ThreadId::from_ulid(Ulid::from_parts(2_000, 1));
    let member = |n, role| {
        at(EventKind::AgentRegistered {
            agent: agent(n),
            name: format!("agent-{n}"),
            role,
            harness: HarnessKind::ClaudeCode,
            launched_by: LaunchedBy::Human,
            parent: None,
            mission: Some(mission_id()),
            workspace: Workspace::at("/work/billing"),
            location: Location::Background,
        })
    };
    vec![
        at(EventKind::ThreadOpened {
            thread: Thread {
                id: thread,
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
                coordinator: Some(agent(COORDINATOR)),
                thread,
                status,
                report: None,
                created_at: Timestamp::from_millis(1),
                ended_at: None,
                assessments: BTreeMap::new(),
            },
        }),
        member(COORDINATOR, Role::Coordinator),
        member(WORKER, Role::Worker),
        at(EventKind::StateChanged {
            agent: agent(WORKER),
            state: AgentState::Working,
            detail: None,
        }),
    ]
}

struct Running {
    socket: PathBuf,
    _dir: tempfile::TempDir,
}

fn start(log: Vec<Event>) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(log)),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: None,
        discovery_interval: volee_daemon::DEFAULT_DISCOVERY_INTERVAL,
        max_discovery_interval: volee_daemon::DEFAULT_DISCOVERY_INTERVAL,
        idle_discovery_interval: volee_daemon::DEFAULT_DISCOVERY_INTERVAL,
        evidence: EvidenceSources::default(),
        volee_bin: None,
        multiplexers: Vec::new(),
        hosts: false,
    };
    tokio::spawn(Daemon::serve(listener, config));
    Running { socket, _dir: dir }
}

async fn snapshot(socket: &Path) -> Registry {
    let client = Client::connect(socket).await.unwrap();
    let batch = client
        .call::<EventsSince>(SinceParams::default())
        .await
        .unwrap();
    let events: Vec<Event> = batch.events.into_iter().map(|e| e.event).collect();
    Registry::from_events(&events).unwrap()
}

/// Waits until the registry satisfies `ready`, for at most ten seconds.
async fn until(socket: &Path, ready: impl Fn(&Registry) -> bool) -> Registry {
    for _ in 0..1_000 {
        let registry = snapshot(socket).await;
        if ready(&registry) {
            return registry;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the registry never reached the expected state");
}

fn params(name: &str, harness: &str, cwd: &str, mission: Option<MissionId>) -> JoinParams {
    JoinParams {
        name: name.into(),
        harness: harness.into(),
        cwd: cwd.into(),
        mission,
    }
}

/// Joins on a new connection, which keeps the agent alive until dropped.
async fn join(socket: &Path, params: JoinParams) -> (Client, Result<Joined, ClientError>) {
    let client = Client::connect(socket).await.unwrap();
    let joined = client.call::<Join>(params).await;
    (client, joined)
}

fn code(result: Result<Joined, ClientError>) -> i64 {
    match result {
        Err(ClientError::Rpc(error)) => error.code,
        other => panic!("expected an error, got {other:?}"),
    }
}

#[tokio::test]
async fn an_agent_joins_once_and_is_found_again() {
    let daemon = start(vec![]);
    let (first, joined) = join(
        &daemon.socket,
        params("infra", "codex", "/work/infra", None),
    )
    .await;
    let joined = joined.unwrap();
    assert!(!joined.rejoined);
    let registry = snapshot(&daemon.socket).await;
    let infra = registry.agent(joined.agent).unwrap();
    assert_eq!(infra.name, "infra");
    assert_eq!(infra.harness, HarnessKind::Other("codex".into()));
    assert_eq!(infra.launched_by, LaunchedBy::Outside);
    assert_eq!(infra.role, Role::Independent);
    assert_eq!(infra.state, AgentState::Idle);
    assert_eq!(infra.workspace.cwd, Path::new("/work/infra"));
    assert!(infra.joined());

    // Its MCP server stops: the connection closes, the agent is stopped.
    drop(first);
    let registry = until(&daemon.socket, |r| {
        r.agent(joined.agent).unwrap().state == AgentState::Stopped
    })
    .await;
    let detail = registry.agent(joined.agent).unwrap().detail.clone();
    assert!(detail.unwrap().contains("MCP server"));

    // The harness starts it again: the same agent, alive again.
    let (_again, rejoined) = join(
        &daemon.socket,
        params("infra", "codex", "/work/infra", None),
    )
    .await;
    let rejoined = rejoined.unwrap();
    assert_eq!(rejoined.agent, joined.agent);
    assert!(rejoined.rejoined);
    let registry = snapshot(&daemon.socket).await;
    assert_eq!(
        registry.agent(joined.agent).unwrap().state,
        AgentState::Idle
    );

    // Another directory or another harness is another agent.
    let (_other_dir, elsewhere) = join(
        &daemon.socket,
        params("infra", "codex", "/work/other", None),
    )
    .await;
    assert_ne!(elsewhere.unwrap().agent, joined.agent);
    let (_other_harness, gemini) = join(
        &daemon.socket,
        params("infra", "gemini-cli", "/work/infra", None),
    )
    .await;
    assert_ne!(gemini.unwrap().agent, joined.agent);
}

#[tokio::test]
async fn it_stays_alive_while_one_of_its_servers_runs() {
    let daemon = start(vec![]);
    let (first, joined) = join(
        &daemon.socket,
        params("docs", "gemini-cli", "/work/docs", None),
    )
    .await;
    let (second, again) = join(
        &daemon.socket,
        params("docs", "gemini-cli", "/work/docs", None),
    )
    .await;
    let agent = joined.unwrap().agent;
    assert_eq!(again.unwrap().agent, agent);
    drop(first);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        snapshot(&daemon.socket).await.agent(agent).unwrap().state,
        AgentState::Idle,
        "the second server still runs"
    );
    drop(second);
    until(&daemon.socket, |r| {
        r.agent(agent).unwrap().state == AgentState::Stopped
    })
    .await;
}

#[tokio::test]
async fn a_member_of_a_mission_writes_with_its_workers_and_is_never_judged() {
    let daemon = start(mission(MissionStatus::Running));
    let (client, joined) = join(
        &daemon.socket,
        params("docs", "gemini-cli", "/work/docs", Some(mission_id())),
    )
    .await;
    let me = joined.unwrap().agent;
    let registry = snapshot(&daemon.socket).await;
    assert_eq!(registry.agent(me).unwrap().mission, Some(mission_id()));
    // The coordinator heard that a member joined.
    let told = registry
        .messages_from(Party::System)
        .into_iter()
        .any(|message| {
            message.kind == MessageKind::Note
                && message.body.contains("docs (gemini-cli) joined")
                && message.receipt(Party::Agent(agent(COORDINATOR))).is_some()
        });
    assert!(told, "the coordinator is told");

    // AI B writes to AI A, and back.
    let send = |from, to| SendParams {
        from: Party::Agent(from),
        to: vec![Party::Agent(to)],
        kind: MessageKind::Question,
        body: "Which endpoint documents invoices?".into(),
        refs: vec![],
        in_reply_to: None,
        thread: None,
    };
    client
        .call::<SendMessage>(send(me, agent(WORKER)))
        .await
        .unwrap();
    client
        .call::<SendMessage>(send(agent(WORKER), me))
        .await
        .unwrap();

    // The supervisor judges workers only: never the joined member.
    let registry = snapshot(&daemon.socket).await;
    let mission = registry.mission(mission_id()).unwrap();
    assert!(!mission.assessments.contains_key(&me));

    // Joined again without the mission: no longer a member.
    drop(client);
    until(&daemon.socket, |r| {
        r.agent(me).unwrap().state == AgentState::Stopped
    })
    .await;
    let (client, again) = join(
        &daemon.socket,
        params("docs", "gemini-cli", "/work/docs", None),
    )
    .await;
    assert_eq!(again.unwrap().agent, me);
    assert_eq!(
        snapshot(&daemon.socket).await.agent(me).unwrap().mission,
        None
    );
    let refused = client.call::<SendMessage>(send(me, agent(WORKER))).await;
    assert!(matches!(refused, Err(ClientError::Rpc(error)) if error.code == rpc::REFUSED));
}

#[tokio::test]
async fn only_an_open_mission_can_be_joined() {
    let daemon = start(mission(MissionStatus::Closed));
    let (_client, joined) = join(
        &daemon.socket,
        params("docs", "gemini-cli", "/work/docs", Some(mission_id())),
    )
    .await;
    assert_eq!(code(joined), rpc::REFUSED);
    let unknown = MissionId::from_ulid(Ulid::from_parts(4_000, 9));
    let (_client, joined) = join(
        &daemon.socket,
        params("docs", "gemini-cli", "/work/docs", Some(unknown)),
    )
    .await;
    assert_eq!(code(joined), rpc::NOT_FOUND);
    assert_eq!(
        snapshot(&daemon.socket).await.agents().count(),
        2,
        "no one joined"
    );
}

#[tokio::test]
async fn names_harnesses_and_directories_are_checked() {
    let daemon = start(vec![]);
    for (name, harness, cwd, expected) in [
        ("", "codex", "/work", rpc::INVALID_PARAMS),
        ("two\nlines", "codex", "/work", rpc::INVALID_PARAMS),
        ("infra", "Codex CLI", "/work", rpc::INVALID_PARAMS),
        ("infra", "codex", "relative/dir", rpc::INVALID_PARAMS),
        ("infra", "claude-code", "/work", rpc::REFUSED),
    ] {
        let (_client, joined) = join(&daemon.socket, params(name, harness, cwd, None)).await;
        assert_eq!(code(joined), expected, "{name:?} {harness:?} {cwd:?}");
    }
    assert_eq!(snapshot(&daemon.socket).await.agents().count(), 0);
}

#[tokio::test]
async fn a_joined_agent_reports_its_activity_and_no_one_else_does() {
    let daemon = start(mission(MissionStatus::Running));
    let (client, joined) = join(
        &daemon.socket,
        params("infra", "codex", "/work/infra", None),
    )
    .await;
    let me = joined.unwrap().agent;
    client
        .call::<RecordActivity>(ActivityParams {
            agent: me,
            activity: "wrote to\nthe human".into(),
        })
        .await
        .unwrap();
    let registry = snapshot(&daemon.socket).await;
    let activity = registry.agent(me).unwrap().last_activity.clone().unwrap();
    assert!(
        activity.summary.contains("wrote to the human"),
        "{activity:?}"
    );
    let refused = client
        .call::<RecordActivity>(ActivityParams {
            agent: agent(WORKER),
            activity: "pretends".into(),
        })
        .await;
    assert!(matches!(refused, Err(ClientError::Rpc(error)) if error.code == rpc::REFUSED));
}

#[tokio::test]
async fn joined_agents_wait_for_their_server_when_the_daemon_starts() {
    let joined = agent(20);
    let log = vec![
        at(EventKind::AgentRegistered {
            agent: joined,
            name: "infra".into(),
            role: Role::Independent,
            harness: HarnessKind::Other("codex".into()),
            launched_by: LaunchedBy::Outside,
            parent: None,
            mission: None,
            workspace: Workspace::at("/work/infra"),
            location: Location::Unknown,
        }),
        at(EventKind::StateChanged {
            agent: joined,
            state: AgentState::Idle,
            detail: None,
        }),
    ];
    let daemon = start(log);
    let registry = snapshot(&daemon.socket).await;
    assert_eq!(registry.agent(joined).unwrap().state, AgentState::Stopped);
}
