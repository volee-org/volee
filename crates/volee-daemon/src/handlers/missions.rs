//! `missions.create`, `missions.abort` and `missions.close` (ADR 0007).
//!
//! The supervisor runs after each commit (see `crate::missions`); these
//! handlers only change a mission's lifecycle on request.

use std::collections::BTreeMap;
use std::sync::Arc;

use volee_core::message::{Message, MessageKind, Party, Receipt, Thread};
use volee_core::mission::{Mission, MissionStatus, Verdict};
use volee_core::role::Role;
use volee_core::{Event, EventKind, MessageId, MissionId, Registry, ThreadId};
use volee_proto::methods::{AbortParams, CloseParams, CreateMissionParams, CreatedMission, Empty};

use super::require_agent;
use crate::error::DaemonError;
use crate::server::Shared;
use crate::state::State;

pub(super) async fn create(
    shared: &Arc<Shared>,
    params: CreateMissionParams,
) -> Result<CreatedMission, DaemonError> {
    let title = params.title.trim();
    if title.is_empty() || params.goal.trim().is_empty() {
        return Err(DaemonError::InvalidParams(
            "a mission needs a title and a goal".into(),
        ));
    }
    if params.repos.is_empty() {
        return Err(DaemonError::InvalidParams(
            "a mission needs at least one repository".into(),
        ));
    }
    if let Some(agent) = params.requested_by {
        require_agent(&*shared.state.lock().await, agent)?;
    }
    // The coordinator is a Claude Code session: without Claude Code, refuse
    // before anything is recorded, rather than leave a mission to abort.
    if shared.harness.is_none() {
        return Err(no_harness(shared));
    }
    let (mission, thread) = open(shared, &params, title).await?;
    let coordinator = match crate::launch::coordinator(shared, mission).await {
        Ok(agent) => Some(agent),
        Err(error) => {
            tracing::warn!(%error, %mission, "cannot launch the coordinator");
            alert_human(
                shared,
                thread,
                format!(
                    "volee could not launch the coordinator of this mission: {error}. The \
                     mission stays in planning: abort it, or fix the problem and create it \
                     again."
                ),
            )
            .await?;
            None
        }
    };
    Ok(CreatedMission {
        mission,
        thread,
        coordinator,
    })
}

/// Why a mission cannot start on this daemon: it found no Claude Code it
/// can use, missing or too old, and says why in `volee daemon status`.
fn no_harness(shared: &Shared) -> DaemonError {
    let why = shared
        .harnesses
        .iter()
        .find_map(|status| status.detail.clone())
        .map_or_else(String::new, |detail| format!(" ({detail})"));
    DaemonError::Unavailable(format!(
        "a mission needs Claude Code to run its coordinator, and this daemon found none it can \
         use{why}; `volee daemon status` says why. Once Claude Code is installed or updated, \
         restart the daemon with `volee daemon stop`: the next command starts it again"
    ))
}

/// Records the mission and its thread.
async fn open(
    shared: &Arc<Shared>,
    params: &CreateMissionParams,
    title: &str,
) -> Result<(MissionId, ThreadId), DaemonError> {
    let mut state = shared.state.lock().await;
    if let Some(agent) = params.requested_by {
        require_agent(&state, agent)?;
    }
    let now = state.now();
    let mission = state.new_mission_id();
    let thread = state.new_thread_id();
    state.commit(vec![
        Event::new(
            now,
            EventKind::ThreadOpened {
                thread: Thread {
                    id: thread,
                    title: format!("Mission: {title}"),
                    mission: Some(mission),
                    participants: vec![Party::Human],
                    opened_at: now,
                },
            },
        ),
        Event::new(
            now,
            EventKind::MissionCreated {
                mission: Mission {
                    id: mission,
                    title: title.to_owned(),
                    kind: params.kind,
                    goal: params.goal.clone(),
                    repos: params.repos.clone(),
                    requested_by: params.requested_by,
                    coordinator: None,
                    thread,
                    status: MissionStatus::Planning,
                    report: None,
                    created_at: now,
                    ended_at: None,
                    assessments: BTreeMap::new(),
                },
            },
        ),
    ])?;
    Ok((mission, thread))
}

/// Tells the human something went wrong, in the mission's thread.
async fn alert_human(
    shared: &Arc<Shared>,
    thread: ThreadId,
    body: String,
) -> Result<(), DaemonError> {
    let mut state = shared.state.lock().await;
    let now = state.now();
    let message = Message {
        id: state.new_message_id(),
        thread,
        from: Party::System,
        kind: MessageKind::Alert,
        body,
        refs: vec![],
        in_reply_to: None,
        at: now,
        receipts: vec![Receipt {
            to: Party::Human,
            delivered_at: None,
            read_at: None,
        }],
    };
    state.commit(vec![Event::new(now, EventKind::MessagePosted { message })])?;
    Ok(())
}

