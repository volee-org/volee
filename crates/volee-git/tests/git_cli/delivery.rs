//! `Git::delivery`: the evidence the cleanup rule relies on (ADR 0011).

use std::path::{Path, PathBuf};

use volee_core::delivery::{Delivery, DiffStat, PushState};
use volee_git::{DeliveryQuery, DeliveryReport, Git, GitError, Head};

use crate::fixture::{Sandbox, path_str};

/// An agent's setup: a clone of `origin` and a worktree on `feat` created
/// by volee from `origin/main`.
struct Agent {
    sandbox: Sandbox,
    clone: PathBuf,
    origin: PathBuf,
    worktree: PathBuf,
    start: String,
}

impl Agent {
    fn new() -> Self {
        let sandbox = Sandbox::new();
        let (clone, origin) = sandbox.cloned("volee");
        let worktree = sandbox.path("wt/feat");
        let created = sandbox
            .cli()
            .add_worktree(&clone, &worktree, "feat", "main")
            .expect("create the agent's worktree");
        Self {
            sandbox,
            clone,
            origin,
            worktree,
            start: created.start_commit,
        }
    }

    /// Commits `file` on whatever the worktree has checked out.
    fn commit(&self, file: &str, contents: &str) -> String {
        self.sandbox
            .commit(&self.worktree, file, contents, &format!("Add {file}"))
    }

    fn git(&self, args: &[&str]) -> String {
        self.sandbox.git(&self.worktree, args)
    }

    /// The evidence of `feat`, given the memory of a previous check.
    fn report(&self, unmerged_tip: Option<&str>) -> DeliveryReport {
        self.report_from(&self.worktree, "feat", unmerged_tip)
    }

    fn report_from(&self, dir: &Path, branch: &str, unmerged_tip: Option<&str>) -> DeliveryReport {
        let query = DeliveryQuery {
            branch,
            base: "main",
            unmerged_tip,
        };
        self.sandbox
            .cli()
            .delivery(dir, &query)
            .expect("read the delivery evidence")
    }

    /// The evidence of `feat`, as the daemon computes it: with the memory of
    /// the previous evidence.
    fn check(&self, previous: &Delivery) -> Delivery {
        self.report(previous.unmerged_tip.as_deref()).delivery
    }

    /// Merges `feat` into `main` in the main checkout and pushes `main`.
    fn merge_and_push(&self, how: &str) {
        let git = |args: &[&str]| self.sandbox.git(&self.clone, args);
        git(&["merge", "--quiet", how, "--message", "Merge feat", "feat"]);
        git(&["push", "--quiet", "origin", "main"]);
    }

    /// A teammate pushes `count` commits to `main`, and the agent's
    /// repository fetches them: `origin/main` moves, the local `main` stays.
    fn teammate_pushes(&self, count: usize) {
        let teammate = self.sandbox.clone_of(&self.origin, "teammate");
        for index in 0..count {
            let file = format!("theirs-{index}.rs");
            self.sandbox
                .commit(&teammate, &file, "theirs\n", &format!("Add {file}"));
        }
        self.sandbox
            .git(&teammate, &["push", "--quiet", "origin", "main"]);
        self.sandbox
            .git(&self.clone, &["fetch", "--quiet", "origin"]);
    }
}

#[test]
fn a_fresh_branch_has_delivered_nothing() {
    let agent = Agent::new();
    let report = agent.report(None);
    assert_eq!(
        report,
        DeliveryReport {
            delivery: Delivery {
                tip: Some(agent.start.clone()),
                ..Delivery::default()
            },
            head: Head {
                branch: Some("feat".into()),
                commit: Some(agent.start.clone()),
            },
            on_branch: true,
            base: Some("origin/main".into()),
            upstream: None,
        }
    );
}

#[test]
fn commits_and_uncommitted_changes_are_counted() {
    let agent = Agent::new();
    agent.commit("src/lib.rs", "one\ntwo\nthree\n");
    let tip = agent.commit("src/lib.rs", "one\n2\nthree\nfour\n");
    let sandbox = &agent.sandbox;
    sandbox.write(&agent.worktree.join("README.md"), "# Changed\n");
    sandbox.write(&agent.worktree.join("notes.txt"), "not added\n");

    let delivery = agent.report(None).delivery;
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
    assert_eq!(delivery.tip.as_deref(), Some(tip.as_str()));
    assert_eq!(delivery.unmerged_tip.as_deref(), Some(tip.as_str()));
}

