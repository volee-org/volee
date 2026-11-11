//! [`GitCli`]: the [`Git`] implementation that runs the `git` executable.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::Duration;

use volee_core::delivery::{Delivery, DiffStat, PushState};

use crate::command::{Completed, Request, Runner};
use crate::parse::{self, StatusSummary};
use crate::{
    DeliveryQuery, DeliveryReport, DiffFormat, DiffOutput, Git, GitError, Head, NewWorktree,
    RemovalRefusal, RepoInfo, RepoSlug, WorktreeInfo,
};

/// The most untracked files [`Git::diff`] lists or shows. The others are
/// counted in a final line, so a forgotten build directory cannot flood the
/// output or start thousands of processes.
const MAX_UNTRACKED_FILES: usize = 100;

/// How many times [`Git::delivery`] reads the evidence before it accepts a
/// read during which the branch moved. A branch rarely moves twice within
/// one read; an agent that commits in a tight loop gets its latest read.
const SNAPSHOT_ATTEMPTS: usize = 3;

/// Diff options that keep the output the same whatever the person's
/// configuration says: no colors, no external diff program, paths from the
/// top of the worktree, and the usual `a/` and `b/` prefixes.
const STABLE_DIFF: [&str; 5] = [
    "--no-color",
    "--no-ext-diff",
    "--no-relative",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

/// Answers [`Git`] queries by running the `git` executable.
///
/// Every command runs as `git -C <dir> --no-optional-locks ...`, with
/// standard input closed, `LC_ALL=C`, prompts disabled
/// (`GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=never`,
/// `SSH_ASKPASS_REQUIRE=never`, and ssh in batch mode for fetches and
/// checkouts unless the person chose their own ssh command), and without the
/// environment variables that would tie git to another repository
/// (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`...). Only porcelain and
/// plumbing formats are parsed. The person's git configuration applies, so
/// the answers match what they see, except where it would change the
/// evidence (untracked files are always looked for) or the format of a diff.
///
/// Queries, writes and fetches have their own timeouts. A command that
/// overruns its timeout is stopped with everything it started: SIGTERM
/// first, so git can clean up, then SIGKILL. At most
/// [`max_concurrent`](GitCli::max_concurrent) commands run at the same time
/// across all the threads that share one `GitCli`.
///
/// # Example
///
/// ```
/// use std::time::Duration;
///
/// use volee_git::GitCli;
///
/// let git = GitCli::with_program("/usr/bin/git").timeout(Duration::from_secs(10));
/// ```
#[derive(Debug)]
pub struct GitCli {
    runner: Runner,
    timeout: Duration,
    write_timeout: Duration,
    fetch_timeout: Duration,
}

impl GitCli {
    /// How long a query may run before it is stopped, unless set with
    /// [`GitCli::timeout`].
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

    /// How long creating or removing a worktree may run before it is
    /// stopped, unless set with [`GitCli::write_timeout`]. A checkout can be
    /// slow: large repositories, Git LFS filters, hooks.
    pub const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(600);

    /// How long a fetch may run before it is stopped, unless set with
    /// [`GitCli::fetch_timeout`].
    pub const DEFAULT_FETCH_TIMEOUT: Duration = Duration::from_secs(300);

    /// How many commands may run at the same time, unless set with
    /// [`GitCli::max_concurrent`].
    pub const DEFAULT_MAX_CONCURRENT: NonZeroUsize = NonZeroUsize::new(4).expect("4 is not zero");

    /// Runs `git` from the `PATH`.
    pub fn new() -> Self {
        Self::with_program("git")
    }

    /// Runs the given git executable.
    pub fn with_program(program: impl Into<PathBuf>) -> Self {
        Self {
            runner: Runner::git(program.into(), Self::DEFAULT_MAX_CONCURRENT),
            timeout: Self::DEFAULT_TIMEOUT,
            write_timeout: Self::DEFAULT_WRITE_TIMEOUT,
            fetch_timeout: Self::DEFAULT_FETCH_TIMEOUT,
        }
    }

    /// Sets an environment variable for every git command. It wins over the
    /// variables `GitCli` sets or removes itself.
    ///
    /// Tests use it to keep git away from the person's configuration, for
    /// example with `GIT_CONFIG_GLOBAL=/dev/null` and a temporary `HOME`.
    #[must_use]
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.runner.set_env(key.into(), value.into());
        self
    }

    /// Sets how long a query may run before it is stopped.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets how long creating or removing a worktree may run before it is
    /// stopped.
    #[must_use]
    pub fn write_timeout(mut self, timeout: Duration) -> Self {
        self.write_timeout = timeout;
        self
    }

    /// Sets how long a fetch may run before it is stopped.
    #[must_use]
    pub fn fetch_timeout(mut self, timeout: Duration) -> Self {
        self.fetch_timeout = timeout;
        self
    }

    /// Sets how many git commands may run at the same time. Callers beyond
    /// the limit wait for a running command to finish.
    #[must_use]
    pub fn max_concurrent(mut self, limit: NonZeroUsize) -> Self {
        self.runner.set_max_concurrent(limit);
        self
    }

    /// A query in `dir`.
    fn git<'a>(&'a self, dir: &'a Path) -> Request<'a> {
        self.runner.request(dir, self.timeout)
    }

    /// A command in `dir` that writes to the repository.
    fn write<'a>(&'a self, dir: &'a Path) -> Request<'a> {
        self.runner.request(dir, self.write_timeout)
    }

    /// The commit `revision` names, or `None` when it names none.
    fn commit_of(&self, dir: &Path, revision: &str) -> Result<Option<String>, GitError> {
        let completed = self
            .git(dir)
            .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
            .arg(format!("{revision}^{{commit}}"))
            .run()?;
        match completed.code {
            Some(0) => completed.object_id().map(Some),
            Some(1) => Ok(None),
            _ => Err(completed.failure()),
        }
    }

    /// The branch to compare with: `origin/<base>` when it exists, `<base>`
    /// otherwise.
    fn resolve_base(&self, dir: &Path, base: &str) -> Result<Base, GitError> {
        if !base.is_empty() {
            let remote = format!("origin/{base}");
            if let Some(commit) = self.commit_of(dir, &format!("refs/remotes/{remote}"))? {
                return Ok(Base {
                    name: remote,
                    commit,
                });
            }
            if let Some(commit) = self.commit_of(dir, base)? {
                return Ok(Base {
                    name: base.to_owned(),
                    commit,
                });
            }
        }
        Err(GitError::UnknownRevision {
            revision: base.to_owned(),
        })
    }

    /// The best common ancestor of two commits; `None` for unrelated
    /// histories.
    fn merge_base(&self, dir: &Path, one: &str, two: &str) -> Result<Option<String>, GitError> {
        let completed = self.git(dir).args(["merge-base", one, two]).run()?;
        match completed.code {
            Some(0) => completed.object_id().map(Some),
            Some(1) => Ok(None),
            _ => Err(completed.failure()),
        }
    }

    /// Whether `ancestor` is `descendant` or one of its ancestors.
    fn is_ancestor(&self, dir: &Path, ancestor: &str, descendant: &str) -> Result<bool, GitError> {
        let completed = self
            .git(dir)
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .run()?;
        match completed.code {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(completed.failure()),
        }
    }

    /// Commits reachable from `tip` and not from `exclude`.
    fn count_commits(&self, dir: &Path, tip: &str, exclude: Option<&str>) -> Result<u32, GitError> {
        let mut request = self.git(dir).args(["rev-list", "--count", tip]);
        if let Some(exclude) = exclude {
            request = request.arg(format!("^{exclude}"));
        }
        request.parse(|stdout| parse::parse_count(&String::from_utf8_lossy(stdout)))
    }

    /// What a working tree has checked out and what it changes. Untracked
    /// files are always looked for, whatever `status.showUntrackedFiles`
    /// says, and changes inside submodules count, whatever
    /// `diff.ignoreSubmodules` says.
    fn status(&self, dir: &Path) -> Result<StatusSummary, GitError> {
        self.git(dir)
            .args([
                "status",
                "--porcelain=v2",
                "--branch",
                "--no-ahead-behind",
                "--untracked-files=normal",
                "--ignore-submodules=none",
                "-z",
            ])
            .parse(|stdout| Ok(parse::parse_status_porcelain_v2(stdout)))
    }

    /// The size of the changes from `from` to `to`, or to the working tree
    /// when `to` is `None`.
    fn shortstat(&self, dir: &Path, from: &str, to: Option<&str>) -> Result<DiffStat, GitError> {
        self.git(dir)
            .args(["diff", "--shortstat"])
            .args(STABLE_DIFF)
            .arg(from)
            .args(to)
            .arg("--")
            .parse(|stdout| parse::parse_shortstat(&String::from_utf8_lossy(stdout)))
    }

    /// The empty tree, to diff against when there is no merge base: before
    /// the first commit, or for unrelated histories. Asking git keeps it
    /// right for SHA-256 repositories.
    fn empty_tree(&self, dir: &Path) -> Result<String, GitError> {
        self.git(dir)
            .args(["hash-object", "-t", "tree", "/dev/null"])
            .run()?
            .success()?
            .object_id()
    }

    /// Where a diff of the working tree of `dir` against `base` starts: the
    /// merge base of `HEAD` and the base, or the empty tree when there is
    /// none. Before the first commit, the base is not looked up: it may not
    /// exist yet, and everything is new anyway.
    fn diff_origin(&self, dir: &Path, base: &str) -> Result<String, GitError> {
        let merge_base = match self.commit_of(dir, "HEAD")? {
            Some(head) => {
                let base = self.resolve_base(dir, base)?;
                self.merge_base(dir, &base.commit, &head)?
            }
            None => None,
        };
        match merge_base {
            Some(commit) => Ok(commit),
            None => self.empty_tree(dir),
        }
    }

    /// What the agent's branch changes against its merge base: the working
    /// tree when the branch is checked out, so uncommitted changes count, and
    /// its tip otherwise.
    fn branch_diff(
        &self,
        dir: &Path,
        tip: Option<&str>,
        merge_base: Option<&str>,
        on_branch: bool,
    ) -> Result<DiffStat, GitError> {
        if tip.is_none() && !on_branch {
            return Ok(DiffStat::default());
        }
        let from = match merge_base {
            Some(commit) => commit.to_owned(),
            None => self.empty_tree(dir)?,
        };
        let to = if on_branch { None } else { tip };
        self.shortstat(dir, &from, to)
    }

    /// The push state of `branch` at `tip`, and the remote-tracking branch
    /// it was compared with: its upstream when that is a remote-tracking
    /// branch, otherwise `origin/<branch>`, for a branch pushed without
    /// `-u`.
    fn push_state(
        &self,
        dir: &Path,
        branch: &str,
        tip: &str,
    ) -> Result<(PushState, Option<String>), GitError> {
        let configured = self
            .git(dir)
            .args(["for-each-ref", "--format=%(upstream)"])
            .arg(format!("refs/heads/{branch}"))
            .run()?
            .success()?
            .text()?;
        let candidates = parse::remote_upstream(&configured)
            .into_iter()
            .chain([format!("refs/remotes/origin/{branch}")]);
        for reference in candidates {
            if let Some(remote) = self.commit_of(dir, &reference)? {
                let ahead = self.count_commits(dir, tip, Some(&remote))?;
                return Ok((
                    parse::push_state(ahead),
                    Some(parse::remote_branch_name(&reference)),
                ));
            }
        }
        Ok((PushState::NoUpstream, None))
    }

    /// The checked-out branch, `None` when `HEAD` is detached.
    fn current_branch(&self, dir: &Path) -> Result<Option<String>, GitError> {
        let completed = self
            .git(dir)
            .args(["symbolic-ref", "--quiet", "HEAD"])
            .run()?;
        match completed.code {
            Some(0) => Ok(Some(parse::branch_name(&completed.text()?))),
            Some(1) => Ok(None),
            _ => Err(completed.failure()),
        }
    }

    /// The main checkout of a repository seen from one of its linked
    /// worktrees.
    fn main_checkout_of_linked(&self, dir: &Path, common_dir: &Path) -> Result<PathBuf, GitError> {
        if let Some(checkout) = parse::checkout_of_git_dir(common_dir) {
            return Ok(checkout);
        }
        // A submodule records its working tree in `core.worktree`, relative
        // to its git directory.
        let completed = self
            .git(dir)
            .args(["config", "--get", "core.worktree"])
            .run()?;
        match completed.code {
            Some(0) => {
                let checkout = common_dir.join(completed.parse(parse::parse_path_line)?);
                Ok(fs::canonicalize(&checkout).unwrap_or(checkout))
            }
            // A bare repository: like `git worktree list`, name the
            // repository itself.
            Some(1) => Ok(common_dir.to_path_buf()),
            _ => Err(completed.failure()),
        }
    }

    fn check_branch_name(&self, repo: &Path, branch: &str) -> Result<(), GitError> {
        let invalid = || GitError::InvalidBranchName {
            name: branch.to_owned(),
        };
        // git refuses these too, but a leading dash reaches `git branch` as
        // an option and fails with a usage message instead of a reason.
        if branch.is_empty() || branch.starts_with('-') || branch == "HEAD" {
            return Err(invalid());
        }
        let completed = self
            .git(repo)
            .arg("check-ref-format")
            .arg(format!("refs/heads/{branch}"))
            .run()?;
        match completed.code {
            Some(0) => Ok(()),
            Some(1) => Err(invalid()),
            _ => Err(completed.failure()),
        }
    }

    /// The environment that makes ssh fail instead of asking for a password,
    /// a passphrase or a host key confirmation. Empty when the person chose
    /// their own ssh command (`GIT_SSH_COMMAND`, `GIT_SSH` or
    /// `core.sshCommand`), which is then left alone.
    fn ssh_env(&self, repo: &Path) -> Result<Vec<(&'static str, &'static str)>, GitError> {
        if self.runner.person_sets("GIT_SSH_COMMAND") || self.runner.person_sets("GIT_SSH") {
            return Ok(Vec::new());
        }
        let configured = self
            .git(repo)
            .args(["config", "--get", "core.sshCommand"])
            .run()?;
        match configured.code {
            Some(0) => Ok(Vec::new()),
            Some(1) => Ok(vec![("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")]),
            _ => Err(configured.failure()),
        }
    }

    /// Why the worktree at `path` must stay, if it must.
    fn removal_refusal(
        &self,
        repo: &Path,
        path: &Path,
    ) -> Result<Option<RemovalRefusal>, GitError> {
        let worktrees = self.worktrees(repo)?;
        let Some((position, worktree)) = worktrees
            .iter()
            .enumerate()
            .find(|(_, worktree)| same_path(&worktree.path, path))
        else {
            return Ok(Some(RemovalRefusal::NotAWorktree));
        };
        if position == 0 {
            return Ok(Some(RemovalRefusal::MainCheckout));
        }
        if let Some(reason) = &worktree.locked {
            return Ok(Some(RemovalRefusal::Locked {
                reason: reason.clone(),
            }));
        }
        if let Some(reason) = &worktree.prunable {
            return Ok(Some(RemovalRefusal::Prunable {
                reason: reason.clone(),
            }));
        }
        let status = self.status(path)?;
        if status.is_dirty() {
            return Ok(Some(RemovalRefusal::UncommittedChanges {
                changed: status.changed,
                untracked: status.untracked,
            }));
        }
        if let (true, Some(head)) = (worktree.detached, &worktree.head) {
            // The worktree's own `HEAD` and reflog go with it: a detached
            // commit is lost unless a shared reference keeps it.
            let keeper = self
                .git(repo)
                .args([
                    "for-each-ref",
                    "--count=1",
                    "--format=%(refname)",
                    "--contains",
                ])
                .arg(head)
                .args(["refs/heads/", "refs/tags/", "refs/remotes/"])
                .run()?
                .success()?
                .text()?;
            if keeper.is_empty() {
                return Ok(Some(RemovalRefusal::UnreachableCommits {
                    head: head.clone(),
                }));
            }
        }
        Ok(None)
    }

    /// Reads the delivery evidence once: see [`Git::delivery`].
    fn read_delivery(
        &self,
        worktree: &Path,
        query: &DeliveryQuery<'_>,
    ) -> Result<DeliveryReport, GitError> {
        let status = self.status(worktree)?;
        let on_branch = status.branch.as_deref() == Some(query.branch);
        let tip = self.commit_of(worktree, &format!("refs/heads/{}", query.branch))?;
        let previous = match query.unmerged_tip {
            Some(revision) => self.commit_of(worktree, revision)?,
            None => None,
        };
        let base = if tip.is_some() || previous.is_some() {
            Some(self.resolve_base(worktree, query.base)?)
        } else {
            None
        };
        let (commits, merge_base) = match (&tip, &base) {
            (Some(tip), Some(base)) => (
                self.count_commits(worktree, tip, Some(&base.commit))?,
                self.merge_base(worktree, &base.commit, tip)?,
            ),
            _ => (0, None),
        };
        let diff = self.branch_diff(worktree, tip.as_deref(), merge_base.as_deref(), on_branch)?;
        let (push, upstream) = match &tip {
            Some(tip) => self.push_state(worktree, query.branch, tip)?,
            None => (PushState::NoUpstream, None),
        };
        // The memory that proves a merge: the tip while it has commits of
        // its own, the previous one otherwise.
        let unmerged_tip = if commits > 0 {
            tip.clone()
        } else {
            query.unmerged_tip.map(str::to_owned)
        };
        let merged_into_base = match (commits, &previous, &base) {
            (0, Some(previous), Some(base)) => {
                self.is_ancestor(worktree, previous, &base.commit)?
            }
            _ => false,
        };
        // A repository without any remote: nowhere to push, no forge.
        let no_remote = self
            .git(worktree)
            .arg("remote")
            .run()?
            .success()?
            .text()?
            .trim()
            .is_empty();
        Ok(DeliveryReport {
            delivery: Delivery {
                commits,
                dirty: status.is_dirty(),
                diff,
                push,
                merged_into_base,
                tip,
                unmerged_tip,
                no_remote,
                ..Delivery::default()
            },
            head: Head {
                branch: status.branch,
                commit: status.oid,
            },
            on_branch,
            base: base.map(|base| base.name),
            upstream,
        })
    }

    /// Whether `branch` held still while `report` was read: it still points
    /// at the tip the report used, and when it is checked out, `HEAD` (as the
    /// status of the tree saw it) was that tip. Otherwise a commit landed in
    /// between, and the report mixes two moments.
    fn held_still(
        &self,
        worktree: &Path,
        branch: &str,
        report: &DeliveryReport,
    ) -> Result<bool, GitError> {
        if report.on_branch && report.head.commit != report.delivery.tip {
            return Ok(false);
        }
        let now = self.commit_of(worktree, &format!("refs/heads/{branch}"))?;
        Ok(now == report.delivery.tip)
    }

    /// The error for a failed step of [`Git::add_worktree`]:
    /// [`GitError::WorktreeIncomplete`] when the worktree exists anyway, so
    /// the caller knows it is there.
    fn creation_failure(&self, repo: &Path, path: &Path, error: GitError) -> GitError {
        let registered = self.worktrees(repo).is_ok_and(|worktrees| {
            worktrees
                .iter()
                .any(|worktree| same_path(&worktree.path, path))
        });
        if registered || path.exists() {
            GitError::WorktreeIncomplete {
                path: path.to_path_buf(),
                source: Box::new(error),
            }
        } else {
            error
        }
    }

    /// Untracked files that are not ignored, relative to `top`. With
    /// `collapse`, an untracked directory is one entry, as in `git status`.
    fn untracked_files(&self, top: &Path, collapse: bool) -> Result<Vec<PathBuf>, GitError> {
        let mut request = self
            .git(top)
            .args(["ls-files", "--others", "--exclude-standard", "-z"]);
        if collapse {
            request = request.args(["--directory", "--no-empty-directory"]);
        }
        request.parse(|stdout| Ok(parse::parse_nul_paths(stdout)))
    }

    /// Appends the untracked files of `top` as new-file diffs, then the
    /// list of those that are not shown: nested repositories (listed as
    /// directories, which git cannot diff against nothing), files beyond
    /// [`MAX_UNTRACKED_FILES`] and files git cannot read.
    fn append_untracked_patches(&self, top: &Path, out: &mut Bounded) -> Result<(), GitError> {
        let entries = self.untracked_files(top, false)?;
        let (mut left_out, files): (Vec<&Path>, Vec<&Path>) = entries
            .iter()
            .map(PathBuf::as_path)
            .partition(|path| parse::names_directory(path));
        for (index, file) in files.into_iter().enumerate() {
            if out.is_full() {
                return Ok(());
            }
            if index >= MAX_UNTRACKED_FILES {
                left_out.push(file);
                continue;
            }
            let completed = self
                .git(top)
                .args(["diff", "--no-index"])
                .args(STABLE_DIFF)
                .args(["--", "/dev/null"])
                .arg(file)
                .max_stdout(out.room())
                .run()?;
            // `--no-index` exits with 1 when the files differ, as they do.
            if matches!(completed.code, Some(0 | 1)) || completed.truncated {
                out.push(&completed.stdout);
            } else {
                // One unreadable file must not hide the rest of the diff.
                tracing::debug!(error = %completed.failure(), "untracked file not shown");
                left_out.push(file);
            }
        }
        out.push_list("Untracked files not shown:", &left_out);
        Ok(())
    }
}

