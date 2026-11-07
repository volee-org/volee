//! Daemon errors and their mapping to protocol errors.

use std::io;
use std::path::PathBuf;

use volee_core::registry::{ApplyError, RouteError};
use volee_proto::rpc::{self, RpcError};

use crate::log::LogError;

/// Everything that can go wrong in the daemon.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// Another daemon already runs for this user.
    #[error("another volee daemon is already running (lock {0})")]
    AlreadyRunning(PathBuf),
    /// Setting up the socket or the state directory failed.
    #[error("{action} {path}: {source}")]
    Setup {
        /// What was attempted.
        action: &'static str,
        /// On which path.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The event log failed.
    #[error(transparent)]
    Log(#[from] LogError),
    /// A stored event does not fit the registry: the log is corrupted.
    #[error("event {seq} does not fit the registry: {source}")]
    Corrupted {
        /// The sequence number of the event.
        seq: u64,
        /// Why it does not fit.
        #[source]
        source: ApplyError,
    },
    /// A request named something that does not exist.
    #[error("{0}")]
    NotFound(String),
    /// A request was refused by a rule.
    #[error("{0}")]
    Refused(String),
    /// A request's parameters are invalid.
    #[error("{0}")]
    InvalidParams(String),
    /// The client speaks another protocol version.
    #[error("{0}")]
    VersionMismatch(String),
    /// An event built from a request did not apply.
    #[error(transparent)]
    Apply(#[from] ApplyError),
    /// A program the request needs (the harness, git) is not available.
    #[error("{0}")]
    Unavailable(String),
    /// The harness failed.
    #[error(transparent)]
    Harness(#[from] volee_harness::HarnessError),
    /// A launch failed after volee created the agent's worktree and branch,
    /// which it keeps (ADR 0011): the message says that the same command
    /// can simply run again (ADR 0016).
    #[error(
        "{}. volee kept the worktree it made for the agent, {}, and its new branch {branch}: \
         once that is fixed, run the same command again, and volee reuses them as long as \
         nothing was done in them",
        clause(error),
        worktree.display()
    )]
    LaunchFailed {
        /// Why the launch failed.
        error: Box<DaemonError>,
        /// The repository's main checkout.
        repo: PathBuf,
        /// The worktree volee created.
        worktree: PathBuf,
        /// The branch volee created.
        branch: String,
    },
    /// A setup command of the repository failed or ran out of time
    /// (ADR 0017).
    #[error("the setup command `{command}` {reason}; what it printed is in {}", log.display())]
    SetupFailed {
        /// The command, as configured.
        command: String,
        /// What went wrong, such as `exited with status 1: <its last line>`.
        reason: String,
        /// The setup log.
        log: PathBuf,
    },
    /// git failed or refused.
    #[error(transparent)]
    Git(#[from] volee_git::GitError),
    /// Writing an agent's files failed.
    #[error("cannot write {path}: {source}")]
    Write {
        /// The file.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
}

impl From<RouteError> for DaemonError {
    fn from(error: RouteError) -> Self {
        match error {
            RouteError::UnknownAgent(_) => Self::NotFound(error.to_string()),
            RouteError::ToSelf | RouteError::NotPermitted { .. } => {
                Self::Refused(error.to_string())
            }
        }
    }
}

impl From<DaemonError> for RpcError {
    fn from(error: DaemonError) -> Self {
        Self::new(code(&error), error.to_string())
    }
}

/// An error's message without its final period, to go before more words:
/// the harness's own errors often end with one.
fn clause(error: &DaemonError) -> String {
    let text = error.to_string();
    text.trim_end().trim_end_matches('.').to_owned()
}

/// The JSON-RPC code of `error`: a failed launch keeps the code of its cause.
fn code(error: &DaemonError) -> i64 {
    match error {
        DaemonError::LaunchFailed { error, .. } => code(error),
        DaemonError::NotFound(_)
        | DaemonError::Harness(volee_harness::HarnessError::UnknownSession { .. }) => {
            rpc::NOT_FOUND
        }
        DaemonError::Refused(_)
        | DaemonError::SetupFailed { .. }
        | DaemonError::Harness(
            volee_harness::HarnessError::NotControllable { .. }
            | volee_harness::HarnessError::ForeignSession { .. }
            | volee_harness::HarnessError::UntrustedDirectory { .. }
            | volee_harness::HarnessError::WorktreeAtRisk { .. }
            | volee_harness::HarnessError::SessionKept { .. },
        )
        | DaemonError::Git(
            volee_git::GitError::BranchExists { .. }
            | volee_git::GitError::PathExists { .. }
            | volee_git::GitError::InvalidBranchName { .. }
            | volee_git::GitError::UnknownRevision { .. }
            | volee_git::GitError::RemovalRefused { .. },
        ) => rpc::REFUSED,
        DaemonError::InvalidParams(_)
        | DaemonError::Harness(volee_harness::HarnessError::InvalidRequest(_)) => {
            rpc::INVALID_PARAMS
        }
        DaemonError::VersionMismatch(_) => rpc::VERSION_MISMATCH,
        DaemonError::Unavailable(_)
        | DaemonError::Harness(
            volee_harness::HarnessError::NotInstalled { .. }
            | volee_harness::HarnessError::UnsupportedVersion { .. },
        ) => rpc::UNAVAILABLE,
        _ => rpc::INTERNAL_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_launch_says_its_cause_once() {
        let error = DaemonError::LaunchFailed {
            error: Box::new(DaemonError::Unavailable(
                "Claude Code cannot start a session.".into(),
            )),
            repo: PathBuf::from("/work/shop"),
            worktree: PathBuf::from("/work/shop/.claude/worktrees/feat-search"),
            branch: "feat/search".into(),
        };
        let message = error.to_string();
        assert!(
            message.starts_with(
                "Claude Code cannot start a session. volee kept the worktree it made for the \
                 agent, /work/shop/.claude/worktrees/feat-search, and its new branch feat/search:"
            ),
            "{message}"
        );
        assert!(!message.contains(".."), "{message}");
    }
}
