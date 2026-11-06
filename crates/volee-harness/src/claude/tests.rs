//! `ClaudeCode` through a fake runner: exact command lines, output handling
//! and errors. No test starts a real `claude`.

use std::path::Path;
use std::time::Duration;

use volee_core::harness::{HarnessKind, SessionKey, SessionOrigin};
use volee_core::state::AgentState;

use super::*;
use crate::error::{ParseError, RequestError};
use crate::spec::PermissionMode;
use crate::testing::FakeRunner;

const SESSION: &str = "4402670e-5b60-4cdc-8cf3-be456e780815";

fn claude(replies: Vec<Result<Output, RunError>>) -> ClaudeCode<FakeRunner> {
    let runner = replies
        .into_iter()
        .fold(FakeRunner::new(), FakeRunner::with_reply);
    ClaudeCode::new(runner)
}

fn only_call(claude: &ClaudeCode<FakeRunner>) -> Invocation {
    let mut calls = claude.runner().calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    calls.remove(0)
}

fn spec() -> SpawnSpec {
    SpawnSpec {
        cwd: "/work/alpha".into(),
        name: "alpha worker".into(),
        model: None,
        prompt: "Fix the flaky test".into(),
        settings_file: "/state/agents/ag_1/settings.json".into(),
        mcp_config_file: "/state/agents/ag_1/mcp.json".into(),
        permission_mode: None,
        allowed_tools: vec![],
        disallowed_tools: vec![],
        append_system_prompt: None,
    }
}

fn backgrounded(id: &str) -> Output {
    Output::success(format!(
        "backgrounded \u{b7} {id} \u{b7} alpha worker\n  claude attach {id}\n  claude logs {id}\n"
    ))
}

#[test]
fn version_runs_claude_version() {
    let claude = claude(vec![Ok(Output::success("2.1.319 (Claude Code)\n"))]);
    assert_eq!(claude.version().unwrap(), Version::new(2, 1, 319));
    let call = only_call(&claude);
    assert_eq!(call.program, Path::new("claude"));
    assert_eq!(call.args_lossy(), ["--version"]);
    assert_eq!(call.cwd, None);
    assert_eq!(call.timeout, Timeouts::default().query);
}

#[test]
fn an_unreadable_version_is_reported() {
    let claude = claude(vec![Ok(Output::success("Claude Code\n"))]);
    let error = claude.version().unwrap_err();
    assert!(
        matches!(
            error,
            HarnessError::UnexpectedOutput {
                source: ParseError::NoVersion(_),
                ..
            }
        ),
        "{error:?}"
    );
}

#[test]
fn capabilities_require_the_minimum_version() {
    let current = claude(vec![Ok(Output::success("2.1.319 (Claude Code)"))]);
    let capabilities = current.capabilities().unwrap();
    assert_eq!(capabilities.version, Version::new(2, 1, 319));
    assert!(capabilities.list_sessions && capabilities.background_sessions);

    let old = claude(vec![Ok(Output::success("2.0.14 (Claude Code)"))]);
    let error = old.capabilities().unwrap_err();
    assert!(
        matches!(
            error,
            HarnessError::UnsupportedVersion {
                harness: HarnessKind::ClaudeCode,
                minimum: MINIMUM_VERSION,
                ..
            }
        ),
        "{error:?}"
    );
    assert!(error.to_string().contains("2.1.300 or newer"), "{error}");
}

#[test]
fn a_missing_claude_is_not_installed() {
    let claude = claude(vec![Err(RunError::NotFound {
        program: "claude".into(),
    })]);
    let error = claude.capabilities().unwrap_err();
    assert!(
        matches!(error, HarnessError::NotInstalled { ref program } if program == Path::new("claude")),
        "{error:?}"
    );
}

#[test]
fn list_runs_agents_json_all() {
    let json = format!(
        r#"[{{"pid":1,"id":"4402670e","cwd":"/work/alpha","kind":"background","startedAt":1,"sessionId":"{SESSION}","name":"alpha worker","status":"busy","state":"working"}}]"#
    );
    let claude = claude(vec![Ok(Output::success(json))]);
    let listing = claude.list().unwrap();
    assert!(listing.complete);
    assert_eq!(listing.sessions.len(), 1);
    assert_eq!(listing.sessions[0].key, SessionKey::claude(SESSION));
    assert_eq!(listing.sessions[0].state, AgentState::Working);
    assert_eq!(
        only_call(&claude).args_lossy(),
        ["agents", "--json", "--all"]
    );
}

