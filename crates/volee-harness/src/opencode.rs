//! The OpenCode profile (ADR 0022).
//!
//! Measured on OpenCode 2.2.5 (2026-11-09):
//!
//! | Need | Command |
//! | --- | --- |
//! | Launch, with a session id volee chooses | `opencode --session <id> --prompt <task>` |
//! | Resume | `opencode --session <id> [--prompt <message>]` |
//! | Approve every permission not denied | `--auto` |
//! | Its processes | `opencode ...`, without the background service (`opencode serve --service`) and the other subcommands |
//!
//! OpenCode has no hooks volee can pass on the command line and no listing
//! of its sessions with their state: an agent in a host gets its state from
//! its screen, and one started elsewhere is found in the process table
//! (ADR 0023), with the session id of its command line when it has one.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use volee_core::harness::{HarnessKind, SessionKey};
use volee_core::reconcile::{ListedSession, Listing};
use volee_core::state::AgentState;

use crate::error::HarnessError;
use crate::runner::{CommandRunner, Invocation, RunError};
use crate::spec::HostLaunch;

/// The profile's name, also the harness's name in volee.
pub const PROFILE: &str = "opencode";

/// OpenCode as volee names it.
pub fn kind() -> HarnessKind {
    HarnessKind::Other(PROFILE.to_owned())
}

/// A new session id in OpenCode's form (`ses_` and letters and digits).
pub fn new_session_id() -> String {
    format!("ses_{}", ulid::Ulid::generate().to_string().to_lowercase())
}

/// The arguments of `opencode` for a session: launched or resumed with
/// `session`, with a first prompt when there is one, approving every
/// permission it does not deny when `auto`.
pub fn args(session: &str, prompt: Option<&str>, auto: bool) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["--session".into(), session.into()];
    if let Some(prompt) = prompt.filter(|prompt| !prompt.trim().is_empty()) {
        args.push("--prompt".into());
        args.push(prompt.into());
    }
    if auto {
        args.push("--auto".into());
    }
    args
}

/// Starts `opencode <args>` in a volee host, in `cwd`, through `volee host
/// spawn`.
pub fn launch_in_host(
    runner: &dyn CommandRunner,
    host: &HostLaunch,
    cwd: &Path,
    opencode_args: Vec<OsString>,
    timeout: Duration,
) -> Result<(), HarnessError> {
    let mut args: Vec<OsString> = vec!["host".into(), "spawn".into()];
    for (name, value) in [
        ("--dir", host.dir.clone().into_os_string()),
        ("--id", host.id.clone().into()),
        ("--cwd", cwd.to_path_buf().into_os_string()),
        ("--cols", host.cols.to_string().into()),
        ("--rows", host.rows.to_string().into()),
        ("--profile", PROFILE.into()),
        ("--name", host.name.clone().into()),
    ] {
        args.push(name.into());
        args.push(value);
    }
    args.push("--".into());
    args.push("opencode".into());
    args.extend(opencode_args);
    let command = format!("{} host spawn --id {}", host.volee_bin.display(), host.id);
    let invocation = Invocation::new(host.volee_bin.clone(), timeout)
        .args(args)
        .cwd(cwd);
    let output = runner.run(&invocation).map_err(HarnessError::Run)?;
    if output.is_success() {
        Ok(())
    } else {
        Err(HarnessError::CommandFailed {
            command,
            code: output.code,
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

/// OpenCode subcommands that are not an agent.
const NOT_AGENTS: &[&str] = &[
    "serve",
    "service",
    "acp",
    "mcp",
    "auth",
    "models",
    "session",
    "debug",
    "upgrade",
    "update",
    "uninstall",
    "plugin",
    "stats",
    "reload",
    "pair",
    "api",
    "completion",
];

/// One process of the process table.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Process {
    pid: u32,
    ppid: u32,
    args: Vec<String>,
}

/// The OpenCode agents running on this machine that volee does not host, as
/// a listing (ADR 0023): one `ps`, and one `lsof` for their directories.
/// The listing is never complete: a process that is gone says nothing about
/// the session, so the daemon decides what its absence means.
pub fn list_processes(runner: &dyn CommandRunner) -> Result<Listing, HarnessError> {
    let ps = Invocation::new("ps", Duration::from_secs(5)).args(["-axo", "pid=,ppid=,args="]);
    let output = match runner.run(&ps) {
        Ok(output) => output,
        Err(RunError::NotFound { .. }) => return Ok(empty()),
        Err(error) => return Err(HarnessError::Run(error)),
    };
    let processes = parse_ps(&String::from_utf8_lossy(&output.stdout));
    let agents = agents(&processes);
    if agents.is_empty() {
        return Ok(empty());
    }
    let pids: Vec<String> = agents
        .iter()
        .map(|process| process.pid.to_string())
        .collect();
    let lsof = Invocation::new("lsof", Duration::from_secs(5)).args([
        "-a".to_owned(),
        "-d".to_owned(),
        "cwd".to_owned(),
        "-Fn".to_owned(),
        "-p".to_owned(),
        pids.join(","),
    ]);
    let cwds = runner
        .run(&lsof)
        .map(|output| parse_lsof(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default();
    let sessions = agents
        .iter()
        .filter_map(|process| {
            let cwd = cwds.get(&process.pid)?.clone();
            let id = session_of(&process.args).unwrap_or_else(|| format!("pid-{}", process.pid));
            Some(ListedSession {
                alive: true,
                background: false,
                pid: Some(process.pid),
                ..ListedSession::new(
                    SessionKey {
                        harness: kind(),
                        id,
                    },
                    cwd,
                    AgentState::Idle,
                )
            })
        })
        .collect();
    Ok(Listing {
        harness: kind(),
        sessions,
        complete: false,
    })
}

fn empty() -> Listing {
    Listing {
        harness: kind(),
        sessions: Vec::new(),
        complete: false,
    }
}

fn parse_ps(text: &str) -> Vec<Process> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            let args: Vec<String> = fields.map(str::to_owned).collect();
            (!args.is_empty()).then_some(Process {
                pid,
                ppid: parent,
                args,
            })
        })
        .collect()
}

/// The OpenCode processes that are agents and that no volee host runs.
fn agents(processes: &[Process]) -> Vec<Process> {
    let by_pid: HashMap<u32, &Process> = processes.iter().map(|p| (p.pid, p)).collect();
    processes
        .iter()
        .filter(|process| program(&process.args[0]) == PROFILE)
        .filter(|process| {
            process
                .args
                .get(1)
                .is_none_or(|first| !NOT_AGENTS.contains(&first.as_str()))
        })
        .filter(|process| {
            // volee's own agents are followed through their host.
            !by_pid
                .get(&process.ppid)
                .is_some_and(|parent| is_volee_host(&parent.args))
        })
        .cloned()
        .collect()
}

fn program(arg: &str) -> &str {
    Path::new(arg)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(arg)
}

/// Whether a command line is a volee host's: `volee host [--dir D] run ...`.
fn is_volee_host(args: &[String]) -> bool {
    program(&args[0]) == "volee"
        && args.iter().any(|arg| arg == "host")
        && args.iter().any(|arg| arg == "run")
}

/// The session id on an OpenCode command line, when it names one.
fn session_of(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--session" || arg == "-s" {
            return iter.next().cloned();
        }
        if let Some(id) = arg.strip_prefix("--session=") {
            return Some(id.to_owned());
        }
    }
    None
}

