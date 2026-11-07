//! Every method of the protocol, with its parameters and result.
//!
//! A method is a type implementing [`Method`]; clients call
//! [`Client::call::<M>`](crate::client::Client::call) and get a typed result.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use volee_core::Event;
use volee_core::delivery::TidyRefusal;
use volee_core::harness::HarnessKind;
use volee_core::ids::{AgentId, MessageId, MissionId, ThreadId};
use volee_core::message::{MessageKind, Party, Reference};
use volee_core::mission::MissionKind;
use volee_core::role::Role;
use volee_core::time::Timestamp;

/// A protocol method.
pub trait Method {
    /// The method name on the wire.
    const NAME: &'static str;
    /// Its parameters.
    type Params: Serialize + DeserializeOwned + Send + 'static;
    /// Its result.
    type Output: Serialize + DeserializeOwned + Send + 'static;
}

macro_rules! method {
    ($(#[$meta:meta])* $name:ident, $wire:literal, $params:ty, $output:ty) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy)]
        pub struct $name;

        impl Method for $name {
            const NAME: &'static str = $wire;
            type Params = $params;
            type Output = $output;
        }
    };
}

/// No parameters or no result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Empty {}

/// An event with its sequence number in the log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeqEvent {
    /// Its position in the log, strictly increasing.
    pub seq: u64,
    /// The event.
    pub event: Event,
}

/// The name of the notification that carries new events to subscribers. Its
/// params are a [`SeqEvent`].
pub const EVENTS_APPENDED: &str = "events.appended";

// ---- daemon ---------------------------------------------------------------

/// Parameters of [`Status`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusParams {
    /// The protocol version the client speaks.
    pub protocol: u32,
}

/// What the daemon knows about a harness on this machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessStatus {
    /// The harness.
    pub kind: HarnessKind,
    /// Whether volee can use it.
    pub available: bool,
    /// Its version, when known.
    pub version: Option<String>,
    /// Why it is unavailable, or a note.
    pub detail: Option<String>,
}

/// What the daemon knows about the forge client it asks for pull requests,
/// such as the GitHub CLI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeStatus {
    /// The program, such as `gh`.
    pub program: String,
    /// Whether volee can use it: installed and signed in.
    pub available: bool,
    /// Its version, when known.
    pub version: Option<String>,
    /// Why it is unavailable, or a note.
    pub detail: Option<String>,
}

/// The result of [`Status`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusInfo {
    /// The daemon's volee version.
    pub version: String,
    /// The daemon's protocol version.
    pub protocol: u32,
    /// The daemon's process id.
    pub pid: u32,
    /// When the daemon started.
    pub started_at: Timestamp,
    /// The last event in the log.
    pub last_seq: Option<u64>,
    /// Agents in the registry.
    pub agents: usize,
    /// Harnesses found on this machine.
    pub harnesses: Vec<HarnessStatus>,
    /// The forge client for pull requests; `None` when the daemon has none,
    /// or has not checked it yet.
    #[serde(default)]
    pub forge: Option<ForgeStatus>,
}

method!(
    /// Daemon version, health and counters. Also checks protocol versions.
    Status,
    "daemon.status",
    StatusParams,
    StatusInfo
);

method!(
    /// Asks the daemon to stop after answering.
    Shutdown,
    "daemon.shutdown",
    Empty,
    Empty
);

// ---- events ---------------------------------------------------------------

/// Parameters of [`EventsSince`] and [`Subscribe`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SinceParams {
    /// Return events after this sequence number; from the start when absent.
    pub after: Option<u64>,
}

/// The result of [`EventsSince`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventBatch {
    /// The events, in log order.
    pub events: Vec<SeqEvent>,
    /// The last sequence number in the log when the batch was taken.
    pub last_seq: Option<u64>,
}

method!(
    /// Every event after a sequence number. Clients replay them into their
    /// own registry replica.
    EventsSince,
    "events.since",
    SinceParams,
    EventBatch
);

method!(
    /// Like [`EventsSince`], then keeps the connection subscribed: every new
    /// event arrives as an [`EVENTS_APPENDED`] notification, in order and
    /// without gaps.
    Subscribe,
    "events.subscribe",
    SinceParams,
    EventBatch
);

// ---- agents ---------------------------------------------------------------

/// Parameters naming one agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentParams {
    /// The agent.
    pub agent: AgentId,
}

method!(
    /// Forgets an agent: removes it from the registry and ignores its
    /// sessions from now on. Never stops a process, never deletes files.
    Forget,
    "agents.forget",
    AgentParams,
    Empty
);

/// Parameters of [`Tidy`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TidyParams {
    /// The agent whose worktree to remove.
    pub agent: AgentId,
    /// Remove even without delivery evidence. Never deletes the branch.
    #[serde(default)]
    pub force: bool,
}

