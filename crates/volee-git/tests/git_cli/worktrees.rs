//! `Git::worktrees`, `Git::default_branch`, `Git::add_worktree` and
//! `Git::remove_worktree`.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use volee_git::{Git, GitError, NewWorktree, RemovalRefusal, WorktreeInfo};

use crate::fixture::{Sandbox, path_str};

#[test]
fn worktrees_lists_the_main_checkout_first_then_every_linked_worktree() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let head = sandbox.head(&repo);
    let feat = sandbox.worktree(&repo, "wt/feat", "feat/x", "main");
    let detached = sandbox.path("wt/detached");
    sandbox.git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            path_str(&detached),
            "main",
        ],
    );
    sandbox.git(
        &repo,
        &[
            "worktree",
            "lock",
            "--reason",
            "on a usb disk",
            path_str(&detached),
        ],
    );
    let gone = sandbox.worktree(&repo, "wt/gone", "gone", "main");
    fs::remove_dir_all(&gone).unwrap();

    let worktrees = sandbox.cli().worktrees(&repo).unwrap();
    assert_eq!(worktrees.len(), 4, "{worktrees:#?}");
    assert_eq!(
        worktrees[0],
        WorktreeInfo {
            path: repo.clone(),
            head: Some(head.clone()),
            branch: Some("main".into()),
            ..WorktreeInfo::default()
        }
    );
    // git sorts linked worktrees by path.
    let find = |path: &Path| {
        worktrees
            .iter()
            .find(|worktree| worktree.path == path)
            .unwrap_or_else(|| panic!("{} is not listed", path.display()))
    };
    assert_eq!(find(&feat).branch.as_deref(), Some("feat/x"));
    assert_eq!(
        *find(&detached),
        WorktreeInfo {
            path: detached.clone(),
            head: Some(head),
            detached: true,
            locked: Some("on a usb disk".into()),
            ..WorktreeInfo::default()
        }
    );
    let gone = find(&gone);
    assert_eq!(gone.branch.as_deref(), Some("gone"));
    assert!(gone.prunable.is_some());
}

#[test]
fn worktrees_reads_the_same_list_from_any_worktree() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let feat = sandbox.worktree(&repo, "wt/feat", "feat", "main");
    let cli = sandbox.cli();
    assert_eq!(cli.worktrees(&feat).unwrap(), cli.worktrees(&repo).unwrap());
}

#[test]
fn a_bare_repository_lists_itself_as_bare() {
    let sandbox = Sandbox::new();
    let bare = sandbox.bare("volee.git");
    assert_eq!(
        sandbox.cli().worktrees(&bare).unwrap(),
        [WorktreeInfo {
            path: bare,
            bare: true,
            ..WorktreeInfo::default()
        }]
    );
}

#[test]
fn the_default_branch_is_the_one_origin_head_points_to() {
    let sandbox = Sandbox::new();
    let (clone, origin) = sandbox.cloned("volee");
    sandbox.git(&clone, &["push", "--quiet", "origin", "main:develop"]);
    sandbox.git(&origin, &["symbolic-ref", "HEAD", "refs/heads/develop"]);
    sandbox.git(&clone, &["remote", "set-head", "origin", "--auto"]);
    // A local `main` exists, but the remote's default wins.
    assert_eq!(
        sandbox.cli().default_branch(&clone).unwrap().as_deref(),
        Some("develop")
    );
}

#[test]
fn without_origin_head_the_default_branch_is_main_then_master_then_develop() {
    let sandbox = Sandbox::new();
    let cli = sandbox.cli();

    let repo = sandbox.repo("with-main");
    sandbox.git(&repo, &["branch", "develop"]);
    sandbox.git(&repo, &["branch", "master"]);
    assert_eq!(cli.default_branch(&repo).unwrap().as_deref(), Some("main"));

    sandbox.git(&repo, &["branch", "--move", "main", "trunk"]);
    assert_eq!(
        cli.default_branch(&repo).unwrap().as_deref(),
        Some("master")
    );

    sandbox.git(&repo, &["branch", "--delete", "master"]);
    assert_eq!(
        cli.default_branch(&repo).unwrap().as_deref(),
        Some("develop")
    );

    sandbox.git(&repo, &["branch", "--delete", "develop"]);
    assert_eq!(cli.default_branch(&repo).unwrap(), None);
}

