//! Where the interface gets its data.
//!
//! The interface keeps its own replica of the [`Registry`](volee_core::Registry),
//! built by applying the same events as the daemon (ADR 0003): a snapshot
//! first, then every new event. Requests that need the outside world (the
//! screen of an agent, a diff, sending a message) go through the source too,
//! so the interface itself does no IO besides the terminal.
//!
//! The daemon client implements [`Source`]; [`DemoSource`](crate::DemoSource)
//! implements it with a simulated set of agents.

use std::future::Future;
use std::process::Command;

use tokio::sync::mpsc;
use volee_core::message::MessageKind;
use volee_core::{AgentId, Event};

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
        to: AgentId,
        kind: MessageKind,
        body: String,
    ) -> impl Future<Output = Result<(), SourceError>> + Send;

    /// The command that takes over an agent in the current terminal, such as
    /// `claude attach <id>`, or `None` when the agent cannot be taken over
    /// from here (for example a session running in another terminal).
    fn attach_command(&self, agent: AgentId) -> Option<Command>;
}

/// The state at subscription time and the events that follow it.
#[derive(Debug)]
pub struct Subscription {
    /// Events that build the current state, oldest first.
    pub snapshot: Vec<Event>,
    /// Every later event, in order. The channel closes when the source stops.
    pub updates: mpsc::Receiver<Event>,
}

/// Why a request to the source failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SourceError {
    /// The source does not know the agent.
    #[error("unknown agent {0}")]
    UnknownAgent(AgentId),
    /// The request does not apply, for example a preview of an agent that has
    /// no terminal.
    #[error("{0}")]
    Unsupported(String),
    /// The source cannot be reached.
    #[error("the source is not reachable: {0}")]
    Disconnected(String),
    /// The request failed.
    #[error("{0}")]
    Failed(String),
}
