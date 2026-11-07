//! The configuration file of volee, `~/.config/volee/config.toml`.
//!
//! Every setting is optional, a missing file means the defaults, and unknown
//! keys are errors: a typo becomes a clear message with a line number instead
//! of a setting that silently does nothing.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// A commented example configuration, also written by `volee config init`.
pub const EXAMPLE: &str = include_str!("example.toml");

/// Why the configuration could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file exists but cannot be read.
    #[error("cannot read {path}: {source}")]
    Read {
        /// The file.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The file is not valid TOML or has unknown or ill-typed keys.
    #[error("{path}: {message}")]
    Parse {
        /// The file.
        path: PathBuf,
        /// The parser's message, with the line and column.
        message: String,
    },
    /// A value is well-formed but unusable.
    #[error("{path}: {message}")]
    Invalid {
        /// The file.
        path: PathBuf,
        /// What is wrong and how to fix it.
        message: String,
    },
}

/// The whole configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Defaults for the agents volee launches.
    pub agents: AgentDefaults,
    /// Repositories volee may launch agents in, by name.
    pub repos: BTreeMap<String, Repo>,
    /// Messaging settings.
    pub messages: Messages,
    /// Interface settings.
    pub tui: Tui,
}

/// Messaging settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Messages {
    /// Whether volee may summarize threads with a model. Off unless the
    /// person turns it on: each summary runs the harness with their account.
    pub summaries: bool,
    /// The model for summaries; `haiku` when unset.
    pub summary_model: Option<String>,
}

impl Messages {
    /// The model summaries use.
    pub fn summary_model(&self) -> &str {
        self.summary_model.as_deref().unwrap_or("haiku")
    }
}

/// Defaults for the agents volee launches.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentDefaults {
    /// The model, unless a repository sets one.
    pub model: Option<String>,
    /// The permission mode passed to the harness.
    pub permission_mode: Option<PermissionMode>,
    /// How long an idle agent's hook waits for messages, in seconds.
    pub wake_timeout: u64,
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            model: None,
            permission_mode: None,
            wake_timeout: 3_600,
        }
    }
}

/// Claude Code permission modes volee may pass on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    /// Edits are accepted, commands still ask.
    AcceptEdits,
    /// A classifier decides.
    Auto,
    /// Every action asks.
    Manual,
    /// Actions that would ask are refused.
    DontAsk,
    /// The agent plans without acting.
    Plan,
}

impl PermissionMode {
    /// The value the harness expects on its command line.
    pub const fn as_flag(self) -> &'static str {
        match self {
            Self::AcceptEdits => "acceptEdits",
            Self::Auto => "auto",
            Self::Manual => "manual",
            Self::DontAsk => "dontAsk",
            Self::Plan => "plan",
        }
    }
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_flag())
    }
}

/// A repository volee may launch agents in.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repo {
    /// The main checkout. `~` is expanded.
    pub path: PathBuf,
    /// The branch work starts from; the repository's default when unset.
    #[serde(default)]
    pub base: Option<String>,
    /// The model for this repository's agents.
    #[serde(default)]
    pub model: Option<String>,
    /// A first step agents run in a new worktree, such as `bun install`.
    #[serde(default)]
    pub setup: Option<String>,
}

/// Interface settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tui {
    /// The theme name.
    pub theme: Option<String>,
    /// Key overrides, by action name. Interpreted by the interface.
    pub keys: BTreeMap<String, String>,
    /// Programs the interface offers to open an agent's worktree with.
    pub tools: Tools,
    /// No travelling animations: message pulses become short highlights
    /// and spinners stop.
    pub reduced_motion: bool,
}

/// Programs the interface offers to open an agent's worktree with, never
/// required. Each is a command line run in the worktree, where `{dir}`
/// stands for the worktree and `{base}` for the branch the agent started
/// from. An empty string turns a tool off; an unset one is looked for on
/// the machine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tools {
    /// An editor that runs in the terminal; `$VISUAL`, then `$EDITOR`, when
    /// unset.
    pub editor: Option<String>,
    /// A diff viewer that runs in the terminal; Neovim with diffview when
    /// unset and both are installed.
    pub diff: Option<String>,
    /// A git interface that runs in the terminal; lazygit when unset and
    /// installed.
    pub git: Option<String>,
    /// An editor with a window of its own, started in the background; VS
    /// Code (`code`) when unset and installed.
    pub ide: Option<String>,
}

