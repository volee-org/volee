//! End-to-end tests: the MCP server of a worker, backed by a real daemon on a
//! temporary socket.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use tokio::net::UnixListener;
use ulid::Ulid;
use volee_core::harness::HarnessKind;
use volee_core::message::{Message, MessageKind, Party};
use volee_core::role::{LaunchedBy, Role};
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};
use volee_core::{AgentId, Event, EventKind, Registry};
use volee_daemon::{Config, Daemon, ManualClock, MemoryLog};
use volee_mcp::VoleeMcp;
use volee_mcp::tools::ListArgs;
use volee_proto::client::Client;
use volee_proto::methods::{EventsSince, SendMessage, SendParams, SinceParams};
use volee_proto::paths::Paths;

fn agent(n: u128) -> AgentId {
    AgentId::from_ulid(Ulid::from_parts(1_000, n))
}

fn registered(id: AgentId, name: &str, role: Role, parent: Option<AgentId>) -> Event {
    Event::new(
        Timestamp::from_millis(1),
        EventKind::AgentRegistered {
            agent: id,
            name: name.to_owned(),
            role,
            harness: HarnessKind::ClaudeCode,
            launched_by: LaunchedBy::Human,
            parent,
            mission: None,
            workspace: Workspace {
                repo: Some("billing".into()),
                branch: Some(format!("feat/{name}")),
                ..Workspace::at("/work/billing")
            },
            location: Location::Background,
        },
    )
}

struct Fixture {
    socket: PathBuf,
    _dir: tempfile::TempDir,
}

/// A daemon with a lead (1), its worker "api" (2) and a stranger "docs" (3).
fn start() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let log = MemoryLog::with(vec![
        registered(agent(1), "lead", Role::Lead, None),
        registered(agent(2), "api", Role::Worker, Some(agent(1))),
        registered(agent(3), "docs", Role::Worker, None),
    ]);
    tokio::spawn(Daemon::serve(
        listener,
        Config {
            paths,
            log: Box::new(log),
            clock: Arc::new(ManualClock::at(5_000)),
            harnesses: vec![],
            spool: None,
        },
    ));
    Fixture { socket, _dir: dir }
}

async fn server_for(fixture: &Fixture, me: AgentId) -> VoleeMcp {
    let client = Client::connect(&fixture.socket).await.unwrap();
    VoleeMcp::connect(me, client).await.unwrap()
}

fn body(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| block.as_text().map(|text| text.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn failed(result: &CallToolResult) -> bool {
    result.is_error == Some(true)
}

async fn registry(fixture: &Fixture) -> Registry {
    let client = Client::connect(&fixture.socket).await.unwrap();
    let batch = client
        .call::<EventsSince>(SinceParams::default())
        .await
        .unwrap();
    let events: Vec<Event> = batch.events.into_iter().map(|e| e.event).collect();
    Registry::from_events(&events).unwrap()
}

/// Waits until the server's replica has seen a change made by another client.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(80)).await;
}

#[tokio::test]
async fn whoami_describes_the_caller() {
    let fixture = start();
    let server = server_for(&fixture, agent(2)).await;
    let result = server.whoami().await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&body(&result)).unwrap();
    assert_eq!(json["name"], "api");
    assert_eq!(json["role"], "worker");
    assert_eq!(json["parent"], "lead");
    assert_eq!(json["repo"], "billing");
}

#[tokio::test]
async fn an_unknown_agent_cannot_start_a_server() {
    let fixture = start();
    let client = Client::connect(&fixture.socket).await.unwrap();
    let error = VoleeMcp::connect(agent(99), client).await.unwrap_err();
    assert!(error.to_string().contains("does not know agent"));
}

#[tokio::test]
async fn list_agents_says_who_the_caller_may_write_to() {
    let fixture = start();
    let server = server_for(&fixture, agent(2)).await;
    let result = server
        .list_agents(Parameters(ListArgs::default()))
        .await
        .unwrap();
    let agents: Vec<serde_json::Value> = serde_json::from_str(&body(&result)).unwrap();
    let can = |name: &str| {
        agents
            .iter()
            .find(|a| a["name"] == name)
            .map(|a| a["can_message"].clone())
            .unwrap()
    };
    assert_eq!(can("lead"), true);
    assert_eq!(can("docs"), false);
    assert_eq!(
        can("api"),
        serde_json::Value::Null,
        "no flag for the caller itself"
    );
}

