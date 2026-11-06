//! The per-agent settings and MCP configuration that connect an agent to
//! volee.

use std::borrow::Cow;
use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};
use volee_core::ids::AgentId;

use super::HookEventName;
use crate::error::RequestError;

/// How long a regular volee hook may run before Claude Code cancels it, in
/// seconds. `volee hook` answers in milliseconds (it spools when the daemon
/// is unreachable), so this only bounds how long a broken installation can
/// hold an agent back; Claude Code's own default is ten minutes.
pub const HOOK_TIMEOUT_SECS: u64 = 10;

/// How much longer than its own wait Claude Code lets the message waiter
/// run, in seconds, so that the waiter always ends on its own (`volee hook
/// wait` gives the daemon five extra seconds to answer).
pub const REWAKE_GRACE_SECS: u64 = 30;

/// The settings that connect an agent's hooks to volee, for `claude
/// --settings`. Claude Code adds them to the user's own hooks; the user's
/// settings file is never edited.
///
/// Every event in [`HookEventName::OBSERVED`] runs `<volee_bin> hook
/// <event-kebab> --agent <agent>` with a [`HOOK_TIMEOUT_SECS`] timeout
/// (except `SessionEnd`, which keeps Claude Code's short exit budget). `Stop`
/// has a second handler, `<volee_bin> hook wait --agent <agent> --timeout
/// <seconds>`, declared with `asyncRewake`: it waits up to `rewake_timeout`
/// (rounded up to whole seconds) for messages while the agent is idle, and
/// wakes it by exiting with [`REWAKE_EXIT_CODE`](super::REWAKE_EXIT_CODE).
/// Claude Code's timeout for it is [`REWAKE_GRACE_SECS`] longer.
///
/// `volee_bin` must be absolute (hooks run in the agent's directory) and
/// valid UTF-8; it is quoted for the shell that runs the hook.
pub fn agent_settings(
    volee_bin: &Path,
    agent: AgentId,
    rewake_timeout: Duration,
) -> Result<Value, RequestError> {
    let bin = shell_quote(absolute_utf8(volee_bin)?).into_owned();
    let agent = agent.to_string();
    let agent = shell_quote(&agent);
    let command = |subcommand: &str| format!("{bin} hook {subcommand} --agent {agent}");
    let wait = whole_seconds(rewake_timeout);

    let mut hooks = Map::new();
    for event in HookEventName::OBSERVED {
        let mut handler = json!({
            "type": "command",
            "command": command(&event.kebab()),
        });
        // SessionEnd hooks share a 1.5 second budget, which Claude Code
        // raises to the longest per-hook timeout: a timeout here would slow
        // down every exit.
        if event != HookEventName::SessionEnd {
            handler["timeout"] = json!(HOOK_TIMEOUT_SECS);
        }
        let mut handlers = vec![handler];
        if event == HookEventName::Stop {
            handlers.push(json!({
                "type": "command",
                "command": format!("{} --timeout {wait}", command("wait")),
                "asyncRewake": true,
                "timeout": wait.saturating_add(REWAKE_GRACE_SECS),
            }));
        }
        hooks.insert(event.as_str().to_owned(), json!([{ "hooks": handlers }]));
    }
    Ok(json!({ "hooks": hooks }))
}

/// The MCP configuration that gives an agent volee's tools, for `claude
/// --mcp-config`: one stdio server, `<volee_bin> mcp --agent <agent>`.
///
/// `volee_bin` must be absolute and valid UTF-8. No shell is involved.
pub fn mcp_config(volee_bin: &Path, agent: AgentId) -> Result<Value, RequestError> {
    let bin = absolute_utf8(volee_bin)?;
    Ok(json!({
        "mcpServers": {
            "volee": {
                "type": "stdio",
                "command": bin,
                "args": ["mcp", "--agent", agent.to_string()],
            }
        }
    }))
}

/// The volee binary's path as text, if it is absolute and valid UTF-8.
fn absolute_utf8(path: &Path) -> Result<&str, RequestError> {
    const WHAT: &str = "volee binary";
    if !path.is_absolute() {
        return Err(RequestError::RelativePath {
            what: WHAT,
            path: path.to_path_buf(),
        });
    }
    path.to_str().ok_or_else(|| RequestError::NonUtf8Path {
        what: WHAT,
        path: path.to_path_buf(),
    })
}

/// A duration in whole seconds, rounded up, at least one.
fn whole_seconds(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() > 0))
        .max(1)
}