#[test]
fn an_untracked_file_alone_makes_the_tree_dirty() {
    let agent = Agent::new();
    agent
        .sandbox
        .write(&agent.worktree.join("deep/new/file.rs"), "new\n");
    let delivery = agent.report(None).delivery;
    assert!(delivery.dirty);
    assert_eq!(delivery.commits, 0);
    assert_eq!(delivery.diff, DiffStat::default());
}

#[test]
fn an_untracked_file_counts_even_when_status_would_hide_it() {
    let agent = Agent::new();
    agent.git(&["config", "status.showUntrackedFiles", "no"]);
    agent.sandbox.write(&agent.worktree.join("new.rs"), "new\n");
    assert!(agent.report(None).delivery.dirty);
}

#[test]
fn ignored_files_do_not_make_the_tree_dirty() {
    let agent = Agent::new();
    agent.commit(".gitignore", "*.log\n");
    agent
        .sandbox
        .write(&agent.worktree.join("debug.log"), "noise\n");
    let delivery = agent.report(None).delivery;
    assert!(!delivery.dirty);
    assert_eq!(delivery.commits, 1);
}

#[test]
fn a_change_inside_a_submodule_makes_the_tree_dirty() {
    let agent = Agent::new();
    let sandbox = &agent.sandbox;
    let lib = sandbox.repo("lib");
    agent.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "--quiet",
        path_str(&lib),
        "vendor/lib",
    ]);
    agent.git(&["commit", "--quiet", "--message", "Add lib"]);
    // The person's configuration hides submodule changes; evidence must not.
    agent.git(&["config", "diff.ignoreSubmodules", "all"]);
    sandbox.write(&agent.worktree.join("vendor/lib/README.md"), "edited\n");
    assert!(agent.report(None).delivery.dirty);
}

#[test]
fn push_state_follows_the_upstream() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    agent.git(&["push", "--quiet", "--set-upstream", "origin", "feat"]);
    let report = agent.report(None);
    assert_eq!(report.delivery.push, PushState::UpToDate);
    assert_eq!(report.upstream.as_deref(), Some("origin/feat"));

    agent.commit("b.rs", "b\n");
    agent.commit("c.rs", "c\n");
    assert_eq!(
        agent.report(None).delivery.push,
        PushState::Ahead { commits: 2 }
    );
}

#[test]
fn a_branch_pushed_without_an_upstream_counts_as_pushed() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    agent.git(&["push", "--quiet", "origin", "feat"]);
    let report = agent.report(None);
    assert_eq!(report.delivery.push, PushState::UpToDate);
    assert_eq!(report.upstream.as_deref(), Some("origin/feat"));

    agent.commit("b.rs", "b\n");
    assert_eq!(
        agent.report(None).delivery.push,
        PushState::Ahead { commits: 1 }
    );
}

#[test]
fn a_branch_whose_remote_branch_was_deleted_is_not_pushed() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    agent.git(&["push", "--quiet", "--set-upstream", "origin", "feat"]);
    agent.git(&["push", "--quiet", "origin", "--delete", "feat"]);
    let report = agent.report(None);
    assert_eq!(report.delivery.push, PushState::NoUpstream);
    assert_eq!(report.upstream, None);
}

#[test]
fn an_upstream_that_is_a_local_branch_is_not_a_push() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    agent.git(&["branch", "--set-upstream-to=main", "feat"]);
    let report = agent.report(None);
    assert_eq!(report.delivery.push, PushState::NoUpstream);
    assert_eq!(report.upstream, None);
}

#[test]
fn a_fast_forward_merge_after_the_tip_was_seen_ahead_is_merged() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    let tip = agent.commit("b.rs", "b\n");
    let first = agent.report(None).delivery;
    assert_eq!(first.commits, 2);
    assert!(!first.merged_into_base);

    agent.merge_and_push("--ff-only");
    let second = agent.check(&first);
    assert_eq!(second.commits, 0);
    assert!(second.merged_into_base);
    assert!(second.is_delivered());
    assert_eq!(second.unmerged_tip.as_deref(), Some(tip.as_str()));
}

#[test]
fn a_merge_commit_after_the_tip_was_seen_ahead_is_merged() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    let first = agent.report(None).delivery;
    agent.merge_and_push("--no-ff");
    let second = agent.check(&first);
    assert_eq!(second.commits, 0);
    assert!(second.merged_into_base);
}

#[test]
fn without_memory_a_merged_branch_is_not_reported_merged() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    agent.merge_and_push("--ff-only");
    // Never seen ahead: nothing tells its commits from `main`'s.
    let delivery = agent.report(None).delivery;
    assert_eq!(delivery.commits, 0);
    assert!(!delivery.merged_into_base);
}

