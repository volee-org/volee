//! `ClaudeCode` through a fake runner: exact command lines, environment,
//! directories, output handling and errors. No test starts a real `claude`.

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use volee_core::harness::{HarnessKind, SessionKey, SessionOrigin};
use volee_core::reconcile::ListedSession;
use volee_core::state::AgentState;

use super::*;
use crate::error::{ParseError, RequestError};
use crate::spec::PermissionMode;
use crate::testing::FakeRunner;

const SESSION: &str = "7d3c1f0a-2b4e-4c6d-9e8f-0a1b2c3d4e5f";

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
        worktree: None,
    }
}

/// A background session as `claude agents` lists it, working in a directory
/// that does not exist (so it is in no git worktree).
fn listed(handle: Option<&str>) -> ListedSession {
    ListedSession {
        alive: true,
        background: handle.is_some(),
        handle: handle.map(str::to_owned),
        ..ListedSession::new(
            SessionKey::claude(SESSION),
            "/volee/no/such/dir",
            AgentState::Idle,
        )
    }
}

fn backgrounded(id: &str) -> Output {
    Output::success(format!(
        "backgrounded \u{b7} {id} \u{b7} alpha worker\n  claude attach {id}\n  claude logs {id}\n"
    ))
}

fn session_variables() -> Vec<OsString> {
    SESSION_VARIABLES.iter().map(OsString::from).collect()
}

#[test]
fn version_runs_claude_version_from_the_query_directory() {
    let claude = claude(vec![Ok(Output::success("2.1.319 (Claude Code)\n"))]);
    assert_eq!(claude.version().unwrap(), Version::new(2, 1, 319));
    let call = only_call(&claude);
    assert_eq!(call.program, Path::new("claude"));
    assert_eq!(call.args_lossy(), ["--version"]);
    assert_eq!(call.cwd.as_deref(), Some(Path::new("/")));
    assert_eq!(call.env_remove, session_variables());
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
        r#"[{{"pid":1,"id":"7d3c1f0a","cwd":"/work/alpha","kind":"background","startedAt":1,"sessionId":"{SESSION}","name":"alpha worker","status":"busy","state":"working"}}]"#
    );
    let claude = claude(vec![Ok(Output::success(json))]);
    let listing = claude.list().unwrap();
    assert!(listing.complete);
    assert_eq!(listing.sessions.len(), 1);
    assert_eq!(listing.sessions[0].key, SessionKey::claude(SESSION));
    assert_eq!(listing.sessions[0].handle.as_deref(), Some("7d3c1f0a"));
    assert_eq!(listing.sessions[0].state, AgentState::Working);
    let call = only_call(&claude);
    assert_eq!(call.args_lossy(), ["agents", "--json", "--all"]);
    assert_eq!(call.cwd.as_deref(), Some(Path::new("/")));
    assert_eq!(call.env_remove, session_variables());
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
        stdout: b"supervisor unavailable\n".to_vec(),
        stderr: Vec::new(),
    };
    let claude = claude(vec![Ok(output)]);
    let error = claude.stop(&listed(Some("7d3c1f0a"))).unwrap_err();
    assert!(
        error.to_string().ends_with(": supervisor unavailable"),
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
fn spawn_passes_every_value_inside_its_option() {
    let claude = claude(vec![Ok(backgrounded("7d3c1f0a"))]);
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
            handle: "7d3c1f0a".into(),
            name: Some("alpha worker".into()),
            origin: SessionOrigin::Started,
        }
    );
    let call = only_call(&claude);
    assert_eq!(call.cwd.as_deref(), Some(Path::new("/work/alpha")));
    assert_eq!(call.timeout, Timeouts::default().launch);
    assert_eq!(call.env_remove, session_variables());
    assert_eq!(call.stdin, None);
    assert_eq!(
        call.args_lossy(),
        [
            "--bg",
            "--name=alpha worker",
            "--model=opus",
            "--settings=/state/agents/ag_1/settings.json",
            "--mcp-config=/state/agents/ag_1/mcp.json",
            "--strict-mcp-config",
            "--permission-mode=acceptEdits",
            "--allowedTools=Bash(cargo test *),Edit",
            "--disallowedTools=WebFetch",
            "--append-system-prompt=-You are a volee worker.",
            "--",
            "Fix the flaky test.\nThen report.",
        ]
    );
}