/// `word` as one word for a POSIX shell: unchanged when it only has
/// characters no shell treats specially, single-quoted otherwise.
fn shell_quote(word: &str) -> Cow<'_, str> {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-./:,+@%".contains(&byte));
    if plain {
        Cow::Borrowed(word)
    } else {
        Cow::Owned(format!("'{}'", word.replace('\'', r"'\''")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> AgentId {
        "ag_01jc5x8m3kq7v2c4b9e0f1g2h3".parse().unwrap()
    }

    #[test]
    fn agent_settings() {
        let settings = super::agent_settings(
            Path::new("/opt/volee/bin/volee"),
            agent(),
            Duration::from_secs(3_600),
        )
        .unwrap();
        insta::with_settings!({ sort_maps => true }, {
            insta::assert_json_snapshot!(settings);
        });
    }

    #[test]
    fn every_observed_event_has_a_hook() {
        let settings =
            super::agent_settings(Path::new("/bin/volee"), agent(), Duration::from_secs(60))
                .unwrap();
        let hooks = settings["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), HookEventName::OBSERVED.len());
        for event in HookEventName::OBSERVED {
            let command = hooks[event.as_str()][0]["hooks"][0]["command"]
                .as_str()
                .unwrap();
            assert_eq!(
                command,
                format!("/bin/volee hook {} --agent {}", event.kebab(), agent())
            );
        }
        assert_eq!(hooks["Stop"][0]["hooks"].as_array().unwrap().len(), 2);
        let waiter = &hooks["Stop"][0]["hooks"][1];
        assert_eq!(waiter["asyncRewake"], true);
        assert_eq!(waiter["timeout"], 60 + REWAKE_GRACE_SECS);
        assert!(
            waiter["command"]
                .as_str()
                .unwrap()
                .ends_with(" hook wait --agent ag_01jc5x8m3kq7v2c4b9e0f1g2h3 --timeout 60")
        );
    }

    #[test]
    fn the_binary_path_is_quoted_for_the_shell() {
        let settings = super::agent_settings(
            Path::new("/Users/me/My Tools/it's/volee"),
            agent(),
            Duration::from_secs(60),
        )
        .unwrap();
        let command = settings["hooks"]["Stop"][0]["hooks"][1]["command"]
            .as_str()
            .unwrap();
        assert_eq!(
            command,
            format!(
                "'/Users/me/My Tools/it'\\''s/volee' hook wait --agent {} --timeout 60",
                agent()
            )
        );
    }

    #[test]
    fn the_rewake_timeout_is_whole_seconds() {
        assert_eq!(whole_seconds(Duration::from_secs(3_600)), 3_600);
        assert_eq!(whole_seconds(Duration::from_millis(1_500)), 2);
        assert_eq!(whole_seconds(Duration::ZERO), 1);
        assert_eq!(whole_seconds(Duration::MAX), u64::MAX);
    }

    #[test]
    fn the_binary_must_be_absolute_utf8() {
        assert!(matches!(
            super::agent_settings(Path::new("volee"), agent(), Duration::from_secs(1)),
            Err(RequestError::RelativePath { .. })
        ));
        assert!(matches!(
            mcp_config(Path::new("./volee"), agent()),
            Err(RequestError::RelativePath { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_binary_is_refused() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let path = Path::new(OsStr::from_bytes(b"/opt/vol\xffee"));
        assert!(matches!(
            mcp_config(path, agent()),
            Err(RequestError::NonUtf8Path { .. })
        ));
    }

    #[test]
    fn mcp_config_declares_one_stdio_server() {
        let config = mcp_config(Path::new("/opt/volee/bin/volee"), agent()).unwrap();
        assert_eq!(
            config,
            json!({
                "mcpServers": {
                    "volee": {
                        "type": "stdio",
                        "command": "/opt/volee/bin/volee",
                        "args": ["mcp", "--agent", "ag_01jc5x8m3kq7v2c4b9e0f1g2h3"],
                    }
                }
            })
        );
    }

    #[test]
    fn plain_words_stay_unquoted() {
        assert_eq!(shell_quote("/usr/local/bin/volee"), "/usr/local/bin/volee");
        assert_eq!(shell_quote("ag_01jc5x"), "ag_01jc5x");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("x=1"), "'x=1'");
        assert_eq!(shell_quote("~/bin"), "'~/bin'");
        assert_eq!(shell_quote("$(rm -rf /)"), "'$(rm -rf /)'");
    }

    #[cfg(unix)]
    #[test]
    fn quoted_words_survive_a_real_shell() {
        use crate::runner::{CommandRunner, Invocation, SystemRunner};

        for word in [
            "/My Tools/it's/volee",
            "$HOME `id` \"q\" \\ *",
            "tab\there",
            "plain",
        ] {
            let script = format!("printf %s {}", shell_quote(word));
            let invocation =
                Invocation::new("sh", Duration::from_secs(10)).args(["-c", script.as_str()]);
            let output = SystemRunner.run(&invocation).unwrap();
            assert_eq!(String::from_utf8(output.stdout).unwrap(), word);
        }
    }
}
