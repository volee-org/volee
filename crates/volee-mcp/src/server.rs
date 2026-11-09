//! The MCP tools.

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use volee_core::message::{MessageKind, Party, Reference};
use volee_core::role::Role;
use volee_core::{AgentId, AgentState, Registry};
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{
    ActivityParams, CloseMission, CloseParams, Join, JoinParams, MarkRead, MarkReadParams,
    RecordActivity, SendMessage, SendParams, Spawn, SpawnParams,
};
use volee_proto::replica::{Replica, ReplicaError};

use crate::view::{AgentView, MessageView, WhoAmI, party_name};

/// What the server tells the model about itself when it connects.
const INSTRUCTIONS: &str = "volee connects you with the other coding agents on this machine and \
with the human who supervises them. Use list_agents to see who works on what, send_message to \
write to an agent or to the human (by id, by name, or \"human\"), read_messages to read what was \
sent to you, and report when your task is done, blocked or failed. A mission's coordinator calls \
close_mission once every worker delivered and its final report is sent. Messages sent to you \
also arrive on their own at the end of your turn.";

/// The same for an agent of another harness that joined (ADR 0014): no
/// hooks hand it messages, so it must read them.
const JOINED_INSTRUCTIONS: &str = "volee connects you with the other coding agents on this machine, \
whatever AI runs them, and with the human who supervises them. You joined volee through this \
server, so messages sent to you wait until you call read_messages: call it when you start a \
task, after you send a question, and before you finish. Use list_agents to see who works on \
what and whom you may write to (can_message), send_message to write to an agent or to the human \
(by id, by name, or \"human\"), and report when your task is done, blocked or failed.";

/// Starting the server failed.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// Talking to the daemon failed.
    #[error(transparent)]
    Client(#[from] ClientError),
    /// The replica of the daemon's registry could not start.
    #[error(transparent)]
    Replica(#[from] ReplicaError),
    /// The agent this server speaks for is unknown to the daemon.
    #[error("volee does not know agent {0}; start `volee mcp` with the id of an agent it knows")]
    UnknownAgent(AgentId),
}

/// The MCP server of one agent.
#[derive(Debug, Clone)]
pub struct VoleeMcp {
    inner: Arc<Inner>,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug)]
struct Inner {
    me: AgentId,
    client: Client,
    replica: Replica,
    /// Whether `me` joined through this server: then no hooks report on it,
    /// and the server records its tool calls as its activity.
    joined: bool,
}

/// Parameters of `list_agents`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListArgs {
    /// Also list agents that stopped, failed or were lost.
    #[serde(default)]
    pub include_finished: bool,
}

/// What a message is for.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum KindArg {
    /// You expect an answer.
    Question,
    /// You answer a question; set `in_reply_to`.
    Answer,
    /// An agreement, such as an interface both sides implement.
    Contract,
    /// A summary of finished work.
    Report,
    /// Something is wrong and needs attention.
    Alert,
    /// Anything else.
    Note,
}

impl From<KindArg> for MessageKind {
    fn from(kind: KindArg) -> Self {
        match kind {
            KindArg::Question => Self::Question,
            KindArg::Answer => Self::Answer,
            KindArg::Contract => Self::Contract,
            KindArg::Report => Self::Report,
            KindArg::Alert => Self::Alert,
            KindArg::Note => Self::Note,
        }
    }
}

/// Something a message points at. Set exactly one of `file`, `commit` or
/// `url`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct RefArg {
    /// A file path, relative to your workspace.
    #[serde(default)]
    pub file: Option<String>,
    /// A one-based line in `file`.
    #[serde(default)]
    pub line: Option<u32>,
    /// A commit id.
    #[serde(default)]
    pub commit: Option<String>,
    /// A web address, for example a pull request.
    #[serde(default)]
    pub url: Option<String>,
}

impl RefArg {
    fn into_reference(self, repo: Option<&str>) -> Option<Reference> {
        if let Some(path) = self.file {
            return Some(Reference::File {
                path,
                line: self.line,
            });
        }
        if let Some(sha) = self.commit {
            return Some(Reference::Commit {
                repo: repo.map(str::to_owned),
                sha,
            });
        }
        self.url.map(|url| {
            if url.contains("/pull/") || url.contains("/merge_requests/") {
                Reference::PullRequest { url }
            } else {
                Reference::Url { url }
            }
        })
    }
}

