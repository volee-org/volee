//! `GitCli` under a hostile git configuration, and with paths that contain
//! spaces: what volee reads must not change.

use volee_core::delivery::Delivery;
use volee_git::{DeliveryQuery, DiffFormat, Git, GitError, RemovalRefusal};

use crate::diff::{busy_worktree, diff};
use crate::fixture::Sandbox;

const FEAT: DeliveryQuery<'static> = DeliveryQuery {
    branch: "feat",
    base: "main",
    unmerged_tip: None,
};

/// The evidence and both diffs of the same work, built in `sandbox`.
fn observe(sandbox: &Sandbox) -> (Delivery, String, String) {
    let worktree = busy_worktree(sandbox, "volee");
    let cli = sandbox.cli();
    // From a subdirectory, where `diff.relative` would narrow the diff.
    let mut delivery = cli
        .delivery(&worktree.join("src"), &FEAT)
        .expect("read the delivery evidence")
        .delivery;
    // Commits differ between sandboxes, since their dates differ.
    delivery.tip = None;
    delivery.unmerged_tip = None;
    let stat = diff(&cli, &worktree.join("src"), DiffFormat::Stat);
    let patch = diff(&cli, &worktree.join("src"), DiffFormat::Patch);
    (delivery, stat, patch)
}

#[test]
fn a_hostile_configuration_changes_nothing_volee_reads() {
    let plain = observe(&Sandbox::new());
    let hostile = observe(&Sandbox::hostile());
    assert_eq!(hostile, plain);

    let (delivery, stat, patch) = hostile;
    assert!(delivery.dirty);
    assert_eq!(delivery.commits, 1);
    assert_eq!(delivery.diff.files, 2);
    assert!(
        stat.ends_with("Untracked files:\n  docs/\n  todo.txt\n"),
        "{stat}"
    );
    assert!(
        patch.contains("diff --git a/src/lib.rs b/src/lib.rs\n"),
        "{patch}"
    );
    assert!(
        patch.contains("diff --git a/todo.txt b/todo.txt\n"),
        "{patch}"
    );
    assert!(!patch.contains("EXTERNAL-DIFF"), "{patch}");
    assert!(!patch.contains('\u{1b}'), "colors in {patch}");
}

#[test]
fn removal_sees_untracked_files_under_a_hostile_configuration() {
    let sandbox = Sandbox::hostile();
    let repo = sandbox.repo("volee");
    let cli = sandbox.cli();
    let path = sandbox.path("wt/feat");
    cli.add_worktree(&repo, &path, "feat", "main").unwrap();
    sandbox.write(&path.join("draft.md"), "unsaved work\n");
    let error = cli.remove_worktree(&repo, &path).unwrap_err();
    assert!(
        matches!(
            &error,
            GitError::RemovalRefused {
                reason: RemovalRefusal::UncommittedChanges {
                    changed: 0,
                    untracked: 1
                },
                ..
            }
        ),
        "{error:?}"
    );
    assert!(path.join("draft.md").is_file());
}

#[test]
fn paths_with_spaces_work_everywhere() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("my repos/the volee");
    let cli = sandbox.cli();
    let worktree = sandbox.path("work trees/feat x");
    cli.add_worktree(&repo, &worktree, "feat", "main").unwrap();

    let info = cli.repo_info(&worktree).unwrap().unwrap();
    assert_eq!(info.top_level, worktree);
    assert_eq!(info.main_checkout, repo);
    assert_eq!(info.name, "the volee");
    assert!(
        cli.worktrees(&repo)
            .unwrap()
            .iter()
            .any(|listed| listed.path == worktree)
    );

    sandbox.commit(&worktree, "a file.rs", "a\n", "Add a file");
    sandbox.write(&worktree.join("new dir/new file.txt"), "new\n");
    let report = cli.delivery(&worktree, &FEAT).unwrap();
    assert_eq!(report.delivery.commits, 1);
    assert!(report.delivery.dirty);
    let patch = diff(&cli, &worktree, DiffFormat::Patch);
    assert!(patch.contains("a file.rs"), "{patch}");
    assert!(patch.contains("new dir/new file.txt"), "{patch}");
    assert!(patch.contains("+new\n"), "{patch}");

    std::fs::remove_dir_all(worktree.join("new dir")).unwrap();
    cli.remove_worktree(&repo, &worktree).unwrap();
    assert!(!worktree.exists());
}
