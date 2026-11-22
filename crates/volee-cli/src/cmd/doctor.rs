//! `volee doctor`: writes the hook settings that make Claude Code post its
//! events to the daemon, and checks that the daemon answers.
//!
//! The entries go to one settings file and nowhere else: the project's
//! `.claude/settings.local.json` by default (Claude Code keeps that file out
//! of version control), or `~/.claude/settings.json` only with `--user`.
//! Everything else in the file is left as it is, and so is any hook that is
//! not volee's; running the command again replaces volee's entries, so it is
//! safe to repeat, and `--remove` takes them out.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use volee_proto::paths::Paths;

use crate::connect::paths;
use crate::hook_token;

/// The hook events volee asks Claude Code to post.
const EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
    "Notification",
    "Stop",
    "StopFailure",
    "SubagentStart",
    "SubagentStop",
    "TaskCreated",
    "TaskCompleted",
    "PreCompact",
];

/// Seconds Claude Code waits for the daemon before it goes on without it.
/// The daemon answers at once; a hung one must not stall a session.
const HOOK_TIMEOUT: u64 = 5;

/// How long Claude Code waits for the daemon to decide a request it holds.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Holds {
    /// A permission request, in seconds: the hook's timeout.
    pub(crate) permission_timeout: u64,
    /// A question, in seconds: the daemon hands it back to the terminal
    /// after this; the hook's timeout adds a margin.
    pub(crate) question_delay: u64,
}

impl Default for Holds {
    fn default() -> Self {
        Self {
            permission_timeout: volee_config::DEFAULT_PERMISSION_TIMEOUT,
            question_delay: volee_rules::Settings::default()
                .held_question_timeout()
                .as_secs(),
        }
    }
}

/// Seconds added to a held question's delay in the hook's timeout, so that
/// the daemon always hands the question back before Claude Code gives up.
const QUESTION_MARGIN: u64 = 15;

/// What `volee doctor` was asked.
#[derive(Debug)]
pub(crate) struct Options {
    /// The project whose `.claude/settings.local.json` is written; the
    /// current directory when absent.
    pub(crate) project: Option<PathBuf>,
    /// Write `~/.claude/settings.json` instead.
    pub(crate) user: bool,
    /// Remove volee's hooks instead of writing them.
    pub(crate) remove: bool,
}

pub(crate) fn run(options: &Options) -> Result<()> {
    let paths = paths()?;
    let (port, mut holds) = settings(&paths);
    let target = settings_file(options)?;
    if options.remove {
        return remove(&target);
    }
    // How long a question is held is a rule (`held_question_timeout`): the
    // project's own, then the personal one. A user-wide file has no project.
    let project = if options.user {
        None
    } else {
        target.parent().and_then(Path::parent)
    };
    let rules = volee_rules::Rules::load(project, Some(&paths.config_dir));
    holds.question_delay = rules.settings().held_question_timeout().as_secs();
    report_rules(&rules);
    if let Some(legacy) = volee_config::legacy_file(&paths.config_dir) {
        println!("warning: {}", volee_config::legacy_warning(&legacy));
    }
    let token = hook_token::read_or_create(&paths.hook_token_file())
        .with_context(|| format!("cannot create {}", paths.hook_token_file().display()))?;
    write(&target, port, holds, &token, options.user)?;
    report_daemon(port);
    Ok(())
}

/// Says what is wrong with the rules files, one line each, with file and
/// line: a broken file makes every answer the safest one, and that must
/// never go unnoticed.
fn report_rules(rules: &volee_rules::Rules) {
    for error in rules.errors() {
        println!("rules error: {error}");
    }
    for warning in rules.warnings() {
        println!("rules warning: {warning}");
    }
    if rules.is_degraded() {
        println!("  until fixed, every change is treated as high risk with no automatic merge");
    }
}

pub(crate) fn settings_port(paths: &Paths) -> u16 {
    settings(paths).0
}