#[test]
fn add_worktree_starts_from_origin_when_it_is_ahead() {
    let sandbox = Sandbox::new();
    let (clone, origin) = sandbox.cloned("volee");
    let local_main = sandbox.head(&clone);
    let teammate = sandbox.clone_of(&origin, "teammate");
    let pushed = sandbox.commit(&teammate, "news.md", "news\n", "Add news");
    sandbox.git(&teammate, &["push", "--quiet", "origin", "main"]);
    sandbox.git(&clone, &["fetch", "--quiet", "origin"]);

    let path = sandbox.path("wt/feat");
    let created = sandbox
        .cli()
        .add_worktree(&clone, &path, "feat/x", "main")
        .unwrap();
    assert_eq!(
        created,
        NewWorktree {
            start_point: "origin/main".into(),
            start_commit: pushed.clone(),
        }
    );
    assert_ne!(pushed, local_main);
    assert_eq!(sandbox.head(&path), pushed);
    assert_eq!(
        sandbox.git(&path, &["symbolic-ref", "--short", "HEAD"]),
        "feat/x"
    );
    // No upstream: the branch is the agent's until it pushes it.
    assert!(!sandbox.has_rev(&path, "@{upstream}"));
}

#[test]
fn add_worktree_starts_from_the_local_base_without_origin() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let path = sandbox.path("wt/feat");
    let created = sandbox
        .cli()
        .add_worktree(&repo, &path, "feat", "main")
        .unwrap();
    assert_eq!(created.start_point, "main");
    assert_eq!(created.start_commit, sandbox.head(&repo));
    assert_eq!(sandbox.head(&path), sandbox.head(&repo));
}

#[test]
fn add_worktree_resolves_a_relative_path_against_the_repository() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    sandbox
        .cli()
        .add_worktree(&repo, Path::new("../worktrees/feat"), "feat", "main")
        .unwrap();
    assert!(sandbox.path("worktrees/feat/README.md").is_file());
}

#[test]
fn add_worktree_refuses_an_existing_branch() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    sandbox.git(&repo, &["branch", "feat"]);
    let path = sandbox.path("wt/feat");
    let error = sandbox
        .cli()
        .add_worktree(&repo, &path, "feat", "main")
        .unwrap_err();
    assert!(
        matches!(&error, GitError::BranchExists { branch } if branch == "feat"),
        "{error:?}"
    );
    assert!(!path.exists());
}

#[test]
fn add_worktree_refuses_an_existing_path_and_creates_no_branch() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let cli = sandbox.cli();
    let full = sandbox.path("wt/full");
    sandbox.write(&full.join("notes.txt"), "mine\n");
    let empty = sandbox.dir("wt/empty");
    for path in [full, empty] {
        let error = cli.add_worktree(&repo, &path, "feat", "main").unwrap_err();
        assert!(
            matches!(&error, GitError::PathExists { path: refused } if *refused == path),
            "{error:?}"
        );
    }
    // git itself would have created the branch before failing on the path.
    assert!(!sandbox.has_rev(&repo, "refs/heads/feat"));
}

#[test]
fn add_worktree_refuses_an_unknown_base() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let path = sandbox.path("wt/feat");
    let error = sandbox
        .cli()
        .add_worktree(&repo, &path, "feat", "no-such-branch")
        .unwrap_err();
    assert!(
        matches!(&error, GitError::UnknownRevision { revision } if revision == "no-such-branch"),
        "{error:?}"
    );
    assert!(!path.exists());
    assert!(!sandbox.has_rev(&repo, "refs/heads/feat"));
}

#[test]
fn add_worktree_refuses_invalid_branch_names() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let cli = sandbox.cli();
    for name in [
        "",
        "-f",
        "HEAD",
        "two..dots",
        "with space",
        "ends.",
        "a@{1}",
    ] {
        let path = sandbox.path("wt/invalid");
        let error = cli.add_worktree(&repo, &path, name, "main").unwrap_err();
        assert!(
            matches!(&error, GitError::InvalidBranchName { name: refused } if refused == name),
            "{name:?}: {error:?}"
        );
        assert!(!path.exists());
    }
}

#[test]
fn remove_worktree_removes_a_clean_worktree_and_keeps_its_branch() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let cli = sandbox.cli();
    let path = sandbox.path("wt/feat");
    cli.add_worktree(&repo, &path, "feat", "main").unwrap();
    let work = sandbox.commit(&path, "work.rs", "fn work() {}\n", "Do the work");

    cli.remove_worktree(&repo, &path).unwrap();
    assert!(!path.exists());
    assert_eq!(sandbox.rev(&repo, "refs/heads/feat"), work);
    assert_eq!(cli.worktrees(&repo).unwrap().len(), 1);
}