/// Parameters of `send_message`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendArgs {
    /// Recipients: agent ids (`ag_...`), agent names as shown by
    /// `list_agents`, or `human`.
    pub to: Vec<String>,
    /// The message, in Markdown. Be specific: the recipient may be busy.
    pub body: String,
    /// What the message is for. Defaults to `note`.
    #[serde(default)]
    pub kind: Option<KindArg>,
    /// The id of the message you answer (`msg_...`), so the conversation
    /// stays in one thread.
    #[serde(default)]
    pub in_reply_to: Option<String>,
    /// Files, commits or links the message refers to.
    #[serde(default)]
    pub refs: Vec<RefArg>,
}

/// Parameters of `read_messages`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadArgs {
    /// Only messages you have not read yet. Defaults to true.
    #[serde(default = "default_true")]
    pub unread_only: bool,
}

const fn default_true() -> bool {
    true
}

/// How a task ended.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OutcomeArg {
    /// The task is finished.
    Done,
    /// You cannot continue without help.
    Blocked,
    /// The task failed.
    Failed,
}

/// Parameters of `spawn_worker`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SpawnArgs {
    /// The repository, as the mission lists it.
    pub repo: String,
    /// A new branch for the worker, such as `feat/invoices`.
    pub branch: String,
    /// The worker's task. It knows only what you write: the context, what to
    /// change or find, the acceptance criteria and how to check the work.
    pub task: String,
    /// A short display name, such as `billing-invoices`.
    #[serde(default)]
    pub name: Option<String>,
    /// The branch to start from; the repository's default branch when
    /// absent.
    #[serde(default)]
    pub base: Option<String>,
}

/// Parameters of `report`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReportArgs {
    /// How the task ended.
    pub outcome: OutcomeArg,
    /// What you did, what is left, and anything the reader must check. A few
    /// lines of Markdown.
    pub summary: String,
    /// Commits, pull requests or files that prove the work.
    #[serde(default)]
    pub refs: Vec<RefArg>,
}

impl VoleeMcp {
    /// Builds the server for agent `me`, subscribing `client` to the daemon's
    /// events.
    pub async fn connect(me: AgentId, mut client: Client) -> Result<Self, McpError> {
        let replica = Replica::subscribe(&mut client).await?;
        if replica.read().await.agent(me).is_none() {
            return Err(McpError::UnknownAgent(me));
        }
        Ok(Self::new(me, client, replica, false))
    }

    /// Joins volee as an agent of another harness (ADR 0014), then builds
    /// its server. The agent stays alive while `client`'s connection is
    /// open, so keep this server for as long as the harness runs.
    pub async fn join(params: JoinParams, mut client: Client) -> Result<Self, McpError> {
        let joined = client.call::<Join>(params).await?;
        let replica = Replica::subscribe(&mut client).await?;
        Ok(Self::new(joined.agent, client, replica, true))
    }

    fn new(me: AgentId, client: Client, replica: Replica, joined: bool) -> Self {
        Self {
            inner: Arc::new(Inner {
                me,
                client,
                replica,
                joined,
            }),
            tool_router: Self::tool_router(),
        }
    }

    /// The agent this server speaks for.
    pub fn agent(&self) -> AgentId {
        self.inner.me
    }

    /// Records a tool call as the activity of a joined agent; agents volee
    /// launched report through their hooks instead. A failure only loses
    /// the line.
    async fn note(&self, activity: impl Into<String>) {
        if !self.inner.joined {
            return;
        }
        let params = ActivityParams {
            agent: self.inner.me,
            activity: activity.into(),
        };
        // The tool's answer matters more than the activity line: a daemon
        // that cannot record it fails the next call on its own.
        let _ = self.inner.client.call::<RecordActivity>(params).await;
    }

