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
use volee_core::{AgentId, Event, MessageId};

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
    ) -> impl Future<Output = Result<String, SourceError>> + Send;

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
}

/// The state at subscription time and the events that follow it.
#[derive(Debug)]
pub struct Subscription {
    /// Events that build the current state, oldest first.
    pub snapshot: Vec<Event>,
    /// Every later event, in order. A source that stops sends why, as an
    /// error, before it closes the channel.
    pub updates: mpsc::Receiver<Result<Event, SourceError>>,
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
}

/// How to take over an agent: a program that runs in this terminal until
/// the person comes back to volee.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TakeOver {
    /// The program, such as `claude`.
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// Where it runs; volee's own directory when absent.
    pub cwd: Option<PathBuf>,
    /// A sentence telling the person how to come back, printed before the
    /// program starts.
    pub hint: Option<String>,
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