#[test]
fn remove_worktree_never_forces() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let cli = sandbox.cli();
    let path = sandbox.path("wt/feat");
    cli.add_worktree(&repo, &path, "feat", "main").unwrap();

    sandbox.write(&path.join("README.md"), "changed\n");
    assert_refused(
        cli.remove_worktree(&repo, &path),
        &RemovalRefusal::UncommittedChanges {
            changed: 1,
            untracked: 0,
        },
    );
    sandbox.git(&path, &["checkout", "--quiet", "--", "README.md"]);

    sandbox.write(&path.join("new.txt"), "new\n");
    assert_refused(
        cli.remove_worktree(&repo, &path),
        &RemovalRefusal::UncommittedChanges {
            changed: 0,
            untracked: 1,
        },
    );
    fs::remove_file(path.join("new.txt")).unwrap();

    sandbox.git(
        &repo,
        &["worktree", "lock", "--reason", "in use", path_str(&path)],
    );
    assert_refused(
        cli.remove_worktree(&repo, &path),
        &RemovalRefusal::Locked {
            reason: "in use".into(),
        },
    );
    assert!(path.join("README.md").is_file());

    assert_refused(
        cli.remove_worktree(&repo, &repo),
        &RemovalRefusal::MainCheckout,
    );
    assert_refused(
        cli.remove_worktree(&repo, &sandbox.dir("elsewhere")),
        &RemovalRefusal::NotAWorktree,
    );
}

#[test]
fn unlock_worktree_lifts_a_lock_and_nothing_else() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let cli = sandbox.cli();
    let path = sandbox.path("wt/feat");
    cli.add_worktree(&repo, &path, "feat", "main").unwrap();
    sandbox.git(
        &repo,
        &[
            "worktree",
            "lock",
            "--reason",
            "claude session",
            path_str(&path),
        ],
    );

    cli.unlock_worktree(&repo, &path).unwrap();
    let worktrees = cli.worktrees(&repo).unwrap();
    assert_eq!(worktrees[1].locked, None);
    assert!(
        path.join("README.md").is_file(),
        "unlocking deletes nothing"
    );
    // Unlocking an unlocked worktree changes nothing.
    cli.unlock_worktree(&repo, &path).unwrap();
    cli.remove_worktree(&repo, &path).unwrap();
    assert!(!path.exists());
}

#[test]
fn unlock_worktree_refuses_what_is_not_a_linked_worktree() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let cli = sandbox.cli();
    assert_refused(
        cli.unlock_worktree(&repo, &repo),
        &RemovalRefusal::MainCheckout,
    );
    assert_refused(
        cli.unlock_worktree(&repo, &sandbox.dir("elsewhere")),
        &RemovalRefusal::NotAWorktree,
    );
}

#[test]
fn remove_worktree_sees_untracked_files_that_status_would_hide() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let cli = sandbox.cli();
    let path = sandbox.path("wt/feat");
    cli.add_worktree(&repo, &path, "feat", "main").unwrap();
    sandbox.git(&repo, &["config", "status.showUntrackedFiles", "no"]);
    sandbox.write(&path.join("notes/draft.md"), "unsaved work\n");
    assert_refused(
        cli.remove_worktree(&repo, &path),
        &RemovalRefusal::UncommittedChanges {
            changed: 0,
            untracked: 1,
        },
    );
    assert!(path.join("notes/draft.md").is_file());
}

#[test]
fn remove_worktree_keeps_a_detached_head_that_nothing_else_contains() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let cli = sandbox.cli();
    let path = sandbox.path("wt/feat");
    cli.add_worktree(&repo, &path, "feat", "main").unwrap();
    sandbox.git(&path, &["checkout", "--quiet", "--detach"]);
    let lonely = sandbox.commit(&path, "lonely.rs", "lonely\n", "Commit on no branch");

    assert_refused(
        cli.remove_worktree(&repo, &path),
        &RemovalRefusal::UnreachableCommits {
            head: lonely.clone(),
        },
    );
    assert!(path.exists());

    // Once a tag keeps the commit, removing loses nothing.
    sandbox.git(&repo, &["tag", "kept", &lonely]);
    cli.remove_worktree(&repo, &path).unwrap();
    assert!(!path.exists());
}