/// The hook port and the holds, from the configuration (defaults when it
/// cannot be read).
fn settings(paths: &Paths) -> (u16, Holds) {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    volee_config::Config::load(&paths.config_file(), &home).map_or(
        (volee_config::DEFAULT_HOOK_PORT, Holds::default()),
        |config| {
            (
                config.hooks.port,
                Holds {
                    permission_timeout: config.hooks.permission_timeout,
                    ..Holds::default()
                },
            )
        },
    )
}

fn settings_file(options: &Options) -> Result<PathBuf> {
    if options.user {
        let home = std::env::var_os("HOME").context("HOME is not set")?;
        return Ok(PathBuf::from(home).join(".claude/settings.json"));
    }
    let project = match &options.project {
        Some(project) => project.clone(),
        None => std::env::current_dir().context("cannot read the current directory")?,
    };
    if !project.is_dir() {
        bail!("{} is not a directory", project.display());
    }
    Ok(project.join(".claude/settings.local.json"))
}

fn write(target: &Path, port: u16, holds: Holds, token: &str, user: bool) -> Result<()> {
    let existing = read_settings(target)?;
    let merged = with_hooks(existing, port, holds, token)?;
    save(target, &merged)?;
    println!("Wrote volee's hooks to {}", target.display());
    println!("  events   {}", EVENTS.join(", "));
    println!("  url      http://127.0.0.1:{port}/hooks/<event>");
    println!("  note     SessionStart takes no http hook: it runs a silent curl to the same url");
    println!("  auth     Authorization: Bearer <token> (written in that file in clear)");
    println!("  timeout  {HOOK_TIMEOUT} s; a daemon that is down never blocks a session");
    println!(
        "  held     PermissionRequest for up to {} s and AskUserQuestion for {} s, until you decide in the browser; then the terminal asks",
        holds.permission_timeout, holds.question_delay
    );
    if user {
        println!(
            "  scope    every Claude Code session of this user. `volee doctor --user --remove` \
             takes them out."
        );
    } else {
        println!(
            "  scope    sessions started in this project. The file holds the token: Claude Code \
             keeps `settings.local.json` out of git, check that yours does."
        );
    }
    Ok(())
}

fn remove(target: &Path) -> Result<()> {
    if !target.exists() {
        println!("{} does not exist: nothing to remove.", target.display());
        return Ok(());
    }
    let existing = read_settings(target)?;
    let (cleaned, removed) = without_hooks(existing)?;
    if removed == 0 {
        println!("No volee hook in {}: nothing to remove.", target.display());
        return Ok(());
    }
    save(target, &cleaned)?;
    println!(
        "Removed {removed} volee hook entries from {}",
        target.display()
    );
    Ok(())
}

/// The settings in `target`, or an empty object when there are none yet. A
/// file that is not a JSON object is refused: it is never overwritten.
fn read_settings(target: &Path) -> Result<Value> {
    let text = match fs::read_to_string(target) {
        Ok(text) if text.trim().is_empty() => return Ok(json!({})),
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read {}", target.display()));
        }
    };
    let value: Value = serde_json::from_str(&text).with_context(|| {
        format!(
            "{} is not valid JSON; fix it by hand, volee does not overwrite it",
            target.display()
        )
    })?;
    if !value.is_object() {
        bail!("{} does not hold a JSON object", target.display());
    }
    Ok(value)
}

fn save(target: &Path, settings: &Value) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(settings)?;
    text.push('\n');
    // Written beside the target and renamed over it: a crash never leaves
    // half a settings file. The file holds the token: owner only.
    let temporary = target.with_extension("json.volee-tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)
        .with_context(|| format!("cannot write {}", temporary.display()))?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    fs::rename(&temporary, target)
        .with_context(|| format!("cannot replace {}", target.display()))?;
    Ok(())
}

/// Whether a hook entry (`{"matcher": ..., "hooks": [...]}`) is volee's: an
/// `http` hook, or the `curl` command used where `http` is not accepted, that
/// posts to a `/hooks/` path on the loopback interface.
fn is_volee(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| hooks.iter().any(is_volee_hook))
}

fn is_volee_hook(hook: &Value) -> bool {
    let target = match hook.get("type").and_then(Value::as_str) {
        Some("http") => hook.get("url"),
        Some("command") => hook.get("command"),
        _ => None,
    };
    target.and_then(Value::as_str).is_some_and(|text| {
        text.contains("http://127.0.0.1:")
            && (text.contains("/hooks/") || text.contains("/decide/"))
    })
}

