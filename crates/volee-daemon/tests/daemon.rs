//! End-to-end tests: a real daemon on a temporary socket, driven through the
//! protocol clients.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UnixListener;
use tokio::task::JoinHandle;
use ulid::Ulid;
use volee_core::harness::HarnessKind;
use volee_core::message::{Message, MessageKind, Party};
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::AgentState;
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};
use volee_core::{AgentId, Event, EventKind, Registry};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_proto::client::{Client, ClientError, appended_event};
use volee_proto::methods::{
    AgentParams, Empty, EventsSince, Forget, HookIngest, HookParams, HookWait, SendMessage,
    SendParams, Shutdown, SinceParams, Status, StatusParams, Subscribe, SubscribeParams,
    WaitParams,
};
use volee_proto::paths::Paths;
use volee_proto::rpc;
use volee_proto::{PROTOCOL_VERSION, blocking};

fn agent(n: u128) -> AgentId {
    AgentId::from_ulid(Ulid::from_parts(1_000, n))
}

fn registered(id: AgentId, parent: Option<AgentId>) -> Event {
    Event::new(
        Timestamp::from_millis(1),
        EventKind::AgentRegistered {
            agent: id,
            name: format!("agent-{}", id.short()),
            role: if parent.is_some() {
                Role::Worker
            } else {
                Role::Lead
            },
            harness: HarnessKind::ClaudeCode,
            launched_by: LaunchedBy::Human,
            parent,
            mission: None,
            workspace: Workspace::at("/work"),
            location: Location::Background,
        },
    )
}

struct Running {
    socket: PathBuf,
    task: JoinHandle<Result<(), volee_daemon::DaemonError>>,
    _dir: tempfile::TempDir,
}

/// Starts a daemon with a lead (agent 1), its worker (agent 2) and a stranger
/// (agent 3).
fn start() -> Running {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let log = MemoryLog::with(vec![
        registered(agent(1), None),
        registered(agent(2), Some(agent(1))),
        registered(agent(3), None),
    ]);
    let config = Config {
        paths,
        log: Box::new(log),
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
    };
    let task = tokio::spawn(Daemon::serve(listener, config));
    Running {
        socket,
        task,
        _dir: dir,
    }
}

async fn connect(socket: &Path) -> Client {
    Client::connect(socket).await.unwrap()
}

/// A daemon configuration on `paths`, with nothing to replay.
fn empty(paths: Paths) -> Config {
    Config {
        paths,
        log: Box::new(MemoryLog::with(vec![])),
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
    }
}

