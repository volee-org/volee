//! The Claude Code adapter.
//!
//! volee drives Claude Code only through its documented commands and hooks,
//! never through its internal files
//! ([ADR 0004](https://volee-org.github.io/volee/adr/0004-sources-and-reconciliation.html)).
//! Measured on Claude Code 2.1.319:
//!
//! | Need | Command |
//! | --- | --- |
//! | Version | `claude --version` |
//! | Every session | `claude agents --json --all` |
//! | Launch | `claude --bg [flags] -- <prompt>` |
//! | Resume | `claude --bg --resume <session-uuid> [flags] -- <prompt>` |
//! | Stop, delete | `claude stop <id>`, `claude rm <id>` |
//! | Preview | `claude logs <id>` |
//! | Take over | `claude attach <id>` |
//!
//! `<id>` is a background session's short id: the first eight characters of
//! its session UUID.

mod launch;
mod listing;
#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;
use volee_core::harness::{HarnessKind, SessionKey, SessionOrigin};
use volee_core::ids::AgentId;
use volee_core::reconcile::Listing;

pub use launch::{Backgrounded, parse_backgrounded};
pub use listing::{input_request, parse_listing, session_state};

use crate::error::HarnessError;
use crate::harness::{Capabilities, Harness};
use crate::hooks;
use crate::runner::{CommandRunner, Invocation, Output, RunError, SystemRunner};
use crate::spec::{SpawnSpec, Spawned};
use crate::version::Version;

/// The oldest Claude Code with background sessions (`claude --bg`, `attach`,
/// `logs`, `stop`, `rm`) and `claude agents --json`.
pub const MINIMUM_VERSION: Version = Version::new(2, 1, 300);

/// What [`MINIMUM_VERSION`] is needed for, in error messages.
const NEEDED_FOR: &str = "background sessions and `claude agents --json`";

/// How long a failure message from Claude Code may be, in characters.
const MAX_FAILURE_MESSAGE: usize = 500;

/// How long Claude Code commands may run before they are killed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// Queries and controls: `--version`, `agents`, `stop`, `rm`, `logs`.
    pub query: Duration,
    /// Launches and resumes, which may first start Claude Code's background
    /// supervisor.
    pub launch: Duration,
}

impl Default for Timeouts {
    /// 15 seconds for queries, 60 seconds for launches.
    fn default() -> Self {
        Self {
            query: Duration::from_secs(15),
            launch: Duration::from_secs(60),
        }
    }
}

/// The Claude Code adapter: runs `claude` through a [`CommandRunner`].
///
/// Use [`ClaudeCode::default`] for the `claude` on the `PATH` with real
/// processes, and [`ClaudeCode::new`] with a fake runner in tests.
#[derive(Debug)]
pub struct ClaudeCode<R = SystemRunner> {
    runner: R,
    program: PathBuf,
    timeouts: Timeouts,
}

impl Default for ClaudeCode {
    fn default() -> Self {
        Self::new(SystemRunner)
    }
}

impl<R: CommandRunner> ClaudeCode<R> {
    /// The adapter for `claude` on the `PATH`, run by `runner`, with the
    /// default [`Timeouts`].
    pub fn new(runner: R) -> Self {
        Self {
            runner,
            program: PathBuf::from("claude"),
            timeouts: Timeouts::default(),
        }
    }

