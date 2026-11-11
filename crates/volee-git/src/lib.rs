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
//! - [`Git::remove_worktree`] never forces and refuses to remove anything
//!   that would lose work; nothing in this crate ever deletes a branch.
//! - [`Git::add_worktree`] creates the branch only once its worktree exists,
//!   so a failure never leaves a stray branch.
//! - git never waits for a person: standard input is closed, credential and
//!   ssh prompts are disabled, and every command has a timeout after which
//!   git and everything it started are stopped.
//!
//! # The base branch
//!
//! The methods that compare with a base branch (`main`, `develop`...)
//! compare with `origin/<base>` when that remote-tracking branch exists,
//! because agents branch from it and pull requests merge into it; the local
//! branch is used otherwise. Fetch first (see [`Git::fetch`]) for fresh
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
//! use volee_git::{DeliveryQuery, DiffFormat, Git, GitCli};
//!
//! # fn main() -> Result<(), volee_git::GitError> {
//! let git = GitCli::new();
//! let worktree = Path::new("/work/volee/.worktrees/feat-x");
//! let query = DeliveryQuery {
//!     branch: "feat/x",
//!     base: "main",
//!     // The `unmerged_tip` of the agent's previous evidence.
//!     unmerged_tip: None,
//! };
//! let report = git.delivery(worktree, &query)?;
//! let summary = report.delivery.summary(); // "2 commits · not pushed"
//! let stat = git.diff(worktree, "main", DiffFormat::Stat, 64 * 1024)?;
//! # let _ = (summary, stat);
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
mod forge;
mod parse;
mod remote;

pub use cli::GitCli;
pub use error::{GitError, RemovalRefusal};
pub use forge::{BranchHead, Forge, ForgeStatus, GhCli};
pub use remote::RepoSlug;

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
    ///
    /// The worktree is created first, detached, and the branch only once the
    /// worktree exists, so a failure never leaves a stray branch. When the
    /// worktree exists but a later step failed (a failing hook, or a
    /// checkout stopped at its timeout), the error is
    /// [`GitError::WorktreeIncomplete`].
    fn add_worktree(
        &self,
        repo: &Path,
        path: &Path,
        branch: &str,
        base: &str,
    ) -> Result<NewWorktree, GitError>;

    /// Removes the worktree at `path` from the repository that contains
    /// `repo`. A relative `path` is resolved against `repo`.
    ///
    /// Never forces (ADR 0011). Refuses, with [`GitError::RemovalRefused`],
    /// a path that is not a worktree of the repository, the main checkout, a
    /// locked or prunable worktree, a worktree with changes to tracked files
    /// or untracked files (whatever `status.showUntrackedFiles` says), and a
    /// detached `HEAD` that no branch, tag or remote-tracking branch
    /// contains. The worktree's branch is always kept.
    ///
    /// Ignored files (build outputs, `.env` files...) are deleted with the
    /// worktree. git may still refuse in rarer cases, such as a worktree that
    /// contains submodules, reported as [`GitError::Failed`].
    ///
    /// Call this only once the cleanup rule
    /// ([`tidy_verdict`](volee_core::delivery::tidy_verdict)) allows it.
    fn remove_worktree(&self, repo: &Path, path: &Path) -> Result<(), GitError>;

    /// Unlocks the worktree at `path` (`git worktree unlock`), which a
    /// harness may leave locked after its session. Does nothing when it is
    /// not locked; refuses, with [`GitError::RemovalRefused`], a path that is
    /// not a linked worktree of the repository. A relative `path` is resolved
    /// against `repo`.
    ///
    /// Unlocking deletes nothing, but it lifts a protection: call it only
    /// right before [`Git::remove_worktree`], once the cleanup rule allows
    /// the removal.
    fn unlock_worktree(&self, repo: &Path, path: &Path) -> Result<(), GitError>;

    /// The delivery evidence of an agent's branch, checked out in
    /// `worktree`, compared with its base branch (see
    /// [the base branch](crate#the-base-branch)).
    ///
    /// The evidence describes `query.branch`, wherever the worktree's `HEAD`
    /// is: [`DeliveryReport::on_branch`] says when `HEAD` moved off it.
    ///
    /// - `tip`: the commit at the tip of the branch; `None` when the branch
    ///   has no commit or no longer exists.
    /// - `commits`: commits on the branch that the base branch does not have.
    ///   Commits the agent pulled from the base branch do not count.
    /// - `dirty`: a change to a tracked file, or an untracked file that is
    ///   not ignored, in the worktree.
    /// - `diff`: what the branch changes against its merge base with the base
    ///   branch. When the branch is checked out, the working tree is
    ///   compared, so uncommitted changes to tracked files count too.
    ///   Untracked files make the tree dirty and are shown by [`Git::diff`],
    ///   but are not counted here.
    /// - `push`: commits of the branch missing from its upstream, or from
    ///   `origin/<branch>` when it was pushed without setting an upstream.
    ///   An upstream that is a local branch does not count.
    ///   [`PushState::NoUpstream`] when no such remote-tracking branch
    ///   exists.
    /// - `no_remote`: the repository has no remote at all, so there is
    ///   nowhere to push and no pull request to open.
    /// - `unmerged_tip` and `merged_into_base`: proving a merge needs memory
    ///   (see [`Delivery`]). Pass the `unmerged_tip` of the agent's previous
    ///   evidence in `query`. When the branch has commits of its own, the
    ///   returned `unmerged_tip` is its tip and the work is not merged.
    ///   Otherwise the previous one is carried forward, and the work is
    ///   merged when that commit is now part of the base branch. A branch
    ///   never seen with commits of its own is never merged. A squash merge
    ///   is not visible in the history: its evidence is the merged pull
    ///   request, from the forge.
    ///
    /// `pull_request` and `report_delivered` are left at their defaults:
    /// they come from the forge and from messages, not from git.
    ///
    /// The evidence takes several git commands. When the branch moves while
    /// they run (a commit lands), the evidence is read again, so it never
    /// mixes the tree before a commit with the branch after it.
    ///
    /// The base branch is looked up only when there is something to compare:
    /// a branch without commits and without memory needs none, which suits a
    /// repository before its first commit. Otherwise a base that names no
    /// commit is a [`GitError::UnknownRevision`].
    ///
    /// [`PushState::NoUpstream`]: volee_core::delivery::PushState::NoUpstream
    fn delivery(
        &self,
        worktree: &Path,
        query: &DeliveryQuery<'_>,
    ) -> Result<DeliveryReport, GitError>;

    /// The changes in `worktree` against the merge base with `base`, as text
    /// for a person to read, in at most `max_bytes` bytes.
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
    /// meant for display: bytes that are not UTF-8 are replaced, and text
    /// past `max_bytes` is cut at a line break and flagged as
    /// [truncated](DiffOutput::truncated).
    fn diff(
        &self,
        worktree: &Path,
        base: &str,
        format: DiffFormat,
        max_bytes: usize,
    ) -> Result<DiffOutput, GitError>;

    /// Fetches `origin` quietly and without prompting, and forgets the
    /// remote-tracking branches deleted on the remote, so a push state never
    /// relies on a branch that is gone.
    ///
    /// Does nothing when the repository has no `origin` remote, so callers
    /// can fetch before [`Git::add_worktree`] or [`Git::delivery`] without
    /// special-casing local-only repositories.
    fn fetch(&self, repo: &Path) -> Result<(), GitError>;

    /// The repository `origin` points to on its forge, as `owner/name` (from
    /// `git remote get-url origin`, which applies `url.*.insteadOf`); `None`
    /// when there is no `origin` remote or its URL names no `owner/name`,
    /// such as a path on this machine.
    fn origin(&self, repo: &Path) -> Result<Option<RepoSlug>, GitError>;
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
    /// The commit the branch starts at.
    pub start_commit: String,
}

