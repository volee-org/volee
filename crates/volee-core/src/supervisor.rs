//! The mission supervisor's rules (ADR 0007).
//!
//! The supervisor is the program half of a mission: it watches workers,
//! checks delivery evidence when they stop, tells the coordinator agent what
//! happened, asks it to present its report to the person once every worker
//! delivered, and reads the person's answer: only their approval lets the
//! coordinator merge (ADR 0032). It never waits in a loop and never costs
//! tokens: the daemon calls [`supervise`] after events that concern a
//! mission, and applies the returned [`Decision`].
//!
//! Every verdict is recorded as a `worker-assessed` event, so the supervisor
//! never repeats a notice and its reasoning stays auditable.

use crate::agent::Agent;
use crate::delivery::{PrState, PushState};
use crate::event::EventKind;
use crate::ids::MissionId;
use crate::message::{MessageKind, Party};
use crate::mission::{Mission, MissionKind, MissionStatus, Verdict};
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

    conclude(
        &mut decision,
        registry,
        record,
        workers.len(),
        coordinator,
        all_delivered,
    );
    decision
}

/// The end of a mission: once every worker delivered, the coordinator is
/// asked to present its report; the person's answer to that report approves
/// the mission or keeps it open; and work that changes meanwhile takes the
/// mission back to running.
fn conclude(
    decision: &mut Decision,
    registry: &Registry,
    record: &Mission,
    workers: usize,
    coordinator: Option<Party>,
    all_delivered: bool,
) {
    let mission = record.id;
    let ready = record.status.is_ready();
    if all_delivered && !ready {
        decision
            .events
            .push(status_update(mission, MissionStatus::Reporting));
        decision.notices.push(Notice {
            to: coordinator.into_iter().chain([Party::Human]).collect(),
            kind: MessageKind::Note,
            body: format!(
                "All {} workers of \"{}\" delivered. Coordinator: check their work, then \
                 present your report to the human with `request_approval` (what is done, the \
                 proof, what is left). Merge nothing and close nothing before the human \
                 answers.",
                workers, record.title
            ),
        });
    } else if !all_delivered && ready {
        decision
            .events
            .push(status_update(mission, MissionStatus::Running));
        // The report the person was asked about no longer describes the
        // work: say so, to them and to the coordinator.
        if record.status != MissionStatus::Reporting {
            decision.notices.push(Notice {
                to: coordinator.into_iter().chain([Party::Human]).collect(),
                kind: MessageKind::Alert,
                body: format!(
                    "The work of \"{}\" changed after its report was presented: that report \
                     no longer stands, and an answer to it approves nothing. Coordinator: \
                     present a new report with `request_approval` once every worker delivered \
                     again.",
                    record.title
                ),
            });
        }
    } else if all_delivered
        && record.status == MissionStatus::AwaitingApproval
        && let Some(answer) = answer_to_report(registry, record)
    {
        answered(decision, record, coordinator, &answer);
    }
}

/// What the person answered to the report the coordinator presented: whether
/// it is the approving answer, and their words.
struct Answer {
    approves: bool,
    words: String,
}

/// The person's answer to the report a mission's coordinator presented, if
/// they answered. Only the answer the question offers for it approves: the
/// same words, whatever their case. Anything else, a sentence that contains
/// them included, keeps the mission open.
fn answer_to_report(registry: &Registry, mission: &Mission) -> Option<Answer> {
    let question = registry.message(mission.report?)?;
    let answer = registry.answer_to(question.id)?;
    Some(Answer {
        approves: question.chosen(&answer.body) == Some(mission.kind.approval()),
        words: answer.body.clone(),
    })
}

/// Records the person's answer and tells the coordinator what it allows.
fn answered(
    decision: &mut Decision,
    mission: &Mission,
    coordinator: Option<Party>,
    answer: &Answer,
) {
    let status = if answer.approves {
        MissionStatus::Approved
    } else {
        MissionStatus::Reporting
    };
    decision.events.push(status_update(mission.id, status));
    let body = match (answer.approves, mission.kind) {
        (true, MissionKind::Change) => format!(
            "The human approved \"{}\": merge each worker's work into its base branch now, \
             then call `close_mission`. A pull request: `gh pr ready <url>`, then `gh pr merge \
             <url>` with the method the repository allows. A repository without a remote: from \
             its main checkout, on the base branch with a clean tree, `git merge --no-ff \
             <branch>` (a plain merge, never a squash: volee proves a local merge by the \
             branch's commits being part of the base branch). Never force, never delete a \
             branch. If a merge fails, stop and tell the human what blocks.",
            mission.title
        ),
        (true, MissionKind::Analysis) => format!(
            "The human approved \"{}\": call `close_mission`.",
            mission.title
        ),
        (false, _) => format!(
            "The human did not approve \"{}\": they answered \"{}\". Nothing is merged and \
             the mission stays open. Do what they ask, tell your lead, and present your report \
             again with `request_approval` when it is ready.",
            mission.title,
            one_line(&answer.words)
        ),
    };
    if let Some(coordinator) = coordinator {
        decision.notices.push(Notice {
            to: vec![coordinator],
            kind: MessageKind::Note,
            body,
        });
    }
}

/// The person's words on one line, cut to a length that fits a notice.
fn one_line(words: &str) -> String {
    let line = words.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= 200 {
        line
    } else {
        let cut: String = line.chars().take(199).collect();
        format!("{cut}\u{2026}")
    }
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
    // Only a pull request whose head is the branch's tip counts: an old pull
    // request of a reused branch name says nothing about this work.
    let pull_request = delivery.pull_request_at_tip();
    let merged =
        delivery.merged_into_base || pull_request.is_some_and(|pr| pr.state == PrState::Merged);
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
            // Without a remote there is nowhere to push and no pull request
            // to open: committed work is all a worker can deliver.
            if delivery.commits == 0 {
                missing.push("no commits");
            } else if !matches!(delivery.push, PushState::UpToDate | PushState::NoRemote) {
                missing.push("commits not pushed");
            }
            let open_pr =
                pull_request.is_some_and(|pr| matches!(pr.state, PrState::Open | PrState::Draft));
            if !delivery.no_remote() && !open_pr {
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
