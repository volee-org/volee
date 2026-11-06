//! Request handlers: one function per method.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use volee_core::harness::{SessionKey, SessionOrigin};
use volee_core::ids::ThreadId;
use volee_core::message::{Message, Party, Receipt, Thread};
use volee_core::state::{AgentState, InputRequest};
use volee_core::time::Timestamp;
use volee_core::{Agent, AgentId, Event, EventKind};
use volee_proto::PROTOCOL_VERSION;
use volee_proto::methods::{
    AgentParams, Empty, EventBatch, EventsSince, Forget, HookIngest, HookParams, HookReply,
    HookWait, MarkRead, MarkReadParams, Method, SendMessage, SendParams, Sent, Shutdown,
    SinceParams, Status, StatusInfo, StatusParams, WaitParams, WaitReply,
};
use volee_proto::rpc::{self, RpcError};

use crate::delivery::{sender_label, stop_block_output};
use crate::error::DaemonError;
use crate::server::Shared;
use crate::state::State;

/// Longest a hook may wait for a message; Claude Code bounds it too.
const MAX_WAIT: Duration = Duration::from_secs(24 * 60 * 60);

/// Dispatches a request to its handler and returns the JSON result.
pub(crate) async fn dispatch(
    shared: &Arc<Shared>,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    match method {
        Status::NAME => respond(status(shared, parse::<Status>(params)?).await),
        Shutdown::NAME => {
            parse::<Shutdown>(params)?;
            shared.request_shutdown();
            respond(Ok(Empty {}))
        }
        EventsSince::NAME => respond(events_since(shared, parse::<EventsSince>(params)?).await),
        Forget::NAME => respond(forget(shared, parse::<Forget>(params)?).await),
        SendMessage::NAME => respond(send_message(shared, parse::<SendMessage>(params)?).await),
        MarkRead::NAME => respond(mark_read(shared, parse::<MarkRead>(params)?).await),
        HookIngest::NAME => {
            respond(hook_ingest(shared, parse::<HookIngest>(params)?, Delivery::Live).await)
        }
        HookWait::NAME => respond(hook_wait(shared, parse::<HookWait>(params)?).await),
        other => Err(RpcError::new(
            rpc::METHOD_NOT_FOUND,
            format!("unknown method `{other}`"),
        )),
    }
}

/// Parses parameters; `null` counts as an empty object, so methods without
/// parameters accept both.
pub(crate) fn parse<M: Method>(params: Value) -> Result<M::Params, RpcError> {
    let params = if params.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        params
    };
    serde_json::from_value(params).map_err(|error| {
        RpcError::new(
            rpc::INVALID_PARAMS,
            format!("invalid parameters for `{}`: {error}", M::NAME),
        )
    })
}

fn respond<T: serde::Serialize>(result: Result<T, DaemonError>) -> Result<Value, RpcError> {
    let output = result.map_err(RpcError::from)?;
    serde_json::to_value(output)
        .map_err(|error| RpcError::new(rpc::INTERNAL_ERROR, error.to_string()))
}

async fn status(shared: &Arc<Shared>, params: StatusParams) -> Result<StatusInfo, DaemonError> {
    if params.protocol != 0 && params.protocol != PROTOCOL_VERSION {
        return Err(DaemonError::VersionMismatch(format!(
            "this daemon speaks protocol {PROTOCOL_VERSION}, the client speaks {}; \
             stop it with `volee daemon stop` and run your command again",
            params.protocol
        )));
    }
    let state = shared.state.lock().await;
    Ok(StatusInfo {
        version: volee_core::VERSION.to_owned(),
        protocol: PROTOCOL_VERSION,
        pid: std::process::id(),
        started_at: state.started_at(),
        last_seq: state.last_seq()?,
        agents: state.registry().agents().count(),
        harnesses: shared.harnesses.clone(),
    })
}

async fn events_since(
    shared: &Arc<Shared>,
    params: SinceParams,
) -> Result<EventBatch, DaemonError> {
    let state = shared.state.lock().await;
    Ok(EventBatch {
        events: state.since(params.after)?,
        last_seq: state.last_seq()?,
    })
}

async fn forget(shared: &Arc<Shared>, params: AgentParams) -> Result<Empty, DaemonError> {
    let mut state = shared.state.lock().await;
    require_agent(&state, params.agent)?;
    let now = state.now();
    state.commit(vec![Event::new(
        now,
        EventKind::AgentForgotten {
            agent: params.agent,
        },
    )])?;
    Ok(Empty {})
}

fn require_agent(state: &State, agent: AgentId) -> Result<(), DaemonError> {
    if state.registry().agent(agent).is_some() {
        Ok(())
    } else {
        Err(DaemonError::NotFound(format!("unknown agent {agent}")))
    }
}

