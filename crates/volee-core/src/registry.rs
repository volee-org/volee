//! The registry: the current state, built by applying events in order.
//!
//! The daemon keeps one registry in memory and persists the events that build
//! it. Every reader (interface, command line, agents) sees the same registry,
//! so there is exactly one rule for every join (ADR 0003).

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::agent::{Activity, Agent, Hold, SessionLink};
use crate::event::{Event, EventKind};
use crate::harness::SessionKey;
use crate::ids::{AgentId, MessageId, MissionId, ThreadId};
use crate::message::{Message, MessageKind, Party, Thread};
use crate::mission::{Assessment, Mission};
use crate::role::LaunchedBy;
use crate::state::AgentState;

/// An event that does not fit the current state. The daemon validates events
/// before persisting them, so this signals a bug or a corrupted log.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
    /// The event names an agent that does not exist.
    #[error("unknown agent {0}")]
    UnknownAgent(AgentId),
    /// The agent already exists.
    #[error("agent {0} is already registered")]
    DuplicateAgent(AgentId),
    /// The session already belongs to another agent.
    #[error("session {session} already belongs to agent {owner}")]
    SessionTaken {
        /// The session.
        session: SessionKey,
        /// Its owner.
        owner: AgentId,
    },
    /// The event names a thread that does not exist.
    #[error("unknown thread {0}")]
    UnknownThread(ThreadId),
    /// The thread already exists.
    #[error("thread {0} already exists")]
    DuplicateThread(ThreadId),
    /// The event names a message that does not exist.
    #[error("unknown message {0}")]
    UnknownMessage(MessageId),
    /// The message already exists.
    #[error("message {0} already exists")]
    DuplicateMessage(MessageId),
    /// The event names a mission that does not exist.
    #[error("unknown mission {0}")]
    UnknownMission(MissionId),
    /// The mission already exists.
    #[error("mission {0} already exists")]
    DuplicateMission(MissionId),
    /// Only agents that joined through volee's MCP server change missions.
    #[error("agent {0} did not join through volee's MCP server, so its mission is fixed")]
    NotJoined(AgentId),
}

/// Why a message may not be sent (ADR 0006).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RouteError {
    /// The sender or the recipient does not exist.
    #[error("unknown agent {0}")]
    UnknownAgent(AgentId),
    /// An agent tried to write to itself.
    #[error("an agent cannot send a message to itself")]
    ToSelf,
    /// The two agents share no mission and no parent link.
    #[error(
        "{from} may not write to {to}: outside a mission, an agent writes to its parent, \
         its children, its coordinator or the human"
    )]
    NotPermitted {
        /// The sender.
        from: AgentId,
        /// The recipient.
        to: AgentId,
    },
}

