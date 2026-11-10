//! Request handlers: one function per method.

mod missions;
mod threads;

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use volee_core::harness::{SessionKey, SessionOrigin};
use volee_core::ids::ThreadId;
use volee_core::message::{Message, Party, Receipt, Thread};
use volee_core::state::AgentState;
use volee_core::time::Timestamp;
use volee_core::{Agent, AgentId, Event, EventKind, Registry};
use volee_harness::hooks::{HookPayload, HookSignal, SessionSource, interpret, stop_block_output};
use volee_proto::PROTOCOL_VERSION;
use volee_proto::methods::{
    AbortMission, Adopt, AgentParams, Attach, CloseMission, CreateMission, Diff, Empty, EventBatch,
    EventsSince, Forget, HookIngest, HookParams, HookReply, HookWait, MarkRead, MarkReadParams,
    Method, Preview, RecordActivity, RefreshDiscovery, SendMessage, SendParams, Sent, Shutdown,
    SinceParams, Spawn, Status, StatusInfo, StatusParams, Stop, Summarize, Tidy, WaitParams,
    WaitReply,
};
use volee_proto::rpc::{self, RpcError};

use crate::control;
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
        RefreshDiscovery::NAME => {
            respond(crate::discovery::refresh(shared, parse::<RefreshDiscovery>(params)?).await)
        }
        Forget::NAME => respond(forget(shared, parse::<Forget>(params)?).await),
        SendMessage::NAME => respond(send_message(shared, parse::<SendMessage>(params)?).await),
        MarkRead::NAME => respond(mark_read(shared, parse::<MarkRead>(params)?).await),
        HookIngest::NAME => {
            respond(hook_ingest(shared, parse::<HookIngest>(params)?, Delivery::Live).await)
        }
        HookWait::NAME => respond(hook_wait(shared, parse::<HookWait>(params)?).await),
        Spawn::NAME => respond(crate::launch::spawn(shared, parse::<Spawn>(params)?).await),
        Preview::NAME => respond(control::preview(shared, parse::<Preview>(params)?).await),
        Diff::NAME => respond(control::diff(shared, parse::<Diff>(params)?).await),
        Attach::NAME => respond(control::attach(shared, parse::<Attach>(params)?).await),
        Stop::NAME => respond(control::stop(shared, parse::<Stop>(params)?).await),
        Adopt::NAME => {
            let params = parse::<Adopt>(params)?;
            respond(
                crate::resume::adopt(shared, params.agent)
                    .await
                    .map(|()| Empty {}),
            )
        }
        Tidy::NAME => respond(crate::tidy::tidy(shared, parse::<Tidy>(params)?).await),
        RecordActivity::NAME => {
            respond(crate::join::activity(shared, parse::<RecordActivity>(params)?).await)
        }
        Summarize::NAME => respond(threads::summarize(shared, parse::<Summarize>(params)?).await),
        CreateMission::NAME => {
            respond(missions::create(shared, parse::<CreateMission>(params)?).await)
        }
        AbortMission::NAME => {
            respond(missions::abort(shared, parse::<AbortMission>(params)?).await)
        }
        CloseMission::NAME => {
            respond(missions::close(shared, parse::<CloseMission>(params)?).await)
        }
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