async fn send_message(shared: &Arc<Shared>, params: SendParams) -> Result<Sent, DaemonError> {
    if params.to.is_empty() {
        return Err(DaemonError::InvalidParams(
            "a message needs at least one recipient".into(),
        ));
    }
    if params.body.trim().is_empty() {
        return Err(DaemonError::InvalidParams("a message needs a body".into()));
    }
    let mut state = shared.state.lock().await;
    let registry = state.registry();
    for party in params.to.iter().chain(std::iter::once(&params.from)) {
        if let Party::Agent(agent) = party
            && registry.agent(*agent).is_none()
        {
            return Err(DaemonError::NotFound(format!("unknown agent {agent}")));
        }
    }
    for to in &params.to {
        registry.check_route(params.from, *to)?;
    }
    let replied = match params.in_reply_to {
        Some(id) => Some(
            registry
                .message(id)
                .ok_or_else(|| DaemonError::NotFound(format!("unknown message {id}")))?
                .thread,
        ),
        None => None,
    };
    let now = state.now();
    let mut events = Vec::new();
    let thread = match params.thread.or(replied) {
        Some(thread) => {
            if registry.thread(thread).is_none() {
                return Err(DaemonError::NotFound(format!("unknown thread {thread}")));
            }
            thread
        }
        None => pick_thread(&state, &params, &mut events),
    };
    let message = Message {
        id: state.new_message_id(),
        thread,
        from: params.from,
        kind: params.kind,
        body: params.body,
        refs: params.refs,
        in_reply_to: params.in_reply_to,
        at: now,
        receipts: params
            .to
            .iter()
            .map(|to| Receipt {
                to: *to,
                delivered_at: None,
                read_at: None,
            })
            .collect(),
    };
    let sent = Sent {
        message: message.id,
        thread,
    };
    events.push(Event::new(now, EventKind::MessagePosted { message }));
    state.commit(events)?;
    Ok(sent)
}

/// Chooses a thread when the sender did not: the mission thread when every
/// party belongs to the same mission (people included), else the direct
/// thread between two parties, else a new group thread. New threads are
/// pushed as events.
fn pick_thread(state: &State, params: &SendParams, events: &mut Vec<Event>) -> ThreadId {
    let registry = state.registry();
    let mission_of = |party: &Party| match party {
        Party::Agent(agent) => registry.agent(*agent).and_then(|a| a.mission),
        Party::Human | Party::System => None,
    };
    let agents: Vec<&Party> = std::iter::once(&params.from)
        .chain(&params.to)
        .filter(|party| matches!(party, Party::Agent(_)))
        .collect();
    let shared_mission = agents
        .first()
        .and_then(|first| mission_of(first))
        .filter(|mission| {
            agents
                .iter()
                .all(|party| mission_of(party) == Some(*mission))
        });
    if let Some(thread) = shared_mission
        .and_then(|mission| registry.mission(mission))
        .map(|mission| mission.thread)
    {
        return thread;
    }
    if let [to] = params.to.as_slice()
        && let Some(thread) = registry.thread_between(params.from, *to)
    {
        return thread.id;
    }
    let mut participants = vec![params.from];
    participants.extend(params.to.iter().copied());
    let title = participants
        .iter()
        .map(|party| sender_label(registry, *party))
        .collect::<Vec<_>>()
        .join(", ");
    let thread = Thread {
        id: state.new_thread_id(),
        title,
        mission: None,
        participants,
        opened_at: state.now(),
    };
    let id = thread.id;
    events.push(Event::new(state.now(), EventKind::ThreadOpened { thread }));
    id
}

async fn mark_read(shared: &Arc<Shared>, params: MarkReadParams) -> Result<Empty, DaemonError> {
    let mut state = shared.state.lock().await;
    let message = state
        .registry()
        .message(params.message)
        .ok_or_else(|| DaemonError::NotFound(format!("unknown message {}", params.message)))?;
    if !message.is_for(params.by) {
        return Err(DaemonError::Refused(format!(
            "{} is not a recipient of message {}",
            params.by, params.message
        )));
    }
    let now = state.now();
    state.commit(vec![Event::new(
        now,
        EventKind::MessageRead {
            message: params.message,
            by: params.by,
        },
    )])?;
    Ok(Empty {})
}

/// Whether a hook event comes from a running hook, which can hand messages to
/// its agent, or from the spool, which cannot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Delivery {
    Live,
    Replay,
}

/// Replays a hook event from the spool: same effects, but pending messages
/// stay pending because nobody reads the reply.
pub(crate) async fn replay_hook(
    shared: &Arc<Shared>,
    params: HookParams,
) -> Result<(), DaemonError> {
    hook_ingest(shared, params, Delivery::Replay)
        .await
        .map(|_| ())
}

