//! Hook payload fixtures for every event volee observes, shaped like what
//! Claude Code 2.1.319 sends.

use std::path::PathBuf;

use serde_json::json;

use super::*;

fn payload(json: &str) -> HookPayload {
    HookPayload::from_json(json.as_bytes()).unwrap()
}

fn signal(json: &str) -> HookSignal {
    interpret(&payload(json))
}

const SESSION_START: &str = r#"{
  "session_id": "4402670e-5b60-4cdc-8cf3-be456e780815",
  "transcript_path": "/Users/me/.claude/projects/-work-alpha/4402670e-5b60-4cdc-8cf3-be456e780815.jsonl",
  "cwd": "/work/alpha",
  "hook_event_name": "SessionStart",
  "model": "claude-opus-5",
  "scratchpad_dir": "/tmp/claude/scratch",
  "session_title": "alpha worker",
  "source": "startup"
}"#;

#[test]
fn session_start() {
    assert_eq!(
        signal(SESSION_START),
        HookSignal::SessionStarted {
            session_id: "4402670e-5b60-4cdc-8cf3-be456e780815".into(),
            source: Some(SessionSource::Startup),
            model: Some("claude-opus-5".into()),
            title: Some("alpha worker".into()),
            transcript_path: Some(PathBuf::from(
                "/Users/me/.claude/projects/-work-alpha/4402670e-5b60-4cdc-8cf3-be456e780815.jsonl"
            )),
        }
    );
}

#[test]
fn session_start_sources() {
    let cases = [
        ("startup", SessionSource::Startup),
        ("resume", SessionSource::Resume),
        ("clear", SessionSource::Clear),
        ("compact", SessionSource::Compact),
        ("fork", SessionSource::Fork),
        ("teleport", SessionSource::Other("teleport".into())),
    ];
    for (source, expected) in cases {
        let json =
            format!(r#"{{"session_id":"s","hook_event_name":"SessionStart","source":"{source}"}}"#);
        let HookSignal::SessionStarted { source, model, .. } = signal(&json) else {
            panic!("expected a session start for {json}");
        };
        assert_eq!(source, Some(expected));
        assert_eq!(model, None, "the model is optional");
    }
}

#[test]
fn user_prompt_submit() {
    let json = r#"{
      "session_id": "s", "transcript_path": "/t.jsonl", "cwd": "/work/alpha",
      "hook_event_name": "UserPromptSubmit", "permission_mode": "default",
      "prompt": "Fix the flaky test", "prompt_id": "6f1c2a8e-0000-0000-0000-000000000000"
    }"#;
    let parsed = payload(json);
    assert_eq!(parsed.prompt.as_deref(), Some("Fix the flaky test"));
    assert_eq!(parsed.permission_mode.as_deref(), Some("default"));
    assert_eq!(interpret(&parsed), HookSignal::PromptSubmitted);
}

#[test]
fn pre_tool_use() {
    let json = r#"{
      "session_id": "s", "transcript_path": "/t.jsonl", "cwd": "/work/volee",
      "hook_event_name": "PreToolUse", "permission_mode": "default",
      "effort": {"level": "medium"},
      "tool_name": "Bash",
      "tool_input": {"command": "cargo test -p volee-core\necho done", "description": "Run tests", "timeout": 120000},
      "tool_use_id": "toolu_01ABC"
    }"#;
    let parsed = payload(json);
    assert_eq!(parsed.tool_use_id.as_deref(), Some("toolu_01ABC"));
    assert_eq!(
        interpret(&parsed),
        HookSignal::ToolStarted {
            tool: "Bash".into(),
            summary: "Bash: cargo test -p volee-core".into(),
        }
    );
}

#[test]
fn post_tool_use() {
    let json = r#"{
      "session_id": "s", "transcript_path": "/t.jsonl", "cwd": "/work/volee",
      "hook_event_name": "PostToolUse", "permission_mode": "default",
      "tool_name": "Edit",
      "tool_input": {"file_path": "/work/volee/src/lib.rs", "old_string": "a", "new_string": "b", "replace_all": false},
      "tool_response": {"filePath": "/work/volee/src/lib.rs", "structuredPatch": []},
      "tool_use_id": "toolu_01ABC",
      "duration_ms": 12
    }"#;
    assert_eq!(
        signal(json),
        HookSignal::ToolFinished {
            tool: "Edit".into(),
            summary: "Edit src/lib.rs".into(),
            files: vec![PathBuf::from("/work/volee/src/lib.rs")],
        }
    );
}

