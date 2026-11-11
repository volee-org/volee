//! Roles of agents and who launched them.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::AgentId;

/// What an agent does in the organization of the work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    /// The agent the human talks to; it launches missions and workers.
    Lead,
    /// The agent that runs a mission: splits it, briefs workers, reports.
    Coordinator,
    /// An agent working in one repository, in its own worktree.
    Worker,
    /// A helper started by another agent inside its own session.
    Subagent,
    /// An agent with no place in the organization of the work: a session
    /// volee did not launch, or a program a person runs under volee
    /// (`volee run`).
    Independent,
}

impl Role {
    /// Every role, in display order.
    pub const ALL: [Self; 5] = [
        Self::Lead,
        Self::Coordinator,
        Self::Worker,
        Self::Subagent,
        Self::Independent,
    ];

    /// The label shown to people.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Lead => "lead",
            Self::Coordinator => "coordinator",
            Self::Worker => "worker",
            Self::Subagent => "subagent",
            Self::Independent => "independent",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Who started an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "by", content = "agent")]
pub enum LaunchedBy {
    /// The human, through volee.
    Human,
    /// Another agent, through volee.
    Agent(AgentId),
    /// Someone outside volee; the session was discovered.
    Outside,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_lowercase_words() {
        for role in Role::ALL {
            let label = role.label();
            assert_eq!(label, label.to_lowercase());
            assert_eq!(role.to_string(), label);
        }
    }

    #[test]
    fn launched_by_serializes_with_a_tag() {
        let json = serde_json::to_string(&LaunchedBy::Human).unwrap();
        assert_eq!(json, r#"{"by":"human"}"#);
    }
}
