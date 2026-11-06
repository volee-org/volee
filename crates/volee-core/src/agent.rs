//! Agents (ADR 0001).

use serde::{Deserialize, Serialize};

use crate::delivery::Delivery;
use crate::harness::{HarnessKind, SessionKey, SessionOrigin};
use crate::ids::{AgentId, MissionId};
use crate::role::{LaunchedBy, Role};
use crate::state::AgentState;
use crate::time::Timestamp;
use crate::workspace::{Location, Workspace};

/// A harness session that belongs to an agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLink {
    /// The session.
    pub key: SessionKey,
    /// How it joined the agent.
    pub origin: SessionOrigin,
    /// When it joined.
    pub at: Timestamp,
}

/// The last thing an agent was seen doing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    /// When.
    pub at: Timestamp,
    /// A one-line description, such as `Edit src/lib.rs`.
    pub summary: String,
}

/// A unit of work done by one or more harness sessions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    /// Its identifier, stable across sessions.
    pub id: AgentId,
    /// A short display name.
    pub name: String,
    /// Its role.
    pub role: Role,
    /// The harness that runs it.
    pub harness: HarnessKind,
    /// Who started it.
    pub launched_by: LaunchedBy,
    /// The agent that launched it, if any.
    pub parent: Option<AgentId>,
    /// The mission it works for, if any.
    pub mission: Option<MissionId>,
    /// Its sessions, oldest first; the last one is current.
    pub sessions: Vec<SessionLink>,
    /// Where it works.
    pub workspace: Workspace,
    /// Where it can be shown.
    pub location: Location,
    /// Its lifecycle state.
    pub state: AgentState,
    /// When it entered its current state.
    pub state_since: Timestamp,
    /// A detail about the state, such as the question asked.
    pub detail: Option<String>,
    /// The last activity seen.
    pub last_activity: Option<Activity>,
    /// The model in use, when known.
    pub model: Option<String>,
    /// What can be proven about its work.
    pub delivery: Delivery,
    /// When volee first registered it.
    pub created_at: Timestamp,
    /// When anything about it last changed.
    pub updated_at: Timestamp,
}

impl Agent {
    /// The current harness session, if any.
    pub fn current_session(&self) -> Option<&SessionLink> {
        self.sessions.last()
    }

    /// Whether `key` is one of the agent's sessions, current or past.
    pub fn owns_session(&self, key: &SessionKey) -> bool {
        self.sessions.iter().any(|link| &link.key == key)
    }
}
