//! `volee daemon install | uninstall`: start the daemon at login, so that
//! agents a reboot interrupted come back without opening volee (ADR 0024).
//!
//! volee never installs this on its own: the person runs the command. macOS
//! gets a launchd agent, Linux a systemd user unit.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// The launchd label and the systemd unit name.
const LABEL: &str = "org.volee.daemon";

/// What starts the daemon at login on this system.
#[derive(Debug, PartialEq, Eq)]
struct LoginItem {
    /// Where its file goes.
    path: PathBuf,
    /// The file.
    contents: String,
    /// The commands that load it now, and those that unload it.
    load: Vec<Vec<String>>,
    unload: Vec<Vec<String>>,
}

/// Writes the login item and loads it; with `print`, only prints the file
/// and where it would go.
pub(crate) fn install(print: bool) -> Result<()> {
    let item = login_item()?;
    if print {
        println!("# {}", item.path.display());
        print!("{}", item.contents);
        return Ok(());
    }
    if let Some(dir) = item.path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    std::fs::write(&item.path, &item.contents)
        .with_context(|| format!("cannot write {}", item.path.display()))?;
    for command in &item.load {
        run(command)?;
    }
    println!(
        "The volee daemon now starts at login ({}). `volee daemon uninstall` undoes it.",
        item.path.display()
    );
    Ok(())
}

/// Unloads the login item and removes its file.
pub(crate) fn uninstall() -> Result<()> {
    let item = login_item()?;
    if !item.path.exists() {
        println!("The volee daemon does not start at login.");
        return Ok(());
    }
    for command in &item.unload {
        // Already unloaded is fine.
        let _ = run(command);
    }
    std::fs::remove_file(&item.path)
        .with_context(|| format!("cannot remove {}", item.path.display()))?;
    println!("The volee daemon no longer starts at login.");
    Ok(())
}

fn run(command: &[String]) -> Result<()> {
    let (program, args) = command.split_first().context("empty command")?;
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("cannot run {program}"))?;
    if !status.success() {
        bail!("`{}` failed ({status})", command.join(" "));
    }
    Ok(())
}

fn login_item() -> Result<LoginItem> {
    let volee = std::env::current_exe().context("cannot find the volee program")?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    let paths = volee_proto::paths::Paths::from_env()?;
    // The daemon starts agents, which need the PATH of the person's shell
    // (claude, git, gh), not the bare one of launchd or systemd.
    let path = std::env::var("PATH").unwrap_or_default();
    let env: Vec<(String, String)> = std::iter::once(("PATH".to_owned(), path))
        .chain(
            [
                "VOLEE_STATE_DIR",
                "VOLEE_CONFIG_DIR",
                "VOLEE_DATA_DIR",
                "VOLEE_SOCKET",
            ]
            .iter()
            .filter_map(|name| Some(((*name).to_owned(), std::env::var(name).ok()?))),
        )
        .collect();
    if cfg!(target_os = "macos") {
        let uid = user_id();
        let path = home.join(format!("Library/LaunchAgents/{LABEL}.plist"));
        let log = paths.state_dir.join("daemon.log");
        Ok(LoginItem {
            contents: launchd_plist(&volee, &env, &log),
            load: vec![vec![
                "launchctl".into(),
                "bootstrap".into(),
                format!("gui/{uid}"),
                path.display().to_string(),
            ]],
            unload: vec![vec![
                "launchctl".into(),
                "bootout".into(),
                format!("gui/{uid}"),
                path.display().to_string(),
            ]],
            path,
        })
    } else {
        let config =
            std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
        let path = config.join("systemd/user/volee.service");
        let systemctl = |args: &[&str]| -> Vec<String> {
            std::iter::once("systemctl")
                .chain(std::iter::once("--user"))
                .chain(args.iter().copied())
                .map(str::to_owned)
                .collect()
        };
        Ok(LoginItem {
            contents: systemd_unit(&volee, &env),
            load: vec![
                systemctl(&["daemon-reload"]),
                systemctl(&["enable", "--now", "volee.service"]),
            ],
            unload: vec![systemctl(&["disable", "--now", "volee.service"])],
            path,
        })
    }
}

fn user_id() -> u32 {
    // The user's id, from the ownership of their home directory: no unsafe
    // call needed.
    std::env::var_os("HOME")
        .and_then(|home| std::fs::metadata(home).ok())
        .map_or(501, |meta| std::os::unix::fs::MetadataExt::uid(&meta))
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A launchd agent that runs `volee daemon run` once at login.
fn launchd_plist(volee: &Path, env: &[(String, String)], log: &Path) -> String {
    let mut variables = String::new();
    for (name, value) in env {
        // Writing to a String cannot fail.
        let _ = write!(
            variables,
            "        <key>{}</key>\n        <string>{}</string>\n",
            xml_escape(name),
            xml_escape(value)
        );
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{volee}</string>
        <string>daemon</string>
        <string>run</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
{variables}    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#,
        volee = xml_escape(&volee.display().to_string()),
        log = xml_escape(&log.display().to_string()),
    )
}

/// A systemd user unit that runs `volee daemon run` at login.
fn systemd_unit(volee: &Path, env: &[(String, String)]) -> String {
    let mut environment = String::new();
    for (name, value) in env {
        // Writing to a String cannot fail.
        let _ = writeln!(
            environment,
            "Environment=\"{name}={}\"",
            value.replace('"', "\\\"")
        );
    }
    format!(
        "[Unit]\nDescription=volee daemon: see and coordinate the coding agents of this machine\n\n\
         [Service]\nExecStart={volee} daemon run\n{environment}\n[Install]\nWantedBy=default.target\n",
        volee = volee.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Vec<(String, String)> {
        vec![
            (
                "PATH".to_owned(),
                "/Users/me/.local/bin:/usr/bin".to_owned(),
            ),
            ("VOLEE_STATE_DIR".to_owned(), "/tmp/a&b".to_owned()),
        ]
    }

    #[test]
    fn the_launchd_agent_runs_the_daemon_once_with_the_shell_path() {
        let plist = launchd_plist(
            Path::new("/Users/me/.local/bin/volee"),
            &env(),
            Path::new("/state/daemon.log"),
        );
        assert!(plist.contains("<string>org.volee.daemon</string>"));
        assert!(plist.contains("<string>/Users/me/.local/bin/volee</string>\n        <string>daemon</string>\n        <string>run</string>"));
        assert!(
            plist.contains(
                "<key>PATH</key>\n        <string>/Users/me/.local/bin:/usr/bin</string>"
            )
        );
        assert!(plist.contains("<string>/tmp/a&amp;b</string>"), "escaped");
        assert!(plist.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert!(
            !plist.contains("KeepAlive"),
            "a daemon that stops stays stopped"
        );
    }

    #[test]
    fn the_systemd_unit_runs_the_daemon_at_login() {
        let unit = systemd_unit(Path::new("/home/me/.local/bin/volee"), &env());
        assert!(unit.contains("ExecStart=/home/me/.local/bin/volee daemon run\n"));
        assert!(unit.contains("Environment=\"PATH=/Users/me/.local/bin:/usr/bin\"\n"));
        assert!(unit.contains("WantedBy=default.target"));
    }
}
