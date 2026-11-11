//! What the tools return: small JSON documents written for language models.

use serde::Serialize;
use volee_core::message::{Message, Party, Reference};
use volee_core::{Agent, AgentId, Registry};

/// An agent as other agents see it.
#[derive(Debug, Serialize)]
pub(crate) struct AgentView {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) role: &'static str,
    pub(crate) state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) mission: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parent: Option<String>,
    pub(crate) harness: String,
    /// Whether the calling agent may write to this one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) can_message: Option<bool>,
}

impl AgentView {
    pub(crate) fn of(registry: &Registry, agent: &Agent, viewer: Option<AgentId>) -> Self {
        Self {
            id: agent.id.to_string(),
            name: agent.name.clone(),
            role: agent.role.label(),
            state: agent.state.label(),
            detail: agent
                .detail
                .clone()
                .or_else(|| agent.last_activity.as_ref().map(|a| a.summary.clone())),
            repo: agent.workspace.repo.clone(),
            branch: agent.workspace.branch.clone(),
            mission: agent
                .mission
                .and_then(|id| registry.mission(id))
                .map(|mission| mission.title.clone()),
            parent: agent
                .parent
                .and_then(|id| registry.agent(id))
                .map(|parent| parent.name.clone()),
            harness: agent.harness.to_string(),
            can_message: viewer.filter(|me| *me != agent.id).map(|me| {
                registry
                    .check_route(Party::Agent(me), Party::Agent(agent.id))
                    .is_ok()
            }),
        }
    }
}

/// The calling agent's own record.
#[derive(Debug, Serialize)]
pub(crate) struct WhoAmI {
    #[serde(flatten)]
    pub(crate) agent: AgentView,
    pub(crate) cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) worktree: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) base_branch: Option<String>,
    pub(crate) hint: &'static str,
}

/// A message as its recipient reads it.
#[derive(Debug, Serialize)]
pub(crate) struct MessageView {
    pub(crate) id: String,
    pub(crate) thread: String,
    pub(crate) from: String,
    pub(crate) kind: &'static str,
    pub(crate) body: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) refs: Vec<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) in_reply_to: Option<String>,
    /// The answers a question to the human offers.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) options: Vec<String>,
    pub(crate) at_ms: i64,
}

impl MessageView {
    pub(crate) fn of(registry: &Registry, message: &Message) -> Self {
        Self {
            id: message.id.to_string(),
            thread: message.thread.to_string(),
            from: party_name(registry, message.from),
            kind: message.kind.label(),
            body: message.body.clone(),
            refs: message.refs.clone(),
            in_reply_to: message.in_reply_to.map(|id| id.to_string()),
            options: message.options.clone(),
            at_ms: message.at.as_millis(),
        }
    }
}

/// `the human`, `volee`, or `name (role, id)`.
pub(crate) fn party_name(registry: &Registry, party: Party) -> String {
    match party {
        Party::Human => "the human".to_owned(),
        Party::System => "volee".to_owned(),
        Party::Agent(id) => match (registry.agent(id), registry.name_of(id)) {
            (Some(agent), _) => format!("{} ({}, {})", agent.name, agent.role, agent.id),
            // It left the list: its name still says who wrote.
            (None, Some(name)) => format!("{name} (gone, {id})"),
            (None, None) => id.to_string(),
        },
    }
}