#[test]
fn new_commits_after_a_merge_are_not_merged() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    let first = agent.report(None).delivery;
    agent.merge_and_push("--ff-only");
    let second = agent.check(&first);
    assert!(second.merged_into_base);

    let newer = agent.commit("b.rs", "b\n");
    let third = agent.check(&second);
    assert_eq!(third.commits, 1);
    assert!(!third.merged_into_base);
    assert_eq!(third.unmerged_tip.as_deref(), Some(newer.as_str()));
}

#[test]
fn an_abandoned_branch_is_not_merged() {
    let agent = Agent::new();
    let abandoned = agent.commit("a.rs", "a\n");
    let first = agent.report(None).delivery;
    agent.git(&["reset", "--quiet", "--hard", "origin/main"]);
    let second = agent.check(&first);
    assert_eq!(second.commits, 0);
    assert!(!second.merged_into_base);
    // The memory is kept: the work might still be merged later.
    assert_eq!(second.unmerged_tip.as_deref(), Some(abandoned.as_str()));
}

#[test]
fn a_deleted_branch_whose_tip_was_merged_is_merged() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    let first = agent.report(None).delivery;
    agent.merge_and_push("--ff-only");
    agent.git(&["checkout", "--quiet", "--detach"]);
    agent
        .sandbox
        .git(&agent.clone, &["branch", "--delete", "--force", "feat"]);
    let second = agent.check(&first);
    assert_eq!(second.tip, None);
    assert!(second.merged_into_base);
}

#[test]
fn a_previous_tip_that_no_longer_exists_proves_nothing() {
    let agent = Agent::new();
    let missing = "0123456789abcdef0123456789abcdef01234567";
    let delivery = agent.report(Some(missing)).delivery;
    assert!(!delivery.merged_into_base);
    assert_eq!(delivery.unmerged_tip.as_deref(), Some(missing));
}

// The three scenarios of the review of #12: the evidence must describe the
// agent's own work, not wherever `HEAD` sits.

#[test]
fn an_agent_without_commits_that_fast_forwards_to_a_moved_base_delivered_nothing() {
    let agent = Agent::new();
    agent.teammate_pushes(2);
    agent.git(&["merge", "--quiet", "--ff-only", "origin/main"]);
    let delivery = agent.report(None).delivery;
    assert_eq!(delivery.commits, 0);
    assert!(!delivery.merged_into_base);
    assert_eq!(delivery.unmerged_tip, None);
}

#[test]
fn detaching_head_onto_the_base_does_not_merge_the_branch() {
    let agent = Agent::new();
    let tip = agent.commit("a.rs", "a\n");
    agent.git(&["checkout", "--quiet", "--detach", "origin/main"]);
    let report = agent.report(None);
    assert!(!report.on_branch);
    assert_eq!(report.head.branch, None);
    assert_eq!(report.head.commit.as_deref(), Some(agent.start.as_str()));
    assert_eq!(report.delivery.commits, 1);
    assert!(!report.delivery.merged_into_base);
    assert_eq!(report.delivery.tip.as_deref(), Some(tip.as_str()));
    // The diff is the branch's, not the detached working tree's.
    assert_eq!(report.delivery.diff.files, 1);
}

#[test]
fn a_commit_rebased_onto_a_moved_base_counts_once() {
    let agent = Agent::new();
    agent.commit("mine.rs", "mine\n");
    agent.teammate_pushes(3);
    agent.git(&["rebase", "--quiet", "origin/main"]);
    let delivery = agent.report(None).delivery;
    assert_eq!(delivery.commits, 1);
    assert_eq!(delivery.diff.files, 1);
}

#[test]
fn another_branch_checked_out_is_reported() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    agent.git(&["switch", "--quiet", "--create", "other"]);
    agent.commit("b.rs", "b\n");
    let report = agent.report(None);
    assert!(!report.on_branch);
    assert_eq!(report.head.branch.as_deref(), Some("other"));
    // Still the evidence of `feat`.
    assert_eq!(report.delivery.commits, 1);
}

#[test]
fn an_unrelated_history_counts_everything_and_is_never_merged() {
    let agent = Agent::new();
    agent.git(&["switch", "--quiet", "--orphan", "lonely"]);
    agent.commit("only.rs", "only\n");
    let report = agent.report_from(&agent.worktree, "lonely", None);
    assert_eq!(report.delivery.commits, 1);
    assert_eq!(report.delivery.diff.files, 1);
    assert!(!report.delivery.merged_into_base);
}