impl Default for GitCli {
    fn default() -> Self {
        Self::new()
    }
}

impl Git for GitCli {
    fn repo_info(&self, dir: &Path) -> Result<Option<RepoInfo>, GitError> {
        let probe = self
            .git(dir)
            .args([
                "rev-parse",
                "--path-format=absolute",
                "--is-inside-work-tree",
                "--git-dir",
                "--git-common-dir",
            ])
            .run()?;
        if probe.code != Some(0) {
            return if parse::reports_no_repository(&probe.stderr) {
                Ok(None)
            } else {
                Err(probe.failure())
            };
        }
        let (inside, git_dir, common_dir) = probe.parse(parse::parse_repository_probe)?;
        if !inside {
            // A bare repository or a `.git` directory: no working tree.
            return Ok(None);
        }
        let top_level = self
            .git(dir)
            .args(["rev-parse", "--show-toplevel"])
            .parse(parse::parse_path_line)?;
        let is_linked_worktree = git_dir != common_dir;
        // Outside linked worktrees, the top level is the main checkout, even
        // for a submodule or a repository whose git directory lives
        // elsewhere.
        let main_checkout = if is_linked_worktree {
            self.main_checkout_of_linked(dir, &common_dir)?
        } else {
            top_level.clone()
        };
        Ok(Some(RepoInfo {
            name: parse::repository_name(&main_checkout),
            branch: self.current_branch(dir)?,
            head: self.commit_of(dir, "HEAD")?,
            top_level,
            main_checkout,
            is_linked_worktree,
        }))
    }

