//! `volee status-line --agent <id>`: the status line of the agents volee
//! launches (ADR 0035).
//!
//! Claude Code runs a session's status line command with what it knows of
//! the session as JSON on stdin, `context_window.used_percentage` among it,
//! and shows what the command prints under the prompt. This command tells
//! the daemon how much of its context the agent uses, then runs the status
//! line the person configured for Claude Code, with the same input, and
//! prints what it prints: the person sees their own status line.
//!
//! Rules, as for hooks: it never slows the agent down and never makes it
//! fail. The daemon gets a short timeout and is never started for it, and
//! the person's command a few seconds.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use volee_core::AgentId;
use volee_proto::blocking;
use volee_proto::methods::{ContextParams, ReportContext};
use volee_proto::paths::Paths;

/// How long the status line waits for the daemon.
const REPORT_TIMEOUT: Duration = Duration::from_millis(300);

/// How long the person's own status line may take.
const OWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Runs once, for one refresh of the status line.
pub(crate) fn run(agent: AgentId) -> ExitCode {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let status: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    if let Some(percent) = context_used(&status)
        && let Ok(paths) = Paths::from_env()
    {
        let params = ContextParams { agent, percent };
        // A daemon that does not answer misses one refresh: the next one
        // tells it again.
        let _ = blocking::call::<ReportContext>(&paths.socket, params, REPORT_TIMEOUT);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(own) = own_status_line(&status, home.as_deref())
        && let Some(output) = run_own(&own, &input, &project_dir(&status))
    {
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(&output);
        let _ = stdout.flush();
    }
    ExitCode::SUCCESS
}

/// The share of its context window the conversation uses, in percent, from
/// the status line's input.
fn context_used(status: &Value) -> Option<u8> {
    let percent = status
        .get("context_window")?
        .get("used_percentage")?
        .as_f64()?;
    if !percent.is_finite() {
        return None;
    }
    // In range by the clamp: the cast keeps the value.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(percent.round().clamp(0.0, 100.0) as u8)
}

/// The directory of the agent's project, where its settings are.
fn project_dir(status: &Value) -> PathBuf {
    status
        .get("workspace")
        .and_then(|workspace| workspace.get("project_dir"))
        .or_else(|| status.get("cwd"))
        .and_then(Value::as_str)
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// The status line command the person configured for Claude Code, as it
/// would apply without volee's: the project's local settings, else the
/// project's, else the person's own. volee's own command is never taken for
/// it.
fn own_status_line(status: &Value, home: Option<&Path>) -> Option<String> {
    let project = project_dir(status).join(".claude");
    let mut files = vec![
        project.join("settings.local.json"),
        project.join("settings.json"),
    ];
    if let Some(home) = home {
        files.push(home.join(".claude").join("settings.json"));
    }
    files.iter().find_map(|file| {
        let text = std::fs::read_to_string(file).ok()?;
        let settings: Value = serde_json::from_str(&text).ok()?;
        let line = settings.get("statusLine")?;
        if line.get("type").and_then(Value::as_str) != Some("command") {
            return None;
        }
        let command = line.get("command")?.as_str()?.trim();
        (!command.is_empty() && !command.contains(" status-line --agent "))
            .then(|| command.to_owned())
    })
}

/// Runs the person's status line through a shell, as Claude Code does, with
/// the same input, in the project's directory. Its output, or nothing when
/// it fails or takes too long.
fn run_own(command: &str, input: &str, dir: &Path) -> Option<Vec<u8>> {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(if dir.is_dir() { dir } else { Path::new(".") })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }
    // Read while it runs: a full pipe would block it.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        let _ = stdout.read_to_end(&mut output);
        output
    });
    let deadline = Instant::now() + OWN_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_context_is_read_rounded_and_kept_in_range() {
        let status = |value: Value| json!({ "context_window": { "used_percentage": value } });
        assert_eq!(context_used(&status(json!(14.6))), Some(15));
        assert_eq!(context_used(&status(json!(0))), Some(0));
        assert_eq!(context_used(&status(json!(130))), Some(100));
        assert_eq!(context_used(&status(json!(-3))), Some(0));
        assert_eq!(context_used(&status(json!("12"))), None);
        assert_eq!(context_used(&json!({})), None);
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn the_persons_status_line_is_the_one_claude_code_would_show() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let status = json!({ "workspace": { "project_dir": project } });
        assert_eq!(own_status_line(&status, Some(&home)), None);
        let line = |command: &str| {
            format!(r#"{{"statusLine": {{"type": "command", "command": "{command}"}}}}"#)
        };
        write(
            &home.join(".claude/settings.json"),
            &line("~/.claude/statusline.sh"),
        );
        assert_eq!(
            own_status_line(&status, Some(&home)).as_deref(),
            Some("~/.claude/statusline.sh")
        );
        // The project's settings come before the person's, the local ones first.
        write(
            &project.join(".claude/settings.json"),
            &line("project-line"),
        );
        assert_eq!(
            own_status_line(&status, Some(&home)).as_deref(),
            Some("project-line")
        );
        write(
            &project.join(".claude/settings.local.json"),
            &line("local-line"),
        );
        assert_eq!(
            own_status_line(&status, Some(&home)).as_deref(),
            Some("local-line")
        );
        // volee's own command is never run as the person's: it would loop.
        write(
            &project.join(".claude/settings.local.json"),
            &line("/bin/volee status-line --agent ag_x"),
        );
        assert_eq!(
            own_status_line(&status, Some(&home)).as_deref(),
            Some("project-line")
        );
    }

    #[test]
    fn the_persons_status_line_gets_the_same_input_and_its_output_is_printed() {
        let dir = tempfile::tempdir().unwrap();
        let output = run_own("cat; printf ' seen'", "{\"a\":1}", dir.path()).unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), "{\"a\":1} seen");
        // A command that fails prints what it printed; a missing one, nothing.
        assert_eq!(run_own("exit 3", "", dir.path()).unwrap(), b"");
    }
}