    async fn send(&self, params: SendParams) -> Result<CallToolResult, ErrorData> {
        match self.inner.client.call::<SendMessage>(params).await {
            Ok(sent) => Ok(text(&format!(
                "Sent message {} in thread {}.",
                sent.message, sent.thread
            ))),
            // Refusals and unknown names are the model's to fix: tell it why.
            Err(ClientError::Rpc(error)) => Ok(CallToolResult::error(vec![ContentBlock::text(
                error.message,
            )])),
            Err(error) => Err(ErrorData::internal_error(error.to_string(), None)),
        }
    }
}

#[tool_router]
impl VoleeMcp {
    /// Your identity in volee.
    #[tool(
        description = "Who you are in volee: your agent id, name, role, mission, parent and \
                       workspace (repository, branch, worktree)."
    )]
    pub async fn whoami(&self) -> Result<CallToolResult, ErrorData> {
        self.note("called whoami").await;
        let registry = self.inner.replica.read().await;
        let Some(me) = registry.agent(self.inner.me) else {
            return Err(ErrorData::internal_error(
                "this agent was forgotten by volee",
                None,
            ));
        };
        json(&WhoAmI {
            agent: AgentView::of(&registry, me, None),
            cwd: me.workspace.cwd.display().to_string(),
            worktree: me
                .workspace
                .worktree
                .as_ref()
                .map(|path| path.display().to_string()),
            base_branch: me.workspace.base.clone(),
            hint: "Use list_agents to find who to talk to, and report when your task ends.",
        })
    }

    /// Every agent and whether you may write to it.
    #[tool(
        description = "Every agent volee knows on this machine, the ones that need attention \
                       first: id, name, role, state, repository, branch, mission, and whether you \
                       may send it a message (can_message)."
    )]
    pub async fn list_agents(
        &self,
        Parameters(args): Parameters<ListArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.note("listed the agents").await;
        let registry = self.inner.replica.read().await;
        let agents: Vec<AgentView> = registry
            .agents_by_urgency()
            .into_iter()
            .filter(|agent| {
                args.include_finished
                    || !matches!(
                        agent.state,
                        AgentState::Stopped | AgentState::Failed | AgentState::Lost
                    )
            })
            .map(|agent| AgentView::of(&registry, agent, Some(self.inner.me)))
            .collect();
        json(&agents)
    }

    /// Writes to agents or to the human.
    #[tool(
        description = "Send a message to one or more agents or to the human. Recipients are agent \
                       ids, agent names from list_agents, or \"human\". Inside a mission you may \
                       write to every member; otherwise to your parent, your children, your \
                       coordinator and the human. A working recipient reads it at the end of its \
                       turn; an idle one wakes up."
    )]
    pub async fn send_message(
        &self,
        Parameters(args): Parameters<SendArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.note(format!("wrote to {}", args.to.join(", "))).await;
        let (to, in_reply_to, repo) = {
            let registry = self.inner.replica.read().await;
            let mut to = Vec::new();
            for name in &args.to {
                match resolve(&registry, self.inner.me, name) {
                    Ok(party) => to.push(party),
                    Err(reason) => return Ok(refuse(&reason)),
                }
            }
            let in_reply_to = match args
                .in_reply_to
                .as_deref()
                .map(str::parse::<volee_core::MessageId>)
            {
                Some(Ok(id)) => Some(id),
                Some(Err(error)) => return Ok(refuse(&error.to_string())),
                None => None,
            };
            let repo = registry
                .agent(self.inner.me)
                .and_then(|me| me.workspace.repo.clone());
            (to, in_reply_to, repo)
        };
        if to.is_empty() {
            return Ok(refuse("name at least one recipient in `to`"));
        }
        self.send(SendParams {
            from: Party::Agent(self.inner.me),
            to,
            kind: args.kind.map_or(MessageKind::Note, MessageKind::from),
            body: args.body,
            refs: args
                .refs
                .into_iter()
                .filter_map(|r| r.into_reference(repo.as_deref()))
                .collect(),
            in_reply_to,
            thread: None,
        })
        .await
    }

    /// Reads messages addressed to you and marks them read.
    #[tool(
        description = "Messages addressed to you, oldest first, with their sender, kind and id. \
                       Returned messages are marked as read. Reply with send_message and \
                       in_reply_to set to the message id."
    )]
    pub async fn read_messages(
        &self,
        Parameters(args): Parameters<ReadArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.note("read its messages").await;
        let me = Party::Agent(self.inner.me);
        let (views, unread) = {
            let registry = self.inner.replica.read().await;
            let mut views = Vec::new();
            let mut unread = Vec::new();
            for thread in registry.threads() {
                for message in registry.thread_messages(thread.id) {
                    let Some(receipt) = message.receipt(me) else {
                        continue;
                    };
                    let is_unread = receipt.read_at.is_none();
                    if args.unread_only && !is_unread {
                        continue;
                    }
                    if is_unread {
                        unread.push(message.id);
                    }
                    views.push((message.id, MessageView::of(&registry, message)));
                }
            }
            views.sort_by_key(|(id, _)| *id);
            (views, unread)
        };
        for message in unread {
            if let Err(error) = self
                .inner
                .client
                .call::<MarkRead>(MarkReadParams { message, by: me })
                .await
            {
                return Err(ErrorData::internal_error(error.to_string(), None));
            }
        }
        let views: Vec<MessageView> = views.into_iter().map(|(_, view)| view).collect();
        if views.is_empty() {
            return Ok(text("No messages."));
        }
        json(&views)
    }

    /// Reports the end of your task to whoever launched you.
    #[tool(
        description = "Report that your task is done, blocked or failed. The report goes to the \
                       agent that launched you, your mission's coordinator and the human. Include \
                       what you did, what is left, and refs to commits or pull requests."
    )]
    pub async fn report(
        &self,
        Parameters(args): Parameters<ReportArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.note("sent a report").await;
        let (to, repo) = {
            let registry = self.inner.replica.read().await;
            let Some(me) = registry.agent(self.inner.me) else {
                return Err(ErrorData::internal_error(
                    "this agent was forgotten by volee",
                    None,
                ));
            };
            let mut to = vec![Party::Human];
            if let Some(parent) = me.parent.filter(|id| registry.agent(*id).is_some()) {
                to.push(Party::Agent(parent));
            }
            if let Some(coordinator) = me
                .mission
                .and_then(|id| registry.mission(id))
                .and_then(|mission| mission.coordinator)
                .filter(|id| *id != me.id && registry.agent(*id).is_some())
                && !to.contains(&Party::Agent(coordinator))
            {
                to.push(Party::Agent(coordinator));
            }
            (to, me.workspace.repo.clone())
        };
        let outcome = match args.outcome {
            OutcomeArg::Done => "Done",
            OutcomeArg::Blocked => "Blocked",
            OutcomeArg::Failed => "Failed",
        };
        self.send(SendParams {
            from: Party::Agent(self.inner.me),
            to,
            kind: if matches!(args.outcome, OutcomeArg::Done) {
                MessageKind::Report
            } else {
                MessageKind::Alert
            },
            body: format!("**{outcome}.** {}", args.summary),
            refs: args
                .refs
                .into_iter()
                .filter_map(|r| r.into_reference(repo.as_deref()))
                .collect(),
            in_reply_to: None,
            thread: None,
        })
        .await
    }

    /// Launches a worker for the mission this agent coordinates.
    #[tool(
        description = "For a mission's coordinator: launch a worker, a new agent on a new branch \
                       in its own git worktree of one of the mission's repositories, with its \
                       task. volee tells you when the worker stops, with the evidence it checked."
    )]
    pub async fn spawn_worker(
        &self,
        Parameters(args): Parameters<SpawnArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.note("asked to launch a worker").await;
        let mission = {
            let registry = self.inner.replica.read().await;
            let Some(me) = registry.agent(self.inner.me) else {
                return Err(ErrorData::internal_error(
                    "this agent was forgotten by volee",
                    None,
                ));
            };
            let Some(mission) = me.mission.and_then(|id| registry.mission(id)) else {
                return Ok(refuse(
                    "you are not part of a mission: only a mission's coordinator launches workers",
                ));
            };
            if mission.coordinator != Some(me.id) {
                return Ok(refuse(
                    "only the mission's coordinator launches workers; ask it with send_message",
                ));
            }
            mission.id
        };
        let spawned = self
            .inner
            .client
            .call::<Spawn>(SpawnParams {
                repo: args.repo,
                branch: args.branch,
                base: args.base,
                task: args.task,
                role: Role::Worker,
                parent: Some(self.inner.me),
                mission: Some(mission),
                model: None,
                name: args.name,
                harness: None,
            })
            .await;
        match spawned {
            Ok(spawned) => Ok(text(&format!(
                "Launched worker {} in {}. It reports to you; volee tells you when it stops.",
                spawned.agent, spawned.worktree
            ))),
            Err(ClientError::Rpc(error)) => Ok(refuse(&error.message)),
            Err(error) => Err(ErrorData::internal_error(error.to_string(), None)),
        }
    }

    /// Closes the mission this agent coordinates.
    #[tool(
        description = "Close the mission you coordinate. Call it once volee told you every worker \
                       delivered and after you sent the final report to the human with the report \
                       tool. volee refuses while a worker has not delivered, and says which."
    )]
    pub async fn close_mission(&self) -> Result<CallToolResult, ErrorData> {
        self.note("asked to close its mission").await;
        let mission = {
            let registry = self.inner.replica.read().await;
            let Some(me) = registry.agent(self.inner.me) else {
                return Err(ErrorData::internal_error(
                    "this agent was forgotten by volee",
                    None,
                ));
            };
            let Some(mission) = me.mission.and_then(|id| registry.mission(id)) else {
                return Ok(refuse("you are not part of a mission"));
            };
            if mission.coordinator != Some(me.id) {
                return Ok(refuse(
                    "only the mission's coordinator closes it; if your task is over, use the \
                     report tool",
                ));
            }
            mission.id
        };
        let closed = self
            .inner
            .client
            .call::<CloseMission>(CloseParams {
                mission,
                report: None,
                force: false,
                by: Some(self.inner.me),
            })
            .await;
        match closed {
            Ok(_) => Ok(text(
                "The mission is closed and the human has your report. You can stop now.",
            )),
            Err(ClientError::Rpc(error)) => Ok(refuse(&error.message)),
            Err(error) => Err(ErrorData::internal_error(error.to_string(), None)),
        }
    }
}

