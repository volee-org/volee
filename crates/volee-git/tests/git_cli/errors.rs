//! How `GitCli` runs git: failures, timeouts and concurrency.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use volee_git::{Git, GitCli, GitError};

use crate::fixture::Sandbox;

#[test]
fn a_missing_git_executable_is_a_spawn_error() {
    let sandbox = Sandbox::new();
    let program = sandbox.path("no-such-git");
    let cli = sandbox.cli_with(GitCli::with_program(&program));
    let error = cli.repo_info(&sandbox.dir("anywhere")).unwrap_err();
    assert!(
        matches!(&error, GitError::Spawn { program: tried, .. } if *tried == program),
        "{error:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_command_that_hangs_is_stopped_at_its_timeout() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let sandbox = Sandbox::new();
    let program = sandbox.path("hanging-git");
    fs::write(&program, "#!/bin/sh\nexec sleep 30\n").unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    let timeout = Duration::from_millis(300);
    let cli = sandbox.cli_with(GitCli::with_program(&program).timeout(timeout));

    let started = Instant::now();
    let error = cli.repo_info(&sandbox.dir("anywhere")).unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "git was not stopped"
    );
    let GitError::TimedOut {
        command,
        timeout: given,
    } = &error
    else {
        panic!("expected a timeout, got {error:?}");
    };
    assert!(command.contains("rev-parse"), "{command}");
    assert_eq!(*given, timeout);
}

#[test]
fn failures_keep_the_command_the_exit_code_and_stderr() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let error = sandbox.cli().remove_worktree(&repo, &repo).unwrap_err();
    let GitError::Failed {
        command,
        code,
        stderr,
    } = &error
    else {
        panic!("expected a git failure, got {error:?}");
    };
    assert!(command.starts_with("git -C "), "{command}");
    assert!(command.contains(" worktree remove -- "), "{command}");
    assert_eq!(*code, Some(128));
    assert!(stderr.contains("is a main working tree"), "{stderr}");
    assert!(error.to_string().contains("exit code 128"), "{error}");
}

#[test]
fn many_threads_can_share_one_cli() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let limit = NonZeroUsize::new(2).unwrap();
    let cli = Arc::new(sandbox.cli().max_concurrent(limit));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let (cli, repo) = (Arc::clone(&cli), repo.clone());
            thread::spawn(move || cli.delivery(&repo, "main", None))
        })
        .collect();
    for thread in threads {
        let delivery = thread.join().unwrap().unwrap();
        assert_eq!(delivery.commits, 0);
    }
}
