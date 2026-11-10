//! Agents (ADR 0001).

use serde::{Deserialize, Serialize};

use crate::delivery::Delivery;
use crate::harness::{HarnessKind, SessionKey, SessionOrigin};
use crate::ids::{AgentId, MissionId};
use crate::role::{LaunchedBy, Role};
use crate::state::{AgentState, StateCategory};
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

/// Why volee does not resume an agent on its own when a message arrives after
/// its session ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "reason")]
pub enum Hold {
    /// The human stopped it. The next message the human sends it lifts the
    /// hold; messages from agents and from volee do not.
    StoppedByHuman {
        /// When the human asked.
        at: Timestamp,
    },
    /// Its mission is over. Nothing lifts this hold.
    MissionOver {
        /// When the mission ended.
        at: Timestamp,
    },
}

impl Hold {
    /// One line for people.
    pub const fn describe(self) -> &'static str {
        match self {
            Self::StoppedByHuman { .. } => "stopped by the human",
            Self::MissionOver { .. } => "its mission is over",
        }
    }
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
    /// Why volee will not resume it on its own, if it will not.
    #[serde(default)]
    pub hold: Option<Hold>,
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

    /// Whether this agent joined through volee's MCP server (`volee mcp
    /// --join`, ADR 0014): an agent of another harness, which volee did not
    /// launch and only knows through its MCP server. Its state comes from
    /// that server, and messages reach it when it reads them.
    ///
    /// volee discovers the sessions of no other harness yet, so an agent of
    /// another harness that volee did not launch is a joined one.
    pub fn joined(&self) -> bool {
        self.launched_by == LaunchedBy::Outside && matches!(self.harness, HarnessKind::Other(_))
    }

    /// Whether volee drives the agent: it launched it, or the agent runs in
    /// one of volee's hosts, as an adopted agent does
    /// ([ADR 0023](https://volee-org.github.io/volee/adr/0023-every-agent-on-the-machine.html)).
    /// volee hands messages to such an agent, stops it and resumes it; any
    /// other agent is read only.
    pub fn managed(&self) -> bool {
        self.launched_by != LaunchedBy::Outside || self.location.host().is_some()
    }

    /// Whether volee only watches the agent
    /// ([ADR 0027](https://volee-org.github.io/volee/adr/0027-open-without-owning.html)):
    /// someone else started it, it runs in none of volee's hosts, and volee
    /// knows it by a session its harness lists. Such an agent is one session:
    /// it carries the session's name, and it leaves the registry when its
    /// harness no longer lists the session.
    pub fn watched(&self) -> bool {
        self.launched_by == LaunchedBy::Outside
            && self.location.host().is_none()
            && !self.sessions.is_empty()
    }

    /// Where the agent sorts when agents are listed by urgency: the family
    /// of its state, except that a watched agent that failed sorts with the
    /// stopped ones. A failure calls for the person who launched the work
    /// through volee; a session started elsewhere that ended, however it
    /// ended, is history.
    pub fn urgency(&self) -> StateCategory {
        match self.state.category() {
            StateCategory::Failed if self.watched() => StateCategory::Stopped,
            category => category,
        }
    }

    /// Whether `key` is one of the agent's sessions, current or past.
    pub fn owns_session(&self, key: &SessionKey) -> bool {
        self.sessions.iter().any(|link| &link.key == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, EventKind};
    use crate::registry::Registry;
    use ulid::Ulid;

    fn agent(launched_by: LaunchedBy, location: Location, session: Option<&str>) -> Agent {
        let id = AgentId::from_ulid(Ulid::from_parts(7, 7));
        let at = Timestamp::from_millis(1);
        let mut registry = Registry::new();
        registry
            .apply(&Event::new(
                at,
                EventKind::AgentRegistered {
                    agent: id,
                    name: "a".into(),
                    role: Role::Independent,
                    harness: HarnessKind::ClaudeCode,
                    launched_by,
                    parent: None,
                    mission: None,
                    workspace: Workspace::at("/work"),
                    location,
                },
            ))
            .unwrap();
        if let Some(session) = session {
            registry
                .apply(&Event::new(
                    at,
                    EventKind::SessionLinked {
                        agent: id,
                        session: SessionKey::claude(session),
                        origin: SessionOrigin::Discovered,
                    },
                ))
                .unwrap();
        }
        registry.agent(id).unwrap().clone()
    }

    #[test]
    fn only_a_session_started_elsewhere_and_outside_a_host_is_watched() {
        let host = Location::Host { id: "h".into() };
        assert!(agent(LaunchedBy::Outside, Location::Background, Some("s")).watched());
        assert!(agent(LaunchedBy::Outside, Location::Terminal, Some("s")).watched());
        assert!(
            !agent(LaunchedBy::Human, Location::Background, Some("s")).watched(),
            "volee launched it"
        );
        assert!(
            !agent(LaunchedBy::Outside, host, Some("s")).watched(),
            "adopted into a host"
        );
        assert!(
            !agent(LaunchedBy::Outside, Location::Unknown, None).watched(),
            "joined through the MCP server: no session"
        );
    }

    #[test]
    fn a_watched_failure_sorts_with_the_stopped() {
        let mut watched = agent(LaunchedBy::Outside, Location::Background, Some("s"));
        watched.state = AgentState::Failed;
        assert_eq!(watched.urgency(), StateCategory::Stopped);
        watched.state = AgentState::Working;
        assert_eq!(watched.urgency(), StateCategory::Working);
        let mut launched = agent(LaunchedBy::Human, Location::Background, Some("s"));
        launched.state = AgentState::Failed;
        assert_eq!(launched.urgency(), StateCategory::Failed);
    }
}
