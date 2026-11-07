//! Finding the daemon, and starting it when needed.

use std::fs::OpenOptions;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use volee_proto::PROTOCOL_VERSION;
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{Status, StatusParams};
use volee_proto::paths::Paths;
use volee_proto::rpc;

/// How long to wait for a freshly started daemon to accept connections.
const STARTUP: Duration = Duration::from_secs(5);

/// The paths of this user's volee installation.
pub(crate) fn paths() -> Result<Paths> {
    Paths::from_env().context("cannot locate volee's directories")
}

/// Connects to the daemon, starting it in the background when it is not
/// running, and checks that it speaks the same protocol.
pub(crate) async fn connect_or_start() -> Result<Client> {
    let paths = paths()?;
    let client = if let Ok(client) = Client::connect(&paths.socket).await {
        client
    } else {
        spawn_daemon(&paths)?;
        wait_for_daemon(&paths).await?
    };
    let status = client
        .call::<Status>(StatusParams {
            protocol: PROTOCOL_VERSION,
        })
        .await;
    match status {
        Ok(_) => Ok(client),
        // A daemon too old or too new to read the request at all.
        Err(ClientError::Rpc(refusal))
            if matches!(
                refusal.code,
                rpc::PARSE_ERROR
                    | rpc::INVALID_REQUEST
                    | rpc::METHOD_NOT_FOUND
                    | rpc::INVALID_PARAMS
            ) =>
        {
            bail!(
                "the running daemon does not match this volee version ({}): stop it with `volee \
                 daemon stop` and run your command again",
                refusal.message
            )
        }
        // The daemon's own words, such as a protocol mismatch, which names
        // both versions and how to restart it.
        Err(ClientError::Rpc(refusal)) => Err(ClientError::Rpc(refusal).into()),
        Err(error) => Err(anyhow::Error::new(error).context(format!(
            "the volee daemon did not answer; see {}",
            paths.log_dir().join("daemon.log").display()
        ))),
    }
}

/// Starts `volee daemon run` detached from this terminal, logging to the state
/// directory.
///
/// Commands often run inside an agent's session or a multiplexer pane (hooks
/// and `volee mcp` do, by design). The daemon outlives them and launches
/// agents of its own, so it must not keep their identity: it starts in the
/// state directory, without the variables that name the session or pane it
/// came from.
pub(crate) fn spawn_daemon(paths: &Paths) -> Result<()> {
    std::fs::create_dir_all(paths.log_dir())
        .with_context(|| format!("cannot create {}", paths.log_dir().display()))?;
    let log_path = paths.log_dir().join("daemon.log");
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("cannot open {}", log_path.display()))?;
    let error_log = log.try_clone().context("cannot duplicate the log handle")?;
    let program = std::env::current_exe().context("cannot find the volee executable")?;
    let mut command = Command::new(program);
    command
        .args(["daemon", "run"])
        .current_dir(&paths.state_dir)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(error_log)
        .process_group(0);
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(names_a_session) {
            command.env_remove(&name);
        }
    }
    command.spawn().context("cannot start the volee daemon")?;
    Ok(())
}

/// Variables that identify the agent session or terminal pane a process runs
/// in, rather than configure it. User settings (for example
/// `CLAUDE_CODE_USE_BEDROCK` or `ANTHROPIC_API_KEY`) are not among them.
const SESSION_VARIABLES: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_AGENT",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_HOST_SHORT_TEMP_DIRS",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_EFFORT",
    "CLAUDE_ENV_FILE",
    "CLAUDE_JOB_DIR",
    "CLAUDE_PID",
    "CLAUDE_PROJECT_DIR",
    "GHOSTTY_SURFACE_ID",
    "ITERM_SESSION_ID",
    "KITTY_WINDOW_ID",
    "TERM_SESSION_ID",
    "TMUX",
    "TMUX_PANE",
    "WEZTERM_PANE",
];

/// Prefixes of variables that multiplexers set in their panes.
const SESSION_PREFIXES: &[&str] = &["HERDR_", "TUIOS_", "ZELLIJ"];

fn names_a_session(name: &str) -> bool {
    SESSION_VARIABLES.contains(&name)
        || SESSION_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

/// Waits until the daemon accepts connections.
pub(crate) async fn wait_for_daemon(paths: &Paths) -> Result<Client> {
    let deadline = tokio::time::Instant::now() + STARTUP;
    loop {
        match Client::connect(&paths.socket).await {
            Ok(client) => return Ok(client),
            Err(error) if tokio::time::Instant::now() >= deadline => {
                return Err(error).with_context(|| {
                    format!(
                        "the volee daemon did not start; see {}",
                        paths.log_dir().join("daemon.log").display()
                    )
                });
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::names_a_session;

    #[test]
    fn session_identity_is_dropped_and_settings_are_kept() {
        for name in [
            "CLAUDECODE",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "TUIOS_PANE_ID",
            "HERDR_ENV",
            "ZELLIJ_SESSION_NAME",
            "TMUX",
        ] {
            assert!(names_a_session(name), "{name}");
        }
        for name in [
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_SUBAGENT_MODEL",
            "ANTHROPIC_API_KEY",
            "PATH",
            "HOME",
            "VOLEE_LOG",
        ] {
            assert!(!names_a_session(name), "{name}");
        }
    }
}
