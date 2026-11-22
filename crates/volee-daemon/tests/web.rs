//! The web interface end to end: a real daemon with the hook server and the
//! web routes on one loopback port, a fake `claude agents` listing, and a
//! plain HTTP client.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::FakeHarness;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixListener};
use volee_core::harness::SessionKey;
use volee_core::reconcile::ListedSession;
use volee_core::state::{AgentState, InputRequest};
use volee_daemon::{
    AgentsViewConfig, Config, Daemon, EvidenceSources, HoldSettings, HookServerConfig, ManualClock,
    MemoryLog,
};
use volee_harness::Harness;
use volee_proto::paths::Paths;

const TOKEN: &str = "web-test-token";

struct Running {
    addr: SocketAddr,
    harness: Arc<FakeHarness>,
    cwd: PathBuf,
    _dir: tempfile::TempDir,
}

async fn start() -> Running {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let cwd = root.join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let paths = Paths::rooted(root.join("volee"));
    std::fs::create_dir_all(&paths.state_dir).unwrap();
    let database = paths.database();
    let listener = UnixListener::bind(&paths.socket).unwrap();
    // A free port: bound and released, the daemon binds it next.
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
            hold: HoldSettings::default(),
            addr,
            token: TOKEN.into(),
            database: database.clone(),
        }),
        agents_view: Some(AgentsViewConfig {
            poll_interval: Duration::from_millis(100),
            database,
        }),
    };
    tokio::spawn(Daemon::serve(listener, config));
    // The listener is bound once the daemon is ready.
    for _ in 0..100 {
        if TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Running {
        addr,
        harness,
        cwd,
        _dir: dir,
    }
}

fn blocked(id: &str, cwd: &std::path::Path) -> ListedSession {
    ListedSession {
        alive: true,
        pid: Some(4242),
        name: Some("billing".into()),
        ..ListedSession::new(
            SessionKey::claude(id),
            cwd,
            AgentState::NeedsInput {
                request: InputRequest::Permission {
                    tool: Some("Bash".into()),
                },
            },
        )
    }
}

/// One request; returns the status, the headers and the body.
async fn get(
    addr: SocketAddr,
    method: &str,
    path: &str,
    host: &str,
    token: Option<&str>,
) -> (u16, String, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let auth = token.map_or_else(String::new, |t| format!("Authorization: Bearer {t}\r\n"));
    let message = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{auth}Content-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(message.as_bytes()).await.unwrap();
    let mut answer = String::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_string(&mut answer))
        .await
        .unwrap()
        .unwrap();
    let status = answer.split_whitespace().nth(1).unwrap().parse().unwrap();
    let (head, body) = answer.split_once("\r\n\r\n").unwrap();
    (status, head.to_owned(), body.to_owned())
}

#[tokio::test]
async fn the_page_is_public_and_carries_its_protections() {
    let daemon = start().await;
    let host = daemon.addr.to_string();
    let (status, head, body) = get(daemon.addr, "GET", "/", &host, None).await;
    assert_eq!(status, 200);
    assert!(body.contains("<html"), "{body}");
    let head = head.to_lowercase();
    assert!(head.contains("content-security-policy"), "{head}");
    assert!(head.contains("text/html"), "{head}");
}

#[tokio::test]
async fn the_api_needs_the_token_and_a_loopback_host() {
    let daemon = start().await;
    let host = daemon.addr.to_string();
    let (status, ..) = get(daemon.addr, "GET", "/api/inbox", &host, None).await;
    assert_eq!(status, 401);
    let (status, ..) = get(daemon.addr, "GET", "/api/inbox", &host, Some("wrong")).await;
    assert_eq!(status, 401);
    let (status, ..) = get(
        daemon.addr,
        "GET",
        "/api/inbox",
        "evil.example",
        Some(TOKEN),
    )
    .await;
    assert_eq!(
        status, 403,
        "a rebinding host is refused even with the token"
    );
    let (status, ..) = get(daemon.addr, "GET", "/", "evil.example", None).await;
    assert_eq!(status, 403);
    let (status, ..) = get(daemon.addr, "POST", "/api/agents/x/terminal", &host, None).await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn the_inbox_lists_a_waiting_agent() {
    let daemon = start().await;
    daemon.harness.set(vec![blocked("s1", &daemon.cwd)]);
    let host = daemon.addr.to_string();
    let mut found = None;
    for _ in 0..50 {
        let (status, _, body) = get(daemon.addr, "GET", "/api/inbox", &host, Some(TOKEN)).await;
        assert_eq!(status, 200);
        let inbox: serde_json::Value = serde_json::from_str(&body).unwrap();
        if inbox["cards"]
            .as_array()
            .is_some_and(|cards| !cards.is_empty())
        {
            found = Some(inbox);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let inbox = found.expect("the waiting agent shows up");
    let card = &inbox["cards"][0];
    assert_eq!(card["id"], "s1");
    assert_eq!(card["kind"], "waiting");
    assert_eq!(card["name"], "billing");
    assert_eq!(card["waiting_for"], "permission for Bash");
}

// Reads until `needle` shows up in what the stream sent.
async fn until(stream: &mut TcpStream, seen: &mut String, buffer: &mut [u8], needle: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !seen.contains(needle) {
            let n = stream.read(buffer).await.unwrap();
            assert!(n > 0, "the stream ended: {seen}");
            seen.push_str(&String::from_utf8_lossy(&buffer[..n]));
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {needle:?} in {seen:?}"));
}

#[tokio::test]
async fn the_events_stream_sends_the_inbox_and_then_each_change() {
    let daemon = start().await;
    let mut stream = TcpStream::connect(daemon.addr).await.unwrap();
    let request = format!(
        "GET /api/events?token={TOKEN} HTTP/1.1\r\nHost: {}\r\nAccept: text/event-stream\r\n\r\n",
        daemon.addr
    );
    stream.write_all(request.as_bytes()).await.unwrap();

    let mut seen = String::new();
    let mut buffer = [0_u8; 4096];
    until(&mut stream, &mut seen, &mut buffer, "event: inbox").await;
    assert!(seen.contains("text/event-stream"), "{seen}");
    assert!(!seen.contains("billing"), "nothing waits yet: {seen}");

    daemon.harness.set(vec![blocked("s2", &daemon.cwd)]);
    until(&mut stream, &mut seen, &mut buffer, "billing").await;
}

#[tokio::test]
async fn the_events_stream_refuses_a_wrong_token() {
    let daemon = start().await;
    let host = daemon.addr.to_string();
    let (status, ..) = get(daemon.addr, "GET", "/api/events?token=nope", &host, None).await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn opening_a_terminal_of_an_unknown_agent_says_so() {
    let daemon = start().await;
    let host = daemon.addr.to_string();
    let (status, _, body) = get(
        daemon.addr,
        "POST",
        "/api/agents/ghost/terminal",
        &host,
        Some(TOKEN),
    )
    .await;
    // No Rex in the test environment, or no such agent: either way a clean
    // refusal that the interface can show.
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("\"ok\":false"), "{body}");
}
