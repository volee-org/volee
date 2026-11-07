//! What it takes to launch an agent, and what a launch returns.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use volee_core::harness::{SessionKey, SessionOrigin};

use crate::error::RequestError;

/// How the harness asks before running tools.
///
/// The names match Claude Code's `--permission-mode` values. A background
/// agent that needs permission waits until a person attaches and answers, so
/// volee shows it as needing input.
///
/// There is deliberately no mode that turns every check off
/// (`bypassPermissions`): volee never launches an agent whose tools run with
/// nobody able to stop them, so no configuration, request or message can ask
/// for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    /// Asks before every tool that needs permission. The safest mode.
    Manual,
    /// Accepts file edits without asking; asks for the rest.
    AcceptEdits,
    /// Plans without changing anything.
    Plan,
    /// Lets the harness decide which actions are safe to run without asking.
    Auto,
    /// Never asks: anything that would ask is refused.
    DontAsk,
}

impl PermissionMode {
    /// The value of Claude Code's `--permission-mode` flag.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::AcceptEdits => "acceptEdits",
            Self::Plan => "plan",
            Self::Auto => "auto",
            Self::DontAsk => "dontAsk",
        }
    }
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Everything needed to launch (or resume) a background agent.
///
/// The daemon builds it from volee's configuration and its own decisions.
/// An agent's request (the MCP tool `spawn_worker`) supplies only the task,
/// the repository and a name: never a permission mode, tool rules or files.
/// What an agent may do without asking is decided by the person who
/// configured volee
/// ([ADR 0011](https://volee-org.github.io/volee/adr/0011-safety-and-delivery-evidence.html)).
///
/// The settings and MCP configuration files are written by the daemon from
/// [`Harness::agent_settings`](crate::Harness::agent_settings) and
/// [`Harness::mcp_config`](crate::Harness::mcp_config) before the launch:
/// they connect the agent's hooks to volee and let it use volee's MCP tools
/// without a permission prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnSpec {
    /// The agent's working directory, usually its worktree. Absolute. A
    /// resumed session must use the directory it started in: the harness
    /// finds conversations by project directory.
    pub cwd: PathBuf,
    /// The display name of the session. Empty lets the harness pick one.
    pub name: String,
    /// The model, when not the harness's default.
    pub model: Option<String>,
    /// The first prompt (for a resume: the message that continues the
    /// conversation).
    pub prompt: String,
    /// The per-agent settings file: volee's hooks, and the rule that allows
    /// volee's MCP tools. Absolute and valid UTF-8.
    pub settings_file: PathBuf,
    /// The per-agent MCP configuration with the volee server. Absolute and
    /// valid UTF-8. It is the only MCP configuration the agent loads, so a
    /// project's own MCP servers can never stall a background session on an
    /// approval prompt.
    pub mcp_config_file: PathBuf,
    /// The permission mode, from volee's configuration, `manual` when it says
    /// nothing. Always passed: the person's own default may skip every check,
    /// and an agent that nobody watches must not inherit it.
    pub permission_mode: PermissionMode,
    /// Tool rules allowed without asking, such as `Bash(cargo test *)`. From
    /// volee's configuration only: a rule here runs with nobody asked.
    pub allowed_tools: Vec<String>,
    /// Tool rules that are always refused, from volee's configuration.
    pub disallowed_tools: Vec<String>,
    /// Text appended to the harness's system prompt.
    pub append_system_prompt: Option<String>,
    /// A worktree to run in, by name, under `<cwd>/.claude/worktrees/`
    /// (`--worktree=<name>`). Claude Code reuses the worktree when it exists
    /// (volee creates it first, on the agent's branch) and creates it
    /// otherwise. With a worktree, `cwd` is the repository's main checkout:
    /// Claude Code checks its trust there, and never trusts a fresh worktree
    /// started directly.
    pub worktree: Option<String>,
}

impl SpawnSpec {
    /// Checks what can be checked before running anything: a prompt,
    /// absolute paths, UTF-8 file paths (they travel inside command-line
    /// options), and tool rules that cannot be mistaken for options.
    pub fn validate(&self) -> Result<(), RequestError> {
        if self.prompt.trim().is_empty() {
            return Err(RequestError::EmptyPrompt);
        }
        check_path("working directory", &self.cwd, false)?;
        check_path("settings file", &self.settings_file, true)?;
        check_path("MCP configuration file", &self.mcp_config_file, true)?;
        if let Some(name) = &self.worktree
            && !is_worktree_name(name)
        {
            return Err(RequestError::InvalidWorktreeName { name: name.clone() });
        }
        if let Some(rule) = self
            .allowed_tools
            .iter()
            .chain(&self.disallowed_tools)
            .find(|rule| rule.trim().is_empty() || rule.starts_with('-'))
        {
            return Err(RequestError::InvalidToolRule { rule: rule.clone() });
        }
        Ok(())
    }
}

