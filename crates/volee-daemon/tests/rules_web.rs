//! The rules in the daemon end to end: a real repository with finished work,
//! a repository rules file and a personal one, the web API, a rules file
//! that changes while the daemon runs, a risk set by hand, and a file that
//! does not load.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::{FakeHarness, git, repository};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixListener};
use volee_core::harness::SessionKey;
use volee_core::reconcile::ListedSession;
use volee_core::state::AgentState;
use volee_daemon::{
    AgentsViewConfig, Config, Daemon, EvidenceSources, HoldSettings, HookServerConfig, ManualClock,
    MemoryLog,
};
use volee_git::{Git, GitCli};
use volee_harness::Harness;
use volee_proto::paths::Paths;
use volee_store::{NewHookEvent, Store};

const TOKEN: &str = "rules-test-token";
const ID: &str = "11111111-2222-4333-8444-555555555555";

struct Running {
    addr: SocketAddr,
    worktree: PathBuf,
    repo: PathBuf,
    config_dir: PathBuf,
    _dir: tempfile::TempDir,
}

/// A repository `alpha` with finished work on `feat/invoices`: a commit that
/// touches `src/auth/login.rs` (and a test), the agent's turn ended.
async fn start(repo_rules: Option<&str>, personal_rules: Option<&str>) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let repo = root.join("alpha");
    repository(&repo);
    let worktree = root.join("alpha-feat");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "feat/invoices",
            worktree.to_str().unwrap(),
            "main",
        ],
    );
    std::fs::create_dir_all(worktree.join("src/auth")).unwrap();
    std::fs::write(worktree.join("src/auth/login.rs"), "pub fn login() {}\n").unwrap();
    std::fs::write(worktree.join("src/auth/login_test.rs"), "fn t() {}\n").unwrap();
    git(&worktree, &["add", "."]);
    git(&worktree, &["commit", "--quiet", "-m", "Add login"]);

    let paths = Paths::rooted(root.join("volee"));
    std::fs::create_dir_all(&paths.state_dir).unwrap();
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    let config_dir = paths.config_dir.clone();
    if let Some(text) = repo_rules {
        std::fs::create_dir_all(repo.join(".volee")).unwrap();
        std::fs::write(repo.join(".volee/rules.lua"), text).unwrap();
    }
    if let Some(text) = personal_rules {
        std::fs::write(config_dir.join("rules.lua"), text).unwrap();
    }
    let database = paths.database();
    let store = Store::open(&database).unwrap();
    for (name, at) in [("UserPromptSubmit", 200), ("Stop", 300)] {
        store
            .append_hook_event(&NewHookEvent {
                received_at: at,
                event: name.to_owned(),
                session_id: Some(ID.to_owned()),
                cwd: None,
                payload: format!(r#"{{"hook_event_name":"{name}","session_id":"{ID}"}}"#),
            })
            .unwrap();
    }
    let listener = UnixListener::bind(&paths.socket).unwrap();
    let addr = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap()
    };
    let harness = Arc::new(FakeHarness::default());
    harness.set(vec![ListedSession {
        alive: true,
        pid: Some(4242),
        name: Some("billing".into()),
        ..ListedSession::new(SessionKey::claude(ID), &worktree, AgentState::Done)
    }]);
    let cli = GitCli::new()
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let config = Config {
        paths,
        log: Box::new(MemoryLog::with(vec![])),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: Some(Arc::clone(&harness) as Arc<dyn Harness>),
        evidence: EvidenceSources {
            git: Some(Arc::new(cli) as Arc<dyn Git>),
            refresh_interval: Duration::from_secs(3_600),
            ..EvidenceSources::default()
        },
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
    for _ in 0..100 {
        if TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Running {
        addr,
        worktree,
        repo,
        config_dir,
        _dir: dir,
    }
}

async fn request(addr: SocketAddr, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let payload = body.map(Value::to_string).unwrap_or_default();
    let message = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {TOKEN}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(message.as_bytes()).await.unwrap();
    let mut answer = String::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_string(&mut answer))
        .await
        .unwrap()
        .unwrap();
    let status = answer.split_whitespace().nth(1).unwrap().parse().unwrap();
    let (_, body) = answer.split_once("\r\n\r\n").unwrap();
    // The body may be chunked: keep what lies between the outermost braces.
    let json = body
        .find('{')
        .and_then(|from| body.rfind('}').map(|to| &body[from..=to]))
        .unwrap_or("null");
    (status, serde_json::from_str(json).unwrap_or(Value::Null))
}

