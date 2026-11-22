//! Merging a finished change (v2, step 9), by the flow of 0.x kept: the
//! daemon merges, with delivery evidence (ADR 0011) and by the repository's
//! method (ADR 0032).
//!
//! - A repository whose `origin` is on a forge: the pull request at the tip
//!   of the branch, through the forge client (`gh`); a draft is marked ready,
//!   the method is the repository's `merge` setting, else the first one it
//!   allows.
//! - Any other repository: a merge commit of the branch into the base branch
//!   in the main checkout, which must have the base checked out and be clean.
//!
//! A merge that fails changes nothing and says why. A merge that went
//! through is checked afterwards: git (or the forge) must show the tip in
//! the base. The branch is never deleted.

use volee_core::delivery::PrState;
use volee_git::{BranchHead, DeliveryQuery, Forge, Git, MergeMethod};

use crate::review::Target;

/// What a merge did, in words for the journal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Merged {
    /// One sentence: what was merged and the evidence.
    pub(crate) message: String,
}

/// Merges `target`'s branch into its base. Blocking (git, the forge).
pub(crate) fn merge(
    git: &dyn Git,
    forge: Option<&dyn Forge>,
    method: Option<MergeMethod>,
    target: &Target,
) -> Result<Merged, String> {
    let tip = target
        .tip()
        .ok_or_else(|| "the branch has no commit to merge".to_owned())?;
    let short = &tip[..tip.len().min(7)];
    let origin = git
        .origin(&target.repo_root)
        .map_err(|error| error.to_string())?;
    let Some(slug) = origin else {
        return local(git, target, &tip, short);
    };
    let forge = forge.ok_or_else(|| {
        "volee has no forge client to merge with (the GitHub CLI, signed in)".to_owned()
    })?;
    let head = BranchHead {
        branch: &target.branch,
        origin: Some(&slug),
        tip: &tip,
    };
    let pull_request = forge
        .pull_request(&target.repo_root, &head)
        .map_err(|error| error.to_string())?
        .filter(|pr| matches!(pr.state, PrState::Open | PrState::Draft))
        .ok_or_else(|| {
            format!(
                "{} has no open pull request at its tip {short}: push the branch and open one",
                target.branch
            )
        })?;
    let used = forge
        .merge(&target.repo_root, &pull_request, method)
        .map_err(|error| error.to_string())?;
    // The evidence: the forge now says the pull request is merged.
    let after = forge
        .pull_request(&target.repo_root, &head)
        .map_err(|error| format!("merged, but the forge cannot confirm it: {error}"))?;
    match after {
        Some(pr) if pr.state == PrState::Merged => Ok(Merged {
            message: format!(
                "Merged {} into {} on the forge ({}, pull request #{}). Evidence: the forge says it is merged.",
                target.branch,
                target.base,
                used.name(),
                pr.number
            ),
        }),
        _ => Err(format!(
            "the forge accepted the merge of #{} but does not show it merged yet: check the pull request",
            pull_request.number
        )),
    }
}

