//! The actions on a finished change end to end (v2, step 9): a real daemon
//! with the web routes, real repositories, a fake session socket, a fake
//! `claude` program and a fake editor.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener as StdUnixListener;
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
    AgentsViewConfig, Config, Daemon, EvidenceSources, FixSources, HoldSettings, HookServerConfig,
    ManualClock, MemoryLog,
};
use volee_git::{Git, GitCli};
use volee_harness::Harness;
use volee_proto::paths::Paths;
use volee_store::{NewHookEvent, Store};

const TOKEN: &str = "actions-test-token";
const ID: &str = "11111111-2222-4333-8444-555555555555";
/// The fake session's process id: this very process, so that the daemon's
/// check that the process exists (it re-reads liveness when it sends a fix
/// request) finds it.
fn pid() -> u32 {
    std::process::id()
}

struct Options<'a> {
    personal_rules: Option<&'a str>,
    alive: bool,
}

struct Running {
    addr: SocketAddr,
    worktree: PathBuf,
    repo: PathBuf,
    sockets: PathBuf,
    projects: PathBuf,
    root: PathBuf,
    _dir: tempfile::TempDir,
}

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A repository `alpha` (no remote) with finished work on `feat/invoices`
/// (docs only, one commit), the agent's turn ended.
#[allow(clippy::too_many_lines)]
async fn start(options: Options<'_>) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let repo = root.join("alpha");
    repository(&repo);
    // The merge commit needs an identity, and CI machines have none.
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["config", "user.email", "t@t"]);
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
    std::fs::create_dir_all(worktree.join("docs")).unwrap();
    std::fs::write(worktree.join("docs/guide.md"), "# Guide\nline two\n").unwrap();
    git(&worktree, &["add", "."]);
    git(&worktree, &["commit", "--quiet", "-m", "Add the guide"]);

    let paths = Paths::rooted(root.join("volee"));
    std::fs::create_dir_all(&paths.state_dir).unwrap();
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    // Short socket directory: a Unix socket path is limited in length.
    let sockets = root.join("s");
    std::fs::create_dir_all(&sockets).unwrap();
    let projects = root.join("projects");
    std::fs::create_dir_all(projects.join("p")).unwrap();
    std::fs::write(projects.join("p").join(format!("{ID}.jsonl")), "").unwrap();
    if let Some(text) = options.personal_rules {
        let text = text.replace("{REPO}", &repo.to_string_lossy());
        std::fs::write(paths.config_dir.join("rules.lua"), text).unwrap();
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
        alive: options.alive,
        pid: options.alive.then(pid),
        name: Some("billing".into()),
        ..ListedSession::new(SessionKey::claude(ID), &worktree, AgentState::Done)
    }]);
    // A fake `claude`: answers `agents --json --all` with the session (with
    // its process only when it is alive), and records the arguments of any
    // other call.
    let fake_claude = root.join("fake-claude.sh");
    let listing = if options.alive {
        format!(r#"[{{"sessionId":"{ID}","pid":{}}}]"#, pid())
    } else {
        format!(r#"[{{"sessionId":"{ID}"}}]"#)
    };
    script(
        &fake_claude,
        &format!(
            "if [ \"$1\" = agents ]; then printf '%s' '{listing}'; exit 0; fi\n\
             for a in \"$@\"; do printf '%s\\n' \"$a\" >> '{}'; done",
            root.join("claude-args.log").display()
        ),
    );
    // A fake editor: records its arguments.
    let fake_editor = root.join("fake-editor.sh");
    script(
        &fake_editor,
        &format!(
            "for a in \"$@\"; do printf '%s\\n' \"$a\" >> '{}'; done",
            root.join("editor-args.log").display()
        ),
    );
    let mut settings = volee_config::Config::default();
    settings.review.enabled = false;
    settings.claude.command = fake_claude.to_string_lossy().into_owned();
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
            settings,
            refresh_interval: Duration::from_secs(3_600),
            fix: FixSources {
                socket_dir: Some(sockets.clone()),
                projects: Some(projects.clone()),
                confirm_within: Some(Duration::from_secs(2)),
            },
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
        sockets,
        projects,
        root,
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
    tokio::time::timeout(Duration::from_secs(20), stream.read_to_string(&mut answer))
        .await
        .unwrap()
        .unwrap();
    let status = answer.split_whitespace().nth(1).unwrap().parse().unwrap();
    let (_, body) = answer.split_once("\r\n\r\n").unwrap();
    let json = body
        .find(['{', '['])
        .and_then(|from| body.rfind(['}', ']']).map(|to| &body[from..=to]))
        .unwrap_or("null");
    (status, serde_json::from_str(json).unwrap_or(Value::Null))
}

/// Waits until the inbox has a finished card.
async fn finished_card(daemon: &Running) -> Value {
    for _ in 0..100 {
        let (_, inbox) = request(daemon.addr, "GET", "/api/inbox", None).await;
        if inbox["cards"][0]["kind"] == "finished" && inbox["cards"][0]["risk"].is_object() {
            return inbox;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no finished card");
}

async fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..150 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("never happened: {what}");
}

async fn journal(daemon: &Running) -> Vec<Value> {
    let (_, entries) = request(daemon.addr, "GET", "/api/journal", None).await;
    entries.as_array().cloned().unwrap_or_default()
}

fn rev(dir: &Path, name: &str) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(["rev-parse", name])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[tokio::test]
async fn one_click_merges_locally_with_evidence_a_journal_entry_and_the_card_goes() {
    let daemon = start(Options {
        personal_rules: None,
        alive: true,
    })
    .await;
    finished_card(&daemon).await;
    let (status, done) = request(
        daemon.addr,
        "POST",
        &format!("/api/agents/{ID}/merge"),
        None,
    )
    .await;
    assert_eq!(status, 200, "{done}");
    assert!(
        done["message"].as_str().unwrap().contains("Evidence"),
        "{done}"
    );
    assert!(
        daemon.repo.join("docs/guide.md").is_file(),
        "main has the work"
    );
    let entries = journal(&daemon).await;
    assert_eq!(entries[0]["kind"], "merge");
    assert_eq!(entries[0]["ok"], true);
    assert_eq!(entries[0]["automatic"], false);
    // The card is gone: the branch is merged.
    for _ in 0..100 {
        let (_, inbox) = request(daemon.addr, "GET", "/api/inbox", None).await;
        if inbox["cards"].as_array().unwrap().is_empty() {
            assert_eq!(
                inbox["journal"][0]["kind"], "merge",
                "the inbox carries the journal"
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the card is still there after the merge");
}

#[tokio::test]
async fn a_merge_that_fails_changes_nothing_and_the_journal_says_why() {
    let daemon = start(Options {
        personal_rules: None,
        alive: true,
    })
    .await;
    finished_card(&daemon).await;
    // The person has uncommitted work in the main checkout: volee never merges over it.
    std::fs::write(daemon.repo.join("README.md"), "mine\n").unwrap();
    let before = rev(&daemon.repo, "main");
    let (status, done) = request(
        daemon.addr,
        "POST",
        &format!("/api/agents/{ID}/merge"),
        None,
    )
    .await;
    assert_eq!(status, 422, "{done}");
    assert!(
        done["message"]
            .as_str()
            .unwrap()
            .contains("nothing changed"),
        "{done}"
    );
    assert_eq!(rev(&daemon.repo, "main"), before);
    assert_eq!(
        std::fs::read_to_string(daemon.repo.join("README.md")).unwrap(),
        "mine\n"
    );
    assert!(!daemon.repo.join("docs/guide.md").exists());
    let entries = journal(&daemon).await;
    assert_eq!(entries[0]["ok"], false);
    assert!(
        entries[0]["message"]
            .as_str()
            .unwrap()
            .contains("Not merged")
    );
}

#[tokio::test]
async fn a_low_risk_change_merges_by_itself_when_the_rules_say_so_and_the_journal_says_which() {
    let daemon = start(Options {
        personal_rules: Some(
            "return { auto_merge_repos = { '{REPO}' }, auto_merge = function(change, risk) \
             if risk == 'low' and volee.touches(change, 'docs/**') then \
               return true, 'docs only, low risk' end return false end }",
        ),
        alive: true,
    })
    .await;
    let mut entries = Vec::new();
    for _ in 0..100 {
        entries = journal(&daemon).await;
        if !entries.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        daemon.repo.join("docs/guide.md").is_file(),
        "main has the work"
    );
    assert_eq!(entries[0]["kind"], "merge");
    assert_eq!(entries[0]["automatic"], true, "{entries:?}");
    let message = entries[0]["message"].as_str().unwrap();
    assert!(message.contains("automatically"), "{message}");
    assert!(
        message.contains("docs only, low risk"),
        "the rule's reason: {message}"
    );
}

#[tokio::test]
async fn a_repository_the_person_did_not_list_is_never_merged_by_itself_whatever_the_rules_say() {
    // auto_merge = true, a low risk change, but `auto_merge_repos` is empty.
    let daemon = start(Options {
        personal_rules: Some("return { auto_merge = true }"),
        alive: true,
    })
    .await;
    let inbox = finished_card(&daemon).await;
    let merge = &inbox["cards"][0]["risk"]["auto_merge"];
    assert_eq!(merge["allowed"], false, "{merge}");
    assert!(
        merge["reason"]
            .as_str()
            .unwrap()
            .contains("not enabled for this repo"),
        "{merge}"
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!daemon.repo.join("docs/guide.md").exists());
    assert_eq!(journal(&daemon).await.len(), 0);
    // A repository's own file cannot list itself.
    std::fs::create_dir_all(daemon.repo.join(".volee")).unwrap();
    let own = format!(
        "return {{ auto_merge_repos = {{ '{}' }} }}",
        daemon.repo.display()
    );
    std::fs::write(daemon.repo.join(".volee/rules.lua"), own).unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!daemon.repo.join("docs/guide.md").exists());
}

#[tokio::test]
async fn a_high_risk_change_is_never_merged_by_itself() {
    let daemon = start(Options {
        personal_rules: Some(
            "return { auto_merge_repos = { '{REPO}' }, auto_merge = true, sensitive_paths = { 'docs/**' } }",
        ),
        alive: true,
    })
    .await;
    let inbox = finished_card(&daemon).await;
    assert_eq!(inbox["cards"][0]["risk"]["level"], "high");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!daemon.repo.join("docs/guide.md").exists());
    assert_eq!(journal(&daemon).await.len(), 0);
}

/// A fake session: listens on `<pid>.sock`, and when it gets a line, records
/// it in the transcript (when `acknowledge`), like Claude Code does.
fn fake_session(daemon: &Running, acknowledge: bool) -> Arc<std::sync::Mutex<Vec<String>>> {
    let path = daemon.sockets.join(format!("{}.sock", pid()));
    let listener = StdUnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let lines = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = Arc::clone(&lines);
    let transcript = daemon.projects.join("p").join(format!("{ID}.jsonl"));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if acknowledge {
                    let value: Value = serde_json::from_str(&line).unwrap();
                    let entry = json!({"type": "user", "message": value["message"]});
                    let mut file = std::fs::OpenOptions::new()
                        .append(true)
                        .open(&transcript)
                        .unwrap();
                    writeln!(file, "{entry}").unwrap();
                }
                seen.lock().unwrap().push(line);
            }
        }
    });
    lines
}

