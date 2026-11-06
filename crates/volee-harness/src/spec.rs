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
    /// Runs every tool without asking, including shell commands and network
    /// access.
    ///
    /// Dangerous: the agent can delete files, rewrite git history or leak
    /// secrets with nobody to stop it. volee passes it only when a launch asks
    /// for it explicitly, and it is meant for sandboxes.
    BypassPermissions,
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
            Self::BypassPermissions => "bypassPermissions",
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
/// The settings and MCP configuration files are written by the daemon from
/// [`Harness::agent_settings`](crate::Harness::agent_settings) and
/// [`Harness::mcp_config`](crate::Harness::mcp_config) before the launch;
/// they connect the agent's hooks and tools to volee.
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
    /// The per-agent settings file with volee's hooks. Absolute.
    pub settings_file: PathBuf,
    /// The per-agent MCP configuration with the volee server. Absolute. It is
    /// the only MCP configuration the agent loads, so a project's own MCP
    /// servers can never stall a background session on an approval prompt.
    pub mcp_config_file: PathBuf,
    /// The permission mode; `None` keeps the user's default.
    pub permission_mode: Option<PermissionMode>,
    /// Tool rules allowed without asking, such as `Bash(cargo test *)`.
    pub allowed_tools: Vec<String>,
    /// Tool rules that are always refused.
    pub disallowed_tools: Vec<String>,
    /// Text appended to the harness's system prompt.
    pub append_system_prompt: Option<String>,
}

impl SpawnSpec {
    /// Checks what can be checked before running anything: a prompt, absolute
    /// paths, and tool rules that cannot be mistaken for options.
    pub fn validate(&self) -> Result<(), RequestError> {
        if self.prompt.trim().is_empty() {
            return Err(RequestError::EmptyPrompt);
        }
        require_absolute("working directory", &self.cwd)?;
        require_absolute("settings file", &self.settings_file)?;
        require_absolute("MCP configuration file", &self.mcp_config_file)?;
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

fn require_absolute(what: &'static str, path: &Path) -> Result<(), RequestError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(RequestError::RelativePath {
            what,
            path: path.to_path_buf(),
        })
    }
}

/// A background session the harness started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spawned {
    /// The harness's short id for the session (for Claude Code, the first
    /// eight characters of the session UUID). The full id arrives later, with
    /// the session's first hook or listing.
    pub short_id: String,
    /// The session's display name, when the harness printed one.
    pub name: Option<String>,
    /// How the session relates to the agent: `Started` for a launch,
    /// `Resumed` when a resume continued the same session, `Forked` when it
    /// created a copy with a new id.
    pub origin: SessionOrigin,
}

impl Spawned {
    /// Whether `session` is the session this launch started.
    pub fn matches(&self, session: &SessionKey) -> bool {
        session.id.starts_with(&self.short_id)
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
            permission_mode: None,
            allowed_tools: vec![],
            disallowed_tools: vec![],
            append_system_prompt: None,
        }
    }

    #[test]
    fn a_complete_spec_is_valid() {
        assert_eq!(spec().validate(), Ok(()));
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
            (PermissionMode::BypassPermissions, "bypassPermissions"),
        ];
        for (mode, name) in modes {
            assert_eq!(mode.as_str(), name);
            assert_eq!(mode.to_string(), name);
            assert_eq!(serde_json::to_value(mode).unwrap(), name);
        }
    }

    #[test]
    fn a_launch_matches_its_full_session_id() {
        let spawned = Spawned {
            short_id: "4402670e".into(),
            name: None,
            origin: SessionOrigin::Started,
        };
        assert!(spawned.matches(&SessionKey::claude("4402670e-5b60-4cdc-8cf3-be456e780815")));
        assert!(!spawned.matches(&SessionKey::claude("b60aa3b9-73ea-4599-91a5-0725e0c83482")));
    }
}
