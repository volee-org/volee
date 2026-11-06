//! `claude agents --json --all`: every Claude Code session on the machine.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;
use volee_core::harness::{HarnessKind, SessionKey};
use volee_core::reconcile::{ListedSession, Listing};
use volee_core::state::{AgentState, InputRequest};

use crate::error::ParseError;
use crate::lenient;

/// One element of the listing, as Claude Code 2.1.319 prints it.
///
/// Only `sessionId` is required. Unknown fields are ignored, and a known
/// field with an unexpected type is treated as missing.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSession {
    session_id: String,
    #[serde(default, deserialize_with = "lenient::optional")]
    cwd: Option<PathBuf>,
    /// `interactive` or `background`.
    #[serde(default, deserialize_with = "lenient::optional")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "lenient::optional")]
    name: Option<String>,
    /// Background sessions only: `working`, `blocked`, `done`, `failed` or
    /// `stopped`.
    #[serde(default, deserialize_with = "lenient::optional")]
    state: Option<String>,
    /// Present while the process is alive.
    #[serde(default)]
    pid: Option<Value>,
    /// While the process is alive: `busy`, `waiting` or `idle`.
    #[serde(default, deserialize_with = "lenient::optional")]
    status: Option<String>,
    /// While waiting: what for, such as `permission prompt`.
    #[serde(default, deserialize_with = "lenient::optional")]
    waiting_for: Option<String>,
}

impl RawSession {
    fn into_listed(self) -> ListedSession {
        let alive = self.pid.is_some();
        ListedSession {
            state: session_state(
                self.state.as_deref(),
                self.status.as_deref(),
                self.waiting_for.as_deref(),
                alive,
            ),
            key: SessionKey::claude(self.session_id),
            name: self.name.filter(|name| !name.trim().is_empty()),
            cwd: self.cwd.unwrap_or_default(),
            alive,
            background: self.kind.as_deref() == Some("background"),
        }
    }
}

/// Maps the output of `claude agents --json --all` to a [`Listing`].
///
/// Each session is keyed by its session UUID, is alive while it has a
/// process id, and is a background session when its kind is `background`;
/// [`session_state`] maps its state. The listing is complete (it includes
/// finished sessions) unless an element could not be read: such an element
/// is skipped and the listing is marked incomplete, so that it can never make
/// a session look gone.
pub fn parse_listing(stdout: &[u8]) -> Result<Listing, ParseError> {
    let elements: Vec<Value> = serde_json::from_slice(stdout).map_err(ParseError::Listing)?;
    let mut complete = true;
    let mut sessions = Vec::with_capacity(elements.len());
    for element in elements {
        match serde_json::from_value::<RawSession>(element) {
            Ok(raw) if !raw.session_id.trim().is_empty() => sessions.push(raw.into_listed()),
            Ok(_) => {
                complete = false;
                tracing::warn!("skipped a Claude Code session without a session id");
            }
            Err(error) => {
                complete = false;
                tracing::warn!(%error, "skipped a Claude Code session in an unknown shape");
            }
        }
    }
    Ok(Listing {
        harness: HarnessKind::ClaudeCode,
        sessions,
        complete,
    })
}

/// Maps the state Claude Code reports for a session to volee's.
///
/// Background sessions report a `state`:
///
/// | `state` | volee state |
/// | --- | --- |
/// | `working` | working |
/// | `blocked` | needs input, with [`input_request`] of `waitingFor` (or "blocked" when absent) |
/// | `done` | done while the process is alive ("ready for the next prompt"), stopped otherwise |
/// | `failed` | failed |
/// | `stopped` | stopped |
///
/// Interactive sessions report only the `status` of their process, which is
/// also the fallback for an unknown or missing `state`:
///
/// | `status` | volee state |
/// | --- | --- |
/// | `busy` | working |
/// | `waiting` | needs input, with [`input_request`] of `waitingFor` (or "waiting" when absent) |
/// | `idle` | idle |
/// | none | idle while the process is alive, stopped otherwise |
pub fn session_state(
    state: Option<&str>,
    status: Option<&str>,
    waiting_for: Option<&str>,
    alive: bool,
) -> AgentState {
    let waiting_for = waiting_for.map(str::trim).filter(|what| !what.is_empty());
    let needs_input = |fallback: &str| AgentState::NeedsInput {
        request: waiting_for.map_or_else(
            || InputRequest::Other {
                detail: fallback.to_owned(),
            },
            input_request,
        ),
    };
    match state.map(str::trim) {
        Some("working") => AgentState::Working,
        Some("blocked") => needs_input("blocked"),
        Some("done") if alive => AgentState::Done,
        Some("done" | "stopped") => AgentState::Stopped,
        Some("failed") => AgentState::Failed,
        _ => match status.map(str::trim) {
            Some("busy") => AgentState::Working,
            Some("waiting") => needs_input("waiting"),
            Some("idle") => AgentState::Idle,
            _ if alive => AgentState::Idle,
            _ => AgentState::Stopped,
        },
    }
}

