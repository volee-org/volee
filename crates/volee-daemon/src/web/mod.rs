//! The web interface (v2, step 3): the bundle, a JSON API and a server-sent
//! events stream, on the same loopback listener as the hook server.
//!
//! | Request | Token | Answer |
//! | --- | --- | --- |
//! | `GET /` and the bundle's files | no | the page (it reads the token from its address) |
//! | `GET /api/inbox` | yes | the inbox, as JSON |
//! | `GET /api/events` | yes | the inbox now, then again each time it changes |
//! | `POST /api/agents/{id}/terminal` | yes | focuses the agent's terminal |
//! | `POST /api/decisions/{id}` | yes | decides a held permission or question |
//!
//! The token is the hook token. A browser's `EventSource` cannot set a
//! header, so the events stream also takes it as `?token=`. Requests whose
//! `Host` is not a loopback name are refused (DNS rebinding).

mod assets;
pub(crate) mod inbox;
pub(crate) mod terminal;

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use futures_util::Stream;
use serde::Deserialize;
use serde_json::json;

use crate::agents_view::Agents;
use crate::decisions::{DecideError, Decision, Decisions};
use terminal::{Target, Terminal};

/// What the web routes need besides the token.
#[derive(Clone, Debug)]
pub(crate) struct Web {
    agents: Arc<Agents>,
    terminal: Option<Arc<dyn Terminal>>,
    decisions: Arc<Decisions>,
}

impl Web {
    /// The web interface over `agents`, with the terminal adapter found on
    /// this machine (Rex), if any.
    pub(crate) fn detect(agents: Arc<Agents>, decisions: Arc<Decisions>) -> Self {
        if !assets::is_bundled() {
            tracing::warn!("this volee was built without the web interface (see ui/)");
        }
        let terminal = std::env::var_os("HOME").and_then(|home| {
            terminal::Rex::detect(std::path::Path::new(&home), &|name| {
                std::env::var(name).ok()
            })
        });
        Self::new(
            agents,
            terminal.map(|rex| Arc::new(rex) as Arc<dyn Terminal>),
            decisions,
        )
    }

    pub(crate) fn new(
        agents: Arc<Agents>,
        terminal: Option<Arc<dyn Terminal>>,
        decisions: Arc<Decisions>,
    ) -> Self {
        Self {
            agents,
            terminal,
            decisions,
        }
    }

    /// The held permission requests and questions.
    pub(crate) fn decisions(&self) -> Arc<Decisions> {
        Arc::clone(&self.decisions)
    }

    /// The agents the web interface shows, which hook events nudge.
    pub(crate) fn agents(&self) -> Arc<Agents> {
        Arc::clone(&self.agents)
    }

    /// The inbox now.
    async fn inbox(&self) -> inbox::Inbox {
        let view = match self.agents.peek() {
            Some(view) => view,
            None => self.agents.latest().await,
        };
        let name = self
            .terminal
            .as_ref()
            .filter(|terminal| terminal.available())
            .map(|terminal| terminal.name());
        inbox::build_with(&view, name, &self.decisions.cards())
    }
}

#[derive(Clone)]
struct Shared {
    token: Arc<str>,
    web: Web,
}

/// The routes of the interface; merged into the hook server's router.
pub(crate) fn router(token: Arc<str>, web: Web) -> Router {
    Router::new()
        .route("/api/inbox", get(get_inbox))
        .route("/api/events", get(events))
        .route("/api/agents/{id}/terminal", post(open_terminal))
        .route("/api/decisions/{id}", post(decide))
        .fallback(get(assets::serve))
        .layer(middleware::from_fn(guard))
        .with_state(Shared { token, web })
}