#[test]
fn spawn_passes_no_permission_mode_unless_configured() {
    let claude = claude(vec![Ok(backgrounded("7d3c1f0a"))]);
    claude.spawn(&spec()).unwrap();
    let args = only_call(&claude).args_lossy();
    assert!(
        !args
            .iter()
            .any(|arg| arg.starts_with("--permission-mode") || arg.contains("dangerously")),
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
    assert_eq!(claude.spawn(&spec()).unwrap().handle, "0a1b2c3d");
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
fn an_untrusted_directory_is_typed() {
    // The message Claude Code documents for a background session it cannot
    // ask about trust (errors reference, "Workspace not trusted when
    // dispatching a background session").
    let refusal = "Workspace not trusted. Run `claude` in /work/alpha once and accept the trust prompt, then retry.\n";
    for reply in [
        Output::failure(1, refusal),
        Output {
            code: Some(0),
            stdout: Vec::new(),
            stderr: refusal.into(),
        },
    ] {
        let claude = claude(vec![Ok(reply)]);
        let error = claude.spawn(&spec()).unwrap_err();
        assert!(
            matches!(error, HarnessError::UntrustedDirectory { ref path } if path == Path::new("/work/alpha")),
            "{error:?}"
        );
    }
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
    let claude = claude(vec![Ok(backgrounded("7d3c1f0a"))]);
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
            "--resume=7d3c1f0a-2b4e-4c6d-9e8f-0a1b2c3d4e5f",
            "--name=alpha worker",
            "--settings=/state/agents/ag_1/settings.json",
            "--mcp-config=/state/agents/ag_1/mcp.json",
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
    assert_eq!(spawned.handle, "9c41e2aa");
    assert_eq!(spawned.origin, SessionOrigin::Forked);
}

#[test]
fn resume_needs_a_full_session_uuid() {
    let claude = claude(vec![]);
    for id in [
        "7d3c1f0a",
        "--continue",
        "7d3c1f0a-2b4e-4c6d-9e8f-0a1b2c3d4e5z",
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
fn stop_remove_and_logs_use_the_listed_id() {
    let claude = claude(vec![
        Ok(Output::success("")),
        Ok(Output::success("")),
        Ok(Output::success(b"\x1b[2J\x1b[1;1Hhello".to_vec())),
    ]);
    let session = listed(Some("7d3c1f0a"));
    claude.stop(&session).unwrap();
    claude.remove(&session, WorktreeRemoval::Refuse).unwrap();
    assert_eq!(claude.logs(&session).unwrap(), b"\x1b[2J\x1b[1;1Hhello");
    let calls = claude.runner().calls();
    let args: Vec<Vec<String>> = calls.iter().map(Invocation::args_lossy).collect();
    assert_eq!(
        args,
        [
            ["stop", "7d3c1f0a"],
            ["rm", "7d3c1f0a"],
            ["logs", "7d3c1f0a"]
        ]
    );
    for call in &calls {
        assert_eq!(call.cwd.as_deref(), Some(Path::new("/")));
        assert_eq!(call.env_remove, session_variables());
    }
}

#[test]
fn the_listed_id_is_used_as_given() {
    let claude = claude(vec![Ok(Output::success(""))]);
    let session = ListedSession {
        handle: Some("job-0042".into()),
        ..listed(None)
    };
    claude.stop(&session).unwrap();
    assert_eq!(only_call(&claude).args_lossy(), ["stop", "job-0042"]);
}

#[test]
fn a_session_without_an_id_cannot_be_controlled() {
    let claude = claude(vec![]);
    let interactive = listed(None);
    let refused = |result: Result<(), HarnessError>| {
        assert!(
            matches!(result, Err(HarnessError::NotControllable { .. })),
            "{result:?}"
        );
    };
    refused(claude.stop(&interactive));
    refused(claude.remove(&interactive, WorktreeRemoval::Delivered));
    refused(claude.logs(&interactive).map(drop));
    refused(claude.attach_command(&interactive).map(drop));
    assert_eq!(claude.runner().calls(), []);
}

#[test]
fn an_unknown_session_is_typed() {
    let claude = claude(vec![
        Ok(Output::failure(
            1,
            "No job matching '7d3c1f0a'. Run 'claude agents' to list running sessions.\n",
        )),
        Ok(Output::failure(1, "No job matching '7d3c1f0a'\n")),
        Ok(Output::failure(
            1,
            "No job matching '7d3c1f0a'. Run 'claude agents' to list running sessions.\n",
        )),
    ]);
    let session = listed(Some("7d3c1f0a"));
    let unknown = |error: HarnessError| {
        assert!(
            matches!(error, HarnessError::UnknownSession { ref handle } if handle == "7d3c1f0a"),
            "{error:?}"
        );
    };
    unknown(claude.stop(&session).unwrap_err());
    unknown(
        claude
            .remove(&session, WorktreeRemoval::Refuse)
            .unwrap_err(),
    );
    unknown(claude.logs(&session).unwrap_err());
}

#[test]
fn remove_refuses_a_session_in_a_linked_worktree_without_evidence() {
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("worktree");
    std::fs::create_dir_all(worktree.join("src")).unwrap();
    std::fs::write(
        worktree.join(".git"),
        "gitdir: /elsewhere/.git/worktrees/x\n",
    )
    .unwrap();
    let session = ListedSession {
        cwd: worktree.join("src"),
        ..listed(Some("7d3c1f0a"))
    };

    let refusing = claude(vec![]);
    let error = refusing
        .remove(&session, WorktreeRemoval::Refuse)
        .unwrap_err();
    assert!(
        matches!(error, HarnessError::WorktreeAtRisk { worktree: ref found, .. } if *found == worktree),
        "{error:?}"
    );
    assert_eq!(refusing.runner().calls(), []);

    let delivered = claude(vec![Ok(Output::success(""))]);
    delivered
        .remove(&session, WorktreeRemoval::Delivered)
        .unwrap();
    assert_eq!(only_call(&delivered).args_lossy(), ["rm", "7d3c1f0a"]);
}

#[test]
fn a_main_checkout_is_not_a_worktree() {
    let root = tempfile::tempdir().unwrap();
    let checkout = root.path().join("repo");
    std::fs::create_dir_all(checkout.join(".git")).unwrap();
    let session = ListedSession {
        cwd: checkout,
        ..listed(Some("7d3c1f0a"))
    };
    let claude = claude(vec![Ok(Output::success(""))]);
    claude.remove(&session, WorktreeRemoval::Refuse).unwrap();
    assert_eq!(only_call(&claude).args_lossy(), ["rm", "7d3c1f0a"]);
}

#[test]
fn remove_reports_a_session_claude_kept() {
    let kept = "kept 7d3c1f0a \u{2014} its worktree is still at \u{201c}/work/alpha/.claude/worktrees/fix\u{201d}\n  2 unpushed commits on \u{201c}claude/fix\u{201d}: a1b2c3d \u{201c}Fix\u{201d} and 1 more.\n";
    for reply in [
        Output::success(kept),
        Output::failure(1, kept),
        Output {
            code: Some(0),
            stdout: Vec::new(),
            stderr: kept.into(),
        },
    ] {
        let claude = claude(vec![Ok(reply)]);
        let error = claude
            .remove(&listed(Some("7d3c1f0a")), WorktreeRemoval::Delivered)
            .unwrap_err();
        assert!(
            matches!(
                error,
                HarnessError::SessionKept { ref handle, ref reason }
                    if handle == "7d3c1f0a" && reason.contains("unpushed commits")
            ),
            "{error:?}"
        );
    }
}

#[test]
fn foreign_and_unsafe_sessions_are_refused() {
    let claude = claude(vec![]);
    let foreign = ListedSession {
        key: SessionKey {
            harness: HarnessKind::Other("codex".into()),
            id: SESSION.into(),
        },
        ..listed(Some("7d3c1f0a"))
    };
    assert!(matches!(
        claude.stop(&foreign),
        Err(HarnessError::ForeignSession { .. })
    ));
    for handle in ["--all", "", "a b", "4402;rm -rf"] {
        let error = claude.stop(&listed(Some(handle))).unwrap_err();
        assert!(
            matches!(error, HarnessError::InvalidSessionId { .. }),
            "{handle:?}: {error:?}"
        );
    }
    assert_eq!(claude.runner().calls(), []);
}

#[test]
fn attach_builds_the_command_without_running_it() {
    let claude = claude(vec![]).with_program("/opt/claude/bin/claude");
    let command = claude.attach_command(&listed(Some("7d3c1f0a"))).unwrap();
    assert_eq!(command.get_program(), "/opt/claude/bin/claude");
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args, ["attach", "7d3c1f0a"]);
    let removed: Vec<_> = command
        .get_envs()
        .filter(|(_, value)| value.is_none())
        .map(|(key, _)| key.to_owned())
        .collect();
    assert_eq!(removed.len(), SESSION_VARIABLES.len());
    assert!(
        SESSION_VARIABLES
            .iter()
            .all(|variable| removed.iter().any(|key| key == variable))
    );
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
        Ok(backgrounded("7d3c1f0a")),
    ])
    .with_program("/usr/local/bin/claude")
    .with_timeouts(timeouts)
    .with_query_dir("/var/empty");
    assert_eq!(claude.program(), Path::new("/usr/local/bin/claude"));
    assert_eq!(claude.timeouts(), timeouts);
    assert_eq!(claude.query_dir(), Path::new("/var/empty"));
    claude.list().unwrap();
    claude.spawn(&spec()).unwrap();
    let calls = claude.runner().calls();
    assert!(
        calls
            .iter()
            .all(|call| call.program == Path::new("/usr/local/bin/claude"))
    );
    assert_eq!(calls[0].timeout, timeouts.query);
    assert_eq!(calls[0].cwd.as_deref(), Some(Path::new("/var/empty")));
    assert_eq!(calls[1].timeout, timeouts.launch);
    assert_eq!(calls[1].cwd.as_deref(), Some(Path::new("/work/alpha")));
}

#[test]
fn the_trait_object_reports_claude_code() {
    let harness: Box<dyn Harness> = Box::new(claude(vec![]));
    assert_eq!(harness.kind(), HarnessKind::ClaudeCode);
}

#[test]
fn session_variables_are_identity_not_configuration() {
    assert!(SESSION_VARIABLES.contains(&"CLAUDECODE"));
    assert!(SESSION_VARIABLES.contains(&"CLAUDE_CODE_SESSION_ID"));
    assert!(SESSION_VARIABLES.contains(&"CLAUDE_CODE_MESSAGING_TOKEN"));
    assert!(!SESSION_VARIABLES.contains(&"CLAUDE_CODE_SUBAGENT_MODEL"));
    assert!(
        !SESSION_VARIABLES
            .iter()
            .any(|variable| variable.starts_with("ANTHROPIC"))
    );
}

#[test]
fn ids_are_checked_before_they_reach_a_command_line() {
    assert!(is_argument_safe("7d3c1f0a"));
    assert!(is_argument_safe(SESSION));
    assert!(!is_argument_safe("-x"));
    assert!(!is_argument_safe("a/b"));
    assert!(is_uuid(SESSION));
    assert!(!is_uuid("7d3c1f0a"));
    assert!(!is_uuid("7d3c1f0a-2b4e-4c6d-9e8f-0a1b2c3d4e5f-0000"));
}
