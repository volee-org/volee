//! `volee host`: start, list, watch, take over and stop hosts (ADR 0019).
//!
//! The daemon launches every agent in a host through `volee host spawn`; these
//! commands also work on hosts directly, without the daemon, for any command
//! line (the generic profile of ADR 0022).

use std::fs::OpenOptions;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use volee_core::AgentId;
use volee_host::{AttachEnd, HostConfig, HostDir, Size, Watch};

/// Where hosts live by default: `<state>/hosts`.
pub(crate) fn host_dir() -> anyhow::Result<HostDir> {
    let paths = volee_proto::paths::Paths::from_env()?;
    Ok(HostDir::new(paths.state_dir.join("hosts")))
}

/// How to start a host.
#[derive(Debug)]
pub(crate) struct Spawn {
    pub(crate) id: Option<String>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) size: Option<(u16, u16)>,
    pub(crate) profile: String,
    pub(crate) name: Option<String>,
    pub(crate) linger: u64,
    pub(crate) command: Vec<String>,
}

/// Starts a host in the background, detached from this terminal, and waits
/// until it answers. Prints its id.
pub(crate) fn spawn(dir: &HostDir, spawn: Spawn) -> anyhow::Result<()> {
    dir.ensure()
        .with_context(|| format!("cannot create {}", dir.path().display()))?;
    let id = spawn
        .id
        .unwrap_or_else(|| AgentId::from_ulid(ulid::Ulid::generate()).to_string());
    check_id(&id)?;
    let socket = dir.socket(&id);
    if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
        bail!("a host named {id} already runs");
    }
    let cwd = match spawn.cwd {
        Some(cwd) => cwd,
        None => std::env::current_dir()?,
    };
    let (cols, rows) = spawn.size.unwrap_or_else(|| {
        volee_host::terminal_size().map_or((160, 45), |size| (size.cols, size.rows))
    });
    let exe = std::env::current_exe().context("cannot find the volee program")?;
    let mut command = Command::new(exe);
    command
        .arg("host")
        .arg("--dir")
        .arg(dir.path())
        .args(["run", "--id", &id, "--profile", &spawn.profile])
        .arg("--cwd")
        .arg(&cwd)
        .args(["--cols", &cols.to_string(), "--rows", &rows.to_string()])
        .args(["--linger", &spawn.linger.to_string()]);
    if let Some(name) = &spawn.name {
        command.args(["--name", name]);
    }
    let mut child = command
        .arg("--")
        .args(&spawn.command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("cannot start the host")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::os::unix::net::UnixStream::connect(&socket).is_err() {
        if let Some(status) = child.try_wait()? {
            bail!(
                "the host stopped before answering ({status}); see {}",
                dir.log(&id).display()
            );
        }
        if Instant::now() > deadline {
            bail!(
                "the host did not answer within 5 seconds; see {}",
                dir.log(&id).display()
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
    println!("{id}");
    Ok(())
}

/// A host id names files in the hosts directory: letters, digits, `_`, `-`
/// and `.`, not starting with `.` or `-`.
fn check_id(id: &str) -> anyhow::Result<()> {
    let valid = !id.is_empty()
        && id.len() <= 64
        && !id.starts_with(['.', '-'])
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if !valid {
        bail!("{id:?} is not a valid host id: use letters, digits, _, - and .");
    }
    Ok(())
}

/// Runs a host in this process: what `spawn` starts. Leaves the terminal's
/// session first, so that closing that terminal does not end the host.
pub(crate) fn run(config: HostConfig) -> anyhow::Result<()> {
    check_id(&config.id)?;
    volee_host::server::detach_from_terminal().context("cannot leave the terminal's session")?;
    config.dir.ensure()?;
    // Appended, so that a second host started by mistake with the same id
    // does not erase the log of the one that runs.
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(config.dir.log(&config.id))?;
    let _ = tracing_subscriber::fmt()
        .with_writer(Mutex::new(log))
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("VOLEE_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();
    if let Err(error) = volee_host::run(config) {
        tracing::error!(%error, "the host could not run");
        return Err(error.into());
    }
    Ok(())
}

/// Lists hosts, oldest first.
pub(crate) fn list(dir: &HostDir, json: bool) -> anyhow::Result<()> {
    let records = dir.list()?;
    if json {
        let rows: Vec<_> = records
            .iter()
            .map(|(record, live)| serde_json::json!({ "live": live, "record": record }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if records.is_empty() {
        println!("no hosts");
        return Ok(());
    }
    for (record, live) in records {
        let state = match (&record.exit, live) {
            (Some(exit), _) => match (&exit.code, &exit.signal) {
                (Some(code), _) => format!("exited {code}"),
                (None, Some(signal)) => format!("killed {signal}"),
                (None, None) => "ended".to_owned(),
            },
            (None, true) => "running".to_owned(),
            // No exit status and no answer: the machine or the host stopped.
            (None, false) => "interrupted".to_owned(),
        };
        println!(
            "{}  {:<11}  {:>3}x{:<3}  {}  ({})",
            record.id,
            state,
            record.size.cols,
            record.size.rows,
            record.name.unwrap_or_else(|| record.command.join(" ")),
            record.cwd.display()
        );
    }
    Ok(())
}

/// Prints a host's screen as plain text.
pub(crate) fn peek(dir: &HostDir, id: &str) -> anyhow::Result<()> {
    check_id(id)?;
    let mut watch = Watch::start(&dir.socket(id), &client_name("peek"))
        .with_context(|| format!("no host {id} answers"))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        watch.pump();
        if let Some(text) = watch.text() {
            println!("{}", text.trim_end());
            return Ok(());
        }
        if watch.closed() || Instant::now() > deadline {
            bail!("the host sent no screen");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Takes a host over in this terminal until Ctrl+Q.
pub(crate) fn attach(dir: &HostDir, id: &str) -> anyhow::Result<()> {
    check_id(id)?;
    // Checked first: connecting takes the input from whoever has it.
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        bail!("taking over needs a terminal on standard input and output");
    }
    let socket = dir.socket(id);
    if !socket.exists() {
        bail!("no host {id}; `volee host ls` lists them");
    }
    let name = volee_host::HostRecord::read(&dir.record(id))
        .ok()
        .and_then(|record| record.name)
        .unwrap_or_else(|| id.to_owned());
    eprintln!("volee: taking over {name}; Ctrl+Q comes back");
    let end = volee_host::attach(&socket, &client_name("attach"))
        .with_context(|| format!("cannot take over {id}"))?;
    match end {
        AttachEnd::Detached => eprintln!("volee: back from {name}; it keeps running"),
        AttachEnd::Exited(exit) => eprintln!(
            "volee: {name} ended ({})",
            exit.code.map_or_else(
                || exit.signal.unwrap_or_default(),
                |code| format!("code {code}")
            )
        ),
        AttachEnd::InputTaken => eprintln!("volee: {name} was taken over from another terminal"),
        AttachEnd::HostGone => eprintln!("volee: the host of {name} went away"),
    }
    Ok(())
}

/// Asks a host to end its program.
pub(crate) fn stop(dir: &HostDir, id: &str) -> anyhow::Result<()> {
    check_id(id)?;
    let connection = volee_host::Connection::open(
        &dir.socket(id),
        volee_host::Role::Watch,
        Size { cols: 0, rows: 0 },
        &client_name("stop"),
    )
    .with_context(|| format!("no host {id} answers"))?;
    connection.sender()?.stop()?;
    println!("asked {id} to stop");
    Ok(())
}

/// Shows running hosts (all of them, or `ids`) as a wall of live screens.
pub(crate) fn wall(dir: &HostDir, ids: &[String]) -> anyhow::Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        bail!("the wall needs a terminal on standard input and output");
    }
    volee_tui::wall::run(dir, &client_name("wall"), ids)?;
    Ok(())
}

fn client_name(what: &str) -> String {
    format!("volee {} {what}", volee_core::VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_ids_stay_in_their_directory() {
        for id in ["ag_01abc", "ag_01abc-x7q2", "probe.1"] {
            assert!(check_id(id).is_ok(), "{id}");
        }
        for id in ["", "../x", "a/b", ".hidden", "-x", "two words"] {
            assert!(check_id(id).is_err(), "{id}");
        }
    }
}
