//! Missions: goals across repositories (ADR 0007).

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

/// Where a mission stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MissionStatus {
    /// The coordinator is splitting the goal.
    Planning,
    /// Workers are running.
    Running,
    /// Every worker delivered; the coordinator writes the report.
    Reporting,
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
    /// The final report, once written.
    pub report: Option<MessageId>,
    /// When it was created.
    pub created_at: Timestamp,
    /// When it closed or was aborted.
    pub ended_at: Option<Timestamp>,
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
    }
}