// The macro expands to async methods that do not always await.
#[allow(clippy::unused_async_trait_impl)]
#[tool_handler(router = self.tool_router)]
impl ServerHandler for VoleeMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("volee", volee_core::VERSION))
            .with_instructions(if self.inner.joined {
                JOINED_INSTRUCTIONS
            } else {
                INSTRUCTIONS
            })
    }
}

/// Resolves a recipient written by a model: `human`, an agent id, or an
/// agent name (case-insensitive, exact).
pub(crate) fn resolve(registry: &Registry, me: AgentId, input: &str) -> Result<Party, String> {
    let input = input.trim();
    if ["human", "the human", "you", "user"]
        .iter()
        .any(|word| input.eq_ignore_ascii_case(word))
    {
        return Ok(Party::Human);
    }
    if let Ok(id) = input.parse::<AgentId>() {
        return if registry.agent(id).is_some() {
            Ok(Party::Agent(id))
        } else {
            Err(format!("no agent has the id {id}; call list_agents"))
        };
    }
    let matches: Vec<_> = registry
        .agents()
        .filter(|agent| agent.id != me && agent.name.eq_ignore_ascii_case(input))
        .collect();
    match matches.as_slice() {
        [agent] => Ok(Party::Agent(agent.id)),
        [] => Err(format!(
            "no agent is named \"{input}\"; call list_agents to see names and ids"
        )),
        several => Err(format!(
            "several agents are named \"{input}\": {}; use an id instead",
            several
                .iter()
                .map(|agent| party_name(registry, Party::Agent(agent.id)))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn json<T: Serialize>(value: &T) -> Result<CallToolResult, ErrorData> {
    serde_json::to_string_pretty(value)
        .map(|body| CallToolResult::success(vec![ContentBlock::text(body)]))
        .map_err(|error| ErrorData::internal_error(error.to_string(), None))
}

fn text(body: &str) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(body)])
}

fn refuse(reason: &str) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(reason)])
}