/// The inbox, once its finished card carries a risk.
async fn inbox_with_risk(daemon: &Running) -> Value {
    let mut last = Value::Null;
    for _ in 0..100 {
        let (_, inbox) = request(daemon.addr, "GET", "/api/inbox", None).await;
        if inbox["cards"][0]["risk"].is_object() {
            return inbox;
        }
        last = inbox;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no risk on the card: {last}");
}

async fn risk_is(daemon: &Running, level: &str) -> Value {
    let mut last = Value::Null;
    for _ in 0..100 {
        let (_, inbox) = request(daemon.addr, "GET", "/api/inbox", None).await;
        if inbox["cards"][0]["risk"]["level"] == level {
            return inbox;
        }
        last = inbox["cards"][0]["risk"].clone();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the risk never became {level}: {last}");
}

fn rewrite(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

#[tokio::test]
async fn a_sensitive_change_is_high_risk_with_every_layer_and_low_risk_once_the_rules_say_so() {
    let daemon = start(
        Some("return { sensitive_paths = { 'src/auth/**' } }"),
        Some("return { layers = { low = { 'summary' } } }"),
    )
    .await;
    let inbox = inbox_with_risk(&daemon).await;
    let card = &inbox["cards"][0];
    assert_eq!(card["kind"], "finished");
    let risk = &card["risk"];
    assert_eq!(risk["level"], "high", "{risk}");
    assert_eq!(risk["overridden"], false);
    assert!(
        risk["reasons"][0]
            .as_str()
            .unwrap()
            .contains("src/auth/login.rs"),
        "{risk}"
    );
    assert_eq!(risk["auto_merge"]["allowed"], false);
    assert_eq!(
        risk["layers"],
        json!(["summary", "plan", "review", "diagram", "diff"])
    );
    assert_eq!(inbox["rules"]["errors"], json!([]));

    // The work screen carries the same risk, for the layers it opens.
    let (status, work) = request(daemon.addr, "GET", &format!("/api/agents/{ID}/work"), None).await;
    assert_eq!(status, 200);
    assert_eq!(work["risk"]["level"], "high");

    // The rules change while the daemon runs: the sensitive path is gone.
    rewrite(
        &daemon.repo.join(".volee/rules.lua"),
        "return { sensitive_paths = { 'src/billing/**' }, size = { medium_lines = 500 } }",
    );
    let inbox = risk_is(&daemon, "low").await;
    assert_eq!(inbox["cards"][0]["risk"]["layers"], json!(["summary"]));
}

/// Reads an events stream until `needle` shows in what it sent, or 15 s pass.
async fn stream_shows(stream: &mut TcpStream, seen: &mut String, needle: &str) -> bool {
    let end = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut chunk = vec![0_u8; 16384];
    loop {
        if seen.contains(needle) {
            return true;
        }
        let left = end.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, stream.read(&mut chunk)).await {
            Ok(Ok(count)) if count > 0 => seen.push_str(&String::from_utf8_lossy(&chunk[..count])),
            _ => return false,
        }
    }
}

#[tokio::test]
async fn an_open_events_stream_pushes_the_new_risk_when_the_rules_change_and_says_the_agent_finished()
 {
    let daemon = start(Some("return { sensitive_paths = { 'src/auth/**' } }"), None).await;
    let mut stream = TcpStream::connect(daemon.addr).await.unwrap();
    let message = format!(
        "GET /api/events?token={TOKEN} HTTP/1.1\r\nHost: {}\r\nAccept: text/event-stream\r\n\r\n",
        daemon.addr
    );
    stream.write_all(message.as_bytes()).await.unwrap();
    let mut seen = String::new();
    // The open work screen reads its card from this stream: kind and risk.
    assert!(
        stream_shows(&mut stream, &mut seen, "\"level\":\"high\"").await,
        "the first inbox event carries the risk: {seen}"
    );
    assert!(seen.contains("\"kind\":\"finished\""), "{seen}");

    // The rules change while the stream is open: no request, the stream tells.
    seen.clear();
    rewrite(
        &daemon.repo.join(".volee/rules.lua"),
        "return { sensitive_paths = { 'src/billing/**' } }",
    );
    assert!(
        stream_shows(&mut stream, &mut seen, "\"level\":\"low\"").await,
        "the stream never pushed the new risk: {seen}"
    );
}

#[tokio::test]
async fn the_person_can_raise_or_lower_the_risk_of_a_tip_and_a_new_commit_is_judged_again() {
    let daemon = start(None, None).await;
    let inbox = inbox_with_risk(&daemon).await;
    assert_eq!(inbox["cards"][0]["risk"]["level"], "low");
    let path = format!("/api/agents/{ID}/risk");
    let (status, risk) = request(
        daemon.addr,
        "POST",
        &path,
        Some(&json!({ "level": "high" })),
    )
    .await;
    assert_eq!(status, 200, "{risk}");
    assert_eq!(risk["level"], "high");
    assert_eq!(risk["overridden"], true);
    assert_eq!(risk["computed"], "low");
    assert_eq!(
        risk["layers"],
        json!(["summary", "plan", "review", "diagram", "diff"])
    );
    assert_eq!(risk["auto_merge"]["allowed"], false);
    let inbox = risk_is(&daemon, "high").await;
    assert_eq!(inbox["cards"][0]["risk"]["overridden"], true);

    let (status, _) = request(
        daemon.addr,
        "POST",
        &path,
        Some(&json!({ "level": "huge" })),
    )
    .await;
    assert_eq!(status, 422);

    // A new commit is a new tip: the choice does not follow it.
    std::fs::write(daemon.worktree.join("more.rs"), "fn more() {}\n").unwrap();
    git(&daemon.worktree, &["add", "more.rs"]);
    git(&daemon.worktree, &["commit", "--quiet", "-m", "More"]);
    let inbox = risk_is(&daemon, "low").await;
    assert_eq!(inbox["cards"][0]["risk"]["overridden"], false);

    // Taking the choice back restores the rules' answer.
    request(
        daemon.addr,
        "POST",
        &path,
        Some(&json!({ "level": "medium" })),
    )
    .await;
    risk_is(&daemon, "medium").await;
    let (_, risk) = request(daemon.addr, "POST", &path, Some(&json!({ "level": null }))).await;
    assert_eq!(risk["level"], "low");
    assert_eq!(risk["overridden"], false);
}

#[tokio::test]
async fn a_broken_rules_file_shows_a_banner_with_file_and_line_and_falls_back_to_safe() {
    let daemon = start(Some("return {\n  risk = ,\n}"), None).await;
    let inbox = inbox_with_risk(&daemon).await;
    let risk = &inbox["cards"][0]["risk"];
    assert_eq!(risk["level"], "high");
    assert_eq!(risk["auto_merge"]["allowed"], false);
    let error = &inbox["rules"]["errors"][0];
    assert!(
        error["file"]
            .as_str()
            .unwrap()
            .ends_with(".volee/rules.lua"),
        "{error}"
    );
    assert_eq!(error["line"], 2, "{error}");
    assert!(
        error["text"].as_str().unwrap().contains("rules.lua:2"),
        "{error}"
    );

    // Fixed while the daemon runs: the banner goes and the rules answer.
    rewrite(
        &daemon.repo.join(".volee/rules.lua"),
        "return { auto_merge = false } ",
    );
    let inbox = risk_is(&daemon, "low").await;
    assert_eq!(inbox["rules"]["errors"], json!([]));
}

#[tokio::test]
async fn a_repository_cannot_lower_the_risk_or_turn_automatic_merging_on_unless_trusted() {
    let lax = "return { auto_merge = true, risk = function() return 'low', { 'trust me' } end }";
    let daemon = start(
        Some(lax),
        Some("return { sensitive_paths = { 'src/auth/**' } }"),
    )
    .await;
    let inbox = inbox_with_risk(&daemon).await;
    let risk = &inbox["cards"][0]["risk"];
    assert_eq!(risk["level"], "high", "{risk}");
    assert_eq!(risk["auto_merge"]["allowed"], false);
    assert!(
        risk["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().contains("raised")),
        "{risk}"
    );
    assert!(
        inbox["rules"]["warnings"][0]
            .as_str()
            .unwrap()
            .contains("not trusted"),
        "{inbox}"
    );

    // The person trusts this repository: its file now decides.
    rewrite(
        &daemon.config_dir.join("rules.lua"),
        &format!(
            "return {{ sensitive_paths = {{ 'src/auth/**' }}, trusted_repos = {{ '{0}' }}, auto_merge_repos = {{ '{0}' }} }}",
            daemon.repo.display()
        ),
    );
    let inbox = risk_is(&daemon, "low").await;
    assert_eq!(inbox["cards"][0]["risk"]["auto_merge"]["allowed"], true);
}
