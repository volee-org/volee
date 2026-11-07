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
//! | Launch | `claude --bg --opt=value... -- <prompt>` |
//! | Resume | `claude --bg --resume=<session-uuid> --opt=value... -- <prompt>` |
//! | Stop, delete | `claude stop <id>`, `claude rm <id>` |
//! | Preview | `claude logs <id>` |
//! | Take over | `claude attach <id>` |
//!
//! `<id>` is the id of a background session, as `claude --bg` prints it and
//! `claude agents` lists it (the session's [`handle`](ListedSession::handle)).
//!
//! Every `claude` runs without the variables that tie a process to a Claude
//! Code session ([`SESSION_VARIABLES`]), so a daemon started from inside an
//! agent never launches or controls sessions as that agent. Queries and
//! controls run from a directory that always exists
//! ([`ClaudeCode::with_query_dir`]), never from a worktree that cleanup may
//! delete.

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
use volee_core::reconcile::{ListedSession, Listing};

pub use launch::{Backgrounded, parse_backgrounded};
pub use listing::{input_request, parse_listing, session_state};

use crate::error::{HarnessError, ParseError};
use crate::harness::{Capabilities, Harness, WorktreeRemoval};
use crate::hooks;
use crate::runner::{CommandRunner, Invocation, Output, RunError, SystemRunner};
use crate::spec::{Completion, SpawnSpec, Spawned};
use crate::version::Version;

/// The oldest Claude Code with background sessions (`claude --bg`, `attach`,
/// `logs`, `stop`, `rm`) and `claude agents --json`.
pub const MINIMUM_VERSION: Version = Version::new(2, 1, 300);

/// The environment variables that Claude Code sets for the processes it
/// starts (its shell commands, hooks, MCP servers and background sessions)
/// and that tie a process to one session: its id, its messaging socket and
/// token, its job directory, its process id, its effort level.
///
/// volee removes them from the environment of every `claude` it runs. The
/// user's own configuration variables, such as `CLAUDE_CODE_SUBAGENT_MODEL`,
/// are kept, and so are the variables of the user's terminal (`TMUX` and the
/// like), which `claude attach` needs. The list covers every Claude Code
/// variable that the command line removes when it starts the daemon.
pub const SESSION_VARIABLES: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_AGENT",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_HOST_SHORT_TEMP_DIRS",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_REMOTE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_EFFORT",
    "CLAUDE_ENV_FILE",
    "CLAUDE_JOB_DIR",
    "CLAUDE_PID",
    "CLAUDE_PROJECT_DIR",
];

/// What [`MINIMUM_VERSION`] is needed for, in error messages.
const NEEDED_FOR: &str = "background sessions and `claude agents --json`";

/// How long a failure message from Claude Code may be, in characters.
const MAX_FAILURE_MESSAGE: usize = 500;

/// How Claude Code 2.1.319 starts its answer when `stop`, `rm` or `logs` is
/// given an id that matches no background session.
const UNKNOWN_SESSION: &str = "No job matching";

/// How Claude Code starts its answer when it refuses to start a background
/// session in a directory the user has not trusted.
const UNTRUSTED: &str = "Workspace not trusted";

/// How long Claude Code commands may run before they are killed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// Queries and controls: `--version`, `agents`, `stop`, `rm`, `logs`.
    pub query: Duration,
    /// Launches and resumes, which may first start Claude Code's background
    /// supervisor.
    pub launch: Duration,
    /// One-off answers (`claude -p`), which wait for the model.
    pub completion: Duration,
}