impl Tools {
    /// Every tool with its name, as written in the configuration.
    pub fn entries(&self) -> [(&'static str, Option<&str>); 4] {
        [
            ("editor", self.editor.as_deref()),
            ("diff", self.diff.as_deref()),
            ("git", self.git.as_deref()),
            ("ide", self.ide.as_deref()),
        ]
    }
}

/// Splits a command line into words the way a shell would for simple
/// commands: spaces separate words, single quotes keep everything, double
/// quotes keep everything but `\"` and `\\`, and a backslash outside quotes
/// keeps the next character. No variables, globs or pipes.
pub fn split_command(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err(format!("a single quote is not closed in `{line}`")),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\')) => word.push(c),
                            Some(c) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => {
                                return Err(format!("a double quote is not closed in `{line}`"));
                            }
                        },
                        Some(c) => word.push(c),
                        None => return Err(format!("a double quote is not closed in `{line}`")),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some(c) => word.push(c),
                    None => return Err(format!("`{line}` ends with a backslash")),
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

impl Config {
    /// Loads `path`, expanding `~` with `home`. A missing file means the
    /// defaults.
    pub fn load(path: &Path, home: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        Self::parse(&text, path, home)
    }

    /// Parses configuration text. `path` only names the file in errors.
    pub fn parse(text: &str, path: &Path, home: &Path) -> Result<Self, ConfigError> {
        let mut config: Self = toml::from_str(text).map_err(|error| ConfigError::Parse {
            path: path.to_owned(),
            message: error.to_string().trim_end().to_owned(),
        })?;
        config.resolve(path, home)?;
        Ok(config)
    }

    fn resolve(&mut self, path: &Path, home: &Path) -> Result<(), ConfigError> {
        let invalid = |message: String| ConfigError::Invalid {
            path: path.to_owned(),
            message,
        };
        for (name, repo) in &mut self.repos {
            if !is_valid_name(name) {
                return Err(invalid(format!(
                    "repository name \"{name}\" must use only letters, digits, `-`, `_` and `.`"
                )));
            }
            repo.path = expand(&repo.path, home);
            if !repo.path.is_absolute() {
                return Err(invalid(format!(
                    "repos.{name}.path must be absolute or start with `~`, got {}",
                    repo.path.display()
                )));
            }
        }
        if self.agents.wake_timeout == 0 {
            return Err(invalid(
                "agents.wake_timeout must be at least 1 second".to_owned(),
            ));
        }
        for (name, line) in self.tui.tools.entries() {
            if let Some(line) = line {
                split_command(line)
                    .map_err(|error| invalid(format!("tui.tools.{name}: {error}")))?;
            }
        }
        Ok(())
    }

    /// The model for an agent in `repo`: the repository's, else the default.
    pub fn model_for(&self, repo: Option<&str>) -> Option<&str> {
        repo.and_then(|name| self.repos.get(name))
            .and_then(|repo| repo.model.as_deref())
            .or(self.agents.model.as_deref())
    }

    /// The repository whose main checkout is `path`, if one is configured.
    pub fn repo_at(&self, path: &Path) -> Option<(&str, &Repo)> {
        self.repos
            .iter()
            .find(|(_, repo)| repo.path == path)
            .map(|(name, repo)| (name.as_str(), repo))
    }
}

/// Expands a leading `~` with `home`.
fn expand(path: &Path, home: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => path.to_owned(),
    }
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config, ConfigError> {
        Config::parse(text, Path::new("config.toml"), Path::new("/home/ada"))
    }

