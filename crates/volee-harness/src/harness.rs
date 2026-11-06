//! The interface the daemon uses to drive any harness.

use std::fmt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde_json::Value;
use volee_core::harness::{HarnessKind, SessionKey};
use volee_core::ids::AgentId;
use volee_core::reconcile::Listing;

use crate::error::HarnessError;
use crate::spec::{SpawnSpec, Spawned};
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

/// A coding harness, as the daemon sees it.
///
/// Every method blocks until the harness's command finishes (each one has a
/// timeout). Implementations are shared between threads.
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
    fn stop(&self, session: &SessionKey) -> Result<(), HarnessError>;

    /// Deletes a background session.
    fn remove(&self, session: &SessionKey) -> Result<(), HarnessError>;

    /// The session's recent terminal output, as raw bytes with terminal
    /// escape sequences, for previews.
    fn logs(&self, session: &SessionKey) -> Result<Vec<u8>, HarnessError>;

    /// The command that opens the session in the current terminal, for
    /// taking it over. The caller runs it with inherited standard streams
    /// after releasing the terminal.
    fn attach_command(&self, session: &SessionKey) -> Result<Command, HarnessError>;

    /// The per-agent settings that connect the agent's hooks to volee,
    /// written to the file passed as [`SpawnSpec::settings_file`].
    ///
    /// `volee_bin` is the absolute path of the `volee` binary the hooks run;
    /// `rewake_timeout` bounds how long an idle agent's message waiter may
    /// wait before the harness ends it.
    fn agent_settings(
        &self,
        volee_bin: &Path,
        agent: AgentId,
        rewake_timeout: Duration,
    ) -> Result<Value, HarnessError>;

    /// The per-agent MCP configuration that gives the agent volee's tools,
    /// written to the file passed as [`SpawnSpec::mcp_config_file`].
    fn mcp_config(&self, volee_bin: &Path, agent: AgentId) -> Result<Value, HarnessError>;
}