/// Turns a hook payload into events. This first version understands the
/// fields every Claude Code hook carries; the harness adapter refines it.
async fn hook_ingest(
    shared: &Arc<Shared>,
    params: HookParams,
    delivery: Delivery,
) -> Result<HookReply, DaemonError> {
    let mut state = shared.state.lock().await;
    let agent = params.agent;
    let current = state
        .registry()
        .agent(agent)
        .ok_or_else(|| DaemonError::NotFound(format!("unknown agent {agent}")))?
        .clone();
    let now = state.now();
    let field = |name: &str| params.payload.get(name).and_then(Value::as_str);
    let mut events = Vec::new();
    let mut reply = HookReply::default();
    let mut next_state: Option<(AgentState, Option<String>)> = None;

    match params.event.as_str() {
        "session-start" => {
            events.extend(session_start_events(&current, &params.payload, now));
            next_state = Some((AgentState::Idle, None));
        }
        "user-prompt-submit" => next_state = Some((AgentState::Working, None)),
        "pre-tool-use" => {
            if let Some(tool) = field("tool_name") {
                events.push(Event::new(
                    now,
                    EventKind::ActivityRecorded {
                        agent,
                        summary: tool.to_owned(),
                    },
                ));
            }
            next_state = Some((AgentState::Working, None));
        }
        "notification" => match field("notification_type") {
            Some("permission_prompt") => {
                next_state = Some((
                    AgentState::NeedsInput {
                        request: InputRequest::Permission { tool: None },
                    },
                    field("message").map(str::to_owned),
                ));
            }
            Some("idle_prompt") => next_state = Some((AgentState::Idle, None)),
            _ => {}
        },
        "stop" => {
            let pending = if delivery == Delivery::Live {
                state.take_pending(agent)?
            } else {
                None
            };
            if let Some(text) = pending {
                reply.stdout = Some(stop_block_output(&text));
                next_state = Some((AgentState::Working, None));
            } else {
                let detail = field("last_assistant_message").map(|text| summarize(text, 200));
                next_state = Some((AgentState::Done, detail));
            }
        }
        "session-end" => {
            next_state = Some((AgentState::Stopped, field("reason").map(str::to_owned)));
        }
        _ => {}
    }

    if let Some((state_value, detail)) = next_state
        && (current.state != state_value || (detail.is_some() && current.detail != detail))
    {
        events.push(Event::new(
            now,
            EventKind::StateChanged {
                agent,
                state: state_value,
                detail,
            },
        ));
    }
    state.commit(events)?;
    Ok(reply)
}

/// Links the session announced by a `session-start` hook to its agent and
/// records the model, unless they are already known.
fn session_start_events(current: &Agent, payload: &Value, now: Timestamp) -> Vec<Event> {
    let field = |name: &str| payload.get(name).and_then(Value::as_str);
    let mut events = Vec::new();
    if let Some(session_id) = field("session_id") {
        let key = SessionKey::claude(session_id);
        if !current.owns_session(&key) {
            let origin = match field("source") {
                Some("resume" | "compact") => SessionOrigin::Resumed,
                Some("fork") => SessionOrigin::Forked,
                _ => SessionOrigin::Started,
            };
            events.push(Event::new(
                now,
                EventKind::SessionLinked {
                    agent: current.id,
                    session: key,
                    origin,
                },
            ));
        }
    }
    if let Some(model) = field("model")
        && current.model.as_deref() != Some(model)
    {
        events.push(Event::new(
            now,
            EventKind::ModelChanged {
                agent: current.id,
                model: model.to_owned(),
            },
        ));
    }
    events
}

/// The first line of `text`, cut to `max` characters.
fn summarize(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() <= max {
        line.to_owned()
    } else {
        let cut: String = line.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}\u{2026}")
    }
}

async fn hook_wait(shared: &Arc<Shared>, params: WaitParams) -> Result<WaitReply, DaemonError> {
    let (token, receiver) = {
        let mut state = shared.state.lock().await;
        require_agent(&state, params.agent)?;
        if let Some(text) = state.take_pending(params.agent)? {
            return Ok(WaitReply { text: Some(text) });
        }
        state.register_waiter(params.agent)
    };
    let timeout = Duration::from_millis(params.timeout_ms).min(MAX_WAIT);
    match tokio::time::timeout(timeout, receiver).await {
        Ok(Ok(text)) => Ok(WaitReply { text: Some(text) }),
        // Another wait replaced this one.
        Ok(Err(_)) => Ok(WaitReply { text: None }),
        Err(_) => {
            shared.state.lock().await.cancel_waiter(params.agent, token);
            Ok(WaitReply { text: None })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_keep_the_first_non_empty_line() {
        assert_eq!(
            summarize("\n\n  Done: 3 files.\nMore", 80),
            "Done: 3 files."
        );
        assert_eq!(summarize("abcdef", 4), "abc\u{2026}");
        assert_eq!(summarize("", 10), "");
    }

    #[test]
    fn null_params_mean_no_params() {
        assert!(parse::<Shutdown>(Value::Null).is_ok());
        assert!(parse::<EventsSince>(Value::Null).is_ok());
        let error = parse::<Forget>(Value::Null).unwrap_err();
        assert_eq!(error.code, rpc::INVALID_PARAMS);
    }
}
