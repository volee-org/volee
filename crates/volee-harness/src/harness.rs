//! The interface the daemon uses to drive any harness.

use std::fmt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde_json::Value;
use volee_core::harness::{HarnessKind, SessionKey};
use volee_core::ids::AgentId;
use volee_core::reconcile::{ListedSession, Listing};

use crate::error::HarnessError;
use crate::spec::{Completion, SpawnSpec, Spawned};
use crate::version::Version;

/// What an installed harness can do. The daemon offers only these actions
/// and degrades to read-only discovery when launching is not supported
/// ([ADR 0002](https://volee-org.github.io/volee/adr/0002-harness-owns-processes.html)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// The installed version.
    pub version: Version,
    /// Lists every session on the machine, including the ones volee did not
    /// launch.
    pub list_sessions: bool,
    /// Launches, resumes, stops, removes, previews and attaches background
    /// sessions.
    pub background_sessions: bool,
}

/// What [`Harness::remove`] may do when the session works in a git worktree,
/// which the harness may delete along with the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreeRemoval {
    /// Refuse to remove a session whose directory is inside a linked git
    /// worktree.
    Refuse,
    /// The work in the session's worktree is delivered: the daemon passes
    /// this only after `volee_core::delivery::tidy_verdict` allowed tidying
    /// the agent
    /// ([ADR 0011](https://volee-org.github.io/volee/adr/0011-safety-and-delivery-evidence.html)).
    Delivered,
}

/// A coding harness, as the daemon sees it.
///
/// Every method blocks until the harness's command finishes (each one has a
/// timeout). Implementations are shared between threads.
///
/// Sessions are acted on as the harness last listed them: the listing gives
/// the handle the harness takes in its commands. A session without a handle
/// (for Claude Code, an interactive session) cannot be stopped, removed,
/// previewed or attached: [`HarnessError::NotControllable`].
///
/// Safety rules stay with the caller: volee stops and removes only sessions
/// it launched, and never on a guess
/// ([ADR 0011](https://volee-org.github.io/volee/adr/0011-safety-and-delivery-evidence.html)).
pub trait Harness: fmt::Debug + Send + Sync {
    /// Which harness this is.
    fn kind(&self) -> HarnessKind;

    /// Checks the installed harness and reports what it supports, or why it
    /// cannot be used (not installed, too old).
    fn capabilities(&self) -> Result<Capabilities, HarnessError>;

    /// Every session the harness knows on this machine, with its state
    /// mapped to volee's.
    fn list(&self) -> Result<Listing, HarnessError>;

    /// Launches a background session for a new agent.
    fn spawn(&self, spec: &SpawnSpec) -> Result<Spawned, HarnessError>;

    /// Continues a stopped session with a new prompt, in the background. The
    /// harness may continue the same session or fork it; the result says
    /// which.
    fn resume(&self, session: &SessionKey, spec: &SpawnSpec) -> Result<Spawned, HarnessError>;

    /// Stops a background session. Its conversation is kept and can be
    /// resumed.
    fn stop(&self, session: &ListedSession) -> Result<(), HarnessError>;

    /// Deletes a background session. A session that works in a git worktree
    /// is removed only with [`WorktreeRemoval::Delivered`].
    fn remove(
        &self,
        session: &ListedSession,
        worktree: WorktreeRemoval,
    ) -> Result<(), HarnessError>;

    /// The session's recent terminal output, as raw bytes with terminal
    /// escape sequences, for previews.
    fn logs(&self, session: &ListedSession) -> Result<Vec<u8>, HarnessError>;

    /// The command that opens the session in the current terminal, for
    /// taking it over. The caller runs it with inherited standard streams
    /// after releasing the terminal.
    fn attach_command(&self, session: &ListedSession) -> Result<Command, HarnessError>;

    /// The per-agent settings that connect the agent's hooks to volee and
    /// allow its volee tools, written to the file passed as
    /// [`SpawnSpec::settings_file`].
    ///
    /// `volee_bin` is the absolute path of the `volee` binary the hooks run;
    /// `rewake_timeout` bounds how long an idle agent's message waiter waits
    /// (at most a day).
    fn agent_settings(
        &self,
        volee_bin: &Path,
        agent: AgentId,
        rewake_timeout: Duration,
    ) -> Result<Value, HarnessError>;

    /// The per-agent MCP configuration that gives the agent volee's tools,
    /// written to the file passed as [`SpawnSpec::mcp_config_file`].
    fn mcp_config(&self, volee_bin: &Path, agent: AgentId) -> Result<Value, HarnessError>;

    /// Asks the harness's model one question, with no tools, no MCP server,
    /// no hooks and no saved session, and returns its answer. volee uses it
    /// only for what the person opts into, such as thread summaries.
    fn complete(&self, request: &Completion) -> Result<String, HarnessError> {
        let _ = request;
        Err(HarnessError::Unsupported {
            harness: self.kind(),
            what: "answer one-off questions",
        })
    }
}
