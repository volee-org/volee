//! [`GitCli`]: the [`Git`] implementation that runs the `git` executable.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::Duration;

use volee_core::delivery::{Delivery, DiffStat, PushState};

use crate::command::{Request, Runner};
use crate::parse::{self, StatusSummary};
use crate::{DiffFormat, Git, GitError, NewWorktree, RepoInfo, WorktreeInfo};

/// The most untracked files [`Git::diff`] lists or shows. The others are
/// counted in a final line, so a forgotten build directory cannot flood the
/// output or start thousands of processes.
const MAX_UNTRACKED_FILES: usize = 100;

/// Answers [`Git`] queries by running the `git` executable.
///
/// Every command runs as `git -C <dir> --no-optional-locks ...`, with
/// standard input closed, `LC_ALL=C`, prompts disabled
/// (`GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=never`) and without the
/// environment variables that would tie git to another repository
/// (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`...). Only porcelain and
/// plumbing formats are parsed. The user's git configuration applies, so the
/// answers match what the user sees.
///
/// A command that runs longer than its timeout is killed, and at most
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
    fetch_timeout: Duration,
}

impl GitCli {
    /// How long a command may run before it is stopped, unless set with
    /// [`GitCli::timeout`].
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

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
            runner: Runner::new(program.into(), Self::DEFAULT_MAX_CONCURRENT),
            timeout: Self::DEFAULT_TIMEOUT,
            fetch_timeout: Self::DEFAULT_FETCH_TIMEOUT,
        }
    }

    /// Sets an environment variable for every git command. It wins over the
    /// variables `GitCli` sets or removes itself.
    ///
    /// Tests use it to keep git away from the user's configuration, for
    /// example with `GIT_CONFIG_GLOBAL=/dev/null` and a temporary `HOME`.
    #[must_use]
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.runner.set_env(key.into(), value.into());
        self
    }

    /// Sets how long a command may run before it is stopped. Fetches have
    /// their own limit, [`GitCli::fetch_timeout`].
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
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

    fn git<'a>(&'a self, dir: &'a Path) -> Request<'a> {
        self.runner.request(dir, self.timeout)
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

    /// The branch, upstream and changes of a working tree. Untracked files
    /// are always looked for, whatever `status.showUntrackedFiles` says.
    fn status(&self, dir: &Path) -> Result<StatusSummary, GitError> {
        self.git(dir)
            .args([
                "status",
                "--porcelain=v2",
                "--branch",
                "--ahead-behind",
                "--untracked-files=normal",
                "-z",
            ])
            .parse(parse::parse_status_porcelain_v2)
    }

    /// The size of the changes from `from` to the working tree.
    fn shortstat(&self, dir: &Path, from: &str) -> Result<DiffStat, GitError> {
        self.git(dir)
            .args([
                "diff",
                "--shortstat",
                "--no-color",
                "--no-ext-diff",
                "--no-relative",
                from,
                "--",
            ])
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

    /// Where a diff of `dir` against `base` starts: their merge base, or the
    /// empty tree when there is none. Before the first commit, the base is
    /// not looked up: it may not exist yet, and everything is new anyway.
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

    fn push_state(
        &self,
        dir: &Path,
        status: &StatusSummary,
        head: &str,
    ) -> Result<PushState, GitError> {
        if let Some((ahead, _behind)) = status.ahead_behind {
            return Ok(parse::push_state(ahead));
        }
        // No upstream, or an upstream deleted on the remote: the branch may
        // still have been pushed under its own name, without `-u`.
        let Some(branch) = status.branch.as_deref() else {
            return Ok(PushState::NoUpstream);
        };
        match self.commit_of(dir, &format!("refs/remotes/origin/{branch}"))? {
            Some(remote) => Ok(parse::push_state(self.count_commits(
                dir,
                head,
                Some(&remote),
            )?)),
            None => Ok(PushState::NoUpstream),
        }
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
    fn append_untracked_patches(&self, top: &Path, text: &mut String) -> Result<(), GitError> {
        let entries = self.untracked_files(top, false)?;
        let (mut left_out, files): (Vec<&Path>, Vec<&Path>) = entries
            .iter()
            .map(PathBuf::as_path)
            .partition(|path| parse::names_directory(path));
        for (index, file) in files.into_iter().enumerate() {
            if index >= MAX_UNTRACKED_FILES {
                left_out.push(file);
                continue;
            }
            let completed = self
                .git(top)
                .args([
                    "diff",
                    "--no-index",
                    "--no-color",
                    "--no-ext-diff",
                    "--",
                    "/dev/null",
                ])
                .arg(file)
                .run()?;
            // `--no-index` exits with 1 when the files differ, as they do.
            if matches!(completed.code, Some(0 | 1)) {
                text.push_str(&String::from_utf8_lossy(&completed.stdout));
            } else {
                // One unreadable file must not hide the rest of the diff.
                tracing::debug!(error = %completed.failure(), "untracked file not shown");
                left_out.push(file);
            }
        }
        append_path_list(text, "Untracked files not shown:", &left_out);
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
        // git creates missing parent directories itself, but only after it
        // has created the branch: failing there would leave a stray branch.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| GitError::Io {
                action: "create the parent directory of",
                path: path.clone(),
                source,
            })?;
        }
        // `--no-track`: the agent's branch must not take `origin/<base>` as
        // its upstream, or its push state would compare with the base.
        self.git(repo)
            .args([
                "worktree",
                "add",
                "--quiet",
                "--no-track",
                "-b",
                branch,
                "--",
            ])
            .arg(&path)
            .arg(&start.commit)
            .stdout()?;
        Ok(NewWorktree {
            start_point: start.name,
            start_commit: start.commit,
        })
    }

    fn remove_worktree(&self, repo: &Path, path: &Path) -> Result<(), GitError> {
        let path = absolute(repo, path)?;
        // Never `--force` (ADR 0011): git's refusal is the answer.
        self.git(repo)
            .args(["worktree", "remove", "--"])
            .arg(&path)
            .stdout()?;
        Ok(())
    }

    fn delivery(
        &self,
        worktree: &Path,
        base: &str,
        start_commit: Option<&str>,
    ) -> Result<Delivery, GitError> {
        let status = self.status(worktree)?;
        let dirty = status.is_dirty();
        let Some(head) = status.oid.as_deref() else {
            // No commit yet: nothing to count, push or merge, and the base
            // branch may not even exist.
            let diff = self.shortstat(worktree, &self.empty_tree(worktree)?)?;
            return Ok(Delivery {
                dirty,
                diff,
                ..Delivery::default()
            });
        };
        let base = self.resolve_base(worktree, base)?;
        let merge_base = self.merge_base(worktree, &base.commit, head)?;
        let start = match start_commit {
            Some(revision) => {
                let start = self.commit_of(worktree, revision)?;
                if start.is_none() {
                    tracing::debug!(
                        revision,
                        "start commit not found, counting from the merge base"
                    );
                }
                start
            }
            None => None,
        };
        let commits =
            self.count_commits(worktree, head, start.as_deref().or(merge_base.as_deref()))?;
        let from = match merge_base {
            Some(commit) => commit,
            None => self.empty_tree(worktree)?,
        };
        let diff = self.shortstat(worktree, &from)?;
        let push = self.push_state(worktree, &status, head)?;
        let merged_into_base = commits > 0 && self.is_ancestor(worktree, head, &base.commit)?;
        Ok(Delivery {
            commits,
            dirty,
            diff,
            push,
            merged_into_base,
            ..Delivery::default()
        })
    }

    fn diff(&self, worktree: &Path, base: &str, format: DiffFormat) -> Result<String, GitError> {
        // Run from the top level, so untracked paths are relative to it.
        let top = self
            .git(worktree)
            .args(["rev-parse", "--show-toplevel"])
            .parse(parse::parse_path_line)?;
        let from = self.diff_origin(&top, base)?;
        let mut request =
            self.git(&top)
                .args(["diff", "--no-color", "--no-ext-diff", "--no-relative"]);
        if format == DiffFormat::Stat {
            request = request.arg("--stat");
        }
        let tracked = request.arg(&from).arg("--").stdout()?;
        let mut text = String::from_utf8_lossy(&tracked).into_owned();
        match format {
            DiffFormat::Stat => {
                let untracked = self.untracked_files(&top, true)?;
                let untracked: Vec<&Path> = untracked.iter().map(PathBuf::as_path).collect();
                append_path_list(&mut text, "Untracked files:", &untracked);
            }
            DiffFormat::Patch => self.append_untracked_patches(&top, &mut text)?,
        }
        Ok(text)
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
        // `FETCH_HEAD` belongs to the person: a background fetch must not
        // replace it.
        self.git(repo)
            .args(["fetch", "--quiet", "--no-write-fetch-head", "origin"])
            .timeout(self.fetch_timeout)
            .stdout()?;
        Ok(())
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

/// Appends `paths` under `title`, at most [`MAX_UNTRACKED_FILES`] of them,
/// after a blank line when `text` is not empty. Appends nothing when there
/// are no paths.
fn append_path_list(text: &mut String, title: &str, paths: &[&Path]) {
    if paths.is_empty() {
        return;
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(title);
    text.push('\n');
    // Writing to a `String` never fails, so the results are ignored.
    for path in paths.iter().take(MAX_UNTRACKED_FILES) {
        let _ = writeln!(text, "  {}", path.display());
    }
    let omitted = paths.len().saturating_sub(MAX_UNTRACKED_FILES);
    if omitted > 0 {
        let _ = writeln!(text, "  ... and {omitted} more");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_lists_follow_the_text_after_a_blank_line() {
        let mut text = " a.txt | 1 +\n 1 file changed, 1 insertion(+)\n".to_owned();
        append_path_list(
            &mut text,
            "Untracked files:",
            &[Path::new("new/"), Path::new("u.txt")],
        );
        assert_eq!(
            text,
            " a.txt | 1 +\n 1 file changed, 1 insertion(+)\n\nUntracked files:\n  new/\n  u.txt\n"
        );
    }

    #[test]
    fn an_empty_path_list_adds_nothing() {
        let mut text = String::new();
        append_path_list(&mut text, "Untracked files:", &[]);
        assert_eq!(text, "");
        append_path_list(&mut text, "Untracked files:", &[Path::new("u.txt")]);
        assert_eq!(text, "Untracked files:\n  u.txt\n");
    }

    #[test]
    fn long_path_lists_are_capped() {
        let names: Vec<String> = (0..MAX_UNTRACKED_FILES + 3)
            .map(|i| format!("f{i}"))
            .collect();
        let paths: Vec<&Path> = names.iter().map(Path::new).collect();
        let mut text = String::new();
        append_path_list(&mut text, "Untracked files:", &paths);
        assert_eq!(text.lines().count(), 1 + MAX_UNTRACKED_FILES + 1);
        assert!(text.ends_with("  ... and 3 more\n"));
    }

    #[test]
    fn relative_paths_are_resolved_against_the_repository() {
        let path = absolute(Path::new("/work/volee"), Path::new("../wt/feat")).unwrap();
        assert_eq!(path, Path::new("/work/volee/../wt/feat"));
        let path = absolute(Path::new("/work/volee"), Path::new("/elsewhere")).unwrap();
        assert_eq!(path, Path::new("/elsewhere"));
    }
}