/// The current state of everything volee tracks.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    agents: BTreeMap<AgentId, Agent>,
    /// The last name of every agent ever registered, the ones that left
    /// included: the threads they wrote in still name them.
    names: HashMap<AgentId, String>,
    by_session: HashMap<SessionKey, AgentId>,
    ignored_sessions: HashSet<SessionKey>,
    threads: BTreeMap<ThreadId, Thread>,
    messages: BTreeMap<MessageId, Message>,
    missions: BTreeMap<MissionId, Mission>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a registry by applying events in order.
    pub fn from_events<'a>(
        events: impl IntoIterator<Item = &'a Event>,
    ) -> Result<Self, ApplyError> {
        let mut registry = Self::new();
        for event in events {
            registry.apply(event)?;
        }
        Ok(registry)
    }

    /// Applies one event. On error the registry is left unchanged.
    #[allow(clippy::too_many_lines)] // One arm per event kind reads best as a single match.
    pub fn apply(&mut self, event: &Event) -> Result<(), ApplyError> {
        let at = event.at;
        match &event.kind {
            EventKind::AgentRegistered {
                agent,
                name,
                role,
                harness,
                launched_by,
                parent,
                mission,
                workspace,
                location,
            } => {
                if self.agents.contains_key(agent) {
                    return Err(ApplyError::DuplicateAgent(*agent));
                }
                self.names.insert(*agent, name.clone());
                self.agents.insert(
                    *agent,
                    Agent {
                        id: *agent,
                        name: name.clone(),
                        role: *role,
                        harness: harness.clone(),
                        launched_by: *launched_by,
                        parent: *parent,
                        mission: *mission,
                        sessions: Vec::new(),
                        workspace: workspace.clone(),
                        location: location.clone(),
                        state: AgentState::Launching,
                        state_since: at,
                        detail: None,
                        last_activity: None,
                        worked: false,
                        model: None,
                        context: None,
                        delivery: crate::delivery::Delivery::default(),
                        hold: None,
                        created_at: at,
                        updated_at: at,
                    },
                );
            }
            EventKind::SessionLinked {
                agent,
                session,
                origin,
            } => {
                if let Some(owner) = self.by_session.get(session)
                    && owner != agent
                {
                    return Err(ApplyError::SessionTaken {
                        session: session.clone(),
                        owner: *owner,
                    });
                }
                let record = self.agent_mut(*agent)?;
                if !record.owns_session(session) {
                    record.sessions.push(SessionLink {
                        key: session.clone(),
                        origin: *origin,
                        at,
                    });
                }
                record.updated_at = at;
                self.by_session.insert(session.clone(), *agent);
            }
            EventKind::StateChanged {
                agent,
                state,
                detail,
            } => {
                let record = self.agent_mut(*agent)?;
                if &record.state != state {
                    record.state = state.clone();
                    record.state_since = at;
                }
                if matches!(state, AgentState::Working | AgentState::NeedsInput { .. }) {
                    record.worked = true;
                }
                record.detail.clone_from(detail);
                record.updated_at = at;
            }
            EventKind::ActivityRecorded { agent, summary } => {
                let record = self.agent_mut(*agent)?;
                record.last_activity = Some(Activity {
                    at,
                    summary: summary.clone(),
                });
                record.worked = true;
                record.updated_at = at;
            }
            EventKind::WorkspaceUpdated { agent, workspace } => {
                let record = self.agent_mut(*agent)?;
                record.workspace = workspace.clone();
                record.updated_at = at;
            }
            EventKind::LocationChanged { agent, location } => {
                let record = self.agent_mut(*agent)?;
                record.location = location.clone();
                record.updated_at = at;
            }
            EventKind::ModelChanged { agent, model } => {
                let record = self.agent_mut(*agent)?;
                record.model = Some(model.clone());
                record.updated_at = at;
            }
            EventKind::ContextUsed { agent, percent } => {
                let record = self.agent_mut(*agent)?;
                record.context = Some((*percent).min(100));
                record.updated_at = at;
            }
            EventKind::DeliveryUpdated { agent, delivery } => {
                let record = self.agent_mut(*agent)?;
                record.delivery = delivery.clone();
                record.updated_at = at;
            }
            EventKind::AgentForgotten { agent } => {
                let record = self
                    .agents
                    .remove(agent)
                    .ok_or(ApplyError::UnknownAgent(*agent))?;
                for link in record.sessions {
                    self.by_session.remove(&link.key);
                    self.ignored_sessions.insert(link.key);
                }
            }
            EventKind::AgentRenamed { agent, name } => {
                let record = self.agent_mut(*agent)?;
                record.name.clone_from(name);
                record.updated_at = at;
                self.names.insert(*agent, name.clone());
            }
            EventKind::SessionReleased { agent, session } => {
                let record = self.agent_mut(*agent)?;
                record.sessions.retain(|link| &link.key != session);
                record.updated_at = at;
                if self.by_session.get(session) == Some(agent) {
                    self.by_session.remove(session);
                }
            }
            EventKind::AgentExpired { agent } => {
                let record = self
                    .agents
                    .remove(agent)
                    .ok_or(ApplyError::UnknownAgent(*agent))?;
                // Not ignored: a session listed again comes back.
                for link in record.sessions {
                    self.by_session.remove(&link.key);
                }
            }
            EventKind::StopRequested { agent } => {
                let record = self.agent_mut(*agent)?;
                // A finished mission's hold is the stronger one: nothing
                // lifts it, so a later stop does not replace it.
                if !matches!(record.hold, Some(Hold::MissionOver { .. })) {
                    record.hold = Some(Hold::StoppedByHuman { at });
                }
                record.updated_at = at;
            }
            EventKind::MembershipChanged { agent, mission } => {
                if let Some(mission) = mission
                    && !self.missions.contains_key(mission)
                {
                    return Err(ApplyError::UnknownMission(*mission));
                }
                let record = self.agent_mut(*agent)?;
                if !record.joined() {
                    return Err(ApplyError::NotJoined(*agent));
                }
                record.mission = *mission;
                record.updated_at = at;
            }
            EventKind::ThreadOpened { thread } => {
                if self.threads.contains_key(&thread.id) {
                    return Err(ApplyError::DuplicateThread(thread.id));
                }
                self.threads.insert(thread.id, thread.clone());
            }
            EventKind::MessagePosted { message } => {
                if !self.threads.contains_key(&message.thread) {
                    return Err(ApplyError::UnknownThread(message.thread));
                }
                if self.messages.contains_key(&message.id) {
                    return Err(ApplyError::DuplicateMessage(message.id));
                }
                if message.from == Party::Human {
                    // Writing to an agent it stopped is how the human lets
                    // volee resume it.
                    for receipt in &message.receipts {
                        if let Party::Agent(id) = receipt.to
                            && let Some(agent) = self.agents.get_mut(&id)
                            && matches!(agent.hold, Some(Hold::StoppedByHuman { .. }))
                        {
                            agent.hold = None;
                            agent.updated_at = at;
                        }
                    }
                }
                self.messages.insert(message.id, message.clone());
            }
            EventKind::MessageDelivered { message, to } => {
                let record = self
                    .messages
                    .get_mut(message)
                    .ok_or(ApplyError::UnknownMessage(*message))?;
                if let Some(receipt) = record.receipts.iter_mut().find(|r| r.to == *to) {
                    receipt.delivered_at.get_or_insert(at);
                }
            }
            EventKind::MessageRead { message, by } => {
                let record = self
                    .messages
                    .get_mut(message)
                    .ok_or(ApplyError::UnknownMessage(*message))?;
                if let Some(receipt) = record.receipts.iter_mut().find(|r| r.to == *by) {
                    receipt.delivered_at.get_or_insert(at);
                    receipt.read_at.get_or_insert(at);
                }
            }
            EventKind::MissionCreated { mission } => {
                if self.missions.contains_key(&mission.id) {
                    return Err(ApplyError::DuplicateMission(mission.id));
                }
                self.missions.insert(mission.id, mission.clone());
            }
            EventKind::WorkerAssessed {
                mission,
                worker,
                verdict,
                evidence,
            } => {
                let record = self
                    .missions
                    .get_mut(mission)
                    .ok_or(ApplyError::UnknownMission(*mission))?;
                record.assessments.insert(
                    *worker,
                    Assessment {
                        verdict: *verdict,
                        evidence: evidence.clone(),
                        at,
                    },
                );
            }
            EventKind::MissionUpdated {
                mission,
                status,
                coordinator,
                report,
            } => {
                let record = self
                    .missions
                    .get_mut(mission)
                    .ok_or(ApplyError::UnknownMission(*mission))?;
                record.status = *status;
                if coordinator.is_some() {
                    record.coordinator = *coordinator;
                }
                if report.is_some() {
                    record.report = *report;
                }
                if status.is_over() && record.ended_at.is_none() {
                    record.ended_at = Some(at);
                    // A finished mission's agents are never resumed on their
                    // own: a stray message must not restart a paid session.
                    for agent in self.agents.values_mut() {
                        if agent.mission == Some(*mission) {
                            agent.hold = Some(Hold::MissionOver { at });
                            agent.updated_at = at;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn agent_mut(&mut self, id: AgentId) -> Result<&mut Agent, ApplyError> {
        self.agents.get_mut(&id).ok_or(ApplyError::UnknownAgent(id))
    }

    /// An agent by identifier.
    pub fn agent(&self, id: AgentId) -> Option<&Agent> {
        self.agents.get(&id)
    }

    /// The name of an agent, whether it is still listed or left the list
    /// (forgotten, tidied or expired): what it wrote stays in its threads,
    /// and reads better under its name than under an identifier.
    pub fn name_of(&self, id: AgentId) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }

    /// Why `id` cannot be forgotten now, in words for the person: an agent
    /// volee launched that still runs, or an agent of an open mission.
    /// `None` when it can be, or when the registry does not know it.
    #[must_use]
    pub fn forget_refusal(&self, id: AgentId) -> Option<String> {
        let record = self.agent(id)?;
        let name = &record.name;
        if record.launched_by != LaunchedBy::Outside && record.state.is_live() {
            return Some(format!(
                "{name} is still running: stop it first (`volee stop {name}`), then forget it"
            ));
        }
        let mission = record.mission.and_then(|mission| self.mission(mission))?;
        (!mission.status.is_over()).then(|| {
            format!(
                "{name} belongs to the open mission \"{}\": close or abort the mission first",
                mission.title
            )
        })
    }

    /// The agent that owns a harness session, current or past.
    pub fn agent_for_session(&self, key: &SessionKey) -> Option<&Agent> {
        self.by_session.get(key).and_then(|id| self.agents.get(id))
    }

    /// Whether a session belonged to a forgotten agent and must be ignored.
    pub fn is_ignored(&self, key: &SessionKey) -> bool {
        self.ignored_sessions.contains(key)
    }

    /// Every agent, oldest first.
    pub fn agents(&self) -> impl Iterator<Item = &Agent> {
        self.agents.values()
    }

    /// Agents sorted by urgency: those who need a person first, then failed,
    /// working, done, idle, stopped and lost. Within a group, the most recent
    /// change comes first. A failed agent that volee only watches sorts with
    /// the stopped ones ([`Agent::urgency`]).
    pub fn agents_by_urgency(&self) -> Vec<&Agent> {
        let mut agents: Vec<&Agent> = self.agents.values().collect();
        agents.sort_by(|a, b| {
            a.urgency()
                .cmp(&b.urgency())
                .then(b.state_since.cmp(&a.state_since))
                .then(a.id.cmp(&b.id))
        });
        agents
    }

    /// Agents launched by `parent`, oldest first.
    pub fn children(&self, parent: AgentId) -> Vec<&Agent> {
        self.agents
            .values()
            .filter(|agent| agent.parent == Some(parent))
            .collect()
    }

    /// Agents with no known parent: leads, independent sessions, and agents
    /// whose parent was forgotten. Oldest first.
    pub fn roots(&self) -> Vec<&Agent> {
        self.agents
            .values()
            .filter(|agent| {
                agent
                    .parent
                    .is_none_or(|parent| !self.agents.contains_key(&parent))
            })
            .collect()
    }

    /// A mission by identifier.
    pub fn mission(&self, id: MissionId) -> Option<&Mission> {
        self.missions.get(&id)
    }

    /// Every mission, oldest first.
    pub fn missions(&self) -> impl Iterator<Item = &Mission> {
        self.missions.values()
    }

    /// The agents of a mission, oldest first.
    pub fn mission_agents(&self, mission: MissionId) -> Vec<&Agent> {
        self.agents
            .values()
            .filter(|agent| agent.mission == Some(mission))
            .collect()
    }

    /// A thread by identifier.
    pub fn thread(&self, id: ThreadId) -> Option<&Thread> {
        self.threads.get(&id)
    }

    /// Every thread, oldest first.
    pub fn threads(&self) -> impl Iterator<Item = &Thread> {
        self.threads.values()
    }

    /// The direct thread between two parties, outside any mission.
    pub fn thread_between(&self, a: Party, b: Party) -> Option<&Thread> {
        self.threads.values().find(|thread| {
            thread.mission.is_none()
                && thread.participants.len() == 2
                && thread.participants.contains(&a)
                && thread.participants.contains(&b)
        })
    }

    /// A message by identifier.
    pub fn message(&self, id: MessageId) -> Option<&Message> {
        self.messages.get(&id)
    }

    /// Every message, oldest first.
    pub fn messages(&self) -> impl Iterator<Item = &Message> {
        self.messages.values()
    }

    /// The messages of a thread, oldest first.
    pub fn thread_messages(&self, thread: ThreadId) -> Vec<&Message> {
        self.messages
            .values()
            .filter(|message| message.thread == thread)
            .collect()
    }

    /// Messages sent by `party`, oldest first.
    pub fn messages_from(&self, party: Party) -> Vec<&Message> {
        self.messages
            .values()
            .filter(|message| message.from == party)
            .collect()
    }

    /// Messages addressed to `agent` that have not reached it yet, oldest
    /// first. The daemon delivers them through the agent's hooks.
    pub fn pending_for(&self, agent: AgentId) -> Vec<&Message> {
        let party = Party::Agent(agent);
        self.messages
            .values()
            .filter(|message| {
                message
                    .receipt(party)
                    .is_some_and(|receipt| receipt.delivered_at.is_none())
            })
            .collect()
    }

    /// Questions sent to the human that no answer from the human refers to,
    /// oldest first.
    pub fn unanswered_questions(&self) -> Vec<&Message> {
        let answered: HashSet<MessageId> = self
            .messages
            .values()
            .filter(|message| message.from == Party::Human)
            .filter_map(|message| message.in_reply_to)
            .collect();
        self.messages
            .values()
            .filter(|message| {
                message.kind == MessageKind::Question
                    && message.is_for(Party::Human)
                    && !answered.contains(&message.id)
            })
            .collect()
    }

    /// The person's answer to `question`: their first message that refers
    /// to it, which is the one that took it off their list.
    pub fn answer_to(&self, question: MessageId) -> Option<&Message> {
        self.messages
            .values()
            .find(|message| message.from == Party::Human && message.in_reply_to == Some(question))
    }

    /// Checks whether `from` may write to `to` (ADR 0006): people and volee
    /// write to anyone, agents write to people, and between agents, members
    /// of the same mission write to each other; otherwise an agent writes to
    /// its parent, its children and its mission's coordinator.
    pub fn check_route(&self, from: Party, to: Party) -> Result<(), RouteError> {
        let (Party::Agent(from), Party::Agent(to)) = (from, to) else {
            return Ok(());
        };
        if from == to {
            return Err(RouteError::ToSelf);
        }
        let sender = self.agent(from).ok_or(RouteError::UnknownAgent(from))?;
        let recipient = self.agent(to).ok_or(RouteError::UnknownAgent(to))?;
        let same_mission = sender.mission.is_some() && sender.mission == recipient.mission;
        let family = sender.parent == Some(to) || recipient.parent == Some(from);
        let coordinator = sender
            .mission
            .and_then(|mission| self.mission(mission))
            .is_some_and(|mission| {
                mission.coordinator == Some(to) || mission.requested_by == Some(to)
            });
        if same_mission || family || coordinator {
            Ok(())
        } else {
            Err(RouteError::NotPermitted { from, to })
        }
    }
}

#[cfg(test)]
mod tests;
