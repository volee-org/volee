//! Errors of the harness adapters.

use std::path::PathBuf;

use volee_core::harness::{HarnessKind, SessionKey};

use crate::runner::RunError;
use crate::version::Version;

/// Why a harness operation failed.
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    /// The harness program is not installed, or not on the `PATH`.
    #[error("`{}` was not found: is the harness installed and on the PATH?", program.display())]
    NotInstalled {
        /// The program volee tried to run.
        program: PathBuf,
    },
    /// The program could not run to completion.
    #[error(transparent)]
    Run(#[from] RunError),
    /// The program ran and reported a failure.
    #[error("`{command}` failed ({}): {message}", describe_exit(*.code))]
    CommandFailed {
        /// The command, without its free-text arguments.
        command: String,
        /// The exit code, or `None` when a signal ended the program.
        code: Option<i32>,
        /// What the program printed about the failure (stderr, or stdout
        /// when stderr is empty), trimmed and shortened.
        message: String,
    },
    /// The program succeeded but its output is not what volee expects.
    #[error("unexpected output from `{command}`")]
    UnexpectedOutput {
        /// The command.
        command: String,
        /// What was wrong with the output.
        #[source]
        source: ParseError,
    },
    /// The installed harness is too old for what volee needs.
    #[error("{harness} {found} is too old: volee needs {minimum} or newer for {needed_for}")]
    UnsupportedVersion {
        /// The harness.
        harness: HarnessKind,
        /// The installed version.
        found: Version,
        /// The oldest version volee supports.
        minimum: Version,
        /// The features that need the minimum version.
        needed_for: &'static str,
    },
    /// The session belongs to another harness.
    #[error("{session} is not a session of this harness")]
    ForeignSession {
        /// The session.
        session: SessionKey,
    },
    /// The session id cannot be passed to the harness safely, or does not
    /// have the shape the operation needs.
    #[error("`{id}` is not a usable session id here")]
    InvalidSessionId {
        /// The id.
        id: String,
    },
    /// The session has no handle the harness accepts in commands: for Claude
    /// Code, an interactive session, which lives in its own terminal and
    /// cannot be stopped, removed, previewed or attached from elsewhere.
    #[error("{session} cannot be controlled from volee: it runs in its own terminal")]
    NotControllable {
        /// The session.
        session: SessionKey,
    },
    /// The harness does not know the session any more.
    #[error("the harness has no session `{handle}`")]
    UnknownSession {
        /// The handle volee asked about.
        handle: String,
    },
    /// The harness refuses to start a session in a directory the user has not
    /// trusted, and cannot ask without a terminal.
    #[error(
        "Claude Code does not trust {}: run `claude` there once and accept the trust prompt",
        path.display()
    )]
    UntrustedDirectory {
        /// The directory.
        path: PathBuf,
    },
    /// The session works in a git worktree that removing it may delete, and
    /// the caller has no delivery evidence for that work (ADR 0011).
    #[error(
        "{session} works in the git worktree {}, which removing the session may delete; \
         remove it only once its work is delivered",
        worktree.display()
    )]
    WorktreeAtRisk {
        /// The session.
        session: SessionKey,
        /// The root of the worktree.
        worktree: PathBuf,
    },
    /// The harness kept the session instead of removing it, and said why (for
    /// Claude Code: its worktree has uncommitted changes or unpushed
    /// commits).
    #[error("the harness kept session `{handle}`: {reason}")]
    SessionKept {
        /// The session's handle.
        handle: String,
        /// The harness's explanation.
        reason: String,
    },
    /// The request cannot become a valid command line or configuration.
    #[error("invalid request")]
    InvalidRequest(#[from] RequestError),
}

/// Why volee cannot turn a request into a command line or a configuration
/// file. These are checked before anything runs.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RequestError {
    /// The prompt is empty or only whitespace.
    #[error("the prompt is empty")]
    EmptyPrompt,
    /// A path that must be absolute is relative.
    #[error("the {what} must be an absolute path, got {}", path.display())]
    RelativePath {
        /// What the path is for.
        what: &'static str,
        /// The path.
        path: PathBuf,
    },
    /// A path that ends up in JSON or a shell command is not valid UTF-8.
    #[error("the {what} is not valid UTF-8: {}", path.display())]
    NonUtf8Path {
        /// What the path is for.
        what: &'static str,
        /// The path.
        path: PathBuf,
    },
    /// A tool rule is empty or starts with `-`, which the harness would read
    /// as an option.
    #[error("`{rule}` is not a valid tool rule")]
    InvalidToolRule {
        /// The rule.
        rule: String,
    },
}

/// Why the output of a harness command could not be understood.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// No `major.minor.patch` version in the output.
    #[error("no version number in {0:?}")]
    NoVersion(String),
    /// No line reporting a started background session.
    #[error("no `backgrounded \u{b7} <id> \u{b7} <name>` line in the output")]
    NoBackgroundedLine,
    /// The reported session id cannot be used as a command-line argument.
    #[error("`{0}` is not a valid short session id")]
    InvalidShortId(String),
    /// The session listing is not a JSON array.
    #[error("the session listing is not a JSON array")]
    Listing(#[source] serde_json::Error),
}

fn describe_exit(code: Option<i32>) -> String {
    code.map_or_else(
        || "ended by a signal".to_owned(),
        |code| format!("exit status {code}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_failures_name_the_command_and_the_status() {
        let error = HarnessError::CommandFailed {
            command: "claude stop 7d3c1f0a".into(),
            code: Some(1),
            message: "no such session".into(),
        };
        assert_eq!(
            error.to_string(),
            "`claude stop 7d3c1f0a` failed (exit status 1): no such session"
        );
        let killed = HarnessError::CommandFailed {
            command: "claude logs 7d3c1f0a".into(),
            code: None,
            message: String::new(),
        };
        assert!(killed.to_string().contains("ended by a signal"));
    }

    #[test]
    fn version_errors_explain_the_minimum() {
        let error = HarnessError::UnsupportedVersion {
            harness: HarnessKind::ClaudeCode,
            found: Version::new(2, 1, 250),
            minimum: Version::new(2, 1, 300),
            needed_for: "background sessions",
        };
        assert_eq!(
            error.to_string(),
            "claude-code 2.1.250 is too old: volee needs 2.1.300 or newer for background sessions"
        );
    }
}