pub(super) async fn abort(shared: &Arc<Shared>, params: AbortParams) -> Result<Empty, DaemonError> {
    let mut state = shared.state.lock().await;
    let record = open_mission(&state, params.mission)?;
    // Only live agents hear it: a stopped agent has nothing to stop, and a
    // message waiting for it must not look like a reason to resume it.
    let mut to = vec![Party::Human];
    to.extend(
        state
            .registry()
            .mission_agents(params.mission)
            .into_iter()
            .filter(|agent| agent.state.is_live())
            .map(|agent| Party::Agent(agent.id)),
    );
    let reason = params
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map_or_else(String::new, |reason| format!(": {reason}"));
    let body = format!(
        "Mission \"{}\" was aborted by the human{reason}. Stop working on it and leave your work \
         committed: volee deletes nothing.",
        record.title
    );
    let thread = record.thread;
    let now = state.now();
    let notice = Message {
        id: state.new_message_id(),
        thread,
        from: Party::System,
        kind: MessageKind::Alert,
        body,
        refs: vec![],
        in_reply_to: None,
        at: now,
        receipts: to
            .into_iter()
            .map(|to| Receipt {
                to,
                delivered_at: None,
                read_at: None,
            })
            .collect(),
    };
    state.commit(vec![
        Event::new(
            now,
            EventKind::MissionUpdated {
                mission: params.mission,
                status: MissionStatus::Aborted,
                coordinator: None,
                report: None,
            },
        ),
        Event::new(now, EventKind::MessagePosted { message: notice }),
    ])?;
    Ok(Empty {})
}

pub(super) async fn close(shared: &Arc<Shared>, params: CloseParams) -> Result<Empty, DaemonError> {
    let mut state = shared.state.lock().await;
    let record = open_mission(&state, params.mission)?;
    let registry = state.registry();
    let report = match params.report {
        Some(report) => Some(report_in_thread(registry, record, report)?),
        None => latest_report(registry, record),
    };
    if let Some(agent) = params.by {
        if record.coordinator != Some(agent) {
            return Err(DaemonError::Refused(format!(
                "only the coordinator of mission \"{}\" can close it",
                record.title
            )));
        }
        if params.force {
            return Err(DaemonError::Refused(
                "only the human can force a mission closed".into(),
            ));
        }
        if report.is_none() {
            return Err(DaemonError::Refused(
                "send the final report to the human with the `report` tool first: what changed, \
                 links to every pull request or report, and what the human must review"
                    .into(),
            ));
        }
    }
    if !params.force && record.status != MissionStatus::Reporting {
        let advice = if params.by.is_some() {
            " Ask them to finish, or tell the human why they cannot."
        } else {
            ""
        };
        return Err(DaemonError::Refused(format!(
            "mission \"{}\" cannot close yet: {}.{advice}",
            record.title,
            undelivered(registry, record)
        )));
    }
    let now = state.now();
    state.commit(vec![Event::new(
        now,
        EventKind::MissionUpdated {
            mission: params.mission,
            status: MissionStatus::Closed,
            coordinator: None,
            report,
        },
    )])?;
    Ok(Empty {})
}

/// A mission that is neither closed nor aborted.
fn open_mission(state: &State, mission: MissionId) -> Result<&Mission, DaemonError> {
    let record = state
        .registry()
        .mission(mission)
        .ok_or_else(|| DaemonError::NotFound(format!("unknown mission {mission}")))?;
    if record.status.is_over() {
        return Err(DaemonError::Refused(format!(
            "mission \"{}\" is already {}",
            record.title,
            record.status.label()
        )));
    }
    Ok(record)
}

fn report_in_thread(
    registry: &Registry,
    mission: &Mission,
    report: MessageId,
) -> Result<MessageId, DaemonError> {
    let message = registry
        .message(report)
        .ok_or_else(|| DaemonError::NotFound(format!("unknown message {report}")))?;
    if message.thread == mission.thread {
        Ok(report)
    } else {
        Err(DaemonError::Refused(format!(
            "message {report} is not in the thread of mission \"{}\"",
            mission.title
        )))
    }
}

/// The coordinator's latest report in the mission's thread.
fn latest_report(registry: &Registry, mission: &Mission) -> Option<MessageId> {
    let coordinator = Party::Agent(mission.coordinator?);
    registry
        .thread_messages(mission.thread)
        .into_iter()
        .filter(|message| message.from == coordinator && message.kind == MessageKind::Report)
        .max_by_key(|message| (message.at, message.id))
        .map(|message| message.id)
}

/// Why a mission is not ready to close, for a refusal.
fn undelivered(registry: &Registry, mission: &Mission) -> String {
    let waiting: Vec<&str> = registry
        .mission_agents(mission.id)
        .into_iter()
        .filter(|agent| agent.role == Role::Worker)
        .filter(|agent| {
            mission
                .assessments
                .get(&agent.id)
                .is_none_or(|assessment| assessment.verdict != Verdict::Delivered)
        })
        .map(|agent| agent.name.as_str())
        .collect();
    if waiting.is_empty() {
        "it has no workers".to_owned()
    } else {
        format!("these workers have not delivered: {}", waiting.join(", "))
    }
}
