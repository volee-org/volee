//! Setup commands that prepare a new worktree before its agent starts
//! (ADR 0017).
//!
//! Each command runs with `/bin/sh -c` in the worktree, in a process group of
//! its own, with standard input closed. What it prints goes to the agent's
//! `setup.log` as it prints it, so the log can be followed while the setup
//! runs and keeps everything when a command is killed. A command that runs
//! out of time is killed with its whole process group, the processes it
//! started included.
//!
//! The launch checks, under the state lock, that the agent is still
//! launching before each command and before its session starts: a mission
//! that ended meanwhile stops the launch, and no session starts.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use volee_core::agent::Hold;
use volee_core::state::AgentState;
use volee_core::{AgentId, Event, EventKind};

use crate::error::DaemonError;
use crate::launch::{Target, blocking};
use crate::server::Shared;

/// How long one setup command may run.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The shell that runs setup commands.
const SHELL: &str = "/bin/sh";

/// How often a running command is checked.
const POLL: Duration = Duration::from_millis(50);

/// How much of the log is read back to find a failed command's last line.
const TAIL: u64 = 4_096;

/// The setup commands of the repository whose main checkout is `root`.
pub(crate) fn commands(shared: &Shared, root: &Path) -> Vec<String> {
    shared
        .settings
        .repo_at(root)
        .map(|(_, repo)| repo.setup.clone())
        .unwrap_or_default()
}

/// Runs `commands` in order in the agent's new `worktree`, and returns the
/// log they wrote. Meanwhile the agent is shown launching, with the command
/// that runs.
///
/// The first command that fails or runs out of time ends the setup with
/// [`DaemonError::SetupFailed`]; the caller marks the agent failed
/// ([`fail`]). [`DaemonError::Interrupted`] means the launch had to stop
/// (its mission ended): the agent is already marked stopped.
pub(crate) async fn run(
    shared: &Arc<Shared>,
    agent: AgentId,
    target: &Target,
    worktree: &Path,
    branch: &str,
    commands: &[String],
) -> Result<PathBuf, DaemonError> {
    let log = shared.paths.agent_dir(agent).join("setup.log");
    let env: Vec<(&'static str, OsString)> = vec![
        ("VOLEE_REPO", target.name.clone().into()),
        ("VOLEE_REPO_ROOT", target.root.clone().into()),
        ("VOLEE_WORKTREE", worktree.into()),
        ("VOLEE_BRANCH", branch.into()),
        ("VOLEE_BASE", target.base.clone().into()),
    ];
    for command in commands {
        launching(
            shared,
            agent,
            Some(format!("setting up: {}", first_line(command))),
        )
        .await?;
        let ran = {
            let (command, cwd, env, log) = (
                command.clone(),
                worktree.to_path_buf(),
                env.clone(),
                log.clone(),
            );
            blocking(move || run_one(&command, &cwd, &env, &log, TIMEOUT)).await?
        };
        let failure = ran.map_err(|source| DaemonError::Write {
            path: log.clone(),
            source,
        })?;
        if let Some(reason) = failure {
            return Err(DaemonError::SetupFailed {
                command: command.clone(),
                reason,
                log,
            });
        }
    }
    launching(shared, agent, None).await?;
    Ok(log)
}

/// Marks the agent of a setup that failed as failed, with why, when it is
/// still launching: a failed launch that a later one can reuse (ADR 0016).
pub(crate) async fn fail(shared: &Arc<Shared>, agent: AgentId, error: &DaemonError) {
    let detail = match error {
        DaemonError::SetupFailed {
            command, reason, ..
        } => format!("setup failed: `{}` {reason}", first_line(command)),
        other => format!("setup failed: {other}"),
    };
    let mut state = shared.state.lock().await;
    let launching = state
        .registry()
        .agent(agent)
        .is_some_and(|record| record.state == AgentState::Launching);
    if !launching {
        return;
    }
    let now = state.now();
    let failed = Event::new(
        now,
        EventKind::StateChanged {
            agent,
            state: AgentState::Failed,
            detail: Some(detail),
        },
    );
    if let Err(error) = state.commit(vec![failed]) {
        tracing::warn!(%agent, %error, "cannot record a failed setup");
    }
}

