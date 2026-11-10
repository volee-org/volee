//! Where the interface gets its data.
//!
//! The interface keeps its own replica of the [`Registry`](volee_core::Registry),
//! built by applying the same events as the daemon (ADR 0003): a snapshot
//! first, then every new event. Requests that need the outside world (the
//! screen of an agent, a diff, sending a message) go through the source too,
//! so the interface itself does no IO besides the terminal.
//!
//! [`DaemonSource`](crate::DaemonSource) implements [`Source`] with the volee
//! daemon; [`DemoSource`](crate::DemoSource) implements it with a simulated
//! set of agents.

use std::future::Future;
use std::path::PathBuf;

use tokio::sync::mpsc;
use volee_core::message::MessageKind;
use volee_core::mission::MissionKind;
use volee_core::{AgentId, Event, MessageId, MissionId, ThreadId};

/// The data side of the interface.
///
/// Requests return futures that the interface runs in background tasks, so a
/// slow request never blocks drawing.
pub trait Source: Send + Sync + 'static {
    /// The events that build the current state, then a channel of every new
    /// event, without a gap between the two.
    fn subscribe(&self) -> impl Future<Output = Result<Subscription, SourceError>> + Send;

    /// The terminal screen of an agent, as raw terminal output: text with
    /// ANSI escape sequences and cursor addressing, as replayed by the
    /// harness (for Claude Code, `claude logs` at about 200x50).
    fn preview(&self, agent: AgentId) -> impl Future<Output = Result<String, SourceError>> + Send;

    /// The changes of an agent against its base branch: a summary
    /// (`git diff --stat`), or the full patch when `full` is true.
    fn diff(
        &self,
        agent: AgentId,
        full: bool,
    ) -> impl Future<Output = Result<Changes, SourceError>> + Send;

    /// Sends a message from the person using volee to an agent. The message
    /// comes back as an event.
    fn send_message(
        &self,
        message: Outgoing,
    ) -> impl Future<Output = Result<(), SourceError>> + Send;

    /// How to take over an agent in the current terminal, such as
    /// `claude attach <id>`. Fails with [`SourceError::Unsupported`] when the
    /// agent cannot be taken over from here, for example a session running
    /// in another terminal.
    fn attach(&self, agent: AgentId) -> impl Future<Output = Result<TakeOver, SourceError>> + Send;

    /// A terminal on an agent, for the interface to show inside its own
    /// screen and type into, `size` being the columns and rows of the panel
    /// that will show it (ADR 0027). Nothing is stopped, forked or resumed
    /// to get it. Fails with [`SourceError::Unsupported`] when the agent has
    /// no terminal the source can reach, with what to do instead.
    fn open(
        &self,
        agent: AgentId,
        size: (u16, u16),
    ) -> impl Future<Output = Result<Door, SourceError>> + Send {
        let _ = (agent, size);
        async {
            Err(SourceError::Unsupported(
                "this source cannot open agents".into(),
            ))
        }
    }

    /// Marks a message to the person as read. The change comes back as an
    /// event.
    fn mark_read(&self, message: MessageId)
    -> impl Future<Output = Result<(), SourceError>> + Send;

    /// A summary of a thread, written by a model. Fails with
    /// [`SourceError::Unsupported`] when summaries are off, with the setting
    /// to change.
    fn summarize(
        &self,
        thread: ThreadId,
    ) -> impl Future<Output = Result<Summary, SourceError>> + Send;

    /// Launches an agent in a new worktree. The agent comes back as events.
    fn spawn(
        &self,
        request: SpawnRequest,
    ) -> impl Future<Output = Result<AgentId, SourceError>> + Send;

    /// Creates a mission, which launches its coordinator. Both come back as
    /// events.
    fn create_mission(
        &self,
        request: MissionRequest,
    ) -> impl Future<Output = Result<NewMission, SourceError>> + Send;

    /// Forgets an agent: the source stops listing it and ignores its
    /// sessions; nothing is stopped or deleted. The removal comes back as an
    /// event. Fails with [`SourceError::Unsupported`] when a rule refuses,
    /// such as an agent volee launched that still runs.
    fn forget(&self, agent: AgentId) -> impl Future<Output = Result<(), SourceError>> + Send;

    /// Moves an agent's conversation into a volee host, so that it can be
    /// taken over (ADR 0023). Fails with [`SourceError::Unsupported`] when the
    /// source cannot, or a rule refuses (an agent that runs in a terminal).
    fn adopt(&self, agent: AgentId) -> impl Future<Output = Result<(), SourceError>> + Send {
        let _ = agent;
        async {
            Err(SourceError::Unsupported(
                "this source cannot adopt agents".into(),
            ))
        }
    }
}

