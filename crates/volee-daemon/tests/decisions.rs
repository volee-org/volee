//! Held permission requests and questions end to end: a real daemon, real
//! HTTP requests standing in for Claude Code's hooks, and the browser's API.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use common::FakeHarness;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixListener};
use volee_daemon::{
    AgentsViewConfig, Config, Daemon, EvidenceSources, HoldSettings, HookServerConfig, ManualClock,
    MemoryLog,
};
use volee_harness::Harness;
use volee_proto::paths::Paths;

const TOKEN: &str = "decisions-test-token";

struct Running {
    addr: SocketAddr,
    harness: Arc<FakeHarness>,
    _dir: tempfile::TempDir,
}

async fn start(permission_ms: u64, question_ms: u64) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let paths = Paths::rooted(root.join("volee"));
    std::fs::create_dir_all(&paths.state_dir).unwrap();
    let database = paths.database();
    let listener = UnixListener::bind(&paths.socket).unwrap();
    let addr = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap()
    };
    let harness = Arc::new(FakeHarness::default());
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
            refresh_interval: Duration::from_secs(3_600),
            ..EvidenceSources::default()
        },
        processes: None,
        hook_server: Some(HookServerConfig {
            addr,
            token: TOKEN.into(),
            database: database.clone(),
            hold: HoldSettings {
                permission_timeout: Duration::from_millis(permission_ms),
                question_delay: Duration::from_millis(question_ms),
            },
        }),
        agents_view: Some(AgentsViewConfig {
            poll_interval: Duration::from_millis(200),
            database,
        }),
    };
    tokio::spawn(Daemon::serve(listener, config));
    for _ in 0..100 {
        if TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Running {
        addr,
        harness,
        _dir: dir,
    }
}

fn message(addr: SocketAddr, method: &str, path: &str, token: &str, body: &str) -> String {
    format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// One request; the status and the body.
async fn call(
    addr: SocketAddr,
    method: &str,
    path: &str,
    token: &str,
    body: &Value,
) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let text = if body.is_null() {
        String::new()
    } else {
        body.to_string()
    };
    stream
        .write_all(message(addr, method, path, token, &text).as_bytes())
        .await
        .unwrap();
    let mut answer = String::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_string(&mut answer))
        .await
        .unwrap()
        .unwrap();
    let status = answer.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = answer.split_once("\r\n\r\n").map_or("", |(_, body)| body);
    (status, body.to_owned())
}

/// Starts a hook request in the background, as Claude Code would send it.
fn hook(addr: SocketAddr, path: &str, body: Value) -> tokio::task::JoinHandle<(u16, String)> {
    let path = path.to_owned();
    tokio::spawn(async move { call(addr, "POST", &path, TOKEN, &body).await })
}

async fn inbox(addr: SocketAddr) -> Value {
    let (status, body) = call(addr, "GET", "/api/inbox", TOKEN, &Value::Null).await;
    assert_eq!(status, 200);
    serde_json::from_str(&body).unwrap()
}

