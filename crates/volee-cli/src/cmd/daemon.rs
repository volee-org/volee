//! `volee daemon run | start | stop | status`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use volee_core::harness::HarnessKind;
use volee_core::time::Elapsed;
use volee_daemon::{
    Clock, Config, DEFAULT_DISCOVERY_INTERVAL, DEFAULT_IDLE_DISCOVERY_INTERVAL, Daemon,
    EvidenceSources, SqliteLog, SystemClock,
};
use volee_git::{GhCli, GitCli};
use volee_harness::{ClaudeCode, Harness};
use volee_proto::client::Client;
use volee_proto::methods::{Empty, HarnessStatus, Shutdown, Status, StatusParams};
use volee_proto::paths::Paths;

use crate::connect::{paths, spawn_daemon, wait_for_daemon};

/// Runs the daemon in the foreground until it is stopped.
pub(crate) async fn run() -> Result<()> {
    init_logging();
    let paths = paths()?;
    std::fs::create_dir_all(&paths.state_dir)
        .with_context(|| format!("cannot create {}", paths.state_dir.display()))?;
    let log = SqliteLog::open(&paths.database())
        .with_context(|| format!("cannot open {}", paths.database().display()))?;
    let volee_bin = std::env::current_exe().ok();
    let (harness, status) = detect_claude_code().await;
    if harness.is_some() {
        tracing::info!(version = ?status.version, "using Claude Code");
    } else {
        tracing::warn!(detail = ?status.detail, "Claude Code is not available");
    }
    let evidence = EvidenceSources {
        git: Some(Arc::new(GitCli::new())),
        // `gh` is optional: without it (or signed out), pull requests are
        // not looked up and `volee daemon status` says why.
        forge: Some(Arc::new(GhCli::new())),
        settings: settings(&paths),
        ..EvidenceSources::default()
    };
    let config = Config {
        spool: Some(paths.spool_dir()),
        paths,
        log: Box::new(log),
        clock: Arc::new(SystemClock),
        harnesses: vec![status],
        harness,
        discovery_interval: DEFAULT_DISCOVERY_INTERVAL,
        idle_discovery_interval: DEFAULT_IDLE_DISCOVERY_INTERVAL,
        evidence,
        volee_bin,
    };
    Daemon::run(config)
        .await
        .context("the daemon stopped with an error")
}

/// Finds Claude Code and checks that its version has what volee needs.
async fn detect_claude_code() -> (Option<Arc<dyn Harness>>, HarnessStatus) {
    let claude = ClaudeCode::default();
    let probed = tokio::task::spawn_blocking(move || {
        let capabilities = claude.capabilities();
        (claude, capabilities)
    })
    .await;
    let mut status = HarnessStatus {
        kind: HarnessKind::ClaudeCode,
        available: false,
        version: None,
        detail: None,
    };
    match probed {
        Ok((claude, Ok(capabilities))) => {
            status.available = true;
            status.version = Some(capabilities.version.to_string());
            (Some(Arc::new(claude)), status)
        }
        Ok((_, Err(error))) => {
            status.detail = Some(error.to_string());
            (None, status)
        }
        Err(error) => {
            status.detail = Some(format!("checking Claude Code failed: {error}"));
            (None, status)
        }
    }
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
    if let Some(forge) = &status.forge {
        let version = forge.version.as_deref().unwrap_or("unknown version");
        if forge.available {
            println!("  forge    {} {version}, available", forge.program);
        } else {
            let reason = forge.detail.as_deref().unwrap_or("unknown reason");
            println!("  forge    {} unavailable: {reason}", forge.program);
        }
    }
    Ok(())
}

/// The person's configuration (repository names and base branches), or the
/// defaults when it cannot be read: the daemon must start even when the file
/// has a mistake, which `volee config check` explains.
fn settings(paths: &Paths) -> volee_config::Config {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    volee_config::Config::load(&paths.config_file(), &home).unwrap_or_else(|error| {
        tracing::warn!(%error, "ignoring the configuration file");
        volee_config::Config::default()
    })
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