    /// Runs another `claude` program, for example an absolute path when the
    /// daemon's `PATH` does not include it.
    #[must_use]
    pub fn with_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = program.into();
        self
    }

    /// Uses other timeouts.
    #[must_use]
    pub fn with_timeouts(mut self, timeouts: Timeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    /// The runner, for example to inspect a fake in tests.
    pub fn runner(&self) -> &R {
        &self.runner
    }

    /// The `claude` program this adapter runs.
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// The timeouts this adapter uses.
    pub fn timeouts(&self) -> Timeouts {
        self.timeouts
    }

    /// The installed Claude Code version, from `claude --version`.
    pub fn version(&self) -> Result<Version, HarnessError> {
        let command = self.describe("--version");
        let output = self.run(&command, &self.query().arg("--version"))?;
        let text = String::from_utf8_lossy(&output.stdout);
        Version::find_in(&text).ok_or_else(|| HarnessError::UnexpectedOutput {
            command,
            source: crate::error::ParseError::NoVersion(text.trim().to_owned()),
        })
    }

    /// An invocation of `claude` with the query timeout.
    fn query(&self) -> Invocation {
        Invocation::new(self.program.clone(), self.timeouts.query)
    }

    /// A command as shown in error messages.
    fn describe(&self, args: &str) -> String {
        format!("{} {args}", self.program.display())
    }

    /// Runs `invocation` and turns a missing program or a failure status into
    /// an error.
    fn run(&self, command: &str, invocation: &Invocation) -> Result<Output, HarnessError> {
        let output = self.runner.run(invocation).map_err(|error| match error {
            RunError::NotFound { program } => HarnessError::NotInstalled { program },
            error => HarnessError::Run(error),
        })?;
        if output.is_success() {
            Ok(output)
        } else {
            Err(HarnessError::CommandFailed {
                command: command.to_owned(),
                code: output.code,
                message: failure_message(&output),
            })
        }
    }

    /// Runs `claude stop`, `rm` or `logs` on a background session.
    fn control(&self, subcommand: &str, session: &SessionKey) -> Result<Output, HarnessError> {
        let id = short_id(session)?;
        let command = self.describe(&format!("{subcommand} {id}"));
        self.run(&command, &self.query().args([subcommand, id]))
    }

    /// Runs `claude --bg` and reads the session it reports.
    fn launch(
        &self,
        command: &str,
        args: Vec<OsString>,
        spec: &SpawnSpec,
    ) -> Result<Backgrounded, HarnessError> {
        let invocation = Invocation::new(self.program.clone(), self.timeouts.launch)
            .args(args)
            .cwd(&spec.cwd);
        let output = self.run(command, &invocation)?;
        parse_backgrounded(&String::from_utf8_lossy(&output.stdout))
            .or_else(|error| {
                parse_backgrounded(&String::from_utf8_lossy(&output.stderr)).map_err(|_| error)
            })
            .map_err(|source| HarnessError::UnexpectedOutput {
                command: command.to_owned(),
                source,
            })
    }
}

impl<R: CommandRunner> Harness for ClaudeCode<R> {
    fn kind(&self) -> HarnessKind {
        HarnessKind::ClaudeCode
    }

    /// Runs `claude --version` and requires [`MINIMUM_VERSION`].
    fn capabilities(&self) -> Result<Capabilities, HarnessError> {
        let version = self.version()?;
        if version < MINIMUM_VERSION {
            return Err(HarnessError::UnsupportedVersion {
                harness: HarnessKind::ClaudeCode,
                found: version,
                minimum: MINIMUM_VERSION,
                needed_for: NEEDED_FOR,
            });
        }
        Ok(Capabilities {
            version,
            list_sessions: true,
            background_sessions: true,
        })
    }

    /// Runs `claude agents --json --all` and maps it with [`parse_listing`].
    /// The listing is complete: it includes finished background sessions.
    fn list(&self) -> Result<Listing, HarnessError> {
        let command = self.describe("agents --json --all");
        let output = self.run(&command, &self.query().args(["agents", "--json", "--all"]))?;
        parse_listing(&output.stdout)
            .map_err(|source| HarnessError::UnexpectedOutput { command, source })
    }

    /// Runs `claude --bg [flags] -- <prompt>` in the spec's directory.
    ///
    /// The agent loads only volee's MCP configuration (`--strict-mcp-config`)
    /// and its permission mode is passed only when the spec sets one, so
    /// `bypassPermissions` is never used unless asked for.
    fn spawn(&self, spec: &SpawnSpec) -> Result<Spawned, HarnessError> {
        spec.validate()?;
        let background = self.launch(&self.describe("--bg"), launch::args(None, spec), spec)?;
        Ok(Spawned {
            short_id: background.short_id,
            name: background.name,
            origin: SessionOrigin::Started,
        })
    }

    /// Runs `claude --bg --resume <session-uuid> [flags] -- <prompt>`.
    ///
    /// Claude Code 2.1.319 continues a stopped session as a fork: a new
    /// session with a new id and the same conversation. The result is
    /// `Forked` when the reported short id differs from the resumed one.
    fn resume(&self, session: &SessionKey, spec: &SpawnSpec) -> Result<Spawned, HarnessError> {
        let id = session_uuid(session)?;
        spec.validate()?;
        let command = self.describe(&format!("--bg --resume {id}"));
        let background = self.launch(&command, launch::args(Some(id), spec), spec)?;
        let origin = if id.starts_with(&background.short_id) {
            SessionOrigin::Resumed
        } else {
            SessionOrigin::Forked
        };
        Ok(Spawned {
            short_id: background.short_id,
            name: background.name,
            origin,
        })
    }

