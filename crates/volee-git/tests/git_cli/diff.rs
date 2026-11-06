//! `Git::diff`.

use std::path::{Path, PathBuf};

use volee_git::{DiffFormat, Git, GitCli};

use crate::fixture::Sandbox;

/// A worktree with a committed change, an uncommitted change and untracked
/// files, one of them in a new directory.
fn busy_worktree(sandbox: &Sandbox) -> PathBuf {
    let repo = sandbox.repo("volee");
    let worktree = sandbox.worktree(&repo, "wt/feat", "feat", "main");
    sandbox.commit(&worktree, "src/lib.rs", "committed line\n", "Add lib");
    sandbox.write(&worktree.join("README.md"), "# Sample\nuncommitted line\n");
    sandbox.write(&worktree.join("todo.txt"), "untracked line\n");
    sandbox.write(&worktree.join("docs/guide.md"), "untracked guide\n");
    worktree
}

/// The diff of `worktree` against `main`.
fn diff(cli: &GitCli, worktree: &Path, format: DiffFormat) -> String {
    cli.diff(worktree, "main", format)
        .expect("diff the worktree")
}

#[test]
fn the_stat_lists_tracked_changes_then_untracked_files() {
    let sandbox = Sandbox::new();
    let worktree = busy_worktree(&sandbox);
    let stat = diff(&sandbox.cli(), &worktree, DiffFormat::Stat);
    assert!(stat.contains(" README.md  | 1 +\n"), "{stat}");
    assert!(stat.contains(" src/lib.rs | 1 +\n"), "{stat}");
    assert!(
        stat.contains(" 2 files changed, 2 insertions(+)\n"),
        "{stat}"
    );
    assert!(
        stat.ends_with("\nUntracked files:\n  docs/\n  todo.txt\n"),
        "{stat}"
    );
}

#[test]
fn the_patch_shows_committed_uncommitted_and_new_files() {
    let sandbox = Sandbox::new();
    let worktree = busy_worktree(&sandbox);
    let patch = diff(&sandbox.cli(), &worktree, DiffFormat::Patch);
    for expected in [
        "diff --git a/README.md b/README.md\n",
        "+uncommitted line\n",
        "diff --git a/src/lib.rs b/src/lib.rs\n",
        "+committed line\n",
        "diff --git a/docs/guide.md b/docs/guide.md\nnew file mode 100644\n",
        "+untracked guide\n",
        "diff --git a/todo.txt b/todo.txt\nnew file mode 100644\n",
        "+untracked line\n",
    ] {
        assert!(
            patch.contains(expected),
            "missing {expected:?} in:\n{patch}"
        );
    }
    assert!(!patch.contains("not shown"), "{patch}");
}

#[test]
fn a_worktree_without_changes_has_an_empty_diff() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let worktree = sandbox.worktree(&repo, "wt/feat", "feat", "main");
    let cli = sandbox.cli();
    assert_eq!(diff(&cli, &worktree, DiffFormat::Stat), "");
    assert_eq!(diff(&cli, &worktree, DiffFormat::Patch), "");
}

#[test]
fn the_diff_of_a_subdirectory_covers_the_whole_worktree() {
    let sandbox = Sandbox::new();
    let worktree = busy_worktree(&sandbox);
    let patch = diff(&sandbox.cli(), &worktree.join("src"), DiffFormat::Patch);
    assert!(patch.contains("+uncommitted line\n"), "{patch}");
    assert!(patch.contains("b/todo.txt\n"), "{patch}");
}

#[test]
fn nested_repositories_are_listed_but_not_diffed() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    sandbox.repo("volee/vendored");
    let patch = diff(&sandbox.cli(), &repo, DiffFormat::Patch);
    assert_eq!(patch, "Untracked files not shown:\n  vendored/\n");
}

#[test]
fn past_a_hundred_untracked_files_the_rest_are_listed_not_diffed() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    for index in 0..103 {
        sandbox.write(&repo.join(format!("out/{index:03}.txt")), "generated\n");
    }
    let patch = diff(&sandbox.cli(), &repo, DiffFormat::Patch);
    assert_eq!(patch.matches("new file mode").count(), 100);
    assert!(
        patch.ends_with(
            "\nUntracked files not shown:\n  out/100.txt\n  out/101.txt\n  out/102.txt\n"
        ),
        "{patch}"
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_untracked_file_is_listed_instead_of_failing_the_diff() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    sandbox.write(&repo.join("readable.txt"), "readable\n");
    let locked = repo.join("locked.txt");
    sandbox.write(&locked, "secret\n");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    // Permissions do not apply to root, so the file stays readable there and
    // this scenario cannot happen. CI runs as an ordinary user.
    let readable_anyway = fs::File::open(&locked).is_ok();

    let patch = diff(&sandbox.cli(), &repo, DiffFormat::Patch);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(patch.contains("+readable\n"), "{patch}");
    if !readable_anyway {
        assert!(
            patch.ends_with("\nUntracked files not shown:\n  locked.txt\n"),
            "{patch}"
        );
    }
}

#[test]
fn a_repository_without_commits_is_diffed_against_nothing() {
    let sandbox = Sandbox::new();
    let repo = sandbox.empty_repo("fresh");
    sandbox.write(&repo.join("staged.rs"), "staged\n");
    sandbox.git(&repo, &["add", "--", "staged.rs"]);
    sandbox.write(&repo.join("loose.rs"), "loose\n");
    let stat = diff(&sandbox.cli(), &repo, DiffFormat::Stat);
    assert!(stat.contains(" staged.rs | 1 +\n"), "{stat}");
    assert!(stat.ends_with("\nUntracked files:\n  loose.rs\n"), "{stat}");
}
