//! `Git::fetch`.

use volee_git::{Git, GitError};

use crate::fixture::{Sandbox, path_str};

#[test]
fn fetch_brings_new_commits_from_origin() {
    let sandbox = Sandbox::new();
    let (clone, origin) = sandbox.cloned("volee");
    let teammate = sandbox.clone_of(&origin, "teammate");
    let pushed = sandbox.commit(&teammate, "news.md", "news\n", "Add news");
    sandbox.git(&teammate, &["push", "--quiet", "origin", "main"]);
    assert_ne!(sandbox.rev(&clone, "origin/main"), pushed);

    sandbox.cli().fetch(&clone).unwrap();
    assert_eq!(sandbox.rev(&clone, "origin/main"), pushed);
    // The person's `FETCH_HEAD` is left alone.
    assert!(!clone.join(".git/FETCH_HEAD").exists());
}

#[test]
fn fetch_without_origin_does_nothing() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("local-only");
    sandbox.cli().fetch(&repo).unwrap();
}

#[test]
fn fetch_from_an_unreachable_origin_fails() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let missing = sandbox.path("missing.git");
    sandbox.git(&repo, &["remote", "add", "origin", path_str(&missing)]);
    let error = sandbox.cli().fetch(&repo).unwrap_err();
    let GitError::Failed {
        command, stderr, ..
    } = &error
    else {
        panic!("expected a git failure, got {error:?}");
    };
    assert!(command.contains("fetch"), "{command}");
    assert!(stderr.contains("missing.git"), "{stderr}");
}