#[test]
fn a_failed_command_reports_its_message() {
    let claude = claude(vec![Ok(Output::failure(1, "  Error: not logged in\n"))]);
    let error = claude.list().unwrap_err();
    match error {
        HarnessError::CommandFailed {
            command,
            code,
            message,
        } => {
            assert_eq!(command, "claude agents --json --all");
            assert_eq!(code, Some(1));
            assert_eq!(message, "Error: not logged in");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn a_failure_without_stderr_reports_stdout() {
    let output = Output {
        code: Some(2),
        stdout: b"no such session: 4402670e\n".to_vec(),
        stderr: Vec::new(),
    };
    let claude = claude(vec![Ok(output)]);
    let error = claude.stop(&SessionKey::claude(SESSION)).unwrap_err();
    assert!(
        error.to_string().ends_with(": no such session: 4402670e"),
        "{error}"
    );
}

#[test]
fn long_failure_messages_are_shortened() {
    let claude = claude(vec![Ok(Output::failure(1, "x".repeat(2_000)))]);
    let HarnessError::CommandFailed { message, .. } = claude.list().unwrap_err() else {
        panic!("expected a command failure");
    };
    assert_eq!(message.chars().count(), MAX_FAILURE_MESSAGE + 1);
    assert!(message.ends_with('\u{2026}'));
}

#[test]
fn a_timeout_is_passed_through() {
    let claude = claude(vec![Err(RunError::TimedOut {
        program: "claude".into(),
        timeout: Duration::from_secs(15),
    })]);
    let error = claude.list().unwrap_err();
    assert!(
        matches!(error, HarnessError::Run(RunError::TimedOut { .. })),
        "{error:?}"
    );
}

#[test]
fn spawn_passes_every_flag_in_a_safe_order() {
    let claude = claude(vec![Ok(backgrounded("4402670e"))]);
    let spec = SpawnSpec {
        model: Some("opus".into()),
        prompt: "Fix the flaky test.\nThen report.".into(),
        permission_mode: Some(PermissionMode::AcceptEdits),
        allowed_tools: vec!["Bash(cargo test *)".into(), "Edit".into()],
        disallowed_tools: vec!["WebFetch".into()],
        append_system_prompt: Some("-You are a volee worker.".into()),
        ..spec()
    };
    let spawned = claude.spawn(&spec).unwrap();
    assert_eq!(
        spawned,
        Spawned {
            short_id: "4402670e".into(),
            name: Some("alpha worker".into()),
            origin: SessionOrigin::Started,
        }
    );
    let call = only_call(&claude);
    assert_eq!(call.cwd.as_deref(), Some(Path::new("/work/alpha")));
    assert_eq!(call.timeout, Timeouts::default().launch);
    assert_eq!(call.stdin, None);
    assert_eq!(
        call.args_lossy(),
        [
            "--bg",
            "--name",
            "alpha worker",
            "--model",
            "opus",
            "--settings",
            "/state/agents/ag_1/settings.json",
            "--mcp-config",
            "/state/agents/ag_1/mcp.json",
            "--strict-mcp-config",
            "--permission-mode",
            "acceptEdits",
            "--allowedTools",
            "Bash(cargo test *)",
            "Edit",
            "--disallowedTools",
            "WebFetch",
            "--append-system-prompt",
            "-You are a volee worker.",
            "--",
            "Fix the flaky test.\nThen report.",
        ]
    );
}

#[test]
fn spawn_never_bypasses_permissions_by_default() {
    let claude = claude(vec![Ok(backgrounded("4402670e"))]);
    claude.spawn(&spec()).unwrap();
    let args = only_call(&claude).args_lossy();
    assert!(
        !args.iter().any(|arg| arg == "--permission-mode"),
        "{args:?}"
    );
    assert!(
        !args.iter().any(|arg| arg.contains("dangerously")),
        "{args:?}"
    );
}

#[test]
fn spawn_reads_the_id_from_stderr_when_stdout_has_none() {
    let output = Output {
        code: Some(0),
        stdout: b"Tip: run claude agents to see every session\n".to_vec(),
        stderr: "backgrounded \u{b7} 0a1b2c3d \u{b7} alpha\n".into(),
    };
    let claude = claude(vec![Ok(output)]);
    assert_eq!(claude.spawn(&spec()).unwrap().short_id, "0a1b2c3d");
}

#[test]
fn spawn_without_a_backgrounded_line_is_an_error() {
    let claude = claude(vec![Ok(Output::success("Started.\n"))]);
    let error = claude.spawn(&spec()).unwrap_err();
    assert!(
        matches!(
            error,
            HarnessError::UnexpectedOutput {
                source: ParseError::NoBackgroundedLine,
                ..
            }
        ),
        "{error:?}"
    );
}

#[test]
fn an_invalid_spec_runs_nothing() {
    let claude = claude(vec![]);
    let relative = SpawnSpec {
        mcp_config_file: "mcp.json".into(),
        ..spec()
    };
    let error = claude.spawn(&relative).unwrap_err();
    assert!(
        matches!(
            error,
            HarnessError::InvalidRequest(RequestError::RelativePath { .. })
        ),
        "{error:?}"
    );
    let empty = SpawnSpec {
        prompt: String::new(),
        ..spec()
    };
    assert!(claude.spawn(&empty).is_err());
    assert_eq!(claude.runner().calls(), []);
}

#[test]
fn resume_continues_by_session_uuid() {
    let claude = claude(vec![Ok(backgrounded("4402670e"))]);
    let spawned = claude
        .resume(&SessionKey::claude(SESSION), &spec())
        .unwrap();
    assert_eq!(spawned.origin, SessionOrigin::Resumed);
    let call = only_call(&claude);
    assert_eq!(call.cwd.as_deref(), Some(Path::new("/work/alpha")));
    assert_eq!(
        call.args_lossy(),
        [
            "--bg",
            "--resume",
            SESSION,
            "--name",
            "alpha worker",
            "--settings",
            "/state/agents/ag_1/settings.json",
            "--mcp-config",
            "/state/agents/ag_1/mcp.json",
            "--strict-mcp-config",
            "--",
            "Fix the flaky test",
        ]
    );
}

#[test]
fn resume_reports_a_fork() {
    let claude = claude(vec![Ok(backgrounded("9c41e2aa"))]);
    let spawned = claude
        .resume(&SessionKey::claude(SESSION), &spec())
        .unwrap();
    assert_eq!(spawned.short_id, "9c41e2aa");
    assert_eq!(spawned.origin, SessionOrigin::Forked);
}

#[test]
fn resume_needs_a_full_session_uuid() {
    let claude = claude(vec![]);
    for id in [
        "4402670e",
        "--continue",
        "4402670e-5b60-4cdc-8cf3-be456e78081z",
    ] {
        let error = claude.resume(&SessionKey::claude(id), &spec()).unwrap_err();
        assert!(
            matches!(error, HarnessError::InvalidSessionId { .. }),
            "{id}: {error:?}"
        );
    }
    assert_eq!(claude.runner().calls(), []);
}

#[test]
fn stop_remove_and_logs_use_the_short_id() {
    let claude = claude(vec![
        Ok(Output::success("")),
        Ok(Output::success("")),
        Ok(Output::success(b"\x1b[2J\x1b[1;1Hhello".to_vec())),
    ]);
    let session = SessionKey::claude(SESSION);
    claude.stop(&session).unwrap();
    claude.remove(&session).unwrap();
    assert_eq!(claude.logs(&session).unwrap(), b"\x1b[2J\x1b[1;1Hhello");
    let calls: Vec<Vec<String>> = claude
        .runner()
        .calls()
        .iter()
        .map(Invocation::args_lossy)
        .collect();
    assert_eq!(
        calls,
        [
            ["stop", "4402670e"],
            ["rm", "4402670e"],
            ["logs", "4402670e"]
        ]
    );
}

#[test]
fn a_known_short_id_is_enough_to_control_a_session() {
    let claude = claude(vec![Ok(Output::success(""))]);
    claude.stop(&SessionKey::claude("4402670e")).unwrap();
    assert_eq!(only_call(&claude).args_lossy(), ["stop", "4402670e"]);
}

#[test]
fn foreign_and_unsafe_sessions_are_refused() {
    let claude = claude(vec![]);
    let foreign = SessionKey {
        harness: HarnessKind::Other("codex".into()),
        id: SESSION.into(),
    };
    assert!(matches!(
        claude.stop(&foreign),
        Err(HarnessError::ForeignSession { .. })
    ));
    for id in ["--all", "", "a b c d e", "4402;rm -rf"] {
        let error = claude.remove(&SessionKey::claude(id)).unwrap_err();
        assert!(
            matches!(error, HarnessError::InvalidSessionId { .. }),
            "{id:?}: {error:?}"
        );
    }
    assert_eq!(claude.runner().calls(), []);
}

#[test]
fn attach_builds_the_command_without_running_it() {
    let claude = claude(vec![]).with_program("/opt/claude/bin/claude");
    let command = claude.attach_command(&SessionKey::claude(SESSION)).unwrap();
    assert_eq!(command.get_program(), "/opt/claude/bin/claude");
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args, ["attach", "4402670e"]);
    assert_eq!(claude.runner().calls(), []);
}

#[test]
fn configuration_overrides_apply_to_every_command() {
    let timeouts = Timeouts {
        query: Duration::from_secs(3),
        launch: Duration::from_secs(9),
    };
    let claude = claude(vec![
        Ok(Output::success("[]")),
        Ok(backgrounded("4402670e")),
    ])
    .with_program("/usr/local/bin/claude")
    .with_timeouts(timeouts);
    assert_eq!(claude.program(), Path::new("/usr/local/bin/claude"));
    assert_eq!(claude.timeouts(), timeouts);
    claude.list().unwrap();
    claude.spawn(&spec()).unwrap();
    let calls = claude.runner().calls();
    assert!(
        calls
            .iter()
            .all(|call| call.program == Path::new("/usr/local/bin/claude"))
    );
    assert_eq!(calls[0].timeout, timeouts.query);
    assert_eq!(calls[1].timeout, timeouts.launch);
}

#[test]
fn the_trait_object_reports_claude_code() {
    let harness: Box<dyn Harness> = Box::new(claude(vec![]));
    assert_eq!(harness.kind(), HarnessKind::ClaudeCode);
}

#[test]
fn ids_are_checked_before_they_reach_a_command_line() {
    assert!(is_argument_safe("4402670e"));
    assert!(is_argument_safe(SESSION));
    assert!(!is_argument_safe("-x"));
    assert!(!is_argument_safe("a/b"));
    assert!(is_uuid(SESSION));
    assert!(!is_uuid("4402670e"));
    assert!(!is_uuid("4402670e-5b60-4cdc-8cf3-be456e780815-0000"));
}
