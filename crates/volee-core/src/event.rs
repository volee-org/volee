//! Events: the append-only facts the state is built from (ADR 0003).

use serde::{Deserialize, Serialize};

use crate::delivery::Delivery;
use crate::harness::{HarnessKind, SessionKey, SessionOrigin};
use crate::ids::{AgentId, MessageId, MissionId};
use crate::message::{Message, Party, Thread};
use crate::mission::{Mission, MissionStatus, Verdict};
use crate::role::{LaunchedBy, Role};
use crate::state::AgentState;
use crate::time::Timestamp;
use crate::workspace::{Location, Workspace};

/// A fact, with the time it happened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// When it happened.
    pub at: Timestamp,
    /// What happened.
    #[serde(flatten)]
    pub kind: EventKind,
}

impl Event {
    /// An event at a given time.
    pub const fn new(at: Timestamp, kind: EventKind) -> Self {
        Self { at, kind }
    }
}

/// Everything that can happen. Each variant is applied by
/// [`Registry::apply`](crate::registry::Registry::apply).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub enum EventKind {
    /// A new agent exists.
    AgentRegistered {
        /// Its identifier.
        agent: AgentId,
        /// Its display name.
        name: String,
        /// Its role.
        role: Role,
        /// Its harness.
        harness: HarnessKind,
        /// Who started it.
        launched_by: LaunchedBy,
        /// The agent that launched it.
        parent: Option<AgentId>,
        /// Its mission.
        mission: Option<MissionId>,
        /// Where it works.
        workspace: Workspace,
        /// Where it can be shown.
        location: Location,
    },
    /// A harness session now belongs to an agent and becomes its current one.
    SessionLinked {
        /// The agent.
        agent: AgentId,
        /// The session.
        session: SessionKey,
        /// How it joined.
        origin: SessionOrigin,
    },
    /// The agent's state changed.
    StateChanged {
        /// The agent.
        agent: AgentId,
        /// The new state.
        state: AgentState,
        /// A detail, such as the question or the last message.
        detail: Option<String>,
    },
    /// The agent did something worth showing.
    ActivityRecorded {
        /// The agent.
        agent: AgentId,
        /// One line, such as `Edit src/lib.rs`.
        summary: String,
    },
    /// The agent's workspace is better known.
    WorkspaceUpdated {
        /// The agent.
        agent: AgentId,
        /// The new workspace.
        workspace: Workspace,
    },
    /// The agent can be shown somewhere else.
    LocationChanged {
        /// The agent.
        agent: AgentId,
        /// The new location.
        location: Location,
    },
    /// The agent uses another model.
    ModelChanged {
        /// The agent.
        agent: AgentId,
        /// The model's name.
        model: String,
    },
    /// New delivery evidence.
    DeliveryUpdated {
        /// The agent.
        agent: AgentId,
        /// The evidence.
        delivery: Delivery,
    },
    /// A person asked volee to forget an agent. Its sessions are ignored from
    /// now on, so it does not come back as an independent agent.
    AgentForgotten {
        /// The agent.
        agent: AgentId,
    },
    /// The agent is known under another name: its harness renamed the
    /// session volee found it by (ADR 0027).
    AgentRenamed {
        /// The agent.
        agent: AgentId,
        /// Its new display name.
        name: String,
    },
    /// A session no longer belongs to the agent. It is not ignored: listed
    /// again, it becomes an agent of its own (ADR 0027).
    SessionReleased {
        /// The agent.
        agent: AgentId,
        /// The session.
        session: SessionKey,
    },
    /// An agent volee only watched is gone: its harness no longer lists its
    /// session, so volee stops showing it. Unlike a forgotten agent's, its
    /// sessions are not ignored: listed again, they come back (ADR 0027).
    AgentExpired {
        /// The agent.
        agent: AgentId,
    },
    /// The human asked volee to stop an agent. volee does not resume it on
    /// its own until the human sends it a message.
    StopRequested {
        /// The agent.
        agent: AgentId,
    },
    /// An agent that joined through volee's MCP server takes part in a
    /// mission, or no longer does (ADR 0014). Only such agents change
    /// missions: the others keep the one they were launched for.
    MembershipChanged {
        /// The agent.
        agent: AgentId,
        /// The mission it takes part in now, if any.
        mission: Option<MissionId>,
    },
    /// A conversation thread was opened.
    ThreadOpened {
        /// The thread.
        thread: Thread,
    },
    /// A message was sent.
    MessagePosted {
        /// The message, with empty receipts.
        message: Message,
    },
    /// A message reached an agent.
    MessageDelivered {
        /// The message.
        message: MessageId,
        /// The recipient.
        to: Party,
    },
    /// A recipient read a message.
    MessageRead {
        /// The message.
        message: MessageId,
        /// The reader.
        by: Party,
    },
    /// A mission was created.
    MissionCreated {
        /// The mission.
        mission: Mission,
    },
    /// The supervisor judged a worker of a mission.
    WorkerAssessed {
        /// The mission.
        mission: MissionId,
        /// The worker.
        worker: AgentId,
        /// The verdict.
        verdict: Verdict,
        /// The evidence, in one line.
        evidence: String,
    },
    /// A mission moved forward.
    MissionUpdated {
        /// The mission.
        mission: MissionId,
        /// Its new status.
        status: MissionStatus,
        /// Its coordinator, when it changes.
        coordinator: Option<AgentId>,
        /// Its report, once written.
        report: Option<MessageId>,
    },
}

impl EventKind {
    /// The agent this event takes out of the registry, if it takes one out:
    /// a person forgot it, or it expired. Everything kept about an agent
    /// outside the registry is dropped on such an event.
    pub const fn removed_agent(&self) -> Option<AgentId> {
        match self {
            Self::AgentForgotten { agent } | Self::AgentExpired { agent } => Some(*agent),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ulid::Ulid;

    #[test]
    fn only_forgetting_and_expiring_remove_an_agent() {
        let agent = AgentId::from_ulid(Ulid::from_parts(5, 6));
        assert_eq!(
            EventKind::AgentForgotten { agent }.removed_agent(),
            Some(agent)
        );
        assert_eq!(
            EventKind::AgentExpired { agent }.removed_agent(),
            Some(agent)
        );
        let renamed = EventKind::AgentRenamed {
            agent,
            name: "other".into(),
        };
        assert_eq!(renamed.removed_agent(), None);
    }

    #[test]
    fn the_radar_events_keep_their_names_on_the_wire() {
        let agent = AgentId::from_ulid(Ulid::from_parts(5, 6));
        let tag = |kind: EventKind| {
            serde_json::to_value(Event::new(Timestamp::from_millis(1), kind)).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(tag(EventKind::AgentExpired { agent }), "agent-expired");
        assert_eq!(
            tag(EventKind::AgentRenamed {
                agent,
                name: "n".into()
            }),
            "agent-renamed"
        );
        assert_eq!(
            tag(EventKind::SessionReleased {
                agent,
                session: SessionKey::claude("s")
            }),
            "session-released"
        );
    }

    #[test]
    fn events_serialize_flat_with_a_type_tag() {
        let agent = AgentId::from_ulid(Ulid::from_parts(5, 6));
        let event = Event::new(
            Timestamp::from_millis(42),
            EventKind::ActivityRecorded {
                agent,
                summary: "Edit src/lib.rs".into(),
            },
        );
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "activity-recorded");
        assert_eq!(json["at"], 42);
        assert_eq!(json["summary"], "Edit src/lib.rs");
        let back: Event = serde_json::from_value(json).unwrap();
        assert_eq!(back, event);
    }
}