    fn worktrees(&self, repo: &Path) -> Result<Vec<WorktreeInfo>, GitError> {
        self.git(repo)
            .args(["worktree", "list", "--porcelain", "-z"])
            .parse(parse::parse_worktree_porcelain)
    }

    fn default_branch(&self, repo: &Path) -> Result<Option<String>, GitError> {
        let origin_head = self
            .git(repo)
            .args(["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"])
            .run()?;
        match origin_head.code {
            Some(0) => {
                let target = origin_head.text()?;
                if let Some(branch) = target.strip_prefix("refs/remotes/origin/") {
                    return Ok(Some(branch.to_owned()));
                }
            }
            Some(1) => {}
            _ => return Err(origin_head.failure()),
        }
        let references = self
            .git(repo)
            .args([
                "for-each-ref",
                "--format=%(refname)",
                "refs/heads/main",
                "refs/heads/master",
                "refs/heads/develop",
            ])
            .run()?
            .success()?
            .text()?;
        Ok(parse::pick_default_branch(&references))
    }

    fn add_worktree(
        &self,
        repo: &Path,
        path: &Path,
        branch: &str,
        base: &str,
    ) -> Result<NewWorktree, GitError> {
        self.check_branch_name(repo, branch)?;
        if self
            .commit_of(repo, &format!("refs/heads/{branch}"))?
            .is_some()
        {
            return Err(GitError::BranchExists {
                branch: branch.to_owned(),
            });
        }
        let path = absolute(repo, path)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => return Err(GitError::PathExists { path }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(GitError::Io {
                    action: "inspect",
                    path,
                    source,
                });
            }
        }
        let start = self.resolve_base(repo, base)?;
        // A checkout may fetch large files over ssh (Git LFS).
        let ssh = self.ssh_env(repo)?;
        // Detached first: if git fails or is stopped while it checks out, no
        // branch exists yet.
        let added = self
            .write(repo)
            .args(["worktree", "add", "--quiet", "--detach", "--"])
            .arg(&path)
            .arg(&start.commit)
            .envs(ssh.iter().copied())
            .run()
            .and_then(Completed::success);
        if let Err(error) = added {
            return Err(self.creation_failure(repo, &path, error));
        }
        // `--no-track`: the agent's branch must not take `origin/<base>` as
        // its upstream, or its push state would compare with the base.
        let switched = self
            .write(&path)
            .args(["switch", "--quiet", "--no-track", "--create", branch])
            .envs(ssh.iter().copied())
            .run()
            .and_then(Completed::success);
        if let Err(error) = switched {
            return Err(GitError::WorktreeIncomplete {
                path,
                source: Box::new(error),
            });
        }
        Ok(NewWorktree {
            start_point: start.name,
            start_commit: start.commit,
        })
    }

    fn remove_worktree(&self, repo: &Path, path: &Path) -> Result<(), GitError> {
        let path = absolute(repo, path)?;
        if let Some(reason) = self.removal_refusal(repo, &path)? {
            return Err(GitError::RemovalRefused { path, reason });
        }
        // Never `--force` (ADR 0011). git checks again that the worktree is
        // clean; make that check see untracked files whatever the
        // configuration says.
        self.write(repo)
            .args([
                "-c",
                "status.showUntrackedFiles=normal",
                "worktree",
                "remove",
                "--",
            ])
            .arg(&path)
            .stdout()?;
        Ok(())
    }

    fn unlock_worktree(&self, repo: &Path, path: &Path) -> Result<(), GitError> {
        let path = absolute(repo, path)?;
        let worktrees = self.worktrees(repo)?;
        let linked = worktrees
            .iter()
            .enumerate()
            .find(|(_, worktree)| same_path(&worktree.path, &path));
        let worktree = match linked {
            Some((position, worktree)) if position > 0 => worktree,
            Some(_) => {
                return Err(GitError::RemovalRefused {
                    path,
                    reason: RemovalRefusal::MainCheckout,
                });
            }
            None => {
                return Err(GitError::RemovalRefused {
                    path,
                    reason: RemovalRefusal::NotAWorktree,
                });
            }
        };
        if worktree.locked.is_none() {
            return Ok(());
        }
        self.write(repo)
            .args(["worktree", "unlock", "--"])
            .arg(&path)
            .stdout()?;
        Ok(())
    }

    fn delivery(
        &self,
        worktree: &Path,
        query: &DeliveryQuery<'_>,
    ) -> Result<DeliveryReport, GitError> {
        // Evidence takes several git commands. A commit that lands while
        // they run would mix the tree before it with the branch after it, a
        // state that never existed: read again until the branch held still.
        let mut report = self.read_delivery(worktree, query)?;
        for _ in 1..SNAPSHOT_ATTEMPTS {
            if self.held_still(worktree, query.branch, &report)? {
                break;
            }
            tracing::debug!(
                branch = query.branch,
                "the branch moved while it was read; reading again"
            );
            report = self.read_delivery(worktree, query)?;
        }
        Ok(report)
    }

    fn diff(
        &self,
        worktree: &Path,
        base: &str,
        format: DiffFormat,
        max_bytes: usize,
    ) -> Result<DiffOutput, GitError> {
        // Run from the top level, so untracked paths are relative to it.
        let top = self
            .git(worktree)
            .args(["rev-parse", "--show-toplevel"])
            .parse(parse::parse_path_line)?;
        let from = self.diff_origin(&top, base)?;
        let mut request = self.git(&top).arg("diff").args(STABLE_DIFF);
        if format == DiffFormat::Stat {
            request = request.arg("--stat");
        }
        let tracked = request
            .arg(&from)
            .arg("--")
            .max_stdout(max_bytes)
            .run()?
            .success()?;
        let mut out = Bounded::new(max_bytes);
        out.push(&tracked.stdout);
        if !out.is_full() {
            match format {
                DiffFormat::Stat => {
                    let untracked = self.untracked_files(&top, true)?;
                    let untracked: Vec<&Path> = untracked.iter().map(PathBuf::as_path).collect();
                    out.push_list("Untracked files:", &untracked);
                }
                DiffFormat::Patch => self.append_untracked_patches(&top, &mut out)?,
            }
        }
        Ok(out.finish())
    }

    fn fetch(&self, repo: &Path) -> Result<(), GitError> {
        let remote = self
            .git(repo)
            .args(["config", "--get", "remote.origin.url"])
            .run()?;
        match remote.code {
            Some(0) => {}
            Some(1) => {
                tracing::debug!(repo = %repo.display(), "no origin remote, nothing to fetch");
                return Ok(());
            }
            _ => return Err(remote.failure()),
        }
        let ssh = self.ssh_env(repo)?;
        // `--prune`: a remote branch deleted on the remote must not keep
        // counting as pushed. `FETCH_HEAD` belongs to the person: a
        // background fetch must not replace it.
        self.git(repo)
            .args([
                "fetch",
                "--quiet",
                "--prune",
                "--no-write-fetch-head",
                "origin",
            ])
            .envs(ssh)
            .timeout(self.fetch_timeout)
            .stdout()?;
        Ok(())
    }

    fn origin(&self, repo: &Path) -> Result<Option<RepoSlug>, GitError> {
        let completed = self.git(repo).args(["remote", "get-url", "origin"]).run()?;
        match completed.code {
            Some(0) => Ok(RepoSlug::from_remote_url(&String::from_utf8_lossy(
                &completed.stdout,
            ))),
            // "No such remote".
            Some(2) => Ok(None),
            _ => Err(completed.failure()),
        }
    }
}

