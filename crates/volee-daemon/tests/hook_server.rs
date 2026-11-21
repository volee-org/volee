//! The hook server end to end: real sockets, a real SQLite database, and the
//! requests Claude Code's `http` hooks send.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use volee_daemon::HookServerConfig;
use volee_daemon::hook_server;
use volee_store::{HookFilter, Store};

const TOKEN: &str = "0123456789abcdef";

struct Server {
    addr: SocketAddr,
    database: PathBuf,
    _dir: tempfile::TempDir,
}

async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("volee.db");
    let config = HookServerConfig {
        addr: "127.0.0.1:0".parse().unwrap(),
        token: TOKEN.into(),
        database: database.clone(),
    };
    let listener = hook_server::bind(&config).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let store = Store::open(&database).unwrap();
    tokio::spawn(hook_server::serve(listener, TOKEN.into(), store));
    Server {
        addr,
        database,
        _dir: dir,
    }
}

/// One HTTP/1.1 request, as a hook sends it; returns the status and the body.
async fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let auth = token.map_or_else(String::new, |t| format!("Authorization: Bearer {t}\r\n"));
    let message = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}Content-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(message.as_bytes()).await.unwrap();
    let mut answer = String::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_string(&mut answer))
        .await
        .unwrap()
        .unwrap();
    let status = answer.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = answer
        .split_once("\r\n\r\n")
        .map_or_else(String::new, |(_, body)| body.to_owned());
    (status, body)
}

fn stored(server: &Server) -> Vec<volee_store::HookEvent> {
    Store::open(&server.database)
        .unwrap()
        .hook_events(&HookFilter::default())
        .unwrap()
}

#[tokio::test]
async fn an_authenticated_hook_is_stored_as_received_and_answered_with_an_empty_200() {
    let server = start().await;
    let payload = r#"{"session_id":"s1","cwd":"/work/alpha","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"ls"}}"#;
    let (status, body) = request(
        server.addr,
        "POST",
        "/hooks/PreToolUse",
        Some(TOKEN),
        payload,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body, "", "an empty body is Claude Code's 'no opinion'");
    let events = stored(&server);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event, "PreToolUse");
    assert_eq!(events[0].session_id.as_deref(), Some("s1"));
    assert_eq!(events[0].cwd.as_deref(), Some("/work/alpha"));
    assert_eq!(
        events[0].payload, payload,
        "the payload is kept byte for byte"
    );
    assert!(events[0].received_at > 1_700_000_000_000);
}

#[tokio::test]
async fn events_are_stored_in_the_order_they_arrive() {
    let server = start().await;
    for name in ["SessionStart", "UserPromptSubmit", "Stop"] {
        let payload = format!(r#"{{"session_id":"s1","hook_event_name":"{name}"}}"#);
        let (status, _) = request(
            server.addr,
            "POST",
            &format!("/hooks/{name}"),
            Some(TOKEN),
            &payload,
        )
        .await;
        assert_eq!(status, 200);
    }
    let names: Vec<String> = stored(&server).into_iter().map(|e| e.event).collect();
    assert_eq!(names, ["SessionStart", "UserPromptSubmit", "Stop"]);
}

#[tokio::test]
async fn a_request_without_the_right_token_is_refused_and_stores_nothing() {
    let server = start().await;
    let payload = r#"{"session_id":"s1","hook_event_name":"Stop"}"#;
    for token in [None, Some("wrong"), Some("0123456789abcdeg"), Some("")] {
        let (status, _) = request(server.addr, "POST", "/hooks/Stop", token, payload).await;
        assert_eq!(status, 401, "{token:?}");
    }
    assert_eq!(stored(&server), []);
}

#[tokio::test]
async fn a_body_that_is_not_a_json_object_is_refused() {
    let server = start().await;
    for body in ["not json", "[1,2]", "\"text\"", ""] {
        let (status, _) = request(server.addr, "POST", "/hooks/Stop", Some(TOKEN), body).await;
        assert_eq!(status, 400, "{body:?}");
    }
    assert_eq!(stored(&server), []);
}

#[tokio::test]
async fn other_paths_and_methods_are_not_found_and_health_needs_no_token() {
    let server = start().await;
    let (status, _) = request(server.addr, "POST", "/elsewhere", Some(TOKEN), "{}").await;
    assert_eq!(status, 404);
    let (status, _) = request(server.addr, "GET", "/hooks/Stop", Some(TOKEN), "").await;
    assert_eq!(status, 405);
    let (status, body) = request(server.addr, "GET", "/healthz", None, "").await;
    assert_eq!((status, body.as_str()), (200, "ok"));
    assert_eq!(stored(&server), []);
}

#[tokio::test]
async fn the_hook_server_never_listens_beyond_the_loopback_interface() {
    let dir = tempfile::tempdir().unwrap();
    for addr in ["0.0.0.0:0", "192.168.1.10:0", "[::]:0"] {
        let config = HookServerConfig {
            addr: addr.parse().unwrap(),
            token: TOKEN.into(),
            database: dir.path().join("volee.db"),
        };
        let error = hook_server::bind(&config).await.unwrap_err();
        assert!(error.to_string().contains("loopback"), "{addr}: {error}");
    }
}

#[tokio::test]
async fn a_port_already_in_use_is_reported_with_the_setting_to_change() {
    let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = HookServerConfig {
        addr: taken.local_addr().unwrap(),
        token: TOKEN.into(),
        database: dir.path().join("volee.db"),
    };
    let error = hook_server::bind(&config).await.unwrap_err();
    assert!(error.to_string().contains("[hooks] port"), "{error}");
}
