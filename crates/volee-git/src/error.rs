//! The error type of this crate.

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
    /// A file system operation done around git failed.
    #[error("could not {action} `{}`: {source}", .path.display())]
    Io {
        /// What was being done, such as `create the parent directory of`.
        action: &'static str,
        /// The path it was done to.
        path: PathBuf,
        /// The operating system's reason.
        #[source]
        source: io::Error,
    },
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
}