/// What [`Git::delivery`] is asked about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeliveryQuery<'a> {
    /// The agent's branch, without `refs/heads/`.
    pub branch: &'a str,
    /// The branch the work goes into, such as `main`.
    pub base: &'a str,
    /// The [`unmerged_tip`](Delivery::unmerged_tip) of the agent's previous
    /// evidence, if any: the memory that proves a merge.
    pub unmerged_tip: Option<&'a str>,
}

/// The answer of [`Git::delivery`]: the evidence, and what it was computed
/// from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryReport {
    /// The evidence, ready for a
    /// [`DeliveryUpdated`](volee_core::event::EventKind::DeliveryUpdated)
    /// event.
    pub delivery: Delivery,
    /// What the worktree has checked out.
    pub head: Head,
    /// Whether the worktree has the agent's branch checked out. When it does
    /// not (`HEAD` detached, or another branch checked out), the evidence
    /// still describes the branch, and `dirty` describes the worktree.
    pub on_branch: bool,
    /// The base branch the commits were counted against, such as
    /// `origin/main`; `None` when no comparison was needed.
    pub base: Option<String>,
    /// The remote-tracking branch the push state compares with, such as
    /// `origin/feat`; `None` when there is none.
    pub upstream: Option<String>,
}

/// What a worktree has checked out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    /// The checked-out branch, without `refs/heads/`; `None` when detached.
    pub branch: Option<String>,
    /// The checked-out commit; `None` before the first commit.
    pub commit: Option<String>,
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

/// The text returned by [`Git::diff`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffOutput {
    /// The diff, for display.
    pub text: String,
    /// Whether the diff was longer than the limit asked for and `text` holds
    /// only its beginning, cut at a line break.
    pub truncated: bool,
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
        let report = DeliveryReport {
            delivery: Delivery::default(),
            head: Head {
                branch: None,
                commit: Some("6adc11fa9619748617663ded520ce631e9292269".into()),
            },
            on_branch: false,
            base: Some("origin/main".into()),
            upstream: None,
        };
        let json = serde_json::to_string(&report).unwrap();
        assert_eq!(
            serde_json::from_str::<DeliveryReport>(&json).unwrap(),
            report
        );
    }
}
