//! Lifecycle states of agents.

use std::fmt;

use serde::{Deserialize, Serialize};

/// What an agent needs from a human.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum InputRequest {
    /// A tool call waits for permission.
    Permission {
        /// The tool asking, when known (for example `Bash`).
        tool: Option<String>,
    },
    /// The agent asked a question.
    Question,
    /// A dialog is open in the agent's session.
    Dialog,
    /// Anything else the harness reports.
    Other {
        /// The harness's own description.
        detail: String,
    },
}

/// The lifecycle state of an agent.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "state")]
pub enum AgentState {
    /// Launched by volee, not seen running yet.
    Launching,
    /// Running a turn.
    Working,
    /// Blocked until a human answers.
    NeedsInput {
        /// What the agent waits for.
        request: InputRequest,
    },
    /// Alive and waiting for its next instruction.
    Idle,
    /// Finished its last task.
    Done,
    /// Ended with an error.
    Failed,
    /// Its process was stopped; the conversation can be resumed.
    Stopped,
    /// volee cannot find it any more. Never treated as "done".
    Lost,
}

/// The coarse family of a state. Two states of the same category carry the
/// same meaning for a person; the richer one is kept when sources disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateCategory {
    /// Needs a human.
    NeedsInput,
    /// Ended with an error.
    Failed,
    /// Running.
    Working,
    /// Being launched.
    Launching,
    /// Finished.
    Done,
    /// Alive and waiting.
    Idle,
    /// Process stopped.
    Stopped,
    /// Missing.
    Lost,
}

impl AgentState {
    /// The coarse family of this state.
    pub const fn category(&self) -> StateCategory {
        match self {
            Self::Launching => StateCategory::Launching,
            Self::Working => StateCategory::Working,
            Self::NeedsInput { .. } => StateCategory::NeedsInput,
            Self::Idle => StateCategory::Idle,
            Self::Done => StateCategory::Done,
            Self::Failed => StateCategory::Failed,
            Self::Stopped => StateCategory::Stopped,
            Self::Lost => StateCategory::Lost,
        }
    }

    /// Whether the agent's process is expected to be alive.
    pub const fn is_live(&self) -> bool {
        matches!(
            self,
            Self::Launching | Self::Working | Self::NeedsInput { .. } | Self::Idle | Self::Done
        )
    }

    /// Whether the agent is busy and must not be disturbed by cleanup.
    pub const fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Launching | Self::Working | Self::NeedsInput { .. }
        )
    }

    /// The label shown to people.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Launching => "launching",
            Self::Working => "working",
            Self::NeedsInput { .. } => "needs you",
            Self::Idle => "idle",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
            Self::Lost => "lost",
        }
    }
}

impl fmt::Display for AgentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_sort_by_urgency() {
        let mut states = [
            AgentState::Idle,
            AgentState::Working,
            AgentState::Lost,
            AgentState::NeedsInput {
                request: InputRequest::Question,
            },
            AgentState::Failed,
            AgentState::Done,
        ];
        states.sort_by_key(AgentState::category);
        let labels: Vec<&str> = states.iter().map(AgentState::label).collect();
        assert_eq!(
            labels,
            ["needs you", "failed", "working", "done", "idle", "lost"]
        );
    }

    #[test]
    fn needs_input_variants_share_a_category() {
        let a = AgentState::NeedsInput {
            request: InputRequest::Permission {
                tool: Some("Bash".into()),
            },
        };
        let b = AgentState::NeedsInput {
            request: InputRequest::Permission { tool: None },
        };
        assert_eq!(a.category(), b.category());
        assert_ne!(a, b);
    }

    #[test]
    fn liveness_and_busyness() {
        assert!(AgentState::Idle.is_live());
        assert!(!AgentState::Idle.is_busy());
        assert!(AgentState::Working.is_busy());
        assert!(!AgentState::Stopped.is_live());
        assert!(!AgentState::Lost.is_live());
    }

    #[test]
    fn serde_shape_is_tagged() {
        let state = AgentState::NeedsInput {
            request: InputRequest::Permission {
                tool: Some("Bash".into()),
            },
        };
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(
            json,
            r#"{"state":"needs-input","request":{"kind":"permission","tool":"Bash"}}"#
        );
        assert_eq!(serde_json::from_str::<AgentState>(&json).unwrap(), state);
    }
}