/// The state at subscription time and the events that follow it.
#[derive(Debug)]
pub struct Subscription {
    /// Events that build the current state, oldest first.
    pub snapshot: Vec<Event>,
    /// Every later event, in order. A source that stops sends why, as an
    /// error, before it closes the channel.
    pub updates: mpsc::Receiver<Result<Event, SourceError>>,
    /// Whether volee can use Claude Code on this machine: the daemon found
    /// it, recent enough to list its sessions and launch agents. The daemon
    /// checks when it starts, and `volee daemon status` says why it cannot.
    pub claude_code_available: bool,
}

/// A message from the person using volee to an agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    /// The agent.
    pub to: AgentId,
    /// What the message is for.
    pub kind: MessageKind,
    /// Markdown text.
    pub body: String,
    /// The question it answers, if any.
    pub in_reply_to: Option<MessageId>,
    /// The thread it goes to; the source picks one when absent (the
    /// mission's thread, or the thread between the person and the agent).
    pub thread: Option<ThreadId>,
}

/// A thread's summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    /// The summary, as plain text.
    pub text: String,
    /// How many messages it covers.
    pub messages: usize,
    /// The model that wrote it.
    pub model: String,
}

/// An agent to launch, as the person describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnRequest {
    /// A repository name from the configuration, or a path to a checkout.
    pub repo: String,
    /// The branch to create.
    pub branch: String,
    /// The task, the agent's first prompt.
    pub task: String,
    /// The model; the configured default when absent.
    pub model: Option<String>,
    /// The harness profile (ADR 0022); Claude Code when absent.
    pub harness: Option<String>,
}

/// A mission to create, as the person describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionRequest {
    /// A short title.
    pub title: String,
    /// What it delivers.
    pub kind: MissionKind,
    /// The repositories involved, by configuration name.
    pub repos: Vec<String>,
    /// The goal, as the coordinator will read it.
    pub goal: String,
}

/// A mission just created.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewMission {
    /// The mission.
    pub mission: MissionId,
    /// Its coordinator, when one could be launched.
    pub coordinator: Option<AgentId>,
}

/// An agent's changes against its base branch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// `git diff --stat` output, or a unified diff.
    pub text: String,
    /// Whether `text` holds only the beginning of a longer diff.
    pub truncated: bool,
}

/// Where the interface finds an agent's terminal ([`Source::open`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Door {
    /// A volee host to connect to.
    Host {
        /// Its socket.
        socket: PathBuf,
        /// Whether it is a view on a session that runs elsewhere, as
        /// opposed to the host the agent runs in: leaving a view ends
        /// nothing but the view.
        view: bool,
    },
    /// Not inside the interface: [`Source::attach`] says how to reach it,
    /// such as bringing its multiplexer pane forward.
    Elsewhere,
}

/// How to take over an agent: a program that runs in this terminal until
/// the person comes back to volee, or, for an agent in a terminal
/// multiplexer pane, the steps that bring that pane forward.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TakeOver {
    /// The program, such as `claude`; empty when `focus` is given.
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// Where it runs; volee's own directory when absent.
    pub cwd: Option<PathBuf>,
    /// Environment variables to remove before it starts: the ones that
    /// would tie it to another agent's session.
    pub env_remove: Vec<String>,
    /// A sentence telling the person how to come back, printed before the
    /// program starts.
    pub hint: Option<String>,
    /// For an agent in a multiplexer pane (ADR 0015): the pane to bring
    /// forward, instead of running `program`. The interface keeps the
    /// terminal.
    pub focus: Option<PaneFocus>,
}

/// How to bring forward the multiplexer pane an agent runs in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneFocus {
    /// The multiplexer, such as `tuios`.
    pub multiplexer: String,
    /// Its session that holds the pane, said for people.
    pub session: String,
    /// The commands to run, in order, in volee's own environment: each is a
    /// program and its arguments. The first focuses the pane in its session.
    pub steps: Vec<Vec<String>>,
    /// The command that shows the pane's session in a terminal, for when no
    /// window of the multiplexer shows it, such as `tuios attach work`;
    /// empty when unknown.
    pub attach: Vec<String>,
}

/// Why a request to the source failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SourceError {
    /// The source does not know the agent.
    #[error("unknown agent {0}")]
    UnknownAgent(AgentId),
    /// The request does not apply or is not available, for example a
    /// preview of an agent that has no terminal, or a daemon that does not
    /// implement it yet. The interface says so and does not retry on its own.
    #[error("{0}")]
    Unsupported(String),
    /// The source cannot be reached.
    #[error("{0}")]
    Disconnected(String),
    /// The request failed.
    #[error("{0}")]
    Failed(String),
}
