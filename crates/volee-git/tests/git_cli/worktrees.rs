//! `Git::worktrees`, `Git::default_branch`, `Git::add_worktree` and
//! `Git::remove_worktree`.

use std::fs;
use std::path::Path;

use volee_git::{Git, GitError, NewWorktree, WorktreeInfo};

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
    assert_refused(cli.remove_worktree(&repo, &path), "modified or untracked");
    sandbox.git(&path, &["checkout", "--quiet", "--", "README.md"]);

    sandbox.write(&path.join("new.txt"), "new\n");
    assert_refused(cli.remove_worktree(&repo, &path), "modified or untracked");
    fs::remove_file(path.join("new.txt")).unwrap();

    sandbox.git(&repo, &["worktree", "lock", path_str(&path)]);
    assert_refused(cli.remove_worktree(&repo, &path), "locked");
    assert!(path.join("README.md").is_file());

    assert_refused(cli.remove_worktree(&repo, &repo), "main working tree");
}

fn assert_refused(result: Result<(), GitError>, reason: &str) {
    match result {
        Err(GitError::Failed { stderr, .. }) => assert!(stderr.contains(reason), "{stderr}"),
        other => panic!("expected git to refuse ({reason}), got {other:?}"),
    }
}