/// A base branch, resolved to a commit.
#[derive(Debug)]
struct Base {
    /// `origin/<base>` or `<base>`.
    name: String,
    /// The commit it points to.
    commit: String,
}

/// The text of [`Git::diff`], gathered within a byte limit.
#[derive(Debug)]
struct Bounded {
    bytes: Vec<u8>,
    limit: usize,
}

impl Bounded {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }

    /// How many more bytes fit.
    fn room(&self) -> usize {
        self.limit.saturating_sub(self.bytes.len())
    }

    /// Whether the text went past the limit: it will be cut, so nothing more
    /// is worth gathering.
    fn is_full(&self) -> bool {
        self.bytes.len() > self.limit
    }

    fn push(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// Appends `paths` under `title`, at most [`MAX_UNTRACKED_FILES`] of
    /// them, after a blank line when text came before. Appends nothing when
    /// there are no paths.
    fn push_list(&mut self, title: &str, paths: &[&Path]) {
        if paths.is_empty() {
            return;
        }
        let mut list = String::new();
        if !self.bytes.is_empty() {
            list.push('\n');
        }
        list.push_str(title);
        list.push('\n');
        // Writing to a `String` never fails, so the results are ignored.
        for path in paths.iter().take(MAX_UNTRACKED_FILES) {
            let _ = writeln!(list, "  {}", path.display());
        }
        let omitted = paths.len().saturating_sub(MAX_UNTRACKED_FILES);
        if omitted > 0 {
            let _ = writeln!(list, "  ... and {omitted} more");
        }
        self.push(list.as_bytes());
    }

    /// The text, cut at a line break when it went past the limit.
    fn finish(self) -> DiffOutput {
        let keep = parse::truncation_point(&self.bytes, self.limit);
        DiffOutput {
            text: String::from_utf8_lossy(&self.bytes[..keep]).into_owned(),
            truncated: keep < self.bytes.len(),
        }
    }
}