/// The hook for `event`. Claude Code accepts `http` hooks on every event but
/// `SessionStart` (which takes `command` and `mcp_tool` only): there a `curl`
/// command posts the same request. It exits 0 whatever happens and prints
/// nothing, since a `SessionStart` hook's output goes to the model.
fn hook_for(event: &str, port: u16, holds: Holds, token: &str) -> Value {
    let url = format!("http://127.0.0.1:{port}/hooks/{event}");
    if event == "SessionStart" {
        return json!({
            "type": "command",
            "command": format!(
                "curl --silent --show-error --max-time {HOOK_TIMEOUT} --output /dev/null \
                 --request POST --header 'Content-Type: application/json' \
                 --header 'Authorization: Bearer {token}' --data-binary @- {url} \
                 2>/dev/null || true"
            ),
            "timeout": HOOK_TIMEOUT + 2,
        });
    }
    // A permission request is held for the person: Claude Code waits that
    // long, and the daemon holds for the same time.
    let timeout = if event == "PermissionRequest" {
        holds.permission_timeout
    } else {
        HOOK_TIMEOUT
    };
    json!({
        "type": "http",
        "url": url,
        "headers": { "Authorization": format!("Bearer {token}") },
        "timeout": timeout,
    })
}

/// The `PreToolUse` entry that holds `AskUserQuestion`, next to the plain
/// one that stores every `PreToolUse`: the held request posts to `/decide/`,
/// stores nothing, and answers after at most the configured delay.
fn question_entry(port: u16, holds: Holds, token: &str) -> Value {
    json!({
        "matcher": "AskUserQuestion",
        "hooks": [{
            "type": "http",
            "url": format!("http://127.0.0.1:{port}/decide/AskUserQuestion"),
            "headers": { "Authorization": format!("Bearer {token}") },
            "timeout": holds.question_delay + QUESTION_MARGIN,
        }],
    })
}

/// `settings` with volee's hooks for `port` and `token`, replacing any
/// earlier volee entries and keeping everything else.
fn with_hooks(settings: Value, port: u16, holds: Holds, token: &str) -> Result<Value> {
    let (mut settings, _) = without_hooks(settings)?;
    let root = settings
        .as_object_mut()
        .context("settings must be an object")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context("`hooks` in the settings must be an object")?;
    for event in EVENTS {
        let entry = json!({ "hooks": [hook_for(event, port, holds, token)] });
        let list = hooks
            .entry((*event).to_owned())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .with_context(|| format!("`hooks.{event}` in the settings must be a list"))?;
        list.push(entry);
        if *event == "PreToolUse" {
            list.push(question_entry(port, holds, token));
        }
    }
    Ok(settings)
}

/// `settings` without volee's hook entries, and how many were removed. Empty
/// lists and an empty `hooks` object left behind are dropped too.
fn without_hooks(mut settings: Value) -> Result<(Value, usize)> {
    let mut removed = 0;
    let root = settings
        .as_object_mut()
        .context("settings must be an object")?;
    if let Some(hooks) = root.get_mut("hooks") {
        let hooks = hooks
            .as_object_mut()
            .context("`hooks` in the settings must be an object")?;
        for (event, entries) in hooks.iter_mut() {
            let list = entries
                .as_array_mut()
                .with_context(|| format!("`hooks.{event}` in the settings must be a list"))?;
            let before = list.len();
            list.retain(|entry| !is_volee(entry));
            removed += before - list.len();
        }
        hooks.retain(|_, entries| entries.as_array().is_none_or(|list| !list.is_empty()));
        if hooks.is_empty() {
            root.remove("hooks");
        }
    }
    Ok((settings, removed))
}

/// Says whether the daemon's hook server answers.
fn report_daemon(port: u16) {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    match health(addr) {
        Ok(()) => println!("  daemon   hook server answers on 127.0.0.1:{port}"),
        Err(why) => println!(
            "  daemon   not answering on 127.0.0.1:{port} ({why}). Start it with `volee daemon \
             start`; until then Claude Code reports each hook as a non-blocking error and the \
             session goes on."
        ),
    }
}