/// A worktree name is one path component made of letters, digits, `.`, `_`
/// and `-`, that cannot be read as an option or climb out of the directory.
fn is_worktree_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '.'])
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn check_path(what: &'static str, path: &Path, utf8: bool) -> Result<(), RequestError> {
    if !path.is_absolute() {
        return Err(RequestError::RelativePath {
            what,
            path: path.to_path_buf(),
        });
    }
    if utf8 && path.to_str().is_none() {
        return Err(RequestError::NonUtf8Path {
            what,
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// One question to the harness's model, answered with no tools, no MCP
/// server, no hooks and no saved session ([`crate::Harness::complete`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    /// Where the harness runs: a directory of volee's own, so that nothing of
    /// the person's projects is read and discovery never takes the run for an
    /// agent. Absolute.
    pub cwd: PathBuf,
    /// The model, or the harness's default.
    pub model: Option<String>,
    /// What the model is asked to be.
    pub system_prompt: String,
    /// The question, with the material to work on.
    pub prompt: String,
}

/// A background session the harness started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spawned {
    /// The harness's handle for the new session, as it printed it (for
    /// Claude Code, the id `claude --bg` prints and `claude agents` lists).
    /// The full session id arrives later, with the session's first hook or
    /// listing.
    pub handle: String,
    /// The session's display name, when the harness printed one.
    pub name: Option<String>,
    /// How the session relates to the agent: `Started` for a launch,
    /// `Resumed` when a resume continued the same session, `Forked` when it
    /// created a copy with a new id.
    pub origin: SessionOrigin,
}

impl Spawned {
    /// Whether `session` is the session this launch started. Claude Code's
    /// handle is the start of the session UUID, so this also links the
    /// session id of the first hook to the launch.
    pub fn matches(&self, session: &SessionKey) -> bool {
        session.id.starts_with(&self.handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SpawnSpec {
        SpawnSpec {
            cwd: "/work/alpha".into(),
            name: "alpha worker".into(),
            model: None,
            prompt: "Fix the flaky test".into(),
            settings_file: "/state/agents/ag_1/settings.json".into(),
            mcp_config_file: "/state/agents/ag_1/mcp.json".into(),
            permission_mode: PermissionMode::Manual,
            allowed_tools: vec![],
            disallowed_tools: vec![],
            append_system_prompt: None,
            worktree: None,
        }
    }

    #[test]
    fn a_complete_spec_is_valid() {
        assert_eq!(spec().validate(), Ok(()));
    }

    #[test]
    fn a_worktree_is_one_plain_path_component() {
        for name in ["feat-invoices", "v1.2", "fix_typo"] {
            let with = SpawnSpec {
                worktree: Some(name.into()),
                ..spec()
            };
            assert_eq!(with.validate(), Ok(()), "{name}");
        }
        for name in ["", "a/b", "../up", ".hidden", "-x", "two words"] {
            let with = SpawnSpec {
                worktree: Some(name.into()),
                ..spec()
            };
            assert_eq!(
                with.validate(),
                Err(RequestError::InvalidWorktreeName { name: name.into() }),
                "{name}"
            );
        }
    }

    #[test]
    fn validation_rejects_what_would_break_the_command_line() {
        let empty = SpawnSpec {
            prompt: " \n".into(),
            ..spec()
        };
        assert_eq!(empty.validate(), Err(RequestError::EmptyPrompt));

        let relative = SpawnSpec {
            settings_file: "settings.json".into(),
            ..spec()
        };
        assert!(matches!(
            relative.validate(),
            Err(RequestError::RelativePath {
                what: "settings file",
                ..
            })
        ));

        let option = SpawnSpec {
            disallowed_tools: vec!["--dangerously-skip-permissions".into()],
            ..spec()
        };
        assert!(matches!(
            option.validate(),
            Err(RequestError::InvalidToolRule { .. })
        ));
    }

    #[test]
    fn permission_modes_use_claude_names() {
        let modes = [
            (PermissionMode::Manual, "manual"),
            (PermissionMode::AcceptEdits, "acceptEdits"),
            (PermissionMode::Plan, "plan"),
            (PermissionMode::Auto, "auto"),
            (PermissionMode::DontAsk, "dontAsk"),
        ];
        for (mode, name) in modes {
            assert_eq!(mode.as_str(), name);
            assert_eq!(mode.to_string(), name);
            assert_eq!(serde_json::to_value(mode).unwrap(), name);
            assert_eq!(
                serde_json::from_value::<PermissionMode>(name.into()).unwrap(),
                mode
            );
        }
    }

    #[test]
    fn no_input_can_ask_to_bypass_permissions() {
        for name in ["bypassPermissions", "BypassPermissions", "dangerously"] {
            assert!(serde_json::from_value::<PermissionMode>(name.into()).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn file_paths_must_be_utf8() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let odd = SpawnSpec {
            mcp_config_file: PathBuf::from(OsStr::from_bytes(b"/state/mcp-\xff.json")),
            ..spec()
        };
        assert!(matches!(
            odd.validate(),
            Err(RequestError::NonUtf8Path {
                what: "MCP configuration file",
                ..
            })
        ));
        let odd_cwd = SpawnSpec {
            cwd: PathBuf::from(OsStr::from_bytes(b"/work/\xff")),
            ..spec()
        };
        assert_eq!(
            odd_cwd.validate(),
            Ok(()),
            "the directory is not an argument"
        );
    }

    #[test]
    fn a_launch_matches_its_full_session_id() {
        let spawned = Spawned {
            handle: "7d3c1f0a".into(),
            name: None,
            origin: SessionOrigin::Started,
        };
        assert!(spawned.matches(&SessionKey::claude("7d3c1f0a-2b4e-4c6d-9e8f-0a1b2c3d4e5f")));
        assert!(!spawned.matches(&SessionKey::claude("c4e5a6b7-8901-4cde-8f01-23456789abcd")));
    }
}
