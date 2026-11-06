//! Finding the daemon, and starting it when needed.

use std::fs::OpenOptions;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use volee_proto::PROTOCOL_VERSION;
use volee_proto::client::Client;
use volee_proto::methods::{Status, StatusParams};
use volee_proto::paths::Paths;

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
    client
        .call::<Status>(StatusParams {
            protocol: PROTOCOL_VERSION,
        })
        .await
        .context("the running daemon does not match this volee version")?;
    Ok(client)
}

/// Starts `volee daemon run` detached from this terminal, logging to the state
/// directory.
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
    Command::new(program)
        .args(["daemon", "run"])
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(error_log)
        .process_group(0)
        .spawn()
        .context("cannot start the volee daemon")?;
    Ok(())
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
