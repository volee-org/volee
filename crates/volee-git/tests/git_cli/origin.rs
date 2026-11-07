//! `Git::origin`.

use volee_git::{Git, RepoSlug};

use crate::fixture::{Sandbox, path_str};

#[test]
fn origin_names_the_repository_on_its_forge() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    sandbox.git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:volee-org/volee.git",
        ],
    );
    assert_eq!(
        sandbox.cli().origin(&repo).unwrap(),
        Some(RepoSlug {
            owner: "volee-org".into(),
            name: "volee".into(),
        })
    );
}

#[test]
fn origin_follows_url_rewrites() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    sandbox.git(&repo, &["remote", "add", "origin", "gh:volee-org/volee"]);
    sandbox.git(
        &repo,
        &["config", "url.https://github.com/.insteadOf", "gh:"],
    );
    let origin = sandbox.cli().origin(&repo).unwrap().unwrap();
    assert_eq!(origin.to_string(), "volee-org/volee");
}

#[test]
fn no_origin_or_a_local_one_names_nothing() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("local-only");
    assert_eq!(sandbox.cli().origin(&repo).unwrap(), None);

    let (clone, origin) = sandbox.cloned("cloned");
    assert!(path_str(&origin).starts_with('/'));
    assert_eq!(sandbox.cli().origin(&clone).unwrap(), None);
}