/// The result of [`Tidy`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "outcome")]
pub enum TidyOutcome {
    /// The worktree was removed; the branch was kept.
    Removed {
        /// The removed worktree.
        worktree: String,
    },
    /// Nothing was removed, for these reasons.
    Kept {
        /// Why.
        reasons: Vec<TidyRefusal>,
    },
}

method!(
    /// Removes an agent's worktree when delivery evidence allows it
    /// (ADR 0011).
    Tidy,
    "agents.tidy",
    TidyParams,
    TidyOutcome
);

/// Parameters of [`Spawn`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnParams {
    /// A repository name from the configuration, or a path to a checkout.
    pub repo: String,
    /// The branch to create for the agent.
    pub branch: String,
    /// The base branch; the repository's default branch when absent.
    pub base: Option<String>,
    /// The task, written as the agent's first prompt.
    pub task: String,
    /// The agent's role; usually `worker`.
    pub role: Role,
    /// The agent that asks for this one.
    pub parent: Option<AgentId>,
    /// The mission it works for.
    pub mission: Option<MissionId>,
    /// The model; the harness default when absent.
    pub model: Option<String>,
    /// A display name; derived from the repository and branch when absent.
    pub name: Option<String>,
}

/// The result of [`Spawn`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spawned {
    /// The new agent.
    pub agent: AgentId,
    /// Its worktree.
    pub worktree: String,
}

method!(
    /// Creates a worktree and launches an agent in it.
    Spawn,
    "agents.spawn",
    SpawnParams,
    Spawned
);

/// The result of [`Preview`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewText {
    /// The agent's recent terminal output, with ANSI sequences, to be
    /// rendered by a terminal emulator.
    pub screen: String,
}

method!(
    /// The agent's recent terminal output. Refused for an agent in a terminal
    /// of its own (an interactive session), which only that terminal shows.
    Preview,
    "agents.preview",
    AgentParams,
    PreviewText
);

/// Parameters of [`Diff`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffParams {
    /// The agent.
    pub agent: AgentId,
    /// The full patch instead of statistics.
    #[serde(default)]
    pub full: bool,
}

/// The result of [`Diff`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffText {
    /// A unified diff, or `git diff --stat` output.
    pub text: String,
    /// Whether the diff was longer than the daemon returns and `text` holds
    /// only its beginning, cut at a line break.
    #[serde(default)]
    pub truncated: bool,
}

method!(
    /// The agent's changes against the merge base with its base branch.
    Diff,
    "agents.diff",
    DiffParams,
    DiffText
);

/// The result of [`Attach`]: a command the client runs with its own
/// terminal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachCommand {
    /// The program.
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// Its working directory.
    pub cwd: Option<String>,
    /// Environment variables to remove before running it: the ones that
    /// would tie it to another agent's session.
    #[serde(default)]
    pub env_remove: Vec<String>,
    /// A sentence telling the person how to come back.
    pub hint: String,
}

method!(
    /// How to take over an agent in the client's terminal. Refused for an
    /// agent in a terminal of its own: switch to that terminal instead.
    Attach,
    "agents.attach",
    AgentParams,
    AttachCommand
);

method!(
    /// Stops an agent's session; its conversation is kept and can be
    /// resumed. Refused for agents volee did not launch (ADR 0011).
    Stop,
    "agents.stop",
    AgentParams,
    Empty
);

// ---- messages -------------------------------------------------------------

/// Parameters of [`SendMessage`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendParams {
    /// The sender.
    pub from: Party,
    /// The recipients.
    pub to: Vec<Party>,
    /// What the message is for.
    pub kind: MessageKind,
    /// Markdown text.
    pub body: String,
    /// What it points at.
    #[serde(default)]
    pub refs: Vec<Reference>,
    /// The message it answers.
    #[serde(default)]
    pub in_reply_to: Option<MessageId>,
    /// The thread; chosen by the daemon when absent (the mission thread, or
    /// the direct thread between the two parties).
    #[serde(default)]
    pub thread: Option<ThreadId>,
}

/// The result of [`SendMessage`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sent {
    /// The new message.
    pub message: MessageId,
    /// Its thread.
    pub thread: ThreadId,
}

method!(
    /// Sends a message after checking the routing rule (ADR 0006).
    SendMessage,
    "messages.send",
    SendParams,
    Sent
);

/// Parameters of [`MarkRead`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkReadParams {
    /// The message.
    pub message: MessageId,
    /// The reader.
    pub by: Party,
}

method!(
    /// Marks a message as read.
    MarkRead,
    "messages.read",
    MarkReadParams,
    Empty
);

// ---- threads --------------------------------------------------------------

/// Parameters of [`Summarize`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummarizeParams {
    /// The thread.
    pub thread: ThreadId,
}

