//! `volee daemon run | start | stop | status`.

use std::sync::Arc;

use anyhow::{Context, Result};
use volee_core::time::Elapsed;
use volee_daemon::{Clock, Config, Daemon, SqliteLog, SystemClock};
use volee_proto::client::Client;
use volee_proto::methods::{Empty, Shutdown, Status, StatusParams};

use crate::connect::{paths, spawn_daemon, wait_for_daemon};

/// Runs the daemon in the foreground until it is stopped.
pub(crate) async fn run() -> Result<()> {
    init_logging();
    let paths = paths()?;
    std::fs::create_dir_all(&paths.state_dir)
        .with_context(|| format!("cannot create {}", paths.state_dir.display()))?;
    let log = SqliteLog::open(&paths.database())
        .with_context(|| format!("cannot open {}", paths.database().display()))?;
    let config = Config {
        spool: Some(paths.spool_dir()),
        paths,
        log: Box::new(log),
        clock: Arc::new(SystemClock),
        harnesses: vec![],
    };
    Daemon::run(config)
        .await
        .context("the daemon stopped with an error")
}

/// Starts the daemon in the background unless it already runs.
pub(crate) async fn start() -> Result<()> {
    let paths = paths()?;
    if Client::connect(&paths.socket).await.is_ok() {
        println!("The volee daemon is already running.");
        return Ok(());
    }
    spawn_daemon(&paths)?;
    wait_for_daemon(&paths).await?;
    println!("Started the volee daemon.");
    Ok(())
}

/// Asks the running daemon to stop.
pub(crate) async fn stop() -> Result<()> {
    let paths = paths()?;
    let Ok(client) = Client::connect(&paths.socket).await else {
        println!("The volee daemon is not running.");
        return Ok(());
    };
    client.call::<Shutdown>(Empty {}).await?;
    println!("Stopped the volee daemon.");
    Ok(())
}

/// Prints whether the daemon runs, without starting it.
pub(crate) async fn status() -> Result<()> {
    let paths = paths()?;
    let Ok(client) = Client::connect(&paths.socket).await else {
        println!("The volee daemon is not running. Start it with `volee daemon start`.");
        return Ok(());
    };
    let status = client
        .call::<Status>(StatusParams {
            protocol: volee_proto::PROTOCOL_VERSION,
        })
        .await?;
    let uptime = Elapsed::between(status.started_at, SystemClock.now());
    println!(
        "volee daemon {} (pid {}), up {uptime}",
        status.version, status.pid
    );
    println!("  agents   {}", status.agents);
    println!(
        "  events   {}",
        status
            .last_seq
            .map_or_else(|| "none".to_owned(), |seq| seq.to_string())
    );
    println!("  socket   {}", paths.socket.display());
    println!("  database {}", paths.database().display());
    for harness in &status.harnesses {
        let version = harness.version.as_deref().unwrap_or("unknown version");
        let state = if harness.available {
            "available"
        } else {
            "unavailable"
        };
        println!("  harness  {} {version}, {state}", harness.kind);
    }
    Ok(())
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("VOLEE_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}