fn local(git: &dyn Git, target: &Target, tip: &str, short: &str) -> Result<Merged, String> {
    let commit = git
        .merge(&target.repo_root, &target.branch, &target.base)
        .map_err(|error| error.to_string())?;
    let report = git
        .delivery(
            &target.cwd,
            &DeliveryQuery {
                branch: &target.branch,
                base: &target.base,
                unmerged_tip: Some(tip),
            },
        )
        .map_err(|error| format!("merged, but the evidence cannot be read: {error}"))?;
    if !report.delivery.merged_into_base {
        return Err(format!(
            "git made the merge commit {} but the tip {short} is not in {}: check the repository",
            &commit[..commit.len().min(7)],
            target.base
        ));
    }
    Ok(Merged {
        message: format!(
            "Merged {} into {} locally (merge commit {}). Evidence: its tip {short} is part of {}.",
            target.branch,
            target.base,
            &commit[..commit.len().min(7)],
            target.base
        ),
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Mutex;

    use volee_core::delivery::{ChecksState, PullRequest};
    use volee_git::{ForgeStatus, GitCli, GitError};

    use super::*;

    fn run(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    /// A repository on `main` with a worktree on `feat` holding one commit.
    fn fixture() -> (tempfile::TempDir, Target) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("alpha");
        std::fs::create_dir_all(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-q", "-m", "init"]);
        let tree = root.join("alpha-feat");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat",
                tree.to_str().unwrap(),
            ],
        );
        std::fs::write(tree.join("b.txt"), "two\n").unwrap();
        run(&tree, &["add", "."]);
        run(&tree, &["commit", "-q", "-m", "feat: add b"]);
        let target = Target {
            repo_name: "alpha".into(),
            repo_root: repo,
            cwd: tree,
            branch: "feat".into(),
            base: "main".into(),
        };
        (dir, target)
    }

    #[test]
    fn a_repository_without_a_remote_merges_locally_with_evidence() {
        let (_dir, target) = fixture();
        let merged = merge(&GitCli::new(), None, None, &target).unwrap();
        assert!(merged.message.contains("locally"), "{}", merged.message);
        assert!(merged.message.contains("Evidence"));
        assert!(
            target.repo_root.join("b.txt").is_file(),
            "the base has the work"
        );
        let log = Command::new("git")
            .current_dir(&target.repo_root)
            .args(["branch", "--list", "feat"])
            .output()
            .unwrap();
        assert!(!log.stdout.is_empty(), "the branch is kept");
    }

    #[test]
    fn a_failed_local_merge_changes_nothing_and_says_why() {
        let (_dir, target) = fixture();
        // An uncommitted change in the main checkout: volee does not merge over it.
        std::fs::write(target.repo_root.join("a.txt"), "dirty\n").unwrap();
        let before = Command::new("git")
            .current_dir(&target.repo_root)
            .args(["rev-parse", "main"])
            .output()
            .unwrap()
            .stdout;
        let error = merge(&GitCli::new(), None, None, &target).unwrap_err();
        assert!(
            error.to_lowercase().contains("uncommitted") || error.contains("changes"),
            "{error}"
        );
        let after = Command::new("git")
            .current_dir(&target.repo_root)
            .args(["rev-parse", "main"])
            .output()
            .unwrap()
            .stdout;
        assert_eq!(before, after, "main did not move");
        assert!(!target.repo_root.join("b.txt").exists());
        assert_eq!(
            std::fs::read_to_string(target.repo_root.join("a.txt")).unwrap(),
            "dirty\n",
            "the person's change is untouched"
        );
    }

    /// A forge with one pull request, which a merge turns merged.
    struct FakeForge {
        state: Mutex<PrState>,
        merges: Mutex<Vec<(PathBuf, Option<MergeMethod>)>>,
        refuse: bool,
        silent: bool,
    }

    impl FakeForge {
        fn new(state: PrState) -> Self {
            Self {
                state: Mutex::new(state),
                merges: Mutex::new(Vec::new()),
                refuse: false,
                silent: false,
            }
        }
    }

    impl Forge for FakeForge {
        fn status(&self) -> ForgeStatus {
            ForgeStatus {
                program: "fake".into(),
                available: true,
                version: None,
                detail: None,
            }
        }
        fn pull_request(
            &self,
            _repo: &Path,
            head: &BranchHead<'_>,
        ) -> Result<Option<PullRequest>, GitError> {
            Ok(Some(PullRequest {
                number: 7,
                url: "https://github.com/acme/alpha/pull/7".into(),
                state: *self.state.lock().unwrap(),
                checks: ChecksState::Passing,
                head: Some(head.tip.to_owned()),
            }))
        }
        fn merge(
            &self,
            repo: &Path,
            _pull_request: &PullRequest,
            method: Option<MergeMethod>,
        ) -> Result<MergeMethod, GitError> {
            if self.refuse {
                return Err(GitError::Failed {
                    command: "gh pr merge".into(),
                    code: Some(1),
                    stderr: "required checks are failing".into(),
                });
            }
            self.merges
                .lock()
                .unwrap()
                .push((repo.to_path_buf(), method));
            if !self.silent {
                *self.state.lock().unwrap() = PrState::Merged;
            }
            Ok(method.unwrap_or(MergeMethod::Merge))
        }
    }

    fn with_origin(target: &Target) {
        run(
            &target.repo_root,
            &["remote", "add", "origin", "git@github.com:acme/alpha.git"],
        );
    }

    #[test]
    fn a_repository_on_a_forge_merges_its_pull_request_with_the_repository_method() {
        let (_dir, target) = fixture();
        with_origin(&target);
        let forge = FakeForge::new(PrState::Draft);
        let merged = merge(
            &GitCli::new(),
            Some(&forge),
            Some(MergeMethod::Squash),
            &target,
        )
        .unwrap();
        assert!(merged.message.contains("squash"), "{}", merged.message);
        assert!(merged.message.contains("#7"));
        let calls = forge.merges.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, Some(MergeMethod::Squash));
    }

    #[test]
    fn a_forge_that_refuses_leaves_everything_as_it_was() {
        let (_dir, target) = fixture();
        with_origin(&target);
        let mut forge = FakeForge::new(PrState::Open);
        forge.refuse = true;
        let error = merge(&GitCli::new(), Some(&forge), None, &target).unwrap_err();
        assert!(error.contains("required checks are failing"), "{error}");
        assert_eq!(*forge.state.lock().unwrap(), PrState::Open);
    }

    #[test]
    fn a_merge_the_forge_does_not_show_is_not_reported_as_done() {
        let (_dir, target) = fixture();
        with_origin(&target);
        let mut forge = FakeForge::new(PrState::Open);
        forge.silent = true;
        let error = merge(&GitCli::new(), Some(&forge), None, &target).unwrap_err();
        assert!(error.contains("does not show it merged"), "{error}");
    }

    #[test]
    fn no_pull_request_and_no_forge_are_said() {
        let (_dir, target) = fixture();
        with_origin(&target);
        let error = merge(&GitCli::new(), None, None, &target).unwrap_err();
        assert!(error.contains("no forge client"), "{error}");
        let forge = FakeForge::new(PrState::Merged);
        let error = merge(&GitCli::new(), Some(&forge), None, &target).unwrap_err();
        assert!(error.contains("no open pull request"), "{error}");
    }
}