fn human_note(to: AgentId, body: &str) -> SendParams {
    SendParams {
        from: Party::Human,
        to: vec![Party::Agent(to)],
        kind: MessageKind::Note,
        body: body.to_owned(),
        refs: vec![],
        in_reply_to: None,
        thread: None,
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

#[tokio::test]
async fn status_reports_the_registry() {
    let daemon = start();
    let client = connect(&daemon.socket).await;
    let status = client
        .call::<Status>(StatusParams {
            protocol: PROTOCOL_VERSION,
        })
        .await
        .unwrap();
    assert_eq!(status.agents, 3);
    assert_eq!(status.last_seq, Some(3));
    assert_eq!(status.protocol, PROTOCOL_VERSION);
}

#[tokio::test]
async fn a_client_with_another_protocol_is_told_so() {
    let daemon = start();
    let client = connect(&daemon.socket).await;
    let error = client
        .call::<Status>(StatusParams { protocol: 999 })
        .await
        .unwrap_err();
    let ClientError::Rpc(error) = error else {
        panic!("expected an RPC error, got {error:?}");
    };
    assert_eq!(error.code, rpc::VERSION_MISMATCH);
    assert!(error.message.contains("protocol"));
}

#[tokio::test]
async fn subscribers_get_the_backlog_then_every_new_event_in_order() {
    let daemon = start();
    let mut subscriber = connect(&daemon.socket).await;
    let mut notifications = subscriber.take_notifications().unwrap();
    let backlog = subscriber
        .call::<Subscribe>(SubscribeParams {
            after: Some(1),
            interactive: false,
        })
        .await
        .unwrap();
    let seqs: Vec<u64> = backlog.events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [2, 3]);

    let sender = connect(&daemon.socket).await;
    sender
        .call::<SendMessage>(human_note(agent(2), "first"))
        .await
        .unwrap();
    let mut seen = Vec::new();
    while seen.len() < 2 {
        let notification = tokio::time::timeout(Duration::from_secs(2), notifications.recv())
            .await
            .unwrap()
            .unwrap();
        seen.push(appended_event(&notification).unwrap());
    }
    // A new direct thread, then the message.
    assert_eq!(seen[0].seq, 4);
    assert!(matches!(seen[0].event.kind, EventKind::ThreadOpened { .. }));
    assert_eq!(seen[1].seq, 5);
    assert!(matches!(
        seen[1].event.kind,
        EventKind::MessagePosted { .. }
    ));
}

#[tokio::test]
async fn strangers_cannot_message_each_other() {
    let daemon = start();
    let client = connect(&daemon.socket).await;
    let mut params = human_note(agent(3), "hello");
    params.from = Party::Agent(agent(2));
    let error = client.call::<SendMessage>(params).await.unwrap_err();
    let ClientError::Rpc(error) = error else {
        panic!("expected an RPC error, got {error:?}");
    };
    assert_eq!(error.code, rpc::REFUSED);
}

#[tokio::test]
async fn a_reply_lands_in_the_thread_of_the_question() {
    let daemon = start();
    let client = connect(&daemon.socket).await;
    let question = client
        .call::<SendMessage>(SendParams {
            from: Party::Agent(agent(2)),
            to: vec![Party::Human],
            kind: MessageKind::Question,
            body: "HT or TTC?".into(),
            refs: vec![],
            in_reply_to: None,
            thread: None,
        })
        .await
        .unwrap();
    let mut answer = human_note(agent(2), "TTC");
    answer.kind = MessageKind::Answer;
    answer.in_reply_to = Some(question.message);
    let sent = client.call::<SendMessage>(answer).await.unwrap();
    assert_eq!(sent.thread, question.thread);
    assert_eq!(
        registry(&client).await.unanswered_questions(),
        Vec::<&Message>::new()
    );
}

#[tokio::test]
async fn a_stop_hook_hands_pending_messages_to_the_agent() {
    let daemon = start();
    let client = connect(&daemon.socket).await;
    client
        .call::<SendMessage>(human_note(agent(2), "Also update the changelog."))
        .await
        .unwrap();
    let reply = client
        .call::<HookIngest>(HookParams {
            agent: agent(2),
            event: "stop".into(),
            payload: serde_json::json!({ "last_assistant_message": "Done." }),
        })
        .await
        .unwrap();
    let stdout: serde_json::Value = serde_json::from_str(&reply.stdout.unwrap()).unwrap();
    assert_eq!(stdout["decision"], "block");
    assert!(
        stdout["reason"]
            .as_str()
            .unwrap()
            .contains("Also update the changelog.")
    );
    let registry = registry(&client).await;
    assert_eq!(registry.pending_for(agent(2)), Vec::<&Message>::new());
    assert_eq!(registry.agent(agent(2)).unwrap().state, AgentState::Working);
}

#[tokio::test]
async fn a_stop_without_messages_marks_the_agent_done() {
    let daemon = start();
    let client = connect(&daemon.socket).await;
    let reply = client
        .call::<HookIngest>(HookParams {
            agent: agent(2),
            event: "stop".into(),
            payload: serde_json::json!({ "last_assistant_message": "\nAll tests pass.\nBye" }),
        })
        .await
        .unwrap();
    assert_eq!(reply.stdout, None);
    let registry = registry(&client).await;
    let worker = registry.agent(agent(2)).unwrap();
    assert_eq!(worker.state, AgentState::Done);
    assert_eq!(worker.detail.as_deref(), Some("All tests pass."));
}

#[tokio::test]
async fn a_waiting_agent_wakes_up_when_a_message_arrives() {
    let daemon = start();
    let waiter = connect(&daemon.socket).await;
    let wait = tokio::spawn(async move {
        waiter
            .call::<HookWait>(WaitParams {
                agent: agent(2),
                timeout_ms: 5_000,
            })
            .await
    });
    // Give the wait time to register before the message arrives.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let sender = connect(&daemon.socket).await;
    sender
        .call::<SendMessage>(human_note(agent(2), "Wake up and rebase."))
        .await
        .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(3), wait)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(reply.text.unwrap().contains("Wake up and rebase."));
    assert_eq!(
        registry(&sender).await.pending_for(agent(2)),
        Vec::<&Message>::new()
    );
}

