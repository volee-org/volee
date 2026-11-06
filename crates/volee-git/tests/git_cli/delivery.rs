//! `Git::delivery`: the evidence the cleanup rule relies on (ADR 0011).

use std::path::PathBuf;

use volee_core::delivery::{Delivery, DiffStat, PushState};
use volee_git::{Git, GitError};

use crate::fixture::Sandbox;

/// An agent's setup: a clone of `origin` and a worktree on `feat` created
/// by volee, with the commit it started from.
struct Agent {
    sandbox: Sandbox,
    clone: PathBuf,
    worktree: PathBuf,
    start: String,
}

impl Agent {
    fn new() -> Self {
        let sandbox = Sandbox::new();
        let (clone, _origin) = sandbox.cloned("volee");
        let worktree = sandbox.path("wt/feat");
        let created = sandbox
            .cli()
            .add_worktree(&clone, &worktree, "feat", "main")
            .expect("create the agent's worktree");
        Self {
            sandbox,
            clone,
            worktree,
            start: created.start_commit,
        }
    }

    fn commit(&self, file: &str, contents: &str) -> String {
        self.sandbox
            .commit(&self.worktree, file, contents, &format!("Add {file}"))
    }

    fn delivery(&self, start: Option<&str>) -> Delivery {
        self.sandbox
            .cli()
            .delivery(&self.worktree, "main", start)
            .expect("read the delivery evidence")
    }

    fn known_start(&self) -> Delivery {
        self.delivery(Some(&self.start))
    }

    /// Merges `feat` into `main` in the main checkout and pushes `main`.
    fn merge_and_push(&self, how: &str) {
        let git = |args: &[&str]| self.sandbox.git(&self.clone, args);
        git(&["merge", "--quiet", how, "--message", "Merge feat", "feat"]);
        git(&["push", "--quiet", "origin", "main"]);
    }
}

#[test]
fn a_fresh_branch_has_delivered_nothing() {
    let agent = Agent::new();
    assert_eq!(agent.known_start(), Delivery::default());
    assert_eq!(agent.delivery(None), Delivery::default());
}

#[test]
fn commits_and_uncommitted_changes_are_counted() {
    let agent = Agent::new();
    agent.commit("src/lib.rs", "one\ntwo\nthree\n");
    agent.commit("src/lib.rs", "one\n2\nthree\nfour\n");
    let sandbox = &agent.sandbox;
    sandbox.write(&agent.worktree.join("README.md"), "# Changed\n");
    sandbox.write(&agent.worktree.join("notes.txt"), "not added\n");

    let delivery = agent.known_start();
    assert_eq!(delivery.commits, 2);
    assert!(delivery.dirty);
    // `src/lib.rs` is new (4 lines) and `README.md` changed one line. The
    // untracked `notes.txt` makes the tree dirty but is not counted.
    assert_eq!(
        delivery.diff,
        DiffStat {
            files: 2,
            insertions: 5,
            deletions: 1,
        }
    );
    assert_eq!(delivery.push, PushState::NoUpstream);
    assert!(!delivery.merged_into_base);
}

#[test]
fn an_untracked_file_alone_makes_the_tree_dirty() {
    let agent = Agent::new();
    agent
        .sandbox
        .write(&agent.worktree.join("deep/new/file.rs"), "new\n");
    let delivery = agent.known_start();
    assert!(delivery.dirty);
    assert_eq!(delivery.commits, 0);
    assert_eq!(delivery.diff, DiffStat::default());
}

#[test]
fn an_untracked_file_counts_even_when_status_would_hide_it() {
    let agent = Agent::new();
    let sandbox = &agent.sandbox;
    sandbox.git(
        &agent.worktree,
        &["config", "status.showUntrackedFiles", "no"],
    );
    sandbox.write(&agent.worktree.join("new.rs"), "new\n");
    assert!(agent.known_start().dirty);
}

#[test]
fn ignored_files_do_not_make_the_tree_dirty() {
    let agent = Agent::new();
    agent.commit(".gitignore", "*.log\n");
    agent
        .sandbox
        .write(&agent.worktree.join("debug.log"), "noise\n");
    let delivery = agent.known_start();
    assert!(!delivery.dirty);
    assert_eq!(delivery.commits, 1);
}

#[test]
fn push_state_follows_the_upstream() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    let sandbox = &agent.sandbox;
    sandbox.git(
        &agent.worktree,
        &["push", "--quiet", "--set-upstream", "origin", "feat"],
    );
    assert_eq!(agent.known_start().push, PushState::UpToDate);

    agent.commit("b.rs", "b\n");
    agent.commit("c.rs", "c\n");
    assert_eq!(agent.known_start().push, PushState::Ahead { commits: 2 });
}

#[test]
fn a_branch_pushed_without_an_upstream_counts_as_pushed() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    let sandbox = &agent.sandbox;
    sandbox.git(&agent.worktree, &["push", "--quiet", "origin", "feat"]);
    assert_eq!(agent.known_start().push, PushState::UpToDate);

    agent.commit("b.rs", "b\n");
    assert_eq!(agent.known_start().push, PushState::Ahead { commits: 1 });
}

#[test]
fn a_branch_whose_remote_branch_was_deleted_is_not_pushed() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    let sandbox = &agent.sandbox;
    sandbox.git(
        &agent.worktree,
        &["push", "--quiet", "--set-upstream", "origin", "feat"],
    );
    sandbox.git(
        &agent.worktree,
        &["push", "--quiet", "origin", "--delete", "feat"],
    );
    assert_eq!(agent.known_start().push, PushState::NoUpstream);
}