pub(crate) fn health(addr: SocketAddr) -> std::result::Result<(), String> {
    let timeout = Duration::from_secs(1);
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .map_err(|e| e.to_string())?;
    let mut answer = String::new();
    stream
        .read_to_string(&mut answer)
        .map_err(|e| e.to_string())?;
    if answer.starts_with("HTTP/1.1 200") && answer.ends_with("ok") {
        Ok(())
    } else {
        Err("something else answers".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "t0k3n";

    fn urls(settings: &Value, event: &str) -> Vec<String> {
        settings["hooks"][event]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| entry["hooks"][0]["url"].as_str().map(str::to_owned))
            .collect()
    }

    #[test]
    fn every_event_posts_to_its_own_path_with_the_token_and_a_short_timeout() {
        let settings = with_hooks(json!({}), 4000, Holds::default(), TOKEN).unwrap();
        for event in EVENTS.iter().filter(|event| **event != "SessionStart") {
            let mut expected = vec![format!("http://127.0.0.1:4000/hooks/{event}")];
            if *event == "PreToolUse" {
                expected.push("http://127.0.0.1:4000/decide/AskUserQuestion".to_owned());
            }
            assert_eq!(urls(&settings, event), expected);
        }
        let hook = &settings["hooks"]["PermissionRequest"][0]["hooks"][0];
        assert_eq!(hook["type"], "http");
        assert_eq!(hook["headers"]["Authorization"], "Bearer t0k3n");
        assert_eq!(
            hook["timeout"], 3600,
            "a permission request is held: a long, explicit timeout"
        );
        let plain = &settings["hooks"]["PostToolUse"][0]["hooks"][0];
        assert_eq!(plain["timeout"], HOOK_TIMEOUT);
        assert!(
            hook.get("onFailure").is_none(),
            "never asks to block on failure"
        );
    }

    #[test]
    fn session_start_takes_no_http_hook_so_it_gets_a_silent_curl_that_never_fails() {
        let settings = with_hooks(json!({}), 4000, Holds::default(), TOKEN).unwrap();
        let hook = &settings["hooks"]["SessionStart"][0]["hooks"][0];
        assert_eq!(hook["type"], "command");
        let command = hook["command"].as_str().unwrap();
        assert!(
            command.contains("http://127.0.0.1:4000/hooks/SessionStart"),
            "{command}"
        );
        assert!(command.contains("Authorization: Bearer t0k3n"), "{command}");
        assert!(
            command.contains("--data-binary @-"),
            "the payload is on stdin: {command}"
        );
        assert!(command.ends_with("|| true"), "never a failure: {command}");
        assert!(
            command.contains("--output /dev/null"),
            "no output reaches the model"
        );
        let (without, removed) = without_hooks(settings).unwrap();
        assert_eq!(
            removed,
            EVENTS.len() + 1,
            "the curl entry is recognized as volee's"
        );
        assert_eq!(without, json!({}));
    }

    #[test]
    fn the_events_the_plan_requires_are_all_there() {
        for required in [
            "SessionStart",
            "SessionEnd",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PermissionRequest",
            "Notification",
            "Stop",
            "SubagentStart",
            "SubagentStop",
            "TaskCreated",
            "TaskCompleted",
        ] {
            assert!(EVENTS.contains(&required), "{required}");
        }
    }

    #[test]
    fn other_settings_and_other_hooks_are_kept() {
        let existing = json!({
            "model": "haiku",
            "permissions": { "allow": ["Bash(ls)"] },
            "hooks": {
                "PreToolUse": [
                    { "matcher": "Bash", "hooks": [{ "type": "command", "command": "check.sh" }] },
                    { "hooks": [{ "type": "http", "url": "http://localhost:9000/other" }] }
                ],
                "Elsewhere": [{ "hooks": [{ "type": "command", "command": "x" }] }]
            }
        });
        let settings = with_hooks(existing, 4000, Holds::default(), TOKEN).unwrap();
        assert_eq!(settings["model"], "haiku");
        assert_eq!(settings["permissions"]["allow"][0], "Bash(ls)");
        assert_eq!(
            settings["hooks"]["Elsewhere"][0]["hooks"][0]["command"],
            "x"
        );
        let pre = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 4, "the two earlier hooks and volee's two");
        assert_eq!(pre[0]["hooks"][0]["command"], "check.sh");
        assert_eq!(pre[1]["hooks"][0]["url"], "http://localhost:9000/other");
    }

    #[test]
    fn running_again_replaces_volees_entries_instead_of_adding_more() {
        let once = with_hooks(json!({}), 4000, Holds::default(), TOKEN).unwrap();
        let twice = with_hooks(once.clone(), 4000, Holds::default(), TOKEN).unwrap();
        assert_eq!(once, twice);
        let moved = with_hooks(twice, 5000, Holds::default(), "other").unwrap();
        assert_eq!(
            urls(&moved, "Stop"),
            ["http://127.0.0.1:5000/hooks/Stop"],
            "a new port replaces the old entry"
        );
        assert_eq!(
            moved["hooks"]["Stop"][0]["hooks"][0]["headers"]["Authorization"],
            "Bearer other"
        );
    }

    #[test]
    fn removing_takes_out_volees_hooks_and_leaves_the_rest() {
        let existing = json!({
            "model": "haiku",
            "hooks": { "PreToolUse": [
                { "hooks": [{ "type": "command", "command": "check.sh" }] }
            ] }
        });
        let with = with_hooks(existing.clone(), 4000, Holds::default(), TOKEN).unwrap();
        let (without, removed) = without_hooks(with).unwrap();
        assert_eq!(removed, EVENTS.len() + 1);
        assert_eq!(without, existing);
        let (nothing, removed) =
            without_hooks(with_hooks(json!({}), 4000, Holds::default(), TOKEN).unwrap()).unwrap();
        assert_eq!(removed, EVENTS.len() + 1);
        assert_eq!(nothing, json!({}), "an empty hooks object is dropped too");
    }

    #[test]
    fn a_settings_file_that_is_not_json_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".claude/settings.local.json");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "{ not json").unwrap();
        let error = write(&file, 4000, Holds::default(), TOKEN, false).unwrap_err();
        assert!(error.to_string().contains("not valid JSON"), "{error:#}");
        assert_eq!(fs::read_to_string(&file).unwrap(), "{ not json");
        fs::write(&file, "[1]").unwrap();
        assert!(write(&file, 4000, Holds::default(), TOKEN, false).is_err());
    }

    #[test]
    fn the_file_is_written_private_and_removing_restores_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".claude/settings.local.json");
        write(&file, 4000, Holds::default(), TOKEN, false).unwrap();
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let written: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(urls(&written, "Stop").len(), 1);
        remove(&file).unwrap();
        let after: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(after, json!({}));
        assert!(
            !file.with_extension("json.volee-tmp").exists(),
            "no temporary file is left"
        );
    }

    #[test]
    fn the_health_check_tells_a_hook_server_from_silence() {
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = free.local_addr().unwrap();
        drop(free);
        assert!(health(addr).is_err(), "nothing listens there");
    }

    #[test]
    fn the_holds_come_from_the_configuration() {
        let holds = Holds {
            permission_timeout: 30,
            question_delay: 9,
        };
        let settings = with_hooks(json!({}), 4000, holds, TOKEN).unwrap();
        assert_eq!(
            settings["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"],
            30
        );
        let question = &settings["hooks"]["PreToolUse"][1];
        assert_eq!(question["matcher"], "AskUserQuestion");
        assert_eq!(
            question["hooks"][0]["url"],
            "http://127.0.0.1:4000/decide/AskUserQuestion"
        );
        assert_eq!(question["hooks"][0]["timeout"], 9 + QUESTION_MARGIN);
        let plain = &settings["hooks"]["PreToolUse"][0];
        assert!(
            plain.get("matcher").is_none(),
            "every PreToolUse is still stored"
        );
        assert_eq!(plain["hooks"][0]["timeout"], HOOK_TIMEOUT);
    }
}