/// `path` relative to `repo`, made absolute without touching the file
/// system, so git and this process agree on what it names.
fn absolute(repo: &Path, path: &Path) -> Result<PathBuf, GitError> {
    let joined = repo.join(path);
    std::path::absolute(&joined).map_err(|source| GitError::Io {
        action: "resolve",
        path: joined,
        source,
    })
}

/// Whether `listed`, a worktree path as git prints it (with symbolic links
/// resolved), is `path`.
fn same_path(listed: &Path, path: &Path) -> bool {
    listed == path || fs::canonicalize(path).is_ok_and(|resolved| resolved == listed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(out: Bounded) -> String {
        out.finish().text
    }

    #[test]
    fn path_lists_follow_the_text_after_a_blank_line() {
        let mut out = Bounded::new(usize::MAX);
        out.push(b" a.txt | 1 +\n 1 file changed, 1 insertion(+)\n");
        out.push_list("Untracked files:", &[Path::new("new/"), Path::new("u.txt")]);
        assert_eq!(
            text(out),
            " a.txt | 1 +\n 1 file changed, 1 insertion(+)\n\nUntracked files:\n  new/\n  u.txt\n"
        );
    }

    #[test]
    fn an_empty_path_list_adds_nothing() {
        let mut out = Bounded::new(usize::MAX);
        out.push_list("Untracked files:", &[]);
        assert_eq!(text(out), "");
        let mut out = Bounded::new(usize::MAX);
        out.push_list("Untracked files:", &[Path::new("u.txt")]);
        assert_eq!(text(out), "Untracked files:\n  u.txt\n");
    }

    #[test]
    fn long_path_lists_are_capped() {
        let names: Vec<String> = (0..MAX_UNTRACKED_FILES + 3)
            .map(|i| format!("f{i}"))
            .collect();
        let paths: Vec<&Path> = names.iter().map(Path::new).collect();
        let mut out = Bounded::new(usize::MAX);
        out.push_list("Untracked files:", &paths);
        let text = text(out);
        assert_eq!(text.lines().count(), 1 + MAX_UNTRACKED_FILES + 1);
        assert!(text.ends_with("  ... and 3 more\n"));
    }

    #[test]
    fn bounded_text_is_cut_at_a_line_break() {
        let mut out = Bounded::new(10);
        assert_eq!(out.room(), 10);
        out.push(b"one\ntwo\nthree\n");
        assert!(out.is_full());
        assert_eq!(
            out.finish(),
            DiffOutput {
                text: "one\ntwo\n".into(),
                truncated: true,
            }
        );
        let mut out = Bounded::new(10);
        out.push(b"one\n");
        assert!(!out.is_full());
        assert!(!out.finish().truncated);
    }

    #[test]
    fn relative_paths_are_resolved_against_the_repository() {
        let path = absolute(Path::new("/work/volee"), Path::new("../wt/feat")).unwrap();
        assert_eq!(path, Path::new("/work/volee/../wt/feat"));
        let path = absolute(Path::new("/work/volee"), Path::new("/elsewhere")).unwrap();
        assert_eq!(path, Path::new("/elsewhere"));
    }
}