#[test]
fn a_tool_event_without_a_tool_is_ignored() {
    assert_eq!(
        signal(r#"{"session_id":"s","hook_event_name":"PreToolUse"}"#),
        HookSignal::Ignored
    );
    assert_eq!(
        signal(r#"{"session_id":"s","hook_event_name":"PostToolUse","tool_input":{}}"#),
        HookSignal::Ignored
    );
}

#[test]
fn a_tool_without_input_is_summarized_by_its_name() {
    assert_eq!(
        signal(r#"{"session_id":"s","hook_event_name":"PostToolUse","tool_name":"Write"}"#),
        HookSignal::ToolFinished {
            tool: "Write".into(),
            summary: "Write".into(),
            files: vec![],
        }
    );
}

#[test]
fn asking_the_user_a_question_needs_input() {
    let json = r#"{
      "session_id": "s", "hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
      "tool_input": {"questions": [
        {"question": "Which database should the cache use?", "header": "Cache", "options": [{"label": "SQLite"}, {"label": "Redis"}], "multiSelect": false},
        {"question": "  Keep the old API?  ", "header": "API", "options": []}
      ]}
    }"#;
    assert_eq!(
        signal(json),
        HookSignal::NeedsInput {
            request: InputRequest::Question,
            detail: Some("Which database should the cache use?\nKeep the old API?".into()),
        }
    );
    let empty = r#"{"session_id":"s","hook_event_name":"PreToolUse","tool_name":"AskUserQuestion","tool_input":{}}"#;
    assert_eq!(
        signal(empty),
        HookSignal::NeedsInput {
            request: InputRequest::Question,
            detail: None,
        }
    );
}

#[test]
fn notification_permission_prompt() {
    let json = r#"{
      "session_id": "s", "transcript_path": "/t.jsonl", "cwd": "/w",
      "hook_event_name": "Notification",
      "message": "Claude needs your permission to use Bash",
      "title": "Permission needed",
      "notification_type": "permission_prompt"
    }"#;
    assert_eq!(
        signal(json),
        HookSignal::NeedsInput {
            request: InputRequest::Permission {
                tool: Some("Bash".into())
            },
            detail: Some("Claude needs your permission to use Bash".into()),
        }
    );
    let generic = r#"{"session_id":"s","hook_event_name":"Notification","message":"Claude needs your permission","notification_type":"permission_prompt"}"#;
    assert_eq!(
        signal(generic),
        HookSignal::NeedsInput {
            request: InputRequest::Permission { tool: None },
            detail: Some("Claude needs your permission".into()),
        }
    );
}

#[test]
fn notification_dialogs_need_input() {
    for kind in ["elicitation_dialog", "elicitation_url_dialog"] {
        let json = format!(
            r#"{{"session_id":"s","hook_event_name":"Notification","message":"The github server needs input","notification_type":"{kind}"}}"#
        );
        assert_eq!(
            signal(&json),
            HookSignal::NeedsInput {
                request: InputRequest::Dialog,
                detail: Some("The github server needs input".into()),
            },
            "{kind}"
        );
    }
}

#[test]
fn other_notifications_are_ignored() {
    for kind in [
        "idle_prompt",
        "auth_success",
        "agent_needs_input",
        "agent_completed",
        "plugin_notification",
        "something_new",
    ] {
        let json = format!(
            r#"{{"session_id":"s","hook_event_name":"Notification","message":"Claude is waiting for your input","notification_type":"{kind}"}}"#
        );
        assert_eq!(signal(&json), HookSignal::Ignored, "{kind}");
    }
    assert_eq!(
        signal(r#"{"session_id":"s","hook_event_name":"Notification","message":"hello"}"#),
        HookSignal::Ignored
    );
}

#[test]
fn stop() {
    let json = r#"{
      "session_id": "s", "transcript_path": "/t.jsonl", "cwd": "/w", "permission_mode": "default",
      "hook_event_name": "Stop",
      "stop_hook_active": false,
      "last_assistant_message": "I fixed the flaky test.",
      "background_tasks": [{"id": "task-001", "type": "shell", "status": "running", "description": "tail logs", "command": "tail -f x"}],
      "session_crons": []
    }"#;
    let parsed = payload(json);
    assert_eq!(parsed.stop_hook_active, Some(false));
    assert_eq!(
        interpret(&parsed),
        HookSignal::Stopped {
            last_message: Some("I fixed the flaky test.".into())
        }
    );
}

#[test]
fn session_end() {
    let json = r#"{"session_id":"s","transcript_path":"/t.jsonl","cwd":"/w","hook_event_name":"SessionEnd","reason":"other"}"#;
    assert_eq!(
        signal(json),
        HookSignal::SessionEnded {
            reason: Some("other".into())
        }
    );
}

#[test]
fn subagent_start_and_stop() {
    let start = r#"{"session_id":"s","transcript_path":"/t.jsonl","cwd":"/w","hook_event_name":"SubagentStart","agent_id":"agent-abc123","agent_type":"Explore"}"#;
    assert_eq!(
        signal(start),
        HookSignal::SubagentStarted {
            id: "agent-abc123".into(),
            kind: Some("Explore".into()),
        }
    );
    let stop = r#"{
      "session_id": "s", "transcript_path": "/t.jsonl", "cwd": "/w", "permission_mode": "default",
      "hook_event_name": "SubagentStop", "stop_hook_active": false,
      "agent_id": "agent-abc123", "agent_type": "Explore",
      "agent_transcript_path": "/t/subagents/agent-abc123.jsonl",
      "last_assistant_message": "Found 3 callers.",
      "background_tasks": [], "session_crons": []
    }"#;
    assert_eq!(
        signal(stop),
        HookSignal::SubagentStopped {
            id: "agent-abc123".into(),
            last_message: Some("Found 3 callers.".into()),
        }
    );
    assert_eq!(
        signal(r#"{"session_id":"s","hook_event_name":"SubagentStart"}"#),
        HookSignal::Ignored
    );
}

#[test]
fn unknown_events_are_ignored() {
    let json = r#"{"session_id":"s","hook_event_name":"StopFailure","error":"overloaded"}"#;
    let parsed = payload(json);
    assert_eq!(
        parsed.hook_event_name,
        HookEventName::Other("StopFailure".into())
    );
    assert_eq!(interpret(&parsed), HookSignal::Ignored);
}

#[test]
fn unexpected_types_are_tolerated() {
    let json = r#"{
      "session_id": "s", "hook_event_name": "Stop", "cwd": 42,
      "stop_hook_active": "yes", "last_assistant_message": ["not", "text"],
      "model": {"id": "claude-opus-5", "display_name": "Opus"}
    }"#;
    let parsed = payload(json);
    assert_eq!(parsed.cwd, None);
    assert_eq!(parsed.stop_hook_active, None);
    assert_eq!(parsed.model.as_deref(), Some("claude-opus-5"));
    assert_eq!(
        interpret(&parsed),
        HookSignal::Stopped { last_message: None }
    );
}

