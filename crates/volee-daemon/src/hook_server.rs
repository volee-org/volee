//! The hook server (v2, step 1): receives the events Claude Code posts through
//! `http` hooks and stores them.
//!
//! It decides nothing by itself. Every authenticated request whose body is a
//! JSON object is stored as received and answered with an empty `200`, which
//! Claude Code reads as "no opinion": the session goes on as if the hook did
//! not exist. Two requests are the exception when the web interface is
//! served: a `PermissionRequest` (`/hooks/PermissionRequest`) and the
//! `PreToolUse` of `AskUserQuestion` (`/decide/AskUserQuestion`) are held
//! until the person decides in the browser (see `decisions.rs`). A request
//! without the token is refused with `401`, a body that is not a JSON object
//! with `400`. Claude Code treats any non-2xx answer, a
//! refused connection and a timeout alike as a non-blocking error for these
//! hooks, so a failure here never stops a session.
//!
//! The server listens on the loopback interface only, and every hook request
//! carries the per-install token in an `Authorization: Bearer` header.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use tokio::net::TcpListener;
use volee_store::{NewHookEvent, Store};

use crate::decisions::{DecisionKind, Held, HoldSettings, Observed};
use crate::error::DaemonError;

/// The biggest request body the server reads: a transcript excerpt or a long
/// tool result can be large, but never this large.
const MAX_BODY: usize = 8 * 1024 * 1024;

/// How the hook server is set up.
#[derive(Clone, Debug)]
pub struct HookServerConfig {
    /// Where it listens. Must be a loopback address.
    pub addr: SocketAddr,
    /// The token every request must carry.
    pub token: String,
    /// The SQLite database the events go to.
    pub database: PathBuf,
    /// How long permission requests and questions are held.
    pub hold: HoldSettings,
}

/// Binds the listener of `config`, refusing an address that is not loopback.
pub async fn bind(config: &HookServerConfig) -> Result<TcpListener, DaemonError> {
    if !config.addr.ip().is_loopback() {
        return Err(DaemonError::Unavailable(format!(
            "the hook server only listens on the loopback interface, not on {}",
            config.addr.ip()
        )));
    }
    TcpListener::bind(config.addr).await.map_err(|error| {
        DaemonError::Unavailable(format!(
            "cannot listen for hooks on {}: {error}; set another `[hooks] port`",
            config.addr
        ))
    })
}

/// Serves hook requests on `listener` until the future is dropped.
pub async fn serve(listener: TcpListener, token: String, store: Store) -> std::io::Result<()> {
    axum::serve(listener, router(token, store, None)).await
}

/// Serves hook requests and, when `web` is given, the web interface and its
/// API on the same listener.
pub(crate) async fn serve_with_web(
    listener: TcpListener,
    token: String,
    store: Store,
    web: Option<crate::web::Web>,
) -> std::io::Result<()> {
    axum::serve(listener, router(token, store, web)).await
}

#[derive(Clone)]
struct Shared {
    token: Arc<str>,
    store: Arc<Mutex<Store>>,
    /// Told when an event is stored, so that the agents are read again now.
    agents: Option<Arc<crate::agents_view::Agents>>,
    /// The held requests; without the web interface nobody could decide, so
    /// nothing is held.
    decisions: Option<Arc<crate::decisions::Decisions>>,
}

fn router(token: String, store: Store, web: Option<crate::web::Web>) -> Router {
    let token: Arc<str> = token.into();
    let shared = Shared {
        token: Arc::clone(&token),
        store: Arc::new(Mutex::new(store)),
        agents: web.as_ref().map(crate::web::Web::agents),
        decisions: web.as_ref().map(crate::web::Web::decisions),
    };
    let hooks = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/hooks/{event}", post(hook))
        .route("/decide/{tool}", post(decide))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(shared);
    match web {
        Some(web) => hooks.merge(crate::web::router(token, web)),
        None => hooks,
    }
}