/// Refuses a `Host` that is not a loopback name, and tells browsers what the
/// page may do: load its own files, nothing from elsewhere.
async fn guard(request: Request, next: Next) -> Response {
    let host_ok = request
        .headers()
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .is_some_and(loopback_host);
    if !host_ok {
        return (StatusCode::FORBIDDEN, "forbidden host").into_response();
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

/// Whether a `Host` header (with or without a port) names this machine.
fn loopback_host(host: &str) -> bool {
    let name = match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name,
        _ => host,
    };
    matches!(name, "127.0.0.1" | "localhost" | "[::1]")
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

/// Whether the request carries the token, in the header or (for the events
/// stream) in the query.
fn allowed(shared: &Shared, headers: &HeaderMap, query: Option<&str>) -> bool {
    crate::hook_server::authorized(headers, &shared.token)
        || query.is_some_and(|given| {
            crate::hook_server::same(given.as_bytes(), shared.token.as_bytes())
        })
}

async fn get_inbox(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
) -> Response {
    if !allowed(&shared, &headers, query.token.as_deref()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(shared.web.inbox().await).into_response()
}

async fn events(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
) -> Response {
    if !allowed(&shared, &headers, query.token.as_deref()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let sse: Sse<_> = Sse::new(inbox_stream(shared.web)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    );
    sse.into_response()
}

/// The inbox, once at once, then each time the agents or the held requests
/// change. A client that connects late gets the whole current inbox first,
/// so nothing is replayed.
fn inbox_stream(web: Web) -> impl Stream<Item = Result<Event, Infallible>> {
    let agents = web.agents.subscribe();
    let decisions = web.decisions.subscribe();
    futures_util::stream::unfold(
        (web, agents, decisions, true),
        |(web, mut agents, mut decisions, first)| async move {
            if !first {
                tokio::select! {
                    changed = agents.changed() => if changed.is_err() { return None; },
                    changed = decisions.changed() => if changed.is_err() { return None; },
                }
            }
            let inbox = web.inbox().await;
            let event = Event::default()
                .event("inbox")
                .json_data(&inbox)
                .unwrap_or_else(|_| Event::default().comment("the inbox could not be encoded"));
            Some((Ok(event), (web, agents, decisions, false)))
        },
    )
}

/// Applies what the person chose on a held card. `404` when the card is gone
/// (decided elsewhere, expired, or the terminal answered first), `422` when
/// the decision does not fit the card.
async fn decide(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(decision): Json<Decision>,
) -> Response {
    if !allowed(&shared, &headers, None) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match shared.web.decisions.decide(&id, decision) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(DecideError::Gone) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "message": "this request is gone: it was answered in the terminal or it expired" })),
        )
            .into_response(),
        Err(DecideError::Invalid(message)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "ok": false, "message": message })),
        )
            .into_response(),
    }
}

async fn open_terminal(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !allowed(&shared, &headers, None) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let reply = |status: StatusCode, ok: bool, message: String| {
        (status, Json(json!({ "ok": ok, "message": message }))).into_response()
    };
    let Some(terminal) = shared.web.terminal.clone().filter(|t| t.available()) else {
        return reply(
            StatusCode::NOT_FOUND,
            false,
            "no terminal program found".to_owned(),
        );
    };
    let view = shared.web.agents.latest().await;
    let Some(agent) = view.agents.iter().find(|agent| agent.session_id == id) else {
        return reply(
            StatusCode::NOT_FOUND,
            false,
            "this agent is gone".to_owned(),
        );
    };
    let target = Target {
        pid: agent.pid,
        cwd: agent.cwd.clone(),
    };
    match tokio::task::spawn_blocking(move || terminal.open(&target)).await {
        Ok(Ok(message)) => reply(StatusCode::OK, true, message),
        Ok(Err(message)) => reply(StatusCode::UNPROCESSABLE_ENTITY, false, message),
        Err(error) => reply(StatusCode::INTERNAL_SERVER_ERROR, false, error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_hosts_are_served() {
        assert!(loopback_host("127.0.0.1:47835"));
        assert!(loopback_host("localhost:47835"));
        assert!(loopback_host("localhost"));
        assert!(loopback_host("[::1]:80"));
        assert!(!loopback_host("evil.example:47835"));
        assert!(!loopback_host("127.0.0.1.evil.example"));
        assert!(!loopback_host("192.168.1.10:47835"));
        assert!(!loopback_host(""));
    }
}