#[tokio::test]
async fn messages_reach_named_recipients_and_strangers_are_refused() {
    let fixture = start();
    let server = server_for(&fixture, agent(2)).await;
    let sent = server
        .send_message(Parameters(
            serde_json::from_value(serde_json::json!({
                "to": ["LEAD"],
                "kind": "question",
                "body": "Should the invoice total include VAT?"
            }))
            .unwrap(),
        ))
        .await
        .unwrap();
    assert!(!failed(&sent), "{}", body(&sent));
    let registry = registry(&fixture).await;
    let pending = registry.pending_for(agent(1));
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].kind, MessageKind::Question);
    assert_eq!(pending[0].from, Party::Agent(agent(2)));

    let refused = server
        .send_message(Parameters(
            serde_json::from_value(serde_json::json!({ "to": ["docs"], "body": "hello" })).unwrap(),
        ))
        .await
        .unwrap();
    assert!(failed(&refused));
    assert!(
        body(&refused).contains("may not write"),
        "{}",
        body(&refused)
    );

    let unknown = server
        .send_message(Parameters(
            serde_json::from_value(serde_json::json!({ "to": ["nobody"], "body": "hello" }))
                .unwrap(),
        ))
        .await
        .unwrap();
    assert!(failed(&unknown));
    assert!(body(&unknown).contains("list_agents"));
}

#[tokio::test]
async fn read_messages_returns_unread_messages_once() {
    let fixture = start();
    let server = server_for(&fixture, agent(2)).await;
    let human = Client::connect(&fixture.socket).await.unwrap();
    human
        .call::<SendMessage>(SendParams {
            from: Party::Human,
            to: vec![Party::Agent(agent(2))],
            kind: MessageKind::Note,
            body: "Rebase on main before opening the PR.".into(),
            refs: vec![],
            in_reply_to: None,
            thread: None,
        })
        .await
        .unwrap();
    settle().await;
    let args = || serde_json::from_value(serde_json::json!({})).unwrap();
    let first = server.read_messages(Parameters(args())).await.unwrap();
    assert!(body(&first).contains("Rebase on main"), "{}", body(&first));
    assert!(body(&first).contains("the human"));
    settle().await;
    let second = server.read_messages(Parameters(args())).await.unwrap();
    assert_eq!(body(&second), "No messages.");
    let all = server
        .read_messages(Parameters(
            serde_json::from_value(serde_json::json!({ "unread_only": false })).unwrap(),
        ))
        .await
        .unwrap();
    assert!(body(&all).contains("Rebase on main"));
}

#[tokio::test]
async fn report_reaches_the_parent_and_the_human() {
    let fixture = start();
    let server = server_for(&fixture, agent(2)).await;
    let result = server
        .report(Parameters(
            serde_json::from_value(serde_json::json!({
                "outcome": "done",
                "summary": "Invoices endpoint added, tests green.",
                "refs": [{ "url": "https://github.com/acme/billing/pull/42" }]
            }))
            .unwrap(),
        ))
        .await
        .unwrap();
    assert!(!failed(&result), "{}", body(&result));
    let registry = registry(&fixture).await;
    let report: &Message = registry.pending_for(agent(1))[0];
    assert_eq!(report.kind, MessageKind::Report);
    assert!(report.is_for(Party::Human));
    assert!(report.body.starts_with("**Done.**"));
    assert_eq!(report.refs.len(), 1);
}

#[tokio::test]
async fn a_real_mcp_client_sees_the_five_tools() {
    let fixture = start();
    let server = server_for(&fixture, agent(2)).await;
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let running = server.serve(server_io).await.unwrap();
        let _ = running.waiting().await;
    });
    let client = ().serve(client_io).await.unwrap();
    let mut names: Vec<String> = client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "list_agents",
            "read_messages",
            "report",
            "send_message",
            "whoami"
        ]
    );
    let result = client
        .call_tool(CallToolRequestParams::new("whoami"))
        .await
        .unwrap();
    assert!(body(&result).contains("\"api\""));
}
