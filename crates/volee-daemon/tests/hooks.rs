//! Hook ingestion end to end: Claude Code hook payloads, as `volee hook`
//! forwards them, change the agent's state and activity.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::net::UnixListener;
use ulid::Ulid;
use volee_core::harness::{HarnessKind, SessionKey};
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::{AgentState, InputRequest};
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};
use volee_core::{Agent, AgentId, Event, EventKind, Registry};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_proto::client::Client;
use volee_proto::methods::{EventsSince, HookIngest, HookParams, SinceParams};
use volee_proto::paths::Paths;

fn agent_id() -> AgentId {
    AgentId::from_ulid(Ulid::from_parts(1_000, 1))
}

struct Running {
    socket: PathBuf,
    _dir: tempfile::TempDir,
}

/// A daemon that knows one agent launched by volee, before its first hook.
fn start() -> Running {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let registered = Event::new(
        Timestamp::from_millis(1),
        EventKind::AgentRegistered {
            agent: agent_id(),
            name: "invoices".into(),
            role: Role::Worker,
            harness: HarnessKind::ClaudeCode,
            launched_by: LaunchedBy::Human,
            parent: None,
            mission: None,
            workspace: Workspace::at("/work/alpha"),
            location: Location::Background,
        },
    );
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(vec![registered])),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: None,
        discovery_interval: volee_daemon::DEFAULT_DISCOVERY_INTERVAL,
        idle_discovery_interval: volee_daemon::DEFAULT_DISCOVERY_INTERVAL,
        evidence: EvidenceSources::default(),
        volee_bin: None,
    };
    tokio::spawn(Daemon::serve(listener, config));
    Running { socket, _dir: dir }
}

async fn hook(daemon: &Running, event: &str, payload: Value) -> Option<String> {
    let client = Client::connect(&daemon.socket).await.unwrap();
    client
        .call::<HookIngest>(HookParams {
            agent: agent_id(),
            event: event.into(),
            payload,
        })
        .await
        .unwrap()
        .stdout
}

async fn agent(daemon: &Running) -> Agent {
    let client = Client::connect(&daemon.socket).await.unwrap();
    let batch = client
        .call::<EventsSince>(SinceParams::default())
        .await
        .unwrap();
    let events: Vec<Event> = batch.events.into_iter().map(|e| e.event).collect();
    Registry::from_events(&events)
        .unwrap()
        .agent(agent_id())
        .unwrap()
        .clone()
}

#[tokio::test]
async fn session_start_links_the_session_and_records_the_model() {
    let daemon = start();
    hook(
        &daemon,
        "session-start",
        json!({
            "hook_event_name": "SessionStart",
            "session_id": "aaaa1111-2222-3333-4444-555566667777",
            "source": "startup",
            "model": "claude-sonnet-5-5",
        }),
    )
    .await;
    let agent = agent(&daemon).await;
    assert!(agent.owns_session(&SessionKey::claude("aaaa1111-2222-3333-4444-555566667777")));
    assert_eq!(agent.model.as_deref(), Some("claude-sonnet-5-5"));
    assert_eq!(agent.state, AgentState::Idle);
}

#[tokio::test]
async fn a_tool_is_recorded_as_readable_activity() {
    let daemon = start();
    hook(
        &daemon,
        "pre-tool-use",
        json!({
            "hook_event_name": "PreToolUse",
            "session_id": "s",
            "tool_name": "Bash",
            "tool_input": { "command": "cargo test --workspace" },
        }),
    )
    .await;
    let agent = agent(&daemon).await;
    assert_eq!(agent.state, AgentState::Working);
    let activity = agent.last_activity.unwrap().summary;
    assert!(activity.contains("cargo test"), "{activity}");
}

#[tokio::test]
async fn a_permission_prompt_and_a_question_mean_the_agent_needs_you() {
    let daemon = start();
    hook(
        &daemon,
        "notification",
        json!({
            "hook_event_name": "Notification",
            "session_id": "s",
            "notification_type": "permission_prompt",
            "message": "Claude needs your permission to use Bash",
        }),
    )
    .await;
    let agent_now = agent(&daemon).await;
    assert!(
        matches!(
            agent_now.state,
            AgentState::NeedsInput {
                request: InputRequest::Permission { .. }
            }
        ),
        "{:?}",
        agent_now.state
    );
    assert!(agent_now.detail.unwrap().contains("permission"));

    hook(
        &daemon,
        "pre-tool-use",
        json!({
            "hook_event_name": "PreToolUse",
            "session_id": "s",
            "tool_name": "AskUserQuestion",
            "tool_input": { "questions": [{ "question": "Which base branch?" }] },
        }),
    )
    .await;
    assert!(matches!(
        agent(&daemon).await.state,
        AgentState::NeedsInput {
            request: InputRequest::Question
        }
    ));
}

#[tokio::test]
async fn the_end_of_a_turn_is_done_with_its_last_words() {
    let daemon = start();
    let stdout = hook(
        &daemon,
        "stop",
        json!({
            "hook_event_name": "Stop",
            "session_id": "s",
            "last_assistant_message": "Opened the pull request.\nDetails follow.",
        }),
    )
    .await;
    assert_eq!(stdout, None, "nothing to deliver, nothing printed");
    let agent = agent(&daemon).await;
    assert_eq!(agent.state, AgentState::Done);
    assert_eq!(agent.detail.as_deref(), Some("Opened the pull request."));
}

#[tokio::test]
async fn clear_replaces_the_session_without_stopping_the_agent() {
    let daemon = start();
    hook(
        &daemon,
        "user-prompt-submit",
        json!({ "hook_event_name": "UserPromptSubmit", "session_id": "s", "prompt": "go" }),
    )
    .await;
    hook(
        &daemon,
        "session-end",
        json!({ "hook_event_name": "SessionEnd", "session_id": "s", "reason": "clear" }),
    )
    .await;
    assert_eq!(agent(&daemon).await.state, AgentState::Working);

    hook(
        &daemon,
        "session-end",
        json!({ "hook_event_name": "SessionEnd", "session_id": "s", "reason": "logout" }),
    )
    .await;
    let agent = agent(&daemon).await;
    assert_eq!(agent.state, AgentState::Stopped);
    assert_eq!(agent.detail.as_deref(), Some("logout"));
}

#[tokio::test]
async fn a_payload_without_its_event_name_takes_it_from_the_command_line() {
    let daemon = start();
    hook(&daemon, "user-prompt-submit", json!({ "prompt": "go" })).await;
    assert_eq!(agent(&daemon).await.state, AgentState::Working);
}