#[test]
fn the_session_and_the_event_are_required() {
    assert!(HookPayload::from_json(br#"{"hook_event_name":"Stop"}"#).is_err());
    assert!(HookPayload::from_json(br#"{"session_id":"s"}"#).is_err());
    assert!(HookPayload::from_json(b"not json").is_err());
}

#[test]
fn signals_imply_states() {
    let working = [
        HookSignal::PromptSubmitted,
        HookSignal::ToolStarted {
            tool: "Bash".into(),
            summary: "Bash".into(),
        },
        HookSignal::ToolFinished {
            tool: "Bash".into(),
            summary: "Bash".into(),
            files: vec![],
        },
    ];
    for signal in working {
        assert_eq!(
            signal.implied_state(),
            Some(AgentState::Working),
            "{signal:?}"
        );
    }
    assert_eq!(
        HookSignal::NeedsInput {
            request: InputRequest::Dialog,
            detail: None
        }
        .implied_state(),
        Some(AgentState::NeedsInput {
            request: InputRequest::Dialog
        })
    );
    assert_eq!(
        HookSignal::Stopped { last_message: None }.implied_state(),
        Some(AgentState::Done)
    );
    let ended = |reason: Option<&str>| {
        HookSignal::SessionEnded {
            reason: reason.map(str::to_owned),
        }
        .implied_state()
    };
    assert_eq!(ended(Some("logout")), Some(AgentState::Stopped));
    assert_eq!(ended(Some("prompt_input_exit")), Some(AgentState::Stopped));
    assert_eq!(ended(None), Some(AgentState::Stopped));
    assert_eq!(ended(Some("clear")), None);
    assert_eq!(ended(Some("resume")), None);
    for silent in [
        signal(SESSION_START),
        HookSignal::SubagentStarted {
            id: "a".into(),
            kind: None,
        },
        HookSignal::SubagentStopped {
            id: "a".into(),
            last_message: None,
        },
        HookSignal::Ignored,
    ] {
        assert_eq!(silent.implied_state(), None, "{silent:?}");
    }
}

#[test]
fn event_names_convert_to_and_from_kebab_case() {
    for event in HookEventName::OBSERVED {
        assert_eq!(HookEventName::from_kebab(&event.kebab()), event);
        assert_eq!(HookEventName::from(event.as_str()), event);
        assert_eq!(event.to_string(), event.as_str());
    }
    assert_eq!(HookEventName::PreToolUse.kebab(), "pre-tool-use");
    assert_eq!(
        HookEventName::UserPromptSubmit.kebab(),
        "user-prompt-submit"
    );
    assert_eq!(HookEventName::Stop.kebab(), "stop");
    let other = HookEventName::from_kebab("stop-failure");
    assert_eq!(other, HookEventName::Other("StopFailure".into()));
    assert_eq!(other.kebab(), "stop-failure");
}

#[test]
fn payloads_deserialize_from_values_too() {
    let value = json!({"session_id": "s", "hook_event_name": "UserPromptSubmit", "prompt": "hi"});
    let parsed: HookPayload = serde_json::from_value(value).unwrap();
    assert_eq!(parsed.hook_event_name, HookEventName::UserPromptSubmit);
}

#[test]
fn permission_tools_come_from_the_message() {
    assert_eq!(
        permission_tool("Claude needs your permission to use mcp__volee__send_message"),
        Some("mcp__volee__send_message".into())
    );
    assert_eq!(
        permission_tool("Claude needs your permission to use Bash."),
        Some("Bash".into())
    );
    assert_eq!(
        permission_tool("Claude needs your permission to use "),
        None
    );
    assert_eq!(permission_tool("Waiting"), None);
}
