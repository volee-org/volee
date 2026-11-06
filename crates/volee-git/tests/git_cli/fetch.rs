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

#[test]
fn fetch_forgets_remote_branches_deleted_on_the_remote() {
    let sandbox = Sandbox::new();
    let (clone, origin) = sandbox.cloned("volee");
    let teammate = sandbox.clone_of(&origin, "teammate");
    sandbox.git(&teammate, &["push", "--quiet", "origin", "main:topic"]);
    let cli = sandbox.cli();
    cli.fetch(&clone).unwrap();
    assert!(sandbox.has_rev(&clone, "refs/remotes/origin/topic"));

    sandbox.git(
        &teammate,
        &["push", "--quiet", "origin", "--delete", "topic"],
    );
    cli.fetch(&clone).unwrap();
    assert!(!sandbox.has_rev(&clone, "refs/remotes/origin/topic"));
}

#[cfg(unix)]
#[test]
fn ssh_never_prompts_unless_the_person_chose_their_own_ssh_command() {
    use std::fs;

    // A developer's shell may export its own ssh command, which volee then
    // leaves alone; CI exports none.
    if std::env::var_os("GIT_SSH_COMMAND").is_some() || std::env::var_os("GIT_SSH").is_some() {
        return;
    }
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let log = sandbox.path("ssh.log");
    // A fake ssh that records its arguments and fails like an unreachable
    // host: nothing leaves the machine.
    let recorder = format!("#!/bin/sh\necho \"$@\" >> '{}'\nexit 255\n", log.display());
    sandbox.script("bin/ssh", &recorder);
    let path = format!(
        "{}:{}",
        sandbox.path("bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    sandbox.git(
        &repo,
        &["remote", "add", "origin", "git.example.invalid:volee.git"],
    );
    let cli = sandbox.cli().env("PATH", &path);

    assert!(cli.fetch(&repo).is_err());
    let calls = fs::read_to_string(&log).unwrap();
    assert!(calls.contains("BatchMode=yes"), "{calls}");

    fs::remove_file(&log).unwrap();
    let own = sandbox.script("bin/my-ssh", &recorder);
    sandbox.git(&repo, &["config", "core.sshCommand", path_str(&own)]);
    assert!(cli.fetch(&repo).is_err());
    let calls = fs::read_to_string(&log).unwrap();
    assert!(!calls.contains("BatchMode"), "{calls}");
    assert!(calls.contains("git.example.invalid"), "{calls}");
}
