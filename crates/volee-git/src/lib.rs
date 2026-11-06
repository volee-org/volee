//! Git queries of volee: repositories, worktrees and delivery evidence.
//!
//! The daemon learns everything it needs from git through the [`Git`] trait
//! ([ADR 0010]): which repository, worktree and branch a directory belongs
//! to, the worktrees of a repository, and the delivery evidence of an agent's
//! work (commits, uncommitted changes, diff size, push and merge state) that
//! the cleanup rule of [ADR 0011] relies on. It also creates and removes the
//! worktrees volee launches agents in, and fetches.
//!
//! [`GitCli`] implements the trait by running the `git` executable, so every
//! answer is the one the user's own git would give, configuration included.
//! Code that depends on git takes a `&dyn Git` (or a generic `G: Git`) and is
//! tested with a fake implementation.
//!
//! # Safety
//!
//! - Queries never change a repository: they take no optional lock, so they
//!   never refresh the index of a worktree an agent is working in.
//! - [`Git::remove_worktree`] never forces, and nothing in this crate ever
//!   deletes a branch.
//! - git never waits for a person: standard input is closed, credential
//!   prompts are disabled and every command has a timeout.
//!
//! # The base branch
//!
//! The methods that compare a worktree with its base branch (`main`,
//! `develop`...) compare with `origin/<base>` when that remote-tracking branch
//! exists, because agents branch from it and pull requests merge into it; the
//! local branch is used otherwise. Fetch first (see [`Git::fetch`]) for fresh
//! answers.
//!
//! # Requirements
//!
//! git 2.40 or later, on macOS or Linux, as listed in the
//! [installation guide](https://volee-org.github.io/volee/guide/installation.html).
//!
//! # Example
//!
//! ```no_run
//! use std::path::Path;
//!
//! use volee_git::{DiffFormat, Git, GitCli};
//!
//! # fn main() -> Result<(), volee_git::GitError> {
//! let git = GitCli::new();
//! if let Some(repo) = git.repo_info(Path::new("."))? {
//!     let delivery = git.delivery(&repo.top_level, "main", None)?;
//!     let summary = delivery.summary(); // "2 commits · not pushed · uncommitted changes"
//!     let stat = git.diff(&repo.top_level, "main", DiffFormat::Stat)?;
//! #   let _ = (summary, stat);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! [ADR 0010]: https://volee-org.github.io/volee/adr/0010-git-access.html
//! [ADR 0011]: https://volee-org.github.io/volee/adr/0011-safety-and-delivery-evidence.html

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use volee_core::delivery::Delivery;

mod cli;
mod command;
mod error;
mod parse;

pub use cli::GitCli;
pub use error::GitError;

/// Everything volee asks of git.
///
/// Implementations must be usable from several threads at once: the daemon
/// refreshes many worktrees concurrently. Every method is blocking; async
/// callers run them on a blocking thread pool.
///
/// Directories may be given as relative paths, which are resolved against
/// the current directory of the process, except where a method says
/// otherwise.
pub trait Git: Send + Sync {
    /// Where `dir` sits in a git repository.
    ///
    /// Returns `None` when `dir` is not inside the working tree of a
    /// repository: outside any repository, inside a bare repository or
    /// inside a `.git` directory. A directory that does not exist is an
    /// error, not `None`.
    fn repo_info(&self, dir: &Path) -> Result<Option<RepoInfo>, GitError>;

    /// The worktrees of the repository that contains `repo`, the main
    /// checkout (or the bare repository) first, as `git worktree list`
    /// reports them.
    fn worktrees(&self, repo: &Path) -> Result<Vec<WorktreeInfo>, GitError>;

    /// The branch new work should start from.
    ///
    /// This is the branch `origin/HEAD` points to when the remote's default
    /// branch is known locally, otherwise the first of `main`, `master` and
    /// `develop` that exists as a local branch, otherwise `None`.
    fn default_branch(&self, repo: &Path) -> Result<Option<String>, GitError>;

    /// Creates `branch` from `base` and checks it out in a new worktree at
    /// `path`.
    ///
    /// The branch starts from `origin/<base>` when that remote-tracking
    /// branch exists (fetch first), from `base` otherwise, and has no
    /// upstream until the agent pushes it. A relative `path` is resolved
    /// against `repo`.
    ///
    /// Refuses, before touching the repository, when `branch` is not a valid
    /// branch name ([`GitError::InvalidBranchName`]), when it already exists
    /// ([`GitError::BranchExists`]), when `path` already exists
    /// ([`GitError::PathExists`]) or when `base` names no commit
    /// ([`GitError::UnknownRevision`]).
    fn add_worktree(
        &self,
        repo: &Path,
        path: &Path,
        branch: &str,
        base: &str,
    ) -> Result<NewWorktree, GitError>;

    /// Removes the worktree at `path` from the repository that contains
    /// `repo`.
    ///
    /// Never forces (ADR 0011): git refuses to remove a worktree with
    /// uncommitted or untracked files, a locked worktree or the main
    /// checkout, and that refusal is returned as an error. The worktree's
    /// branch is always kept. A relative `path` is resolved against `repo`.
    ///
    /// Call this only once the cleanup rule
    /// ([`tidy_verdict`](volee_core::delivery::tidy_verdict)) allows it.
    fn remove_worktree(&self, repo: &Path, path: &Path) -> Result<(), GitError>;