/// Maps Claude Code's `waitingFor` text to what the session needs.
///
/// `permission prompt` is a permission (the tool is not reported), `input
/// needed` a question, `dialog open` a dialog; anything else, such as
/// `sandbox request` or `worker request`, is kept as is.
pub fn input_request(waiting_for: &str) -> InputRequest {
    match waiting_for.trim() {
        "permission prompt" => InputRequest::Permission { tool: None },
        "input needed" => InputRequest::Question,
        "dialog open" => InputRequest::Dialog,
        other => InputRequest::Other {
            detail: other.to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two elements measured on Claude Code 2.1.319, verbatim.
    const MEASURED: &str = r#"[
{"id":"4402670e","cwd":"/Users/jeff/Developer/414Aptitudes/wibeo","kind":"background","startedAt":1790611230754,"sessionId":"4402670e-5b60-4cdc-8cf3-be456e780815","name":"neon billing investigation","state":"done"},
{"pid":95631,"id":"b60aa3b9","cwd":"/Users/jeff/Developer/Labs/tuios-cockpit/.claude/worktrees/fix-x","kind":"background","startedAt":1793984015804,"sessionId":"b60aa3b9-73ea-4599-91a5-0725e0c83482","name":"cockpit open launch failure","status":"busy","state":"working"}
]"#;

    fn parse(json: &str) -> Listing {
        parse_listing(json.as_bytes()).unwrap()
    }

    fn only(json: &str) -> ListedSession {
        let mut listing = parse(json);
        assert_eq!(listing.sessions.len(), 1, "{listing:?}");
        listing.sessions.remove(0)
    }

    fn needs(request: InputRequest) -> AgentState {
        AgentState::NeedsInput { request }
    }

    #[test]
    fn maps_the_measured_elements() {
        let listing = parse(MEASURED);
        assert_eq!(listing.harness, HarnessKind::ClaudeCode);
        assert!(listing.complete);
        assert_eq!(
            listing.sessions,
            [
                ListedSession {
                    key: SessionKey::claude("4402670e-5b60-4cdc-8cf3-be456e780815"),
                    name: Some("neon billing investigation".into()),
                    cwd: "/Users/jeff/Developer/414Aptitudes/wibeo".into(),
                    state: AgentState::Stopped,
                    alive: false,
                    background: true,
                },
                ListedSession {
                    key: SessionKey::claude("b60aa3b9-73ea-4599-91a5-0725e0c83482"),
                    name: Some("cockpit open launch failure".into()),
                    cwd: "/Users/jeff/Developer/Labs/tuios-cockpit/.claude/worktrees/fix-x".into(),
                    state: AgentState::Working,
                    alive: true,
                    background: true,
                },
            ]
        );
    }

    #[test]
    fn maps_interactive_sessions_by_process_status() {
        let session = |status: &str| {
            only(&format!(
                r#"[{{"pid":1,"cwd":"/w","kind":"interactive","startedAt":1,"sessionId":"aaaa1111-0000-0000-0000-000000000000","name":"","status":"{status}"}}]"#
            ))
        };
        let busy = session("busy");
        assert_eq!(busy.state, AgentState::Working);
        assert!(busy.alive);
        assert!(!busy.background);
        assert_eq!(busy.name, None, "an empty name is no name");
        assert_eq!(
            session("waiting").state,
            needs(InputRequest::Other {
                detail: "waiting".into()
            })
        );
        assert_eq!(session("idle").state, AgentState::Idle);
    }

    #[test]
    fn an_interactive_session_waiting_for_permission_says_so() {
        let session = only(
            r#"[{"pid":1,"cwd":"/w","kind":"interactive","sessionId":"s1","status":"waiting","waitingFor":"permission prompt"}]"#,
        );
        assert_eq!(
            session.state,
            needs(InputRequest::Permission { tool: None })
        );
    }

    #[test]
    fn maps_every_documented_waiting_reason() {
        let cases = [
            ("permission prompt", InputRequest::Permission { tool: None }),
            ("input needed", InputRequest::Question),
            ("dialog open", InputRequest::Dialog),
            (
                "sandbox request",
                InputRequest::Other {
                    detail: "sandbox request".into(),
                },
            ),
            (
                "worker request",
                InputRequest::Other {
                    detail: "worker request".into(),
                },
            ),
        ];
        for (waiting_for, request) in cases {
            let session = only(&format!(
                r#"[{{"pid":7,"id":"cccc3333","cwd":"/w","kind":"background","sessionId":"cccc3333-0000-0000-0000-000000000000","status":"waiting","waitingFor":"{waiting_for}","state":"blocked"}}]"#
            ));
            assert_eq!(session.state, needs(request), "{waiting_for}");
        }
    }

    #[test]
    fn blocked_without_a_reason_still_needs_input() {
        assert_eq!(
            session_state(Some("blocked"), None, None, true),
            needs(InputRequest::Other {
                detail: "blocked".into()
            })
        );
        assert_eq!(
            session_state(Some("blocked"), None, Some("  "), true),
            needs(InputRequest::Other {
                detail: "blocked".into()
            })
        );
    }

    #[test]
    fn done_means_ready_while_alive_and_stopped_otherwise() {
        assert_eq!(
            session_state(Some("done"), Some("idle"), None, true),
            AgentState::Done
        );
        assert_eq!(
            session_state(Some("done"), None, None, false),
            AgentState::Stopped
        );
    }

    #[test]
    fn failed_and_stopped_map_directly() {
        assert_eq!(
            session_state(Some("failed"), None, None, false),
            AgentState::Failed
        );
        assert_eq!(
            session_state(Some("stopped"), None, None, false),
            AgentState::Stopped
        );
    }

    #[test]
    fn a_missing_or_unknown_state_falls_back_to_the_process() {
        assert_eq!(session_state(None, None, None, true), AgentState::Idle);
        assert_eq!(session_state(None, None, None, false), AgentState::Stopped);
        assert_eq!(
            session_state(Some("paused"), Some("busy"), None, true),
            AgentState::Working
        );
        let session = only(
            r#"[{"pid":3,"id":"dddd4444","cwd":"/w","kind":"background","sessionId":"dddd4444-0000-0000-0000-000000000000","status":"idle"}]"#,
        );
        assert_eq!(session.state, AgentState::Idle);
    }

    #[test]
    fn unknown_fields_are_ignored_and_missing_ones_tolerated() {
        let session = only(
            r#"[{"sessionId":"eeee5555-0000-0000-0000-000000000000","state":"working","pid":12,"color":"teal","worktree":{"path":"/w"},"startedAt":"yesterday","kind":42}]"#,
        );
        assert_eq!(session.state, AgentState::Working);
        assert_eq!(session.cwd, PathBuf::new());
        assert_eq!(session.name, None);
        assert!(session.alive);
        assert!(!session.background, "an unreadable kind is not background");
    }

    #[test]
    fn an_unreadable_element_is_skipped_and_the_listing_is_incomplete() {
        let listing = parse(
            r#"[{"cwd":"/w","kind":"background","state":"done"},{"sessionId":""},{"sessionId":"ffff6666-0000-0000-0000-000000000000","state":"done","pid":null}]"#,
        );
        assert!(!listing.complete);
        assert_eq!(listing.sessions.len(), 1);
        let session = &listing.sessions[0];
        assert!(!session.alive, "a null pid is no process");
        assert_eq!(session.state, AgentState::Stopped);
    }

    #[test]
    fn an_empty_listing_is_complete() {
        let listing = parse("[]\n");
        assert!(listing.complete);
        assert_eq!(listing.sessions, []);
    }

    #[test]
    fn anything_but_an_array_is_an_error() {
        for bad in ["", "{}", "No sessions", "[{]"] {
            assert!(
                matches!(parse_listing(bad.as_bytes()), Err(ParseError::Listing(_))),
                "{bad:?}"
            );
        }
    }
}