/// Shows the agent launching with `detail` (`None` clears it), unless the
/// launch must stop: the agent was forgotten, is no longer launching, or is
/// held, as when its mission ended. Then the agent, still launching, is
/// marked stopped, and the launch ends with [`DaemonError::Interrupted`].
async fn launching(
    shared: &Arc<Shared>,
    agent: AgentId,
    detail: Option<String>,
) -> Result<(), DaemonError> {
    let mut state = shared.state.lock().await;
    let Some(record) = state.registry().agent(agent) else {
        return Err(DaemonError::Interrupted(
            "the agent was forgotten during its setup; no session started".to_owned(),
        ));
    };
    let name = record.name.clone();
    let still_launching = record.state == AgentState::Launching;
    let stop = match &record.hold {
        Some(Hold::MissionOver { .. }) => Some("its mission ended during its setup"),
        Some(Hold::StoppedByHuman { .. }) => Some("it was stopped during its setup"),
        None if !still_launching => Some("it was no longer launching after its setup"),
        None => None,
    };
    let now = state.now();
    let Some(why) = stop else {
        state.commit(vec![Event::new(
            now,
            EventKind::StateChanged {
                agent,
                state: AgentState::Launching,
                detail,
            },
        )])?;
        return Ok(());
    };
    if still_launching {
        state.commit(vec![Event::new(
            now,
            EventKind::StateChanged {
                agent,
                state: AgentState::Stopped,
                detail: Some(format!("no session started: {why}")),
            },
        )])?;
    }
    Err(DaemonError::Interrupted(format!(
        "volee stopped launching {name}: {why}. No session started; its worktree and branch stay"
    )))
}

/// The first line of a command, to show it in one line.
fn first_line(command: &str) -> &str {
    command.lines().next().unwrap_or_default().trim()
}

/// Runs `command` with `/bin/sh -c` in `cwd`, with `env` added and standard
/// input closed, in a process group of its own, appending what it prints to
/// `log`. Returns why it failed, if it did, such as `exited with status 1:
/// <its last line>`; an error only when the log cannot be written.
fn run_one(
    command: &str,
    cwd: &Path,
    env: &[(&'static str, OsString)],
    log: &Path,
    timeout: Duration,
) -> io::Result<Option<String>> {
    let mut file = OpenOptions::new().create(true).append(true).open(log)?;
    writeln!(file, "$ {command}")?;
    let printed_from = file.metadata()?.len();
    let spawned = Command::new(SHELL)
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .envs(env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::from(file.try_clone()?))
        .stderr(Stdio::from(file.try_clone()?))
        .process_group(0)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            let reason = format!("could not run: {error}");
            writeln!(file, "[{reason}]")?;
            return Ok(Some(reason));
        }
    };
    let Some(status) = wait(&mut child, timeout)? else {
        let reason = format!(
            "did not finish within {} and was killed",
            duration_words(timeout)
        );
        writeln!(file, "[{reason}]")?;
        return Ok(Some(reason));
    };
    if status.success() {
        return Ok(None);
    }
    let ended = status.code().map_or_else(
        || "was stopped by a signal".to_owned(),
        |code| format!("exited with status {code}"),
    );
    let last = last_line(log, printed_from)?;
    writeln!(file, "[{ended}]")?;
    Ok(Some(match last {
        Some(line) => format!("{ended}: {line}"),
        None => ended,
    }))
}

/// Waits for `child` until it exits or `timeout` passes; then kills its
/// whole process group and returns `None`.
fn wait(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if started.elapsed() >= timeout {
            kill_group(child);
            return Ok(None);
        }
        std::thread::sleep(POLL);
    }
}

