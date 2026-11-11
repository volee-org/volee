//! Missions: goals across repositories (ADR 0007).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, MessageId, MissionId, ThreadId};
use crate::time::Timestamp;

/// What a mission delivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MissionKind {
    /// Branches and pull requests.
    Change,
    /// Reports.
    Analysis,
}

/// The answer that keeps a mission open, in the question its coordinator
/// asks the person.
pub const DECLINE: &str = "no";

impl MissionKind {
    /// The label shown to people.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Change => "change",
            Self::Analysis => "analysis",
        }
    }

    /// The answer by which the person approves a mission of this kind: a
    /// change mission is merged, an analysis mission, which has nothing to
    /// merge, is closed.
    pub const fn approval(self) -> &'static str {
        match self {
            Self::Change => "ok merge",
            Self::Analysis => "ok close",
        }
    }

    /// The answers the coordinator's question offers the person, the
    /// approving one first.
    pub fn approval_options(self) -> Vec<String> {
        vec![self.approval().to_owned(), DECLINE.to_owned()]
    }
}

/// Where a mission stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MissionStatus {
    /// The coordinator is splitting the goal.
    Planning,
    /// Workers are running.
    Running,
    /// Every worker delivered; the coordinator checks the work and writes
    /// its report.
    Reporting,
    /// The coordinator presented its report and asked the person whether
    /// to merge (to close, for an analysis mission). Nothing is merged or
    /// closed until they answer.
    AwaitingApproval,
    /// The person approved: the coordinator merges, then closes.
    Approved,
    /// Closed with a report.
    Closed,
    /// Stopped by a person. Nothing was deleted.
    Aborted,
}

impl MissionStatus {
    /// Whether the mission is over.
    pub const fn is_over(self) -> bool {
        matches!(self, Self::Closed | Self::Aborted)
    }

    /// Whether every worker delivered and the mission is on its way to its
    /// end: the report, the person's answer, the merge.
    pub const fn is_ready(self) -> bool {
        matches!(
            self,
            Self::Reporting | Self::AwaitingApproval | Self::Approved
        )
    }

    /// The label shown to people.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Planning => "planning",
            Self::Running => "running",
            Self::Reporting => "reporting",
            Self::AwaitingApproval => "waiting for you",
            Self::Approved => "approved",
            Self::Closed => "closed",
            Self::Aborted => "aborted",
        }
    }
}

/// The supervisor's judgement on a worker that stopped working.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// The work landed: pushed with an open pull request (committed, in a
    /// repository without a remote), merged, or reported.
    Delivered,
    /// The worker stopped, but the evidence is missing.
    NotDelivered,
    /// The worker waits for a human.
    NeedsHuman,
    /// The worker failed.
    Failed,
    /// The worker can no longer be found.
    Lost,
}

impl Verdict {
    /// The label shown to people.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::NotDelivered => "not delivered",
            Self::NeedsHuman => "needs you",
            Self::Failed => "failed",
            Self::Lost => "lost",
        }
    }
}

/// The last verdict recorded for a worker, with the evidence it rests on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assessment {
    /// The verdict.
    pub verdict: Verdict,
    /// What was observed, in one line, such as `3 commits · pushed · PR #42 open`.
    pub evidence: String,
    /// When it was recorded.
    pub at: Timestamp,
}

/// A goal that spans one or more repositories.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mission {
    /// Its identifier.
    pub id: MissionId,
    /// A short title.
    pub title: String,
    /// What it delivers.
    pub kind: MissionKind,
    /// The goal, as given.
    pub goal: String,
    /// The repositories involved, by registry name.
    pub repos: Vec<String>,
    /// The agent that asked for it, if an agent did.
    pub requested_by: Option<AgentId>,
    /// Its coordinator agent, once launched.
    pub coordinator: Option<AgentId>,
    /// Its conversation thread.
    pub thread: ThreadId,
    /// Where it stands.
    pub status: MissionStatus,
    /// The report its coordinator presented to the person, which is the
    /// question that asks for their approval; for a mission the person
    /// closed themselves, the coordinator's last report.
    pub report: Option<MessageId>,
    /// When it was created.
    pub created_at: Timestamp,
    /// When it closed or was aborted.
    pub ended_at: Option<Timestamp>,
    /// The supervisor's last verdict on each worker.
    #[serde(default)]
    pub assessments: BTreeMap<AgentId, Assessment>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_closed_and_aborted_are_over() {
        assert!(MissionStatus::Closed.is_over());
        assert!(MissionStatus::Aborted.is_over());
        assert!(!MissionStatus::Running.is_over());
        assert!(!MissionStatus::Reporting.is_over());
        assert!(!MissionStatus::AwaitingApproval.is_over());
        assert!(!MissionStatus::Approved.is_over());
    }

    #[test]
    fn a_mission_is_ready_from_its_report_to_its_merge() {
        for status in [
            MissionStatus::Reporting,
            MissionStatus::AwaitingApproval,
            MissionStatus::Approved,
        ] {
            assert!(status.is_ready(), "{status:?}");
        }
        for status in [
            MissionStatus::Planning,
            MissionStatus::Running,
            MissionStatus::Closed,
            MissionStatus::Aborted,
        ] {
            assert!(!status.is_ready(), "{status:?}");
        }
        assert_eq!(MissionStatus::AwaitingApproval.label(), "waiting for you");
    }

    #[test]
    fn the_approval_question_offers_the_approving_answer_first() {
        assert_eq!(MissionKind::Change.approval_options(), ["ok merge", "no"]);
        assert_eq!(MissionKind::Analysis.approval_options(), ["ok close", "no"]);
        // What the question offers passes the rules of an offer.
        for kind in [MissionKind::Change, MissionKind::Analysis] {
            assert_eq!(
                crate::message::check_options(&kind.approval_options()),
                Ok(())
            );
        }
    }
}