/// Waits until the inbox holds `count` decision cards, and returns them.
async fn decisions(addr: SocketAddr, count: usize) -> Vec<Value> {
    for _ in 0..100 {
        let found = inbox(addr).await["decisions"].as_array().unwrap().clone();
        if found.len() == count {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the inbox never held {count} decision cards");
}

async fn first_id(addr: SocketAddr) -> String {
    decisions(addr, 1).await[0]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn permission_request(session: &str, command: &str) -> Value {
    json!({
        "hook_event_name": "PermissionRequest",
        "session_id": session,
        "cwd": "/work/app",
        "tool_name": "Bash",
        "tool_input": { "command": command },
        "permission_suggestions": [{
            "type": "addRules",
            "rules": [{ "toolName": "Bash", "ruleContent": command }],
            "behavior": "allow",
            "destination": "localSettings",
        }],
    })
}

fn question_request(session: &str) -> Value {
    json!({
        "hook_event_name": "PreToolUse",
        "session_id": session,
        "tool_name": "AskUserQuestion",
        "tool_use_id": "toolu_1",
        "tool_input": { "questions": [{
            "question": "Which color?",
            "header": "Color",
            "multiSelect": false,
            "options": [
                { "label": "Red", "description": "warm" },
                { "label": "Blue", "description": "cold" },
            ],
        }] },
    })
}

async fn decide(addr: SocketAddr, id: &str, body: Value) -> (u16, String) {
    call(addr, "POST", &format!("/api/decisions/{id}"), TOKEN, &body).await
}

#[tokio::test]
async fn a_permission_is_held_until_the_browser_allows_it() {
    let daemon = start(10_000, 10_000).await;
    let held = hook(
        daemon.addr,
        "/hooks/PermissionRequest",
        permission_request("s1", "npm test"),
    );
    let cards = decisions(daemon.addr, 1).await;
    assert_eq!(cards[0]["kind"], "permission");
    assert_eq!(cards[0]["tool_name"], "Bash");
    assert_eq!(cards[0]["summary"], "npm test");
    assert_eq!(cards[0]["session_id"], "s1");
    assert_eq!(cards[0]["suggestions"][0]["index"], 0);
    assert!(!held.is_finished(), "the request waits for the person");

    let id = cards[0]["id"].as_str().unwrap();
    let (status, _) = decide(
        daemon.addr,
        id,
        json!({ "action": "allow", "suggestion": 0 }),
    )
    .await;
    assert_eq!(status, 200);
    let (status, body) = held.await.unwrap();
    assert_eq!(status, 200);
    let body: Value = serde_json::from_str(&body).unwrap();
    let decision = &body["hookSpecificOutput"]["decision"];
    assert_eq!(decision["behavior"], "allow");
    assert_eq!(
        decision["updatedPermissions"][0]["rules"][0]["ruleContent"],
        "npm test"
    );
    decisions(daemon.addr, 0).await;
}

#[tokio::test]
async fn a_permission_is_denied_with_a_message() {
    let daemon = start(10_000, 10_000).await;
    let held = hook(
        daemon.addr,
        "/hooks/PermissionRequest",
        permission_request("s1", "rm -rf /"),
    );
    let id = first_id(daemon.addr).await;
    let (status, _) = decide(
        daemon.addr,
        &id,
        json!({ "action": "deny", "message": "no way" }),
    )
    .await;
    assert_eq!(status, 200);
    let (_, body) = held.await.unwrap();
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["hookSpecificOutput"]["decision"]["behavior"], "deny");
    assert_eq!(body["hookSpecificOutput"]["decision"]["message"], "no way");
}

#[tokio::test]
async fn answering_in_the_terminal_first_clears_the_card() {
    let daemon = start(10_000, 10_000).await;
    let held = hook(
        daemon.addr,
        "/hooks/PermissionRequest",
        permission_request("s1", "ls"),
    );
    let id = first_id(daemon.addr).await;
    // The tool ran: the terminal's dialog was answered.
    let (status, _) = call(
        daemon.addr,
        "POST",
        "/hooks/PostToolUse",
        TOKEN,
        &json!({
            "hook_event_name": "PostToolUse",
            "session_id": "s1",
            "tool_name": "Bash",
            "tool_input": { "command": "ls" },
        }),
    )
    .await;
    assert_eq!(status, 200);
    let (status, body) = held.await.unwrap();
    assert_eq!((status, body.as_str()), (200, ""), "nothing is answered");
    decisions(daemon.addr, 0).await;
    let (status, _) = decide(daemon.addr, &id, json!({ "action": "allow" })).await;
    assert_eq!(status, 404, "a late click finds nothing to decide");
}

#[tokio::test]
async fn a_permission_nobody_decides_runs_out_and_the_terminal_keeps_its_dialog() {
    let daemon = start(300, 10_000).await;
    let held = hook(
        daemon.addr,
        "/hooks/PermissionRequest",
        permission_request("s1", "ls"),
    );
    decisions(daemon.addr, 1).await;
    let (status, body) = held.await.unwrap();
    assert_eq!((status, body.as_str()), (200, ""));
    decisions(daemon.addr, 0).await;
}

#[tokio::test]
async fn a_connection_closed_by_claude_code_clears_the_card() {
    let daemon = start(10_000, 10_000).await;
    let mut stream = TcpStream::connect(daemon.addr).await.unwrap();
    let body = permission_request("s1", "ls").to_string();
    stream
        .write_all(
            message(
                daemon.addr,
                "POST",
                "/hooks/PermissionRequest",
                TOKEN,
                &body,
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    decisions(daemon.addr, 1).await;
    drop(stream);
    decisions(daemon.addr, 0).await;
}

#[tokio::test]
async fn what_the_terminal_keeps_is_not_held() {
    let daemon = start(10_000, 10_000).await;
    let mut request = permission_request("s1", "x");
    request["tool_name"] = json!("AskUserQuestion");
    let (status, body) = call(
        daemon.addr,
        "POST",
        "/hooks/PermissionRequest",
        TOKEN,
        &request,
    )
    .await;
    assert_eq!((status, body.as_str()), (200, ""));
    assert_eq!(
        inbox(daemon.addr).await["decisions"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn a_wrong_decision_is_refused_and_keeps_the_card() {
    let daemon = start(10_000, 10_000).await;
    let held = hook(
        daemon.addr,
        "/hooks/PermissionRequest",
        permission_request("s1", "ls"),
    );
    let id = first_id(daemon.addr).await;
    let (status, _) = call(
        daemon.addr,
        "POST",
        &format!("/api/decisions/{id}"),
        "wrong",
        &json!({ "action": "allow" }),
    )
    .await;
    assert_eq!(status, 401);
    let (status, _) = decide(
        daemon.addr,
        &id,
        json!({ "action": "allow", "suggestion": 7 }),
    )
    .await;
    assert_eq!(status, 422);
    let (status, _) = decide(
        daemon.addr,
        &id,
        json!({ "action": "answer", "answers": {} }),
    )
    .await;
    assert_eq!(status, 422);
    let (status, _) = decide(daemon.addr, "nope", json!({ "action": "allow" })).await;
    assert_eq!(status, 404);
    assert_eq!(decisions(daemon.addr, 1).await.len(), 1);
    decide(daemon.addr, &id, json!({ "action": "terminal" })).await;
    let (_, body) = held.await.unwrap();
    assert_eq!(body, "", "the terminal asks");
}

async fn until(stream: &mut TcpStream, seen: &mut String, from: usize, needle: &str) {
    let mut buffer = [0_u8; 8192];
    tokio::time::timeout(Duration::from_secs(10), async {
        while !seen[from..].contains(needle) {
            let n = stream.read(&mut buffer).await.unwrap();
            assert!(n > 0, "the stream ended: {seen}");
            seen.push_str(&String::from_utf8_lossy(&buffer[..n]));
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {needle:?} in {seen:?}"));
}

#[tokio::test]
async fn two_tabs_both_see_the_card_come_and_go() {
    let daemon = start(10_000, 10_000).await;
    let mut tabs = Vec::new();
    for _ in 0..2 {
        let mut stream = TcpStream::connect(daemon.addr).await.unwrap();
        let request = format!(
            "GET /api/events?token={TOKEN} HTTP/1.1\r\nHost: {}\r\nAccept: text/event-stream\r\n\r\n",
            daemon.addr
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        tabs.push((stream, String::new()));
    }
    let held = hook(
        daemon.addr,
        "/hooks/PermissionRequest",
        permission_request("s1", "make"),
    );
    for (stream, seen) in &mut tabs {
        until(stream, seen, 0, "\"summary\":\"make\"").await;
    }
    let marks: Vec<usize> = tabs.iter().map(|(_, seen)| seen.len()).collect();
    let id = first_id(daemon.addr).await;
    decide(daemon.addr, &id, json!({ "action": "allow" })).await;
    held.await.unwrap();
    for ((stream, seen), mark) in tabs.iter_mut().zip(marks) {
        // What the tab received after the card was there: no decision left.
        until(stream, seen, mark, "\"decisions\":[]").await;
    }
}

#[tokio::test]
async fn a_question_is_answered_from_the_browser_and_the_questions_are_echoed() {
    let daemon = start(10_000, 10_000).await;
    let held = hook(
        daemon.addr,
        "/decide/AskUserQuestion",
        question_request("s1"),
    );
    let cards = decisions(daemon.addr, 1).await;
    assert_eq!(cards[0]["kind"], "question");
    assert_eq!(cards[0]["questions"][0]["options"][1]["label"], "Blue");
    let id = cards[0]["id"].as_str().unwrap();
    let (status, _) = decide(
        daemon.addr,
        id,
        json!({ "action": "answer", "answers": { "Which color?": "Blue" } }),
    )
    .await;
    assert_eq!(status, 200);
    let (status, body) = held.await.unwrap();
    assert_eq!(status, 200);
    let body: Value = serde_json::from_str(&body).unwrap();
    let output = &body["hookSpecificOutput"];
    assert_eq!(output["permissionDecision"], "allow");
    assert_eq!(output["updatedInput"]["answers"]["Which color?"], "Blue");
    assert_eq!(output["updatedInput"]["questions"][0]["header"], "Color");
}

#[tokio::test]
async fn a_question_can_be_denied_with_a_reason() {
    let daemon = start(10_000, 10_000).await;
    let held = hook(
        daemon.addr,
        "/decide/AskUserQuestion",
        question_request("s1"),
    );
    let id = first_id(daemon.addr).await;
    decide(
        daemon.addr,
        &id,
        json!({ "action": "deny", "message": "pick one yourself" }),
    )
    .await;
    let (_, body) = held.await.unwrap();
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["hookSpecificOutput"]["permissionDecision"], "deny");
    assert_eq!(
        body["hookSpecificOutput"]["permissionDecisionReason"],
        "pick one yourself"
    );
}

#[tokio::test]
async fn an_unanswered_question_goes_back_to_the_terminal_after_the_delay() {
    let daemon = start(10_000, 300).await;
    let started = std::time::Instant::now();
    let held = hook(
        daemon.addr,
        "/decide/AskUserQuestion",
        question_request("s1"),
    );
    let cards = decisions(daemon.addr, 1).await;
    assert!(cards[0]["expires_at"].as_i64().unwrap() > cards[0]["created_at"].as_i64().unwrap());
    let (status, body) = held.await.unwrap();
    assert_eq!((status, body.as_str()), (200, ""));
    assert!(started.elapsed() >= Duration::from_millis(250));
    decisions(daemon.addr, 0).await;
}

#[tokio::test]
async fn a_hook_without_the_token_is_refused_and_holds_nothing() {
    let daemon = start(10_000, 10_000).await;
    let (status, _) = call(
        daemon.addr,
        "POST",
        "/decide/AskUserQuestion",
        "wrong",
        &question_request("s1"),
    )
    .await;
    assert_eq!(status, 401);
    let (status, _) = call(
        daemon.addr,
        "POST",
        "/hooks/PermissionRequest",
        "wrong",
        &permission_request("s1", "ls"),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(
        inbox(daemon.addr).await["decisions"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn a_slow_reading_of_the_agents_never_delays_a_card() {
    let daemon = start(10_000, 10_000).await;
    daemon.harness.slow_listings(Duration::from_secs(4));
    // A hook event makes the poller read now, and the reading takes seconds.
    let (status, _) = call(
        daemon.addr,
        "POST",
        "/hooks/UserPromptSubmit",
        TOKEN,
        &json!({ "hook_event_name": "UserPromptSubmit", "session_id": "s1" }),
    )
    .await;
    assert_eq!(status, 200);
    tokio::time::sleep(Duration::from_millis(900)).await;
    let started = std::time::Instant::now();
    let held = hook(
        daemon.addr,
        "/hooks/PermissionRequest",
        permission_request("s1", "ls"),
    );
    let id = first_id(daemon.addr).await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the card waited for the reading: {:?}",
        started.elapsed()
    );
    decide(daemon.addr, &id, json!({ "action": "terminal" })).await;
    held.await.unwrap();
    assert_eq!(decisions(daemon.addr, 0).await.len(), 0);
}