    #[test]
    fn the_example_parses_to_the_defaults() {
        let config = parse(EXAMPLE).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn an_empty_file_is_the_defaults() {
        assert_eq!(parse("").unwrap(), Config::default());
        assert_eq!(Config::default().agents.wake_timeout, 3_600);
    }

    #[test]
    fn repositories_expand_the_home_directory() {
        let config = parse(
            r#"
            [repos.billing]
            path = "~/code/billing"
            base = "develop"
            model = "sonnet"
            setup = "bun install"
            "#,
        )
        .unwrap();
        let repo = &config.repos["billing"];
        assert_eq!(repo.path, PathBuf::from("/home/ada/code/billing"));
        assert_eq!(repo.base.as_deref(), Some("develop"));
        assert_eq!(config.model_for(Some("billing")), Some("sonnet"));
        assert_eq!(
            config
                .repo_at(Path::new("/home/ada/code/billing"))
                .unwrap()
                .0,
            "billing"
        );
    }

    #[test]
    fn repository_models_override_the_default() {
        let config = parse(
            r#"
            [agents]
            model = "opus"
            [repos.docs]
            path = "/srv/docs"
            "#,
        )
        .unwrap();
        assert_eq!(config.model_for(Some("docs")), Some("opus"));
        assert_eq!(config.model_for(None), Some("opus"));
    }

    #[test]
    fn unknown_keys_are_errors_with_a_location() {
        let error = parse("[agents]\nunknown_setting = \"opus\"\n").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("unknown_setting"), "{message}");
        assert!(message.contains("line 2"), "{message}");
    }

    #[test]
    fn permission_modes_use_the_harness_spelling() {
        let config = parse("[agents]\npermission_mode = \"acceptEdits\"\n").unwrap();
        assert_eq!(
            config.agents.permission_mode,
            Some(PermissionMode::AcceptEdits)
        );
        assert_eq!(PermissionMode::DontAsk.as_flag(), "dontAsk");
        assert!(parse("[agents]\npermission_mode = \"bypassPermissions\"\n").is_err());
    }

    #[test]
    fn the_interface_can_stop_its_animations() {
        assert!(!Config::default().tui.reduced_motion);
        let config = parse("[tui]\nreduced_motion = true\n").unwrap();
        assert!(config.tui.reduced_motion);
    }

    #[test]
    fn relative_repository_paths_are_refused() {
        let error = parse("[repos.app]\npath = \"code/app\"\n").unwrap_err();
        assert!(error.to_string().contains("must be absolute"), "{error}");
    }

    #[test]
    fn repository_names_are_checked() {
        let error = parse("[repos.\"my app\"]\npath = \"/srv/app\"\n").unwrap_err();
        assert!(error.to_string().contains("my app"), "{error}");
    }

    #[test]
    fn worktrees_cannot_be_moved_out_of_the_repository() {
        // Claude Code never trusts a worktree started outside its repository
        // (ADR 0013), so the old `[worktrees]` table is an error, not a no-op.
        let error = parse("[worktrees]\nroot = \"~/wt\"\n").unwrap_err();
        assert!(error.to_string().contains("worktrees"), "{error}");
    }

    #[test]
    fn tools_are_command_lines_checked_when_loaded() {
        let config = parse(
            r#"
            [tui.tools]
            editor = "hx"
            diff = "nvim -c 'DiffviewOpen {base}'"
            git = ""
            "#,
        )
        .unwrap();
        let tools = &config.tui.tools;
        assert_eq!(tools.editor.as_deref(), Some("hx"));
        assert_eq!(
            tools.git.as_deref(),
            Some(""),
            "an empty tool is turned off"
        );
        assert_eq!(tools.ide, None);
        let error = parse(
            "[tui.tools]
diff = \"nvim -c 'DiffviewOpen\"\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("tui.tools.diff"), "{error}");
        let error = parse(
            "[tui.tools]
pager = \"less\"\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("pager"), "{error}");
    }

    #[test]
    fn command_lines_split_like_a_shell() {
        let split = |line: &str| split_command(line).unwrap();
        assert_eq!(
            split("nvim -c 'DiffviewOpen {base}'"),
            ["nvim", "-c", "DiffviewOpen {base}"]
        );
        assert_eq!(
            split(r#"code  --new-window "{dir}""#),
            ["code", "--new-window", "{dir}"]
        );
        assert_eq!(
            split(r#"say "a \"b\" \\ c" d\ e ''"#),
            ["say", r#"a "b" \ c"#, "d e", ""]
        );
        assert_eq!(split("   "), Vec::<String>::new());
        assert!(split_command("nvim 'open").is_err());
        assert!(split_command("nvim \"open").is_err());
        assert!(split_command("nvim \\").is_err());
    }

    #[test]
    fn a_missing_file_is_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(&dir.path().join("none.toml"), dir.path()).unwrap();
        assert_eq!(config, Config::default());
    }
}