    /// The delivery evidence of the work in `worktree`, compared with the
    /// base branch `base` (see [the base branch](crate#the-base-branch)).
    ///
    /// - `commits`: commits since `start_commit`, or since the merge base
    ///   with the base branch when the start commit is unknown or no longer
    ///   exists.
    /// - `dirty`: any change to a tracked file, or any untracked file that
    ///   is not ignored.
    /// - `diff`: the working tree against the merge base, so committed and
    ///   uncommitted changes to tracked files both count. Untracked files
    ///   make the tree dirty and are shown by [`Git::diff`], but are not
    ///   counted here.
    /// - `push`: compared with the branch's upstream, or with
    ///   `origin/<branch>` when the branch was pushed without setting an
    ///   upstream. [`PushState::NoUpstream`] when neither exists, including
    ///   for a detached `HEAD`.
    /// - `merged_into_base`: the branch has at least one commit (as counted
    ///   above) and its tip is an ancestor of the base branch. A branch with
    ///   no commit is never merged. Without a start commit, a merged branch
    ///   cannot be told apart from an empty one, so it is reported as not
    ///   merged: absence of evidence keeps the worktree (ADR 0011).
    ///
    /// `pull_request` and `report_delivered` are left at their defaults:
    /// they come from the forge and from messages, not from git.
    ///
    /// Before the first commit, only `dirty` and `diff` (against the empty
    /// tree) are filled, and the base branch is not looked up, since it may
    /// not exist yet. Otherwise a base that names no commit is a
    /// [`GitError::UnknownRevision`].
    ///
    /// [`PushState::NoUpstream`]: volee_core::delivery::PushState::NoUpstream
    fn delivery(
        &self,
        worktree: &Path,
        base: &str,
        start_commit: Option<&str>,
    ) -> Result<Delivery, GitError>;

    /// The changes in `worktree` against the merge base with `base`, as text
    /// for a person to read.
    ///
    /// This is `git diff <merge-base>`: committed and uncommitted changes to
    /// tracked files. Untracked files that are not ignored come after it, so
    /// the files an agent created are visible: listed in
    /// [`DiffFormat::Stat`], shown as new-file diffs in
    /// [`DiffFormat::Patch`]. Implementations may cap how many untracked
    /// files they show, and then say how many were left out.
    ///
    /// Without a merge base (before the first commit, or for unrelated
    /// histories), everything is compared with the empty tree. The output is
    /// meant for display: bytes that are not UTF-8 are replaced.
    fn diff(&self, worktree: &Path, base: &str, format: DiffFormat) -> Result<String, GitError>;

    /// Fetches `origin` quietly, without prompting for credentials.
    ///
    /// Does nothing when the repository has no `origin` remote, so callers
    /// can fetch before [`Git::add_worktree`] or [`Git::delivery`] without
    /// special-casing local-only repositories.
    fn fetch(&self, repo: &Path) -> Result<(), GitError>;
}

/// Where a directory sits in a git repository: see [`Git::repo_info`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoInfo {
    /// The root of the working tree that contains the directory: the main
    /// checkout or a linked worktree.
    pub top_level: PathBuf,
    /// The main checkout of the repository. It is the same for all of the
    /// repository's worktrees, so it identifies the repository. For a bare
    /// repository with linked worktrees, it is the bare repository itself,
    /// as `git worktree list` reports it.
    pub main_checkout: PathBuf,
    /// A display name for the repository: the directory name of the main
    /// checkout, without a `.git` suffix. For a hidden bare repository such
    /// as `project/.bare`, the name of the directory that holds it.
    pub name: String,
    /// Whether `top_level` is a linked worktree (created by `git worktree
    /// add`) rather than the main checkout.
    pub is_linked_worktree: bool,
    /// The checked-out branch, without `refs/heads/`; `None` when `HEAD` is
    /// detached.
    pub branch: Option<String>,
    /// The checked-out commit; `None` before the first commit.
    pub head: Option<String>,
}

/// One worktree of a repository: see [`Git::worktrees`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeInfo {
    /// The worktree's directory.
    pub path: PathBuf,
    /// The checked-out commit; `None` for a bare repository and before the
    /// first commit.
    pub head: Option<String>,
    /// The checked-out branch, without `refs/heads/`; `None` when detached
    /// and for a bare repository.
    pub branch: Option<String>,
    /// Whether `HEAD` is detached.
    pub detached: bool,
    /// Whether this entry is a bare repository rather than a working tree.
    pub bare: bool,
    /// `Some` when the worktree is locked against pruning and removal, with
    /// the reason given to `git worktree lock` (empty when none was given).
    pub locked: Option<String>,
    /// `Some` when git would prune this worktree, typically because its
    /// directory is gone, with git's explanation.
    pub prunable: Option<String>,
}

/// A worktree created by [`Git::add_worktree`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewWorktree {
    /// What the branch was created from: `origin/<base>` or `<base>`.
    pub start_point: String,
    /// The commit the branch starts at. Record it as the workspace's start
    /// commit, so [`Git::delivery`] counts the agent's commits only.
    pub start_commit: String,
}

/// How much of a diff [`Git::diff`] returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffFormat {
    /// One line per changed file and a summary (`git diff --stat`), then the
    /// untracked files.
    #[default]
    Stat,
    /// The full unified diff, then the content of the untracked files as
    /// new-file diffs.
    Patch,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trait_is_object_safe_and_the_cli_is_shareable() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GitCli>();
        let git: Box<dyn Git> = Box::new(GitCli::new());
        drop(git);
    }

    #[test]
    fn types_round_trip_through_serde() {
        assert_eq!(
            serde_json::to_string(&DiffFormat::Patch).unwrap(),
            "\"patch\""
        );
        let worktree = WorktreeInfo {
            path: PathBuf::from("/work/volee"),
            branch: Some("feat/git".into()),
            locked: Some(String::new()),
            ..WorktreeInfo::default()
        };
        let json = serde_json::to_string(&worktree).unwrap();
        assert_eq!(
            serde_json::from_str::<WorktreeInfo>(&json).unwrap(),
            worktree
        );
    }
}