#[tokio::test]
async fn a_fix_request_reaches_a_live_session_by_its_socket_and_is_confirmed_by_the_transcript() {
    let daemon = start(Options {
        personal_rules: None,
        alive: true,
    })
    .await;
    let lines = fake_session(&daemon, true);
    finished_card(&daemon).await;
    let (status, done) = request(
        daemon.addr,
        "POST",
        &format!("/api/agents/{ID}/fix"),
        Some(&json!({ "text": "Rename `guide` to `manual`." })),
    )
    .await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["delivered"], true);
    let got = lines.lock().unwrap().clone();
    assert_eq!(got.len(), 1);
    let value: Value = serde_json::from_str(&got[0]).unwrap();
    assert_eq!(value["type"], "user");
    let content = value["message"]["content"].as_str().unwrap();
    assert!(content.contains("Rename `guide` to `manual`."));
    assert!(
        content.starts_with("This is a request from the person you work for"),
        "{content}"
    );
    assert!(
        content.contains("volee, which relays it to you"),
        "{content}"
    );
    let entries = journal(&daemon).await;
    assert_eq!(entries[0]["kind"], "fix");
    assert_eq!(entries[0]["ok"], true);
    assert!(entries[0]["message"].as_str().unwrap().contains("socket"));
    assert!(
        !daemon.root.join("claude-args.log").exists(),
        "a live session is never resumed"
    );
}

