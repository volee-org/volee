//! `Git::repo_info`.

use std::fs;

use volee_git::{Git, GitError, RepoInfo};

use crate::fixture::{Sandbox, path_str};

#[test]
fn a_directory_outside_any_repository_has_no_repo_info() {
    let sandbox = Sandbox::new();
    let outside = sandbox.dir("outside");
    assert_eq!(sandbox.cli().repo_info(&outside).unwrap(), None);
}

#[test]
fn the_main_checkout_reports_its_branch_and_commit() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let info = sandbox.cli().repo_info(&repo).unwrap();
    assert_eq!(
        info,
        Some(RepoInfo {
            top_level: repo.clone(),
            main_checkout: repo.clone(),
            name: "volee".into(),
            is_linked_worktree: false,
            branch: Some("main".into()),
            head: Some(sandbox.head(&repo)),
        })
    );
}

#[test]
fn a_subdirectory_reports_its_top_level() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let deep = sandbox.dir("volee/src/deep");
    let info = sandbox.cli().repo_info(&deep).unwrap().unwrap();
    assert_eq!(info.top_level, repo);
    assert_eq!(info.main_checkout, repo);
}

#[test]
fn a_linked_worktree_points_to_its_main_checkout() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let worktree = sandbox.worktree(&repo, "wt/feat", "feat/x", "main");
    let info = sandbox.cli().repo_info(&worktree).unwrap();
    assert_eq!(
        info,
        Some(RepoInfo {
            top_level: worktree,
            main_checkout: repo.clone(),
            name: "volee".into(),
            is_linked_worktree: true,
            branch: Some("feat/x".into()),
            head: Some(sandbox.head(&repo)),
        })
    );
}

#[test]
fn a_detached_head_has_a_commit_but_no_branch() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    sandbox.git(&repo, &["checkout", "--quiet", "--detach"]);
    let info = sandbox.cli().repo_info(&repo).unwrap().unwrap();
    assert_eq!(info.branch, None);
    assert_eq!(info.head, Some(sandbox.head(&repo)));
}

#[test]
fn a_repository_without_commits_has_a_branch_but_no_commit() {
    let sandbox = Sandbox::new();
    let repo = sandbox.empty_repo("fresh");
    let info = sandbox.cli().repo_info(&repo).unwrap().unwrap();
    assert_eq!(info.branch.as_deref(), Some("main"));
    assert_eq!(info.head, None);
}

#[test]
fn bare_repositories_and_git_directories_have_no_working_tree() {
    let sandbox = Sandbox::new();
    let bare = sandbox.bare("volee.git");
    let repo = sandbox.repo("volee");
    let cli = sandbox.cli();
    assert_eq!(cli.repo_info(&bare).unwrap(), None);
    assert_eq!(cli.repo_info(&repo.join(".git")).unwrap(), None);
}

#[test]
fn worktrees_of_a_hidden_bare_repository_are_named_after_its_folder() {
    let sandbox = Sandbox::new();
    let seed = sandbox.repo("seed");
    let project = sandbox.dir("project");
    let bare = project.join(".bare");
    sandbox.git(
        &project,
        &["clone", "--quiet", "--bare", path_str(&seed), ".bare"],
    );
    fs::write(project.join(".git"), "gitdir: ./.bare\n").unwrap();
    let worktree = sandbox.worktree(&project, "project/feat", "feat", "main");

    let info = sandbox.cli().repo_info(&worktree).unwrap().unwrap();
    assert_eq!(info.top_level, worktree);
    assert_eq!(info.main_checkout, bare);
    assert_eq!(info.name, "project");
    assert!(info.is_linked_worktree);
}

#[test]
fn a_submodule_is_the_same_repository_in_all_of_its_worktrees() {
    let sandbox = Sandbox::new();
    let lib = sandbox.repo("lib");
    let app = sandbox.repo("app");
    sandbox.git(
        &app,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "--quiet",
            path_str(&lib),
            "vendor/lib",
        ],
    );
    let submodule = app.join("vendor/lib");
    let linked = sandbox.worktree(&submodule, "lib-feat", "feat", "main");

    let cli = sandbox.cli();
    let main = cli.repo_info(&submodule).unwrap().unwrap();
    assert_eq!(main.top_level, submodule);
    assert_eq!(main.main_checkout, submodule);
    assert_eq!(main.name, "lib");
    assert!(!main.is_linked_worktree);

    let linked = cli.repo_info(&linked).unwrap().unwrap();
    assert!(linked.is_linked_worktree);
    assert_eq!(linked.main_checkout, submodule);
}

#[test]
fn a_missing_directory_is_an_error_not_an_absence() {
    let sandbox = Sandbox::new();
    let missing = sandbox.path("missing");
    let error = sandbox.cli().repo_info(&missing).unwrap_err();
    let GitError::Failed {
        command,
        code,
        stderr,
    } = error
    else {
        panic!("expected a git failure, got {error:?}");
    };
    assert!(command.contains("rev-parse"), "{command}");
    assert_eq!(code, Some(128));
    assert!(stderr.contains("cannot change to"), "{stderr}");
}