/// The result of [`Summarize`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadSummary {
    /// The summary, as plain text.
    pub summary: String,
    /// How many messages it covers.
    pub messages: usize,
    /// The model that wrote it.
    pub model: String,
}

method!(
    /// Summarizes a thread with a model. Refused unless the person turned
    /// summaries on in the configuration.
    Summarize,
    "threads.summarize",
    SummarizeParams,
    ThreadSummary
);

// ---- missions -------------------------------------------------------------

/// Parameters of [`CreateMission`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateMissionParams {
    /// A short title.
    pub title: String,
    /// What the mission delivers.
    pub kind: MissionKind,
    /// The goal, as the coordinator will read it.
    pub goal: String,
    /// The repositories involved, by configuration name.
    pub repos: Vec<String>,
    /// The agent asking for the mission, if an agent does.
    #[serde(default)]
    pub requested_by: Option<AgentId>,
}

/// The result of [`CreateMission`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedMission {
    /// The new mission.
    pub mission: MissionId,
    /// Its conversation thread.
    pub thread: ThreadId,
    /// Its coordinator, when volee could launch one.
    #[serde(default)]
    pub coordinator: Option<AgentId>,
}

method!(
    /// Creates a mission and its thread, in the planning status.
    CreateMission,
    "missions.create",
    CreateMissionParams,
    CreatedMission
);

/// Parameters of [`AbortMission`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbortParams {
    /// The mission.
    pub mission: MissionId,
    /// Why, told to its agents.
    #[serde(default)]
    pub reason: Option<String>,
}

method!(
    /// Aborts a mission and tells its agents to stop. Nothing is deleted.
    AbortMission,
    "missions.abort",
    AbortParams,
    Empty
);

/// Parameters of [`CloseMission`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseParams {
    /// The mission.
    pub mission: MissionId,
    /// The final report, a message in the mission's thread. Without it, the
    /// coordinator's latest report in that thread is used.
    #[serde(default)]
    pub report: Option<MessageId>,
    /// Close even if some workers have not delivered. Only the human can.
    #[serde(default)]
    pub force: bool,
    /// The agent closing the mission, when an agent does. It must be the
    /// mission's coordinator and must have sent its report.
    #[serde(default)]
    pub by: Option<AgentId>,
}

method!(
    /// Closes a mission whose workers all delivered (ADR 0007).
    CloseMission,
    "missions.close",
    CloseParams,
    Empty
);

// ---- hooks ----------------------------------------------------------------

/// Parameters of [`HookIngest`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HookParams {
    /// The agent the hook belongs to.
    pub agent: AgentId,
    /// The hook event, in kebab case (`session-start`, `stop`...).
    pub event: String,
    /// The JSON payload the harness passed to the hook.
    pub payload: serde_json::Value,
}

/// The result of [`HookIngest`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookReply {
    /// What the hook command must print on stdout, if anything. For a `Stop`
    /// hook with pending messages, this is the JSON that hands them to the
    /// agent.
    pub stdout: Option<String>,
}

method!(
    /// A hook event from an agent volee launched.
    HookIngest,
    "hooks.ingest",
    HookParams,
    HookReply
);

/// Parameters of [`HookWait`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitParams {
    /// The idle agent waiting for a message.
    pub agent: AgentId,
    /// How long to wait before giving up, in milliseconds.
    pub timeout_ms: u64,
}

/// The result of [`HookWait`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitReply {
    /// The messages to hand to the agent, or nothing when the wait timed out
    /// or another wait replaced this one.
    pub text: Option<String>,
}

method!(
    /// Blocks until a message for an idle agent arrives (the `asyncRewake`
    /// Stop hook, ADR 0006).
    HookWait,
    "hooks.wait",
    WaitParams,
    WaitReply
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_names_are_unique_and_namespaced() {
        let names = [
            Status::NAME,
            Shutdown::NAME,
            EventsSince::NAME,
            Subscribe::NAME,
            Forget::NAME,
            Tidy::NAME,
            Spawn::NAME,
            Preview::NAME,
            Diff::NAME,
            Attach::NAME,
            Stop::NAME,
            SendMessage::NAME,
            MarkRead::NAME,
            HookIngest::NAME,
            HookWait::NAME,
            Summarize::NAME,
            CreateMission::NAME,
            AbortMission::NAME,
            CloseMission::NAME,
        ];
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
        assert!(names.iter().all(|name| name.contains('.')));
    }

    #[test]
    fn tidy_outcome_is_tagged() {
        let kept = TidyOutcome::Kept {
            reasons: vec![TidyRefusal::NotDelivered],
        };
        let json = serde_json::to_value(&kept).unwrap();
        assert_eq!(json["outcome"], "kept");
        assert_eq!(json["reasons"][0]["reason"], "not-delivered");
    }
}