#[tokio::test]
async fn a_write_nobody_shows_is_reported_not_delivered_with_the_fallback_to_offer() {
    let daemon = start(Options {
        personal_rules: None,
        alive: true,
    })
    .await;
    let _lines = fake_session(&daemon, false);
    finished_card(&daemon).await;
    let (status, done) = request(
        daemon.addr,
        "POST",
        &format!("/api/agents/{ID}/fix"),
        Some(&json!({ "text": "Do it." })),
    )
    .await;
    assert_eq!(status, 422, "{done}");
    assert_eq!(done["delivered"], false);
    assert!(
        done["message"].as_str().unwrap().contains("NOT delivered"),
        "{done}"
    );
    assert!(
        !daemon.root.join("claude-args.log").exists(),
        "never resumed while alive"
    );
    assert_eq!(journal(&daemon).await[0]["ok"], false);
}

#[tokio::test]
async fn a_stopped_session_is_resumed_in_the_background_with_nothing_but_the_text() {
    let daemon = start(Options {
        personal_rules: None,
        alive: false,
    })
    .await;
    // A stopped session is no card in the inbox, but its work is still readable.
    for _ in 0..100 {
        let (status, _) =
            request(daemon.addr, "GET", &format!("/api/agents/{ID}/work"), None).await;
        if status == 200 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (status, done) = request(
        daemon.addr,
        "POST",
        &format!("/api/agents/{ID}/fix"),
        Some(&json!({ "text": "Add a title." })),
    )
    .await;
    assert_eq!(status, 200, "{done}");
    let log = std::fs::read_to_string(daemon.root.join("claude-args.log")).unwrap();
    let args: Vec<&str> = log.lines().collect();
    assert_eq!(args[0], "--bg");
    assert_eq!(args[1], "--resume");
    assert_eq!(args[2], ID);
    assert!(
        args[3].starts_with("This is a request from the person you work for"),
        "{args:?}"
    );
    assert!(log.contains("Add a title."));
    let entries = journal(&daemon).await;
    assert!(entries[0]["message"].as_str().unwrap().contains("resume"));
}

#[tokio::test]
async fn a_comment_is_kept_per_tip_listed_on_the_work_and_removable() {
    let daemon = start(Options {
        personal_rules: None,
        alive: true,
    })
    .await;
    finished_card(&daemon).await;
    let path = format!("/api/agents/{ID}/comments");
    let (status, comment) = request(
        daemon.addr,
        "POST",
        &path,
        Some(&json!({ "file": "docs/guide.md", "line": 2, "text": "Why two lines?" })),
    )
    .await;
    assert_eq!(status, 200, "{comment}");
    let (status, _) = request(
        daemon.addr,
        "POST",
        &path,
        Some(&json!({ "file": "docs/guide.md", "line": 0, "text": "x" })),
    )
    .await;
    assert_eq!(status, 422);
    let (_, work) = request(daemon.addr, "GET", &format!("/api/agents/{ID}/work"), None).await;
    assert_eq!(work["comments"][0]["text"], "Why two lines?");
    assert_eq!(work["comments"][0]["line"], 2);
    // A new commit is a new tip: the comment does not follow it.
    std::fs::write(daemon.worktree.join("docs/more.md"), "more\n").unwrap();
    git(&daemon.worktree, &["add", "."]);
    git(&daemon.worktree, &["commit", "--quiet", "-m", "More"]);
    let (_, none) = request(daemon.addr, "GET", &path, None).await;
    assert_eq!(none, json!([]));
}

#[tokio::test]
async fn an_external_editor_opens_the_file_with_exactly_its_argv() {
    let daemon = start(Options {
        personal_rules: None,
        alive: true,
    })
    .await;
    finished_card(&daemon).await;
    // The editors come from the rules: a repository file, here.
    let editor = daemon.root.join("fake-editor.sh");
    std::fs::create_dir_all(daemon.repo.join(".volee")).unwrap();
    std::fs::write(
        daemon.repo.join(".volee/rules.lua"),
        format!(
            "return {{ editors = {{ {{ name = 'fake', kind = 'external', cmd = '{} --at {{line}} {{file}}' }}, \
             {{ name = 'nvim', kind = 'terminal', cmd = 'nvim +{{line}} {{file}}' }} }} }}",
            editor.display()
        ),
    )
    .unwrap();
    // The rules are read again on their own.
    let mut usable = Value::Null;
    for _ in 0..100 {
        let (_, work) = request(daemon.addr, "GET", &format!("/api/agents/{ID}/work"), None).await;
        if work["editors"]
            .as_array()
            .is_some_and(|list| list.len() == 2)
        {
            usable = work["editors"].clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(usable[0]["name"], "fake");
    assert_eq!(usable[1]["kind"], "terminal");
    let (status, done) = request(
        daemon.addr,
        "POST",
        &format!("/api/agents/{ID}/editor"),
        Some(&json!({ "editor": "fake", "file": "docs/guide.md", "line": 2 })),
    )
    .await;
    assert_eq!(status, 200, "{done}");
    let log = daemon.root.join("editor-args.log");
    wait_for("the editor ran", || log.is_file()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let lines: Vec<String> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(lines[0], "--at");
    assert_eq!(lines[1], "2");
    assert!(lines[2].ends_with("alpha-feat/docs/guide.md"), "{lines:?}");
    // A path that leaves the worktree is refused, and a terminal editor with no terminal says so.
    let (status, refused) = request(
        daemon.addr,
        "POST",
        &format!("/api/agents/{ID}/editor"),
        Some(&json!({ "editor": "fake", "file": "../alpha/README.md", "line": 1 })),
    )
    .await;
    assert_eq!(status, 422, "{refused}");
}
