//! `Git::base_of`: the branch another branch was really created from.

use volee_git::{BaseGuess, BaseSource, Git};

use crate::fixture::{Sandbox, path_str};

#[allow(clippy::unnecessary_wraps)] // Compared with `base_of`, which returns an option.
fn guess(branch: &str, source: BaseSource) -> Option<BaseGuess> {
    Some(BaseGuess {
        branch: branch.to_owned(),
        source,
    })
}

/// A repository with `main`, and `develop` two commits ahead of it.
fn with_develop(sandbox: &Sandbox) -> std::path::PathBuf {
    let repo = sandbox.repo("volee");
    sandbox.git(&repo, &["branch", "develop"]);
    sandbox.git(&repo, &["switch", "--quiet", "develop"]);
    sandbox.commit(&repo, "one.rs", "one\n", "Add one");
    sandbox.commit(&repo, "two.rs", "two\n", "Add two");
    sandbox.git(&repo, &["switch", "--quiet", "main"]);
    repo
}

#[test]
fn a_branch_made_with_a_start_point_says_so_in_its_reflog() {
    let sandbox = Sandbox::new();
    let repo = with_develop(&sandbox);
    let tree = sandbox.path("wt/feat");
    sandbox.git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "feat",
            path_str(&tree),
            "develop",
        ],
    );
    sandbox.commit(&tree, "feat.rs", "feat\n", "Add feat");
    assert_eq!(
        sandbox.cli().base_of(&repo, "feat").unwrap(),
        guess("develop", BaseSource::Reflog),
        "not main, which is the default branch"
    );
}

#[test]
fn the_branch_a_branch_tracks_comes_before_anything_else() {
    let sandbox = Sandbox::new();
    let repo = with_develop(&sandbox);
    // Created from main, then made to track develop.
    sandbox.git(&repo, &["branch", "feat", "main"]);
    sandbox.git(&repo, &["branch", "--set-upstream-to=develop", "feat"]);
    assert_eq!(
        sandbox.cli().base_of(&repo, "feat").unwrap(),
        guess("develop", BaseSource::Upstream)
    );
}

#[test]
fn a_branch_that_tracks_its_own_copy_on_the_remote_says_nothing() {
    let sandbox = Sandbox::new();
    let (clone, _origin) = sandbox.cloned("volee");
    sandbox.git(&clone, &["switch", "--quiet", "-c", "feat"]);
    sandbox.commit(&clone, "feat.rs", "feat\n", "Add feat");
    sandbox.git(
        &clone,
        &["push", "--quiet", "--set-upstream", "origin", "feat"],
    );
    let found = sandbox.cli().base_of(&clone, "feat").unwrap().unwrap();
    assert_eq!(found.branch, "main");
    assert_ne!(found.source, BaseSource::Upstream);
}

#[test]
fn without_a_usable_reflog_the_nearest_branch_wins_over_the_default() {
    let sandbox = Sandbox::new();
    let repo = with_develop(&sandbox);
    // Made from HEAD while develop was checked out: the reflog only says
    // "HEAD".
    sandbox.git(&repo, &["switch", "--quiet", "develop"]);
    sandbox.git(&repo, &["switch", "--quiet", "-c", "feat"]);
    sandbox.commit(&repo, "feat.rs", "feat\n", "Add feat");
    assert_eq!(
        sandbox.cli().base_of(&repo, "feat").unwrap(),
        guess("develop", BaseSource::Nearest)
    );
}

#[test]
fn a_tie_between_branches_goes_to_the_default_branch() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    sandbox.git(&repo, &["branch", "other"]);
    sandbox.git(&repo, &["switch", "--quiet", "-c", "feat"]);
    sandbox.commit(&repo, "feat.rs", "feat\n", "Add feat");
    assert_eq!(
        sandbox.cli().base_of(&repo, "feat").unwrap(),
        guess("main", BaseSource::Nearest),
        "main and other are equally near"
    );
}

#[test]
fn a_branch_that_does_not_exist_falls_back_to_the_default() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    assert_eq!(
        sandbox.cli().base_of(&repo, "nowhere").unwrap(),
        guess("main", BaseSource::Default)
    );
}