/// `lsof -Fn` output: `p<pid>` then `n<path>` lines.
fn parse_lsof(text: &str) -> HashMap<u32, PathBuf> {
    let mut cwds = HashMap::new();
    let mut pid = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse().ok();
        } else if let (Some(value), Some(current)) = (line.strip_prefix('n'), pid) {
            cwds.insert(current, PathBuf::from(value));
        }
    }
    cwds
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = "\
  43862     1 /opt/homebrew/Cellar/opencode/2.2.5/bin/opencode serve --service
  44025     1 /usr/local/bin/volee host --dir /state/hosts run --id ag_1-x -- opencode --session ses_a
  44026 44025 opencode --session ses_a --prompt Reply
  50000   900 opencode --session ses_b
  50001   900 /opt/homebrew/bin/opencode -c
  50002   900 opencode models
  50003   900 vim notes.txt
";

    #[test]
    fn agents_are_opencode_processes_volee_does_not_host() {
        let found: Vec<u32> = agents(&parse_ps(PS)).iter().map(|p| p.pid).collect();
        assert_eq!(found, [50000, 50001]);
    }

    #[test]
    fn a_session_id_comes_from_the_command_line() {
        let args = |line: &str| line.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            session_of(&args("opencode --session ses_b")).as_deref(),
            Some("ses_b")
        );
        assert_eq!(
            session_of(&args("opencode -s ses_c")).as_deref(),
            Some("ses_c")
        );
        assert_eq!(
            session_of(&args("opencode --session=ses_d")).as_deref(),
            Some("ses_d")
        );
        assert_eq!(session_of(&args("opencode -c")), None);
    }

    #[test]
    fn directories_come_from_lsof() {
        let cwds = parse_lsof("p50000\nfcwd\nn/work/alpha\np50001\nfcwd\nn/work/beta\n");
        assert_eq!(cwds.get(&50000), Some(&PathBuf::from("/work/alpha")));
        assert_eq!(cwds.get(&50001), Some(&PathBuf::from("/work/beta")));
    }

    #[test]
    fn launch_arguments_carry_the_session_and_the_prompt() {
        let strings = |args: Vec<OsString>| {
            args.into_iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            strings(args("ses_x", Some("Fix it"), true)),
            ["--session", "ses_x", "--prompt", "Fix it", "--auto"]
        );
        assert_eq!(
            strings(args("ses_x", Some("  "), false)),
            ["--session", "ses_x"]
        );
        assert!(new_session_id().starts_with("ses_"));
    }
}
