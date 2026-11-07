//! The mission supervisor's rules (ADR 0007).
//!
//! The supervisor is the program half of a mission: it watches workers,
//! checks delivery evidence when they stop, tells the coordinator agent what
//! happened, and asks for the final report once every worker delivered. It
//! never waits in a loop and never costs tokens: the daemon calls
//! [`supervise`] after events that concern a mission, and applies the
//! returned [`Decision`].
//!
//! Every verdict is recorded as a `worker-assessed` event, so the supervisor
//! never repeats a notice and its reasoning stays auditable.

use crate::agent::Agent;
use crate::delivery::{PrState, PushState};
use crate::event::EventKind;
use crate::ids::MissionId;
use crate::message::{MessageKind, Party};
use crate::mission::{MissionKind, MissionStatus, Verdict};
use crate::registry::Registry;
use crate::role::Role;
use crate::state::AgentState;

/// A message the supervisor asks the daemon to send, from volee.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// The recipients.
    pub to: Vec<Party>,
    /// What the message is for.
    pub kind: MessageKind,
    /// Markdown text.
    pub body: String,
}

/// What the daemon must do for a mission.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decision {
    /// Events to commit, in order.
    pub events: Vec<EventKind>,
    /// Messages to send from volee, in the mission's thread.
    pub notices: Vec<Notice>,
}

impl Decision {
    /// Whether there is nothing to do.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty() && self.notices.is_empty()
    }
}

/// Decides what to do for `mission` given the current registry. Calling it
/// again on the registry updated with its own decision returns nothing.
pub fn supervise(registry: &Registry, mission: MissionId) -> Decision {
    let mut decision = Decision::default();
    let Some(record) = registry.mission(mission) else {
        return decision;
    };
    if record.status.is_over() {
        return decision;
    }
    let workers: Vec<&Agent> = registry
        .mission_agents(mission)
        .into_iter()
        .filter(|agent| agent.role == Role::Worker)
        .collect();
    let coordinator = record
        .coordinator
        .filter(|id| registry.agent(*id).is_some())
        .map(Party::Agent);

    if record.status == MissionStatus::Planning && !workers.is_empty() {
        decision
            .events
            .push(status_update(mission, MissionStatus::Running));
    }

    let mut all_delivered = !workers.is_empty();
    for worker in &workers {
        let current = verdict_for(registry, record.kind, worker);
        let delivered = match &current {
            Some((verdict, _)) => *verdict == Verdict::Delivered,
            // Working again after delivering (to answer a question, say):
            // the delivery stands until a new verdict says otherwise.
            None => record
                .assessments
                .get(&worker.id)
                .is_some_and(|assessment| assessment.verdict == Verdict::Delivered),
        };
        all_delivered &= delivered;
        let Some((verdict, evidence)) = current else {
            continue;
        };
        let previous = record.assessments.get(&worker.id);
        if previous.is_some_and(|p| p.verdict == verdict && p.evidence == evidence) {
            continue;
        }
        decision.events.push(EventKind::WorkerAssessed {
            mission,
            worker: worker.id,
            verdict,
            evidence: evidence.clone(),
        });
        let mut to: Vec<Party> = coordinator.into_iter().collect();
        if matches!(verdict, Verdict::Failed | Verdict::Lost) {
            to.push(Party::Human);
        }
        if !to.is_empty() {
            decision.notices.push(Notice {
                to,
                kind: if delivered {
                    MessageKind::Report
                } else {
                    MessageKind::Alert
                },
                body: worker_notice(worker, verdict, &evidence),
            });
        }
    }

    let reporting = record.status == MissionStatus::Reporting;
    if all_delivered && !reporting {
        decision
            .events
            .push(status_update(mission, MissionStatus::Reporting));
        decision.notices.push(Notice {
            to: coordinator.into_iter().chain([Party::Human]).collect(),
            kind: MessageKind::Note,
            body: format!(
                "All {} workers of \"{}\" delivered. Coordinator: write the final report with \
                 the `report` tool (what changed, links to every pull request or report, what \
                 the human must review), then call `close_mission`.",
                workers.len(),
                record.title
            ),
        });
    } else if !all_delivered && reporting {
        decision
            .events
            .push(status_update(mission, MissionStatus::Running));
    }
    decision
}

