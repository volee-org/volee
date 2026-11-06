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
    let sandbox = Sandbox::new();
    let program = sandbox.script("hanging-git", "#!/bin/sh\nexec sleep 30\n");
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

#[cfg(unix)]
#[test]
fn everything_a_timed_out_command_started_is_stopped() {
    let sandbox = Sandbox::new();
    let pid_file = sandbox.path("child.pid");
    let program = sandbox.script(
        "spawning-git",
        &format!(
            "#!/bin/sh\nsleep 30 &\necho $! > '{}'\nwait\n",
            pid_file.display()
        ),
    );
    let cli = sandbox.cli_with(GitCli::with_program(&program).timeout(Duration::from_millis(500)));

    let error = cli.repo_info(&sandbox.dir("anywhere")).unwrap_err();
    assert!(matches!(&error, GitError::TimedOut { .. }), "{error:?}");
    let pid = std::fs::read_to_string(&pid_file).unwrap();
    assert!(
        is_gone_soon(pid.trim()),
        "the process git started ({}) still runs",
        pid.trim()
    );
}

#[cfg(unix)]
#[test]
fn a_command_that_ignores_sigterm_is_killed() {
    let sandbox = Sandbox::new();
    let pid_file = sandbox.path("child.pid");
    let program = sandbox.script(
        "stubborn-git",
        &format!(
            "#!/bin/sh\ntrap '' TERM\nsleep 30 &\necho $! > '{}'\nwait\n",
            pid_file.display()
        ),
    );
    let cli = sandbox.cli_with(GitCli::with_program(&program).timeout(Duration::from_millis(300)));

    let started = Instant::now();
    let error = cli.repo_info(&sandbox.dir("anywhere")).unwrap_err();
    assert!(matches!(&error, GitError::TimedOut { .. }), "{error:?}");
    // The timeout, the grace period for SIGTERM, and some slack.
    assert!(started.elapsed() < Duration::from_secs(10));
    let pid = std::fs::read_to_string(&pid_file).unwrap();
    assert!(is_gone_soon(pid.trim()));
}

/// Whether the process `pid` disappears within a few seconds.
#[cfg(unix)]
fn is_gone_soon(pid: &str) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        let alive = std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !alive {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn failures_keep_the_command_the_exit_code_and_stderr() {
    let sandbox = Sandbox::new();
    let missing = sandbox.path("missing");
    let error = sandbox.cli().worktrees(&missing).unwrap_err();
    let GitError::Failed {
        command,
        code,
        stderr,
    } = &error
    else {
        panic!("expected a git failure, got {error:?}");
    };
    assert!(command.starts_with("git -C "), "{command}");
    assert!(command.contains(" worktree list "), "{command}");
    assert_eq!(*code, Some(128));
    assert!(stderr.contains("cannot change to"), "{stderr}");
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
            thread::spawn(move || cli.worktrees(&repo))
        })
        .collect();
    for thread in threads {
        let worktrees = thread.join().unwrap().unwrap();
        assert_eq!(worktrees.len(), 1);
    }
}