#[test]
fn a_repository_without_commits_reports_its_staged_files() {
    let sandbox = Sandbox::new();
    let repo = sandbox.empty_repo("fresh");
    sandbox.write(&repo.join("src/main.rs"), "fn main() {}\n");
    sandbox.git(&repo, &["add", "--", "src/main.rs"]);
    let query = DeliveryQuery {
        branch: "main",
        base: "main",
        unmerged_tip: None,
    };
    // `main` does not exist yet, which is fine before the first commit.
    let report = sandbox.cli().delivery(&repo, &query).unwrap();
    assert_eq!(
        report.delivery,
        Delivery {
            dirty: true,
            diff: DiffStat {
                files: 1,
                insertions: 1,
                deletions: 0,
            },
            push: PushState::NoRemote,
            ..Delivery::default()
        }
    );
    assert!(report.on_branch);
    assert_eq!(report.base, None);
}

#[test]
fn a_repository_without_a_remote_says_so_and_proves_a_local_merge() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("local");
    let worktree = sandbox.path("wt/local-feat");
    sandbox
        .cli()
        .add_worktree(&repo, &worktree, "feat", "main")
        .expect("create the agent's worktree");
    let check = |unmerged_tip: Option<&str>| {
        let query = DeliveryQuery {
            branch: "feat",
            base: "main",
            unmerged_tip,
        };
        sandbox.cli().delivery(&worktree, &query).unwrap()
    };
    let tip = sandbox.commit(&worktree, "a.rs", "a\n", "Add a.rs");
    let first = check(None);
    assert!(first.delivery.no_remote());
    assert_eq!(first.delivery.commits, 1);
    assert_eq!(first.delivery.push, PushState::NoRemote);
    assert_eq!(first.base.as_deref(), Some("main"));
    assert_eq!(first.upstream, None);
    assert!(!first.delivery.is_delivered());
    assert_eq!(first.delivery.summary(), "1 commit \u{b7} no remote");

    // The person's go-ahead, then a plain merge in the main checkout: the
    // branch's commits are part of the base branch.
    sandbox.git(
        &repo,
        &[
            "merge",
            "--quiet",
            "--no-ff",
            "--message",
            "Merge feat",
            "feat",
        ],
    );
    let second = check(first.delivery.unmerged_tip.as_deref());
    assert!(second.delivery.no_remote());
    assert_eq!(second.delivery.commits, 0);
    assert!(second.delivery.merged_into_base);
    assert!(second.delivery.is_delivered());
    assert_eq!(second.delivery.unmerged_tip.as_deref(), Some(tip.as_str()));
    assert_eq!(second.delivery.summary(), "no remote \u{b7} merged");
}

#[test]
fn a_squash_in_a_repository_without_a_remote_is_not_a_merge_volee_can_prove() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("local");
    let worktree = sandbox.path("wt/local-feat");
    sandbox
        .cli()
        .add_worktree(&repo, &worktree, "feat", "main")
        .expect("create the agent's worktree");
    sandbox.commit(&worktree, "a.rs", "a\n", "Add a.rs");
    let query = |unmerged_tip| DeliveryQuery {
        branch: "feat",
        base: "main",
        unmerged_tip,
    };
    let first = sandbox.cli().delivery(&worktree, &query(None)).unwrap();
    sandbox.git(&repo, &["merge", "--quiet", "--squash", "feat"]);
    sandbox.git(&repo, &["commit", "--quiet", "--message", "Squash feat"]);
    let second = sandbox
        .cli()
        .delivery(&worktree, &query(first.delivery.unmerged_tip.as_deref()))
        .unwrap();
    // The branch's own commit is not part of the base branch.
    assert_eq!(second.delivery.commits, 1);
    assert!(!second.delivery.merged_into_base);
    assert!(!second.delivery.is_delivered());
}

#[test]
fn a_clone_has_a_remote() {
    let agent = Agent::new();
    agent.commit("a.rs", "a\n");
    let delivery = agent.report(None).delivery;
    assert!(!delivery.no_remote());
    assert_eq!(delivery.push, PushState::NoUpstream);
    assert_eq!(delivery.summary(), "1 commit \u{b7} not pushed");
}

#[test]
fn an_unknown_base_is_an_error() {
    let agent = Agent::new();
    let query = DeliveryQuery {
        branch: "feat",
        base: "no-such-branch",
        unmerged_tip: None,
    };
    let error = agent
        .sandbox
        .cli()
        .delivery(&agent.worktree, &query)
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
    let report = agent.report_from(&agent.worktree.join("src"), "feat", None);
    assert_eq!(report.delivery.commits, 1);
    assert_eq!(report.delivery.diff.files, 1);
}