/// The verdict on a worker and its evidence, or `None` while it is still
/// working.
///
/// A change worker that stopped is judged on its delivery evidence, and only
/// on evidence checked after it stopped: evidence checked before (while it
/// was still working) may miss its last commits, push or pull request, and
/// would tell the coordinator "not delivered" just before "delivered". The
/// verdict waits until the daemon's next check. Evidence that was never
/// checked (no git source configured) is judged as it is.
pub fn verdict_for(
    registry: &Registry,
    kind: MissionKind,
    worker: &Agent,
) -> Option<(Verdict, String)> {
    let finished = matches!(
        worker.state,
        AgentState::Idle | AgentState::Done | AgentState::Stopped
    );
    if finished && kind == MissionKind::Change && worker.delivery.is_stale_at(worker.state_since) {
        return None;
    }
    match &worker.state {
        AgentState::Launching | AgentState::Working => None,
        AgentState::NeedsInput { .. } => Some((
            Verdict::NeedsHuman,
            worker
                .detail
                .clone()
                .unwrap_or_else(|| "waits for a human answer".to_owned()),
        )),
        AgentState::Failed => Some((
            Verdict::Failed,
            worker
                .detail
                .clone()
                .unwrap_or_else(|| "ended with an error".to_owned()),
        )),
        AgentState::Lost => Some((
            Verdict::Lost,
            worker
                .detail
                .clone()
                .unwrap_or_else(|| "volee cannot find this agent any more".to_owned()),
        )),
        AgentState::Idle | AgentState::Done | AgentState::Stopped => {
            Some(delivery_verdict(registry, kind, worker))
        }
    }
}

fn delivery_verdict(registry: &Registry, kind: MissionKind, worker: &Agent) -> (Verdict, String) {
    let delivery = &worker.delivery;
    let merged = delivery.merged_into_base
        || delivery
            .pull_request
            .as_ref()
            .is_some_and(|pr| pr.state == PrState::Merged);
    if merged {
        return (Verdict::Delivered, delivery.summary());
    }
    match kind {
        MissionKind::Analysis => {
            let reported = registry
                .messages_from(Party::Agent(worker.id))
                .iter()
                .any(|message| message.kind == MessageKind::Report);
            if reported {
                (Verdict::Delivered, "report sent".to_owned())
            } else {
                (Verdict::NotDelivered, "no report sent yet".to_owned())
            }
        }
        MissionKind::Change => {
            let mut missing = Vec::new();
            if delivery.commits == 0 {
                missing.push("no commits");
            } else if delivery.push != PushState::UpToDate {
                missing.push("commits not pushed");
            }
            let open_pr = delivery
                .pull_request
                .as_ref()
                .is_some_and(|pr| matches!(pr.state, PrState::Open | PrState::Draft));
            if !open_pr {
                missing.push("no open pull request");
            }
            if delivery.dirty {
                missing.push("uncommitted changes");
            }
            if missing.is_empty() {
                (Verdict::Delivered, delivery.summary())
            } else {
                (Verdict::NotDelivered, missing.join(", "))
            }
        }
    }
}

fn worker_notice(worker: &Agent, verdict: Verdict, evidence: &str) -> String {
    let who = format!("Worker {} ({})", worker.name, worker.workspace.label());
    match verdict {
        Verdict::Delivered => format!("{who} delivered: {evidence}."),
        Verdict::NotDelivered => format!(
            "{who} stopped without delivering: {evidence}. Ask it to finish (send_message), or \
             tell the human why it cannot."
        ),
        Verdict::NeedsHuman => format!("{who} waits for the human: {evidence}."),
        Verdict::Failed => format!("{who} failed: {evidence}. Decide whether to retry or abandon."),
        Verdict::Lost => {
            format!("{who} is lost: {evidence}. Its branch is kept; decide whether to relaunch it.")
        }
    }
}

fn status_update(mission: MissionId, status: MissionStatus) -> EventKind {
    EventKind::MissionUpdated {
        mission,
        status,
        coordinator: None,
        report: None,
    }
}

#[cfg(test)]
mod tests;