pub(crate) fn respond<T: serde::Serialize>(
    result: Result<T, DaemonError>,
) -> Result<Value, RpcError> {
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
        forge: shared.forge_status(),
        multiplexers: shared.panes.statuses(),
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

/// `agents.forget`: volee stops listing the agent and ignores its sessions
/// from then on; nothing else changes. Refused for an agent volee launched
/// that still runs, and for an agent of an open mission: forgetting them
/// would leave a session or a mission that nobody sees.
async fn forget(shared: &Arc<Shared>, params: AgentParams) -> Result<Empty, DaemonError> {
    let mut state = shared.state.lock().await;
    let registry = state.registry();
    if registry.agent(params.agent).is_none() {
        return Err(DaemonError::NotFound(format!(
            "unknown agent {}",
            params.agent
        )));
    }
    if let Some(refusal) = registry.forget_refusal(params.agent) {
        return Err(DaemonError::Refused(refusal));
    }
    let now = state.now();
    state.commit(vec![Event::new(
        now,
        EventKind::AgentForgotten {
            agent: params.agent,
        },
    )])?;
    Ok(Empty {})
}

pub(super) fn require_agent(state: &State, agent: AgentId) -> Result<(), DaemonError> {
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

/// Turns a hook payload into events, with the Claude Code adapter's reading
/// of what each hook means.
async fn hook_ingest(
    shared: &Arc<Shared>,
    params: HookParams,
    delivery: Delivery,
) -> Result<HookReply, DaemonError> {
    // A replayed hook's process is gone: it is the agent's.
    let foreign = delivery == Delivery::Live
        && crate::lineage::foreign(shared, params.agent, params.pid).await;
    let payload = hook_payload(&params)?;
    let signal = interpret(&payload);
    let mut state = shared.state.lock().await;
    let agent = params.agent;
    let current = state
        .registry()
        .agent(agent)
        .ok_or_else(|| DaemonError::NotFound(format!("unknown agent {agent}")))?
        .clone();
    // A hook from outside the agent's volee host comes from a background
    // session that inherited its settings (ADR 0026). One that starts a
    // session continues the agent's conversation elsewhere: Claude Code's
    // agents view moved it to the background, and the host now shows it.
    // Any other counts only for the agent's current session.
    if foreign
        && !matches!(signal, HookSignal::SessionStarted { .. })
        && !crate::lineage::is_current(&current, &payload.session_id)
    {
        tracing::info!(%agent, session = %payload.session_id, "a hook of another session than the agent's, ignored");
        return Ok(HookReply::default());
    }
    let now = state.now();
    let mut events = Vec::new();
    let mut reply = HookReply::default();
    let mut next_state = signal.implied_state().map(|state| (state, None));

    match signal {
        HookSignal::SessionStarted {
            session_id,
            source,
            model,
            ..
        } => {
            events.extend(session_start_events(
                &current,
                &session_id,
                source.as_ref(),
                model.as_deref(),
                now,
            ));
            next_state = Some((AgentState::Idle, None));
        }
        HookSignal::ToolStarted { summary, .. } => {
            events.push(Event::new(
                now,
                EventKind::ActivityRecorded { agent, summary },
            ));
        }
        HookSignal::NeedsInput { request, detail } => {
            next_state = Some((AgentState::NeedsInput { request }, detail));
        }
        HookSignal::Stopped { last_message } => {
            let pending = if delivery == Delivery::Live {
                state.take_pending(agent)?
            } else {
                None
            };
            if let Some(text) = pending {
                reply.stdout = Some(stop_block_output(&text));
                next_state = Some((AgentState::Working, None));
            } else {
                let detail = last_message.as_deref().map(|text| summarize(text, 200));
                next_state = Some((AgentState::Done, detail));
            }
        }
        HookSignal::SessionEnded { reason } => {
            next_state = next_state.map(|(state, _)| (state, reason));
        }
        HookSignal::PromptSubmitted
        | HookSignal::ToolFinished { .. }
        | HookSignal::SubagentStarted { .. }
        | HookSignal::SubagentStopped { .. }
        | HookSignal::Ignored => {}
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

/// The hook's payload as Claude Code sent it. The event name and the session
/// id come from the command line when the payload lacks them, as hand-written
/// or replayed payloads may.
fn hook_payload(params: &HookParams) -> Result<HookPayload, DaemonError> {
    let mut payload = params.payload.clone();
    if !payload.is_object() {
        payload = Value::Object(serde_json::Map::new());
    }
    if let Value::Object(fields) = &mut payload {
        fields
            .entry("hook_event_name")
            .or_insert_with(|| Value::String(pascal_case(&params.event)));
        fields
            .entry("session_id")
            .or_insert_with(|| Value::String(String::new()));
    }
    serde_json::from_value(payload).map_err(|error| {
        DaemonError::InvalidParams(format!("not a Claude Code hook payload: {error}"))
    })
}

/// `pre-tool-use` as Claude Code names it: `PreToolUse`.
fn pascal_case(event: &str) -> String {
    event
        .split('-')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect()
}

/// Links the session announced by a `session-start` hook to its agent and
/// records the model, unless they are already known.
fn session_start_events(
    current: &Agent,
    session_id: &str,
    source: Option<&SessionSource>,
    model: Option<&str>,
    now: Timestamp,
) -> Vec<Event> {
    let mut events = Vec::new();
    if !session_id.is_empty() {
        let key = SessionKey::claude(session_id);
        if !current.owns_session(&key) {
            let origin = match source {
                Some(SessionSource::Resume | SessionSource::Compact) => SessionOrigin::Resumed,
                Some(SessionSource::Fork) => SessionOrigin::Forked,
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
    if let Some(model) = model
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

/// A party as thread titles name it.
fn sender_label(registry: &Registry, party: Party) -> String {
    match party {
        Party::Human => "the human".to_owned(),
        Party::System => "volee".to_owned(),
        Party::Agent(id) => registry.agent(id).map_or_else(
            || id.to_string(),
            |agent| format!("{} ({})", agent.name, agent.role),
        ),
    }
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
    // A session that inherited the agent's hooks takes its messages only
    // when it carries the agent's conversation.
    if crate::lineage::foreign(shared, params.agent, params.pid).await {
        let state = shared.state.lock().await;
        let current = state.registry().agent(params.agent).is_some_and(|agent| {
            params
                .session
                .as_deref()
                .is_some_and(|session| crate::lineage::is_current(agent, session))
        });
        if !current {
            return Ok(WaitReply { text: None });
        }
    }
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