#[test]
fn a_branch_merged_with_a_merge_commit_is_merged() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    agent.merge_and_push("--no-ff");
    let delivery = agent.known_start();
    assert_eq!(delivery.commits, 1);
    assert!(delivery.merged_into_base);
    assert!(delivery.is_delivered());
}

#[test]
fn a_fast_forwarded_branch_is_merged() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    agent.commit("b.rs", "b\n");
    agent.merge_and_push("--ff-only");
    let delivery = agent.known_start();
    assert_eq!(delivery.commits, 2);
    assert!(delivery.merged_into_base);
}

#[test]
fn an_unmerged_branch_is_not_merged() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    let delivery = agent.known_start();
    assert_eq!(delivery.commits, 1);
    assert!(!delivery.merged_into_base);
    assert!(!delivery.is_delivered());
}

#[test]
fn a_branch_without_commits_is_never_merged() {
    let agent = Agent::new();
    // `main` moves on: the agent's tip is now an ancestor of its base.
    let sandbox = &agent.sandbox;
    sandbox.commit(&agent.clone, "other.rs", "other\n", "Other work");
    sandbox.git(&agent.clone, &["push", "--quiet", "origin", "main"]);
    let delivery = agent.known_start();
    assert_eq!(delivery.commits, 0);
    assert!(!delivery.merged_into_base);
}

#[test]
fn without_a_start_commit_a_merged_branch_is_not_reported_merged() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    agent.merge_and_push("--ff-only");
    // After a fast-forward, nothing tells the agent's commits from `main`'s.
    let delivery = agent.delivery(None);
    assert_eq!(delivery.commits, 0);
    assert!(!delivery.merged_into_base);
}

#[test]
fn commits_are_counted_from_the_start_commit_when_it_is_known() {
    let agent = Agent::new();
    let before_the_agent = agent.commit("a.rs", "a\n");
    agent.commit("b.rs", "b\n");
    assert_eq!(agent.delivery(Some(&before_the_agent)).commits, 1);
    assert_eq!(agent.delivery(None).commits, 2);
}

#[test]
fn an_unknown_start_commit_falls_back_to_the_merge_base() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    let missing = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(agent.delivery(Some(missing)).commits, 1);
}

#[test]
fn the_base_is_compared_on_origin_when_it_exists() {
    let agent = Agent::new();
    let sandbox = &agent.sandbox;
    // A teammate pushes to `main`; the local `main` stays behind.
    let origin = sandbox.path("volee-origin.git");
    let teammate = sandbox.clone_of(&origin, "teammate");
    sandbox.commit(&teammate, "theirs.rs", "theirs\n", "Their work");
    sandbox.git(&teammate, &["push", "--quiet", "origin", "main"]);
    sandbox.git(&agent.clone, &["fetch", "--quiet", "origin"]);
    // The agent rebases on the new `origin/main` and commits.
    sandbox.git(&agent.worktree, &["rebase", "--quiet", "origin/main"]);
    agent.commit("mine.rs", "mine\n");

    let delivery = agent.delivery(None);
    assert_eq!(delivery.commits, 1);
    assert_eq!(delivery.diff.files, 1);
}

#[test]
fn a_detached_head_has_no_upstream() {
    let agent = Agent::new();
    let sandbox = &agent.sandbox;
    sandbox.git(&agent.worktree, &["checkout", "--quiet", "--detach"]);
    agent.commit("a.rs", "a\n");
    let delivery = agent.known_start();
    assert_eq!(delivery.commits, 1);
    assert_eq!(delivery.push, PushState::NoUpstream);
}

#[test]
fn an_unrelated_history_counts_everything_and_is_never_merged() {
    let agent = Agent::new();
    let sandbox = &agent.sandbox;
    sandbox.git(
        &agent.worktree,
        &["switch", "--quiet", "--orphan", "lonely"],
    );
    agent.commit("only.rs", "only\n");
    let delivery = agent.delivery(None);
    assert_eq!(delivery.commits, 1);
    assert_eq!(delivery.diff.files, 1);
    assert!(!delivery.merged_into_base);
}

#[test]
fn a_repository_without_commits_reports_its_staged_files() {
    let sandbox = Sandbox::new();
    let repo = sandbox.empty_repo("fresh");
    sandbox.write(&repo.join("src/main.rs"), "fn main() {}\n");
    sandbox.git(&repo, &["add", "--", "src/main.rs"]);
    // `main` does not exist yet, which is fine before the first commit.
    let delivery = sandbox.cli().delivery(&repo, "main", None).unwrap();
    assert_eq!(
        delivery,
        Delivery {
            dirty: true,
            diff: DiffStat {
                files: 1,
                insertions: 1,
                deletions: 0,
            },
            ..Delivery::default()
        }
    );
}

#[test]
fn an_unknown_base_is_an_error() {
    let agent = Agent::new();
    let error = agent
        .sandbox
        .cli()
        .delivery(&agent.worktree, "no-such-branch", None)
        .unwrap_err();
    assert!(
        matches!(&error, GitError::UnknownRevision { revision } if revision == "no-such-branch"),
        "{error:?}"
    );
}

#[test]
fn delivery_works_from_a_subdirectory() {
    let agent = Agent::new();
    agent.commit("src/lib.rs", "lib\n");
    let sandbox = &agent.sandbox;
    let subdirectory = agent.worktree.join("src");
    let delivery = sandbox
        .cli()
        .delivery(&subdirectory, "main", Some(&agent.start))
        .unwrap();
    assert_eq!(delivery.commits, 1);
    assert_eq!(delivery.diff.files, 1);
}