async fn hook(
    State(shared): State<Shared>,
    Path(path_event): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !authorized(&headers, &shared.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(payload) = std::str::from_utf8(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(event) = describe(&path_event, payload) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let store = Arc::clone(&shared.store);
    let name = event.event.clone();
    let stored = tokio::task::spawn_blocking(move || {
        let store = store.lock().unwrap_or_else(PoisonError::into_inner);
        store.append_hook_event(&event)
    })
    .await;
    match stored {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            tracing::error!(%error, "cannot store a hook event");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Err(error) => {
            tracing::error!(%error, "the hook storing task failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    if let Some(agents) = &shared.agents {
        agents.nudge();
    }
    let Some(decisions) = &shared.decisions else {
        return StatusCode::OK.into_response();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return StatusCode::OK.into_response();
    };
    let text = |key: &str| value.get(key).and_then(serde_json::Value::as_str);
    decisions.observe(&Observed {
        event: &name,
        session_id: text("session_id"),
        tool_name: text("tool_name"),
        tool_input: value.get("tool_input"),
    });
    if name != "PermissionRequest" {
        return StatusCode::OK.into_response();
    }
    let (Some(session_id), Some(tool_name)) = (text("session_id"), text("tool_name")) else {
        return StatusCode::OK.into_response();
    };
    // These two need an `updatedInput` the card cannot build: the terminal
    // keeps them.
    if matches!(tool_name, "AskUserQuestion" | "ExitPlanMode") {
        return StatusCode::OK.into_response();
    }
    let held = Held {
        kind: DecisionKind::Permission,
        session_id: session_id.to_owned(),
        tool_name: tool_name.to_owned(),
        tool_input: value.get("tool_input").cloned().unwrap_or_default(),
        suggestions: value
            .get("permission_suggestions")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default(),
    };
    answer(decisions.hold(held).await)
}

/// The `PreToolUse` of `AskUserQuestion`, held for an answer. It is not
/// stored: the generic `PreToolUse` hook of the same call is.
async fn decide(
    State(shared): State<Shared>,
    Path(tool): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !authorized(&headers, &shared.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let (Some(decisions), "AskUserQuestion") = (&shared.decisions, tool.as_str()) else {
        return StatusCode::OK.into_response();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let text = |key: &str| value.get(key).and_then(serde_json::Value::as_str);
    let input = value.get("tool_input").cloned().unwrap_or_default();
    let (Some(session_id), true, true) = (
        text("session_id"),
        text("tool_name") == Some("AskUserQuestion"),
        input
            .get("questions")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|questions| !questions.is_empty()),
    ) else {
        return StatusCode::OK.into_response();
    };
    answer(
        decisions
            .hold(Held {
                kind: DecisionKind::Question,
                session_id: session_id.to_owned(),
                tool_name: "AskUserQuestion".to_owned(),
                tool_input: input,
                suggestions: Vec::new(),
            })
            .await,
    )
}

/// A held request's answer: the body, or an empty `200` for "no opinion".
fn answer(body: Option<serde_json::Value>) -> Response {
    match body {
        Some(body) => Json(body).into_response(),
        None => StatusCode::OK.into_response(),
    }
}

/// The row for a payload, or `None` when the body is not a JSON object or
/// the event name in the path is not a plain word.
fn describe(path_event: &str, payload: &str) -> Option<NewHookEvent> {
    if path_event.is_empty()
        || path_event.len() > 64
        || !path_event.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let object = value.as_object()?;
    let text = |key: &str| {
        object
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    Some(NewHookEvent {
        received_at: now_millis(),
        // The payload names its own event; the path is the fallback.
        event: text("hook_event_name").unwrap_or_else(|| path_event.to_owned()),
        session_id: text("session_id"),
        cwd: text("cwd"),
        payload: payload.to_owned(),
    })
}

pub(crate) fn authorized(headers: &HeaderMap, token: &str) -> bool {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some(given) = value.strip_prefix("Bearer ") else {
        return false;
    };
    same(given.as_bytes(), token.as_bytes())
}

/// Compares two byte strings without stopping at the first difference.
pub(crate) fn same(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0_u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_payload_names_the_event_the_session_and_the_directory() {
        let event = describe(
            "PreToolUse",
            r#"{"hook_event_name":"PreToolUse","session_id":"s1","cwd":"/work","tool_name":"Bash"}"#,
        )
        .unwrap();
        assert_eq!(event.event, "PreToolUse");
        assert_eq!(event.session_id.as_deref(), Some("s1"));
        assert_eq!(event.cwd.as_deref(), Some("/work"));
        assert!(event.payload.contains("Bash"), "the raw payload is kept");
    }

    #[test]
    fn the_path_names_the_event_when_the_payload_does_not() {
        let event = describe("Stop", r#"{"session_id":"s1"}"#).unwrap();
        assert_eq!(event.event, "Stop");
    }

    #[test]
    fn what_is_not_a_json_object_or_a_plain_event_name_is_refused() {
        assert!(describe("Stop", "not json").is_none());
        assert!(describe("Stop", "[1]").is_none());
        assert!(describe("Stop", "\"text\"").is_none());
        assert!(describe("", "{}").is_none());
        assert!(describe("a/b", "{}").is_none());
        assert!(describe("x y", "{}").is_none());
    }

    #[test]
    fn only_the_exact_bearer_token_is_authorized() {
        let with = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::AUTHORIZATION, value.parse().unwrap());
            headers
        };
        assert!(authorized(&with("Bearer secret"), "secret"));
        assert!(!authorized(&with("Bearer secre"), "secret"));
        assert!(!authorized(&with("Bearer secrets"), "secret"));
        assert!(!authorized(&with("secret"), "secret"));
        assert!(!authorized(&with("bearer secret"), "secret"));
        assert!(!authorized(&HeaderMap::new(), "secret"));
    }
}