/// Kills the process group `child` leads, then reaps `child`. The command
/// runs in a group of its own, so this also stops what it started in the
/// background; when `kill` cannot run, only the shell is killed.
fn kill_group(child: &mut Child) {
    let group = format!("-{}", child.id());
    let killed = Command::new("kill")
        .args(["-KILL", "--", &group])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !killed && let Err(error) = child.kill() {
        tracing::debug!(%error, "could not kill a setup command");
    }
    if let Err(error) = child.wait() {
        tracing::debug!(%error, "could not reap a setup command");
    }
}

/// The last line a command printed, read back from the end of the log.
fn last_line(log: &Path, printed_from: u64) -> io::Result<Option<String>> {
    let mut file = File::open(log)?;
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(
        printed_from.max(length.saturating_sub(TAIL)),
    ))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes)
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(|line| line.chars().take(300).collect()))
}

/// `15 minutes`, `2 seconds`.
fn duration_words(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 60 && seconds.is_multiple_of(60) {
        let minutes = seconds / 60;
        format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
    } else {
        let seconds = duration.as_secs_f64();
        format!(
            "{seconds} second{}",
            if (seconds - 1.0).abs() < f64::EPSILON {
                ""
            } else {
                "s"
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(dir: &Path) -> Vec<(&'static str, OsString)> {
        vec![("VOLEE_WORKTREE", dir.into())]
    }

    #[test]
    fn a_command_runs_in_its_folder_and_prints_to_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("setup.log");
        let failure = run_one(
            "echo \"in $VOLEE_WORKTREE\"; pwd > here.txt",
            dir.path(),
            &env(dir.path()),
            &log,
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(failure, None);
        let here = std::fs::read_to_string(dir.path().join("here.txt")).unwrap();
        assert_eq!(
            Path::new(here.trim()).canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            text,
            format!(
                "$ echo \"in $VOLEE_WORKTREE\"; pwd > here.txt\nin {}\n",
                dir.path().display()
            )
        );
    }

    #[test]
    fn a_failure_says_its_status_and_last_line() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("setup.log");
        std::fs::write(&log, "$ earlier\nan older line\n").unwrap();
        let failure = run_one(
            "echo resolving; echo 'lockfile is frozen' >&2; exit 3",
            dir.path(),
            &env(dir.path()),
            &log,
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(
            failure.as_deref(),
            Some("exited with status 3: lockfile is frozen")
        );
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            text.ends_with("resolving\nlockfile is frozen\n[exited with status 3]\n"),
            "{text}"
        );
        // A command that prints nothing still says how it ended.
        let silent = run_one(
            "exit 4",
            dir.path(),
            &env(dir.path()),
            &log,
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(silent.as_deref(), Some("exited with status 4"));
    }

    #[test]
    fn a_command_out_of_time_is_killed_with_what_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("setup.log");
        let started = Instant::now();
        let failure = run_one(
            "(sleep 1; touch late) & echo waiting; sleep 30",
            dir.path(),
            &env(dir.path()),
            &log,
            Duration::from_millis(300),
        )
        .unwrap();
        assert_eq!(
            failure.as_deref(),
            Some("did not finish within 0.3 seconds and was killed")
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        // What it printed before is kept.
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("waiting\n"), "{text}");
        // The background process went with it.
        std::thread::sleep(Duration::from_millis(1_500));
        assert!(!dir.path().join("late").exists());
    }

    #[test]
    fn durations_read_as_words() {
        assert_eq!(duration_words(TIMEOUT), "15 minutes");
        assert_eq!(duration_words(Duration::from_secs(60)), "1 minute");
        assert_eq!(duration_words(Duration::from_secs(2)), "2 seconds");
        assert_eq!(duration_words(Duration::from_secs(1)), "1 second");
    }
}