    /// Runs `claude stop <id>`. The conversation is kept.
    fn stop(&self, session: &SessionKey) -> Result<(), HarnessError> {
        self.control("stop", session).map(drop)
    }

    /// Runs `claude rm <id>`.
    ///
    /// Claude Code also deletes a worktree it created for the session (with
    /// `--worktree`, which volee never passes) when that is safe, and never
    /// deletes branches. volee never passes `--discard-unpushed` or
    /// `--force-remove-worktree`.
    fn remove(&self, session: &SessionKey) -> Result<(), HarnessError> {
        self.control("rm", session).map(drop)
    }

    /// Runs `claude logs <id>`: a raw replay of the session's terminal (about
    /// 200x50, with cursor addressing and no newlines), meant for a terminal
    /// emulator widget.
    fn logs(&self, session: &SessionKey) -> Result<Vec<u8>, HarnessError> {
        self.control("logs", session).map(|output| output.stdout)
    }

    /// Builds `claude attach <id>`.
    ///
    /// The caller suspends its interface, restores the terminal (leaves the
    /// alternate screen and raw mode) and runs the command with inherited
    /// standard streams. Pressing `Ctrl+Z` in the attached session returns to
    /// the caller with exit status 0; the session keeps running in the
    /// background.
    fn attach_command(&self, session: &SessionKey) -> Result<Command, HarnessError> {
        let id = short_id(session)?;
        let mut command = Command::new(&self.program);
        command.arg("attach").arg(id);
        Ok(command)
    }

    /// See [`hooks::agent_settings`].
    fn agent_settings(
        &self,
        volee_bin: &Path,
        agent: AgentId,
        rewake_timeout: Duration,
    ) -> Result<Value, HarnessError> {
        Ok(hooks::agent_settings(volee_bin, agent, rewake_timeout)?)
    }

    /// See [`hooks::mcp_config`].
    fn mcp_config(&self, volee_bin: &Path, agent: AgentId) -> Result<Value, HarnessError> {
        Ok(hooks::mcp_config(volee_bin, agent)?)
    }
}

/// Checks that `session` is a Claude Code session.
fn claude_session(session: &SessionKey) -> Result<(), HarnessError> {
    if session.harness == HarnessKind::ClaudeCode {
        Ok(())
    } else {
        Err(HarnessError::ForeignSession {
            session: session.clone(),
        })
    }
}

/// The short id of a Claude Code session, checked before it reaches a
/// command line.
fn short_id(session: &SessionKey) -> Result<&str, HarnessError> {
    claude_session(session)?;
    let id = session.short();
    if is_argument_safe(id) {
        Ok(id)
    } else {
        Err(HarnessError::InvalidSessionId {
            id: session.id.clone(),
        })
    }
}

/// The full session UUID of a Claude Code session, which `--resume` needs: a
/// shorter id could be taken for part of a session name.
fn session_uuid(session: &SessionKey) -> Result<&str, HarnessError> {
    claude_session(session)?;
    if is_uuid(&session.id) {
        Ok(&session.id)
    } else {
        Err(HarnessError::InvalidSessionId {
            id: session.id.clone(),
        })
    }
}

/// Whether `id` is safe as one command-line argument: ASCII letters, digits
/// and dashes, not starting with a dash (which would read as an option).
fn is_argument_safe(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// Whether `id` is a UUID in its usual `8-4-4-4-12` hexadecimal form.
fn is_uuid(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups.iter().zip([8, 4, 4, 4, 12]).all(|(group, length)| {
            group.len() == length && group.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

/// What a failed command printed about its failure: stderr, or stdout when
/// stderr is empty, trimmed and shortened.
fn failure_message(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = if stderr.trim().is_empty() {
        String::from_utf8_lossy(&output.stdout)
    } else {
        stderr
    };
    let text = text.trim();
    match text.char_indices().nth(MAX_FAILURE_MESSAGE) {
        Some((cut, _)) => format!("{}\u{2026}", &text[..cut]),
        None => text.to_owned(),
    }
}