#[test]
fn remove_worktree_allows_a_detached_head_that_a_branch_contains() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let cli = sandbox.cli();
    let path = sandbox.path("wt/detached");
    sandbox.git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            path_str(&path),
            "main",
        ],
    );
    cli.remove_worktree(&repo, &path).unwrap();
    assert!(!path.exists());
}

#[test]
fn remove_worktree_deletes_ignored_files_with_the_worktree() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let cli = sandbox.cli();
    let path = sandbox.path("wt/feat");
    cli.add_worktree(&repo, &path, "feat", "main").unwrap();
    sandbox.commit(&path, ".gitignore", "target/\n", "Ignore build output");
    sandbox.write(&path.join("target/debug/app"), "binary\n");
    cli.remove_worktree(&repo, &path).unwrap();
    assert!(!path.exists());
}

#[test]
fn remove_worktree_refuses_a_worktree_whose_directory_is_gone() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let cli = sandbox.cli();
    let path = sandbox.path("wt/feat");
    cli.add_worktree(&repo, &path, "feat", "main").unwrap();
    fs::remove_dir_all(&path).unwrap();
    let error = cli.remove_worktree(&repo, &path).unwrap_err();
    assert!(
        matches!(
            &error,
            GitError::RemovalRefused {
                reason: RemovalRefusal::Prunable { .. },
                ..
            }
        ),
        "{error:?}"
    );
}

#[cfg(unix)]
#[test]
fn add_worktree_leaves_no_branch_when_the_directory_cannot_be_created() {
    use std::os::unix::fs::PermissionsExt;

    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let parent = sandbox.dir("read-only");
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o555)).unwrap();
    // Permissions do not apply to root, so this cannot fail there. CI runs
    // as an ordinary user.
    let writable_anyway = fs::create_dir(parent.join("probe")).is_ok();

    let result = sandbox
        .cli()
        .add_worktree(&repo, &parent.join("feat"), "feat", "main");
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
    if !writable_anyway {
        assert!(result.is_err(), "{result:?}");
        assert!(!sandbox.has_rev(&repo, "refs/heads/feat"));
        assert_eq!(sandbox.cli().worktrees(&repo).unwrap().len(), 1);
    }
}

#[cfg(unix)]
#[test]
fn a_failing_hook_reports_the_worktree_it_left() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    sandbox.script("volee/.git/hooks/post-checkout", "#!/bin/sh\nexit 1\n");
    let path = sandbox.path("wt/feat");
    let error = sandbox
        .cli()
        .add_worktree(&repo, &path, "feat", "main")
        .unwrap_err();
    assert!(
        matches!(&error, GitError::WorktreeIncomplete { path: left, .. } if *left == path),
        "{error:?}"
    );
    assert!(path.exists());
    assert!(!sandbox.has_rev(&repo, "refs/heads/feat"));
}

#[test]
fn a_checkout_stopped_at_its_timeout_leaves_nothing_behind() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    // A clean and smudge filter that hangs, like a stuck Git LFS download.
    sandbox.git(&repo, &["config", "filter.slow.smudge", "sleep 30"]);
    sandbox.git(&repo, &["config", "filter.slow.clean", "cat"]);
    sandbox.commit(
        &repo,
        ".gitattributes",
        "*.bin filter=slow\n",
        "Use the filter",
    );
    sandbox.commit(&repo, "data.bin", "data\n", "Add data");

    let cli = sandbox.cli().write_timeout(Duration::from_secs(1));
    let path = sandbox.path("wt/feat");
    let started = Instant::now();
    let error = cli.add_worktree(&repo, &path, "feat", "main").unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "git was not stopped"
    );
    assert!(matches!(&error, GitError::TimedOut { .. }), "{error:?}");
    // SIGTERM let git remove the half-made worktree, and no branch was made.
    assert!(!path.exists());
    assert!(!sandbox.has_rev(&repo, "refs/heads/feat"));
    assert_eq!(cli.worktrees(&repo).unwrap().len(), 1);
}

fn assert_refused(result: Result<(), GitError>, expected: &RemovalRefusal) {
    match result {
        Err(GitError::RemovalRefused { reason, .. }) => assert_eq!(&reason, expected),
        other => panic!("expected a refusal ({expected}), got {other:?}"),
    }
}