impl Default for Timeouts {
    /// 15 seconds for queries, 60 seconds for launches, 3 minutes for
    /// answers.
    fn default() -> Self {
        Self {
            query: Duration::from_secs(15),
            launch: Duration::from_secs(60),
            completion: Duration::from_secs(180),
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
    query_dir: PathBuf,
    timeouts: Timeouts,
}

impl Default for ClaudeCode {
    fn default() -> Self {
        Self::new(SystemRunner)
    }
}

impl<R: CommandRunner> ClaudeCode<R> {
    /// The adapter for `claude` on the `PATH`, run by `runner`, with the
    /// default [`Timeouts`], running its queries from `/`.
    pub fn new(runner: R) -> Self {
        Self {
            runner,
            program: PathBuf::from("claude"),
            query_dir: PathBuf::from("/"),
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

    /// Runs queries and controls (`--version`, `agents`, `stop`, `rm`,
    /// `logs`) from `dir`. It must outlive every agent, so never a worktree;
    /// the default, `/`, always exists and holds no project settings.
    #[must_use]
    pub fn with_query_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.query_dir = dir.into();
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

    /// The directory queries and controls run from.
    pub fn query_dir(&self) -> &Path {
        &self.query_dir
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
            source: ParseError::NoVersion(text.trim().to_owned()),
        })
    }

    /// An invocation of `claude` without the session variables of the
    /// environment volee runs in.
    fn invocation(&self, timeout: Duration) -> Invocation {
        Invocation::new(self.program.clone(), timeout).env_remove(SESSION_VARIABLES.iter().copied())
    }

    /// An invocation of `claude` for a query or a control.
    fn query(&self) -> Invocation {
        self.invocation(self.timeouts.query).cwd(&self.query_dir)
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
    fn control(&self, subcommand: &str, session: &ListedSession) -> Result<Output, HarnessError> {
        let handle = handle(session)?;
        let command = self.describe(&format!("{subcommand} {handle}"));
        self.run(&command, &self.query().args([subcommand, handle]))
            .map_err(|error| match error {
                HarnessError::CommandFailed { ref message, .. }
                    if message.starts_with(UNKNOWN_SESSION) =>
                {
                    HarnessError::UnknownSession {
                        handle: handle.to_owned(),
                    }
                }
                error => error,
            })
    }

    /// Runs `claude --bg` and reads the session it reports.
    fn launch(
        &self,
        command: &str,
        args: Vec<OsString>,
        spec: &SpawnSpec,
    ) -> Result<Backgrounded, HarnessError> {
        let untrusted = || HarnessError::UntrustedDirectory {
            path: spec.cwd.clone(),
        };
        let invocation = self
            .invocation(self.timeouts.launch)
            .args(args)
            .cwd(&spec.cwd);
        let output = self
            .run(command, &invocation)
            .map_err(|error| match error {
                HarnessError::CommandFailed { ref message, .. } if message.contains(UNTRUSTED) => {
                    untrusted()
                }
                error => error,
            })?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        parse_backgrounded(&stdout)
            .or_else(|error| parse_backgrounded(&stderr).map_err(|_| error))
            .map_err(|source| {
                if stdout.contains(UNTRUSTED) || stderr.contains(UNTRUSTED) {
                    untrusted()
                } else {
                    HarnessError::UnexpectedOutput {
                        command: command.to_owned(),
                        source,
                    }
                }
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

    /// Runs `claude --bg --opt=value... -- <prompt>` in the spec's directory.
    ///
    /// The agent loads only volee's MCP configuration (`--strict-mcp-config`)
    /// and its permission mode is passed only when the spec sets one. Claude
    /// Code refuses to start in a directory the user has not trusted:
    /// [`HarnessError::UntrustedDirectory`].
    fn spawn(&self, spec: &SpawnSpec) -> Result<Spawned, HarnessError> {
        spec.validate()?;
        let background = self.launch(&self.describe("--bg"), launch::args(None, spec), spec)?;
        Ok(Spawned {
            handle: background.id,
            name: background.name,
            origin: SessionOrigin::Started,
        })
    }

    /// Runs `claude --bg --resume=<session-uuid> --opt=value... -- <prompt>`.
    ///
    /// Claude Code 2.1.319 continues a stopped session as a fork: a new
    /// session with a new id and the same conversation. The result is
    /// `Forked` when the reported id is not the start of the resumed
    /// session's id.
    fn resume(&self, session: &SessionKey, spec: &SpawnSpec) -> Result<Spawned, HarnessError> {
        let id = session_uuid(session)?;
        spec.validate()?;
        let command = self.describe(&format!("--bg --resume={id}"));
        let background = self.launch(&command, launch::args(Some(id), spec), spec)?;
        let origin = if id.starts_with(&background.id) {
            SessionOrigin::Resumed
        } else {
            SessionOrigin::Forked
        };
        Ok(Spawned {
            handle: background.id,
            name: background.name,
            origin,
        })
    }

    /// Runs `claude stop <id>`. The conversation is kept.
    fn stop(&self, session: &ListedSession) -> Result<(), HarnessError> {
        self.control("stop", session).map(drop)
    }

    /// Runs `claude rm <id>`.
    ///
    /// Claude Code documents that it deletes a worktree it created for the
    /// session (under `.claude/worktrees/`, when the session started outside
    /// a linked worktree) unless that worktree has uncommitted changes or
    /// unpushed commits, and that it leaves in place a worktree someone else
    /// created and started the session inside. It never deletes branches;
    /// volee never passes `--discard-unpushed` or `--force-remove-worktree`.
    ///
    /// Unless `worktree` is [`WorktreeRemoval::Delivered`], a session whose
    /// directory is inside a linked git worktree is refused with
    /// [`HarnessError::WorktreeAtRisk`] before anything runs. When Claude
    /// Code keeps the session instead of removing it, the result is
    /// [`HarnessError::SessionKept`] with its explanation.
    fn remove(
        &self,
        session: &ListedSession,
        worktree: WorktreeRemoval,
    ) -> Result<(), HarnessError> {
        let handle = handle(session)?;
        if worktree == WorktreeRemoval::Refuse
            && let Some(root) = linked_worktree(&session.cwd)
        {
            return Err(HarnessError::WorktreeAtRisk {
                session: session.key.clone(),
                worktree: root,
            });
        }
        let kept = |text: &str| HarnessError::SessionKept {
            handle: handle.to_owned(),
            reason: text.trim().to_owned(),
        };
        let output = self.control("rm", session).map_err(|error| match error {
            HarnessError::CommandFailed { ref message, .. } if says_kept(message) => kept(message),
            error => error,
        })?;
        for text in [&output.stdout, &output.stderr] {
            let text = String::from_utf8_lossy(text);
            if says_kept(&text) {
                return Err(kept(&text));
            }
        }
        Ok(())
    }

    /// Runs `claude logs <id>`: a raw replay of the session's terminal (about
    /// 200x50, with cursor addressing and no newlines), meant for a terminal
    /// emulator widget.
    fn logs(&self, session: &ListedSession) -> Result<Vec<u8>, HarnessError> {
        self.control("logs", session).map(|output| output.stdout)
    }

    /// Builds `claude attach <id>`, without the session variables of volee's
    /// environment.
    ///
    /// The caller suspends its interface, restores the terminal (leaves the
    /// alternate screen and raw mode) and runs the command with inherited
    /// standard streams. Pressing `Ctrl+Z` in the attached session returns to
    /// the caller with exit status 0; the session keeps running in the
    /// background.
    fn attach_command(&self, session: &ListedSession) -> Result<Command, HarnessError> {
        let handle = handle(session)?;
        let mut command = Command::new(&self.program);
        command.arg("attach").arg(handle);
        for variable in SESSION_VARIABLES {
            command.env_remove(variable);
        }
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

    /// Runs `claude -p` in `request.cwd`, with no tools, no MCP server, none of
    /// the person's hooks and no saved session.
    fn complete(&self, request: &Completion) -> Result<String, HarnessError> {
        if !request.cwd.is_absolute() {
            return Err(crate::RequestError::RelativePath {
                what: "working directory",
                path: request.cwd.clone(),
            }
            .into());
        }
        let command = self.describe("-p");
        let invocation = self
            .invocation(self.timeouts.completion)
            .args(launch::completion_args(request))
            .cwd(&request.cwd);
        let output = self.run(&command, &invocation)?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
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

/// The id Claude Code takes in commands for `session`, as `claude agents`
/// listed it, checked before it reaches a command line.
fn handle(session: &ListedSession) -> Result<&str, HarnessError> {
    claude_session(&session.key)?;
    let handle = session
        .handle
        .as_deref()
        .ok_or_else(|| HarnessError::NotControllable {
            session: session.key.clone(),
        })?;
    if is_argument_safe(handle) {
        Ok(handle)
    } else {
        Err(HarnessError::InvalidSessionId {
            id: handle.to_owned(),
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

/// The root of the linked git worktree that contains `dir`: the nearest
/// ancestor whose `.git` is a file (a main checkout's `.git` is a
/// directory). `None` outside a linked worktree, or when `dir` is gone.
fn linked_worktree(dir: &Path) -> Option<PathBuf> {
    for ancestor in dir.ancestors() {
        match std::fs::metadata(ancestor.join(".git")) {
            Ok(metadata) if metadata.is_file() => return Some(ancestor.to_path_buf()),
            Ok(_) => return None,
            Err(_) => {}
        }
    }
    None
}

/// Whether `claude rm` said it kept the session (`kept <id> ...`, for a
/// worktree with uncommitted changes or unpushed commits).
fn says_kept(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim_start().starts_with("kept "))
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
