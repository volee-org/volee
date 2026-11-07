//! The error type of this crate.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

/// Why a git query or operation failed.
///
/// The variants that come from running git keep the command line, so a
/// person can run it again by hand to see what went wrong.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GitError {
    /// The git executable could not be started, for example because it is
    /// not installed or not on the `PATH`.
    #[error("could not run `{}`: {source}", .program.display())]
    Spawn {
        /// The program that could not be started.
        program: PathBuf,
        /// The operating system's reason.
        #[source]
        source: io::Error,
    },
    /// git ran and reported a failure.
    #[error("`{command}` failed with {}{}", exit_description(*.code), stderr_suffix(.stderr))]
    Failed {
        /// The command line, quoted for a POSIX shell.
        command: String,
        /// The exit code; `None` when git was stopped by a signal.
        code: Option<i32>,
        /// What git printed on its standard error, trimmed.
        stderr: String,
    },
    /// git did not finish in time and was stopped.
    #[error("`{command}` did not finish within {timeout:?} and was stopped")]
    TimedOut {
        /// The command line, quoted for a POSIX shell.
        command: String,
        /// The time it was given.
        timeout: Duration,
    },
    /// git printed something this crate does not understand.
    #[error("unexpected output from `{command}`: {detail}")]
    UnexpectedOutput {
        /// The command line, quoted for a POSIX shell.
        command: String,
        /// What was wrong with the output.
        detail: String,
    },
    /// A worktree was not created because its branch already exists.
    #[error("branch `{branch}` already exists")]
    BranchExists {
        /// The branch.
        branch: String,
    },
    /// A worktree was not created because its directory already exists.
    #[error("`{}` already exists", .path.display())]
    PathExists {
        /// The directory.
        path: PathBuf,
    },
    /// The name cannot be used for a branch.
    #[error("`{name}` is not a valid branch name")]
    InvalidBranchName {
        /// The rejected name.
        name: String,
    },
    /// The revision names no commit in the repository.
    #[error("`{revision}` does not name a commit")]
    UnknownRevision {
        /// The revision, for example a base branch.
        revision: String,
    },
    /// A worktree was not removed, to protect the work in it (ADR 0011).
    #[error("refusing to remove `{}`: {reason}", .path.display())]
    RemovalRefused {
        /// The worktree.
        path: PathBuf,
        /// Why it stays.
        reason: RemovalRefusal,
    },
    /// Creating a worktree failed after git had created it: the worktree is
    /// still there, but the operation did not complete (no branch, a failing
    /// hook, or a checkout stopped at its timeout).
    #[error("the worktree at `{}` was created, but not completed: {source}", .path.display())]
    WorktreeIncomplete {
        /// The worktree that exists.
        path: PathBuf,
        /// The step that failed.
        #[source]
        source: Box<GitError>,
    },
    /// A file system operation done around git failed.
    #[error("could not {action} `{}`: {source}", .path.display())]
    Io {
        /// What was being done, such as `resolve`.
        action: &'static str,
        /// The path it was done to.
        path: PathBuf,
        /// The operating system's reason.
        #[source]
        source: io::Error,
    },
}

/// Why [`Git::remove_worktree`](crate::Git::remove_worktree) keeps a
/// worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RemovalRefusal {
    /// The path is not a worktree of the repository.
    NotAWorktree,
    /// It is the main checkout of the repository, which is never removed.
    MainCheckout,
    /// The worktree is locked with `git worktree lock`.
    Locked {
        /// The reason given when it was locked; empty when none was.
        reason: String,
    },
    /// git considers the worktree prunable, typically because its directory
    /// is gone: `git worktree prune` forgets it, removing cannot.
    Prunable {
        /// git's explanation.
        reason: String,
    },
    /// The worktree has changes that are not committed.
    UncommittedChanges {
        /// Changed tracked files, conflicts included.
        changed: usize,
        /// Untracked files and directories that are not ignored.
        untracked: usize,
    },
    /// `HEAD` is detached on a commit that no branch, tag or remote-tracking
    /// branch contains: removing the worktree would lose it.
    UnreachableCommits {
        /// The detached commit.
        head: String,
    },
}