#[tokio::test]
async fn a_wait_times_out_without_a_message() {
    let daemon = start();
    let client = connect(&daemon.socket).await;
    let reply = client
        .call::<HookWait>(WaitParams {
            agent: agent(2),
            timeout_ms: 50,
        })
        .await
        .unwrap();
    assert_eq!(reply.text, None);
}

#[tokio::test]
async fn forgetting_an_unknown_agent_is_not_found() {
    let daemon = start();
    let client = connect(&daemon.socket).await;
    // Its session ended: nothing runs any more.
    client
        .call::<HookIngest>(HookParams {
            agent: agent(3),
            event: "session-end".into(),
            payload: serde_json::json!({ "hook_event_name": "SessionEnd", "reason": "other" }),
        })
        .await
        .unwrap();
    client
        .call::<Forget>(AgentParams { agent: agent(3) })
        .await
        .unwrap();
    let error = client
        .call::<Forget>(AgentParams { agent: agent(3) })
        .await
        .unwrap_err();
    let ClientError::Rpc(error) = error else {
        panic!("expected an RPC error, got {error:?}");
    };
    assert_eq!(error.code, rpc::NOT_FOUND);
}

#[tokio::test]
async fn a_running_agent_volee_launched_is_not_forgotten() {
    let daemon = start();
    let client = connect(&daemon.socket).await;
    let error = client
        .call::<Forget>(AgentParams { agent: agent(1) })
        .await
        .unwrap_err();
    let ClientError::Rpc(error) = error else {
        panic!("expected an RPC error, got {error:?}");
    };
    assert_eq!(error.code, rpc::REFUSED);
    assert!(error.message.contains("stop it first"), "{}", error.message);
}

#[tokio::test]
async fn the_blocking_client_talks_to_the_daemon() {
    let daemon = start();
    let socket = daemon.socket.clone();
    let status = tokio::task::spawn_blocking(move || {
        blocking::call::<Status>(&socket, StatusParams::default(), Duration::from_secs(2))
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(status.agents, 3);
}

#[tokio::test]
async fn the_blocking_client_reports_an_unreachable_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("none.sock");
    let error = blocking::call::<Status>(
        &missing,
        StatusParams::default(),
        Duration::from_millis(100),
    )
    .unwrap_err();
    assert!(matches!(error, blocking::BlockingError::Unreachable(_)));
}

#[tokio::test]
async fn a_file_that_is_not_a_socket_is_never_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    std::fs::write(&paths.socket, "keep me").unwrap();
    let error = Daemon::run(empty(paths.clone())).await.unwrap_err();
    assert!(error.to_string().contains("not a socket"), "{error}");
    assert_eq!(std::fs::read_to_string(&paths.socket).unwrap(), "keep me");
}

#[tokio::test]
async fn a_stale_socket_is_replaced_by_a_private_one() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    // A socket that nobody listens on, as a crash leaves it.
    drop(std::os::unix::net::UnixListener::bind(&paths.socket).unwrap());
    let daemon = tokio::spawn(Daemon::run(empty(paths.clone())));
    let client = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(client) = Client::connect(&paths.socket).await {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let mode = std::fs::metadata(&paths.socket)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    client.call::<Shutdown>(Empty {}).await.unwrap();
    daemon.await.unwrap().unwrap();
}

#[tokio::test]
async fn shutdown_stops_the_daemon() {
    let daemon = start();
    let client = connect(&daemon.socket).await;
    client.call::<Shutdown>(Empty {}).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), daemon.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
