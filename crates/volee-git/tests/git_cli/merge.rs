//! `Git::merge`: a branch merged into its base branch in the main checkout,
//! on a person's word (ADR 0032). It makes a merge commit, or changes
//! nothing and says why.

use std::path::PathBuf;

use volee_git::{Git, GitError, MergeRefusal};

use crate::fixture::Sandbox;

/// A repository on `main` and a worktree on `feat` with one commit of its
/// own, as an agent leaves it.
struct Work {
    sandbox: Sandbox,
    repo: PathBuf,
    worktree: PathBuf,
    tip: String,
}

impl Work {
    fn new() -> Self {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo("alpha");
        let worktree = sandbox.path("wt/feat");
        sandbox
            .cli()
            .add_worktree(&repo, &worktree, "feat", "main")
            .expect("create the agent's worktree");
        let tip = sandbox.commit(
            &worktree,
            "invoices.rs",
            "pub fn total() {}\n",
            "Add invoices",
        );
        Self {
            sandbox,
            repo,
            worktree,
            tip,
        }
    }

    fn merge(&self) -> Result<String, GitError> {
        self.sandbox.cli().merge(&self.repo, "feat", "main")
    }

    fn refusal(&self) -> MergeRefusal {
        match self.merge() {
            Err(GitError::MergeRefused {
                branch,
                checkout,
                reason,
            }) => {
                assert_eq!(branch, "feat");
                assert_eq!(
                    checkout,
                    self.repo.canonicalize().expect("the repository resolves")
                );
                reason
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    fn main(&self) -> String {
        self.sandbox.rev(&self.repo, "main")
    }
}

#[test]
fn a_branch_is_merged_into_its_base_with_a_merge_commit() {
    let work = Work::new();
    let before = work.main();
    let merged = work.merge().unwrap();
    assert_eq!(merged, work.main());
    assert_ne!(merged, before);
    // A merge commit: the base as it was, then the branch's tip.
    let parents = work
        .sandbox
        .git(&work.repo, &["rev-list", "--parents", "-n", "1", "main"]);
    assert_eq!(
        parents,
        format!("{merged} {before} {}", work.tip),
        "a merge commit, not a fast-forward"
    );
    assert_eq!(
        work.sandbox
            .git(&work.repo, &["log", "-1", "--format=%s", "main"]),
        "Merge branch 'feat'"
    );
    // The work is in the main checkout, and the branch is still there.
    assert!(work.repo.join("invoices.rs").is_file());
    assert_eq!(work.sandbox.rev(&work.repo, "feat"), work.tip);
    assert_eq!(work.sandbox.git(&work.repo, &["status", "--porcelain"]), "");
}

#[test]
fn merging_from_the_agent_s_worktree_merges_in_the_main_checkout() {
    let work = Work::new();
    let merged = work
        .sandbox
        .cli()
        .merge(&work.worktree, "feat", "main")
        .unwrap();
    assert_eq!(merged, work.main());
    assert!(work.repo.join("invoices.rs").is_file());
}

#[test]
fn a_branch_already_in_its_base_is_left_alone() {
    let work = Work::new();
    let first = work.merge().unwrap();
    assert_eq!(work.merge().unwrap(), first, "nothing more to merge");
    assert_eq!(work.main(), first);
}

#[test]
fn a_main_checkout_on_another_branch_is_not_switched() {
    let work = Work::new();
    work.sandbox
        .git(&work.repo, &["switch", "--quiet", "--create", "mine"]);
    let before = work.main();
    assert_eq!(
        work.refusal(),
        MergeRefusal::NotOnBase {
            base: "main".into(),
            checked_out: Some("mine".into()),
        }
    );
    assert_eq!(work.main(), before, "nothing was merged");
    assert_eq!(
        work.sandbox
            .git(&work.repo, &["symbolic-ref", "--short", "HEAD"]),
        "mine",
        "the person's branch is still checked out"
    );
    let said = work.merge().unwrap_err().to_string();
    assert!(
        said.contains("cannot merge `feat`")
            && said.contains("it has `mine` checked out, not `main`"),
        "{said}"
    );
}

#[test]
fn a_detached_main_checkout_is_refused() {
    let work = Work::new();
    work.sandbox
        .git(&work.repo, &["switch", "--quiet", "--detach"]);
    assert_eq!(
        work.refusal(),
        MergeRefusal::NotOnBase {
            base: "main".into(),
            checked_out: None,
        }
    );
}

#[test]
fn a_person_s_uncommitted_changes_are_not_merged_over() {
    let work = Work::new();
    work.sandbox
        .write(&work.repo.join("README.md"), "# Mine, not committed\n");
    let before = work.main();
    assert_eq!(
        work.refusal(),
        MergeRefusal::UncommittedChanges { changed: 1 }
    );
    assert_eq!(work.main(), before);
    assert_eq!(
        std::fs::read_to_string(work.repo.join("README.md")).unwrap(),
        "# Mine, not committed\n",
        "the person's change is untouched"
    );
    // An untracked file that is not in the way does not block a merge.
    work.sandbox
        .git(&work.repo, &["checkout", "--", "README.md"]);
    work.sandbox.write(&work.repo.join("notes.txt"), "mine\n");
    work.merge().unwrap();
    assert!(work.repo.join("notes.txt").is_file());
}

#[test]
fn a_merge_that_conflicts_is_abandoned_and_the_checkout_is_as_it_was() {
    let work = Work::new();
    // The base moved on the same file, another way.
    work.sandbox
        .commit(&work.repo, "invoices.rs", "pub fn sum() {}\n", "Add a sum");
    let before = work.main();
    let reason = work.refusal();
    assert!(
        matches!(reason, MergeRefusal::Abandoned { .. }),
        "{reason:?}"
    );
    assert_eq!(work.main(), before, "nothing was merged");
    // No merge is left in progress, and the file is the base's.
    assert_eq!(work.sandbox.git(&work.repo, &["status", "--porcelain"]), "");
    assert!(!work.repo.join(".git/MERGE_HEAD").exists());
    assert_eq!(
        std::fs::read_to_string(work.repo.join("invoices.rs")).unwrap(),
        "pub fn sum() {}\n"
    );
    let said = work.merge().unwrap_err().to_string();
    assert!(
        said.contains("it was abandoned, the checkout is as it was"),
        "{said}"
    );
}

#[test]
fn an_untracked_file_in_the_way_abandons_the_merge() {
    let work = Work::new();
    work.sandbox
        .write(&work.repo.join("invoices.rs"), "mine, untracked\n");
    let before = work.main();
    assert!(matches!(work.refusal(), MergeRefusal::Abandoned { .. }));
    assert_eq!(work.main(), before);
    assert_eq!(
        std::fs::read_to_string(work.repo.join("invoices.rs")).unwrap(),
        "mine, untracked\n"
    );
}

#[test]
fn a_branch_that_does_not_exist_or_has_no_name_is_an_error() {
    let work = Work::new();
    let cli = work.sandbox.cli();
    assert!(matches!(
        cli.merge(&work.repo, "nowhere", "main"),
        Err(GitError::UnknownRevision { .. })
    ));
    assert!(matches!(
        cli.merge(&work.repo, "--force", "main"),
        Err(GitError::InvalidBranchName { .. })
    ));
    assert_eq!(work.sandbox.git(&work.repo, &["status", "--porcelain"]), "");
}