impl fmt::Display for RemovalRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAWorktree => f.write_str("it is not a worktree of this repository"),
            Self::MainCheckout => f.write_str("it is the main checkout of the repository"),
            Self::Locked { reason } if reason.is_empty() => f.write_str("it is locked"),
            Self::Locked { reason } => write!(f, "it is locked: {reason}"),
            Self::Prunable { reason } => write!(f, "git would prune it ({reason})"),
            Self::UncommittedChanges { changed, untracked } => write!(
                f,
                "it has uncommitted changes ({changed} changed, {untracked} untracked)"
            ),
            Self::UnreachableCommits { head } => write!(
                f,
                "HEAD is detached at {head}, which no branch, tag or remote-tracking branch \
                 contains"
            ),
        }
    }
}

fn exit_description(code: Option<i32>) -> String {
    code.map_or_else(
        || "no exit code (stopped by a signal)".to_owned(),
        |code| format!("exit code {code}"),
    )
}

fn stderr_suffix(stderr: &str) -> String {
    if stderr.is_empty() {
        String::new()
    } else {
        format!(": {stderr}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_shows_the_command_the_code_and_stderr() {
        let error = GitError::Failed {
            command: "git -C /work worktree remove -- /work/wt".into(),
            code: Some(128),
            stderr: "fatal: '/work/wt' contains modified or untracked files".into(),
        };
        assert_eq!(
            error.to_string(),
            "`git -C /work worktree remove -- /work/wt` failed with exit code 128: \
             fatal: '/work/wt' contains modified or untracked files"
        );
    }

    #[test]
    fn a_silent_failure_has_no_dangling_colon() {
        let error = GitError::Failed {
            command: "git status".into(),
            code: None,
            stderr: String::new(),
        };
        assert_eq!(
            error.to_string(),
            "`git status` failed with no exit code (stopped by a signal)"
        );
    }

    #[test]
    fn a_timeout_says_how_long_git_was_given() {
        let error = GitError::TimedOut {
            command: "git fetch --quiet origin".into(),
            timeout: Duration::from_secs(300),
        };
        assert_eq!(
            error.to_string(),
            "`git fetch --quiet origin` did not finish within 300s and was stopped"
        );
    }

    #[test]
    fn refusals_say_why_the_worktree_stays() {
        let refused = |reason| GitError::RemovalRefused {
            path: "/work/wt".into(),
            reason,
        };
        let cases = [
            (
                RemovalRefusal::UncommittedChanges {
                    changed: 1,
                    untracked: 2,
                },
                "it has uncommitted changes (1 changed, 2 untracked)",
            ),
            (
                RemovalRefusal::Locked {
                    reason: String::new(),
                },
                "it is locked",
            ),
            (
                RemovalRefusal::Locked {
                    reason: "on a usb disk".into(),
                },
                "it is locked: on a usb disk",
            ),
            (
                RemovalRefusal::UnreachableCommits {
                    head: "6adc11f".into(),
                },
                "HEAD is detached at 6adc11f, which no branch, tag or remote-tracking branch \
                 contains",
            ),
        ];
        for (reason, expected) in cases {
            assert_eq!(
                refused(reason).to_string(),
                format!("refusing to remove `/work/wt`: {expected}")
            );
        }
    }

    #[test]
    fn an_incomplete_worktree_says_it_exists() {
        let error = GitError::WorktreeIncomplete {
            path: "/work/wt".into(),
            source: Box::new(GitError::Failed {
                command: "git switch".into(),
                code: Some(1),
                stderr: String::new(),
            }),
        };
        assert_eq!(
            error.to_string(),
            "the worktree at `/work/wt` was created, but not completed: \
             `git switch` failed with exit code 1"
        );
    }
}
