//! The configuration file of volee, `~/.config/volee/config.lua`.
//!
//! The file is Lua that returns a table (ADR 0037, amended in step 8), run in
//! the same sandbox as the rules files. Every setting is optional, a missing
//! file means the defaults, and unknown keys are errors: a typo becomes a
//! clear message instead of a setting that silently does nothing. The
//! decisions about changes (risk, layers, merge, the held question delay)
//! are not here: they live in the rules files.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// A commented example configuration, also written by `volee config init`.
pub const EXAMPLE: &str = include_str!("example.lua");

/// The name of the configuration file inside volee's config directory.
pub const CONFIG_FILE: &str = "config.lua";

/// The name of the TOML file of volee 0.x, which is not read any more.
pub const LEGACY_CONFIG_FILE: &str = "config.toml";

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
    /// The file cannot be written.
    #[error("cannot write {path}: {source}")]
    Write {
        /// The file.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The file does not run, or has unknown or ill-typed keys.
    #[error("{path}: {message}")]
    Parse {
        /// The file.
        path: PathBuf,
        /// What Lua or the reader said, with the line when there is one.
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
    /// What the daemon does with git in the background.
    pub git: GitSettings,
    /// Where the daemon receives Claude Code's hooks.
    pub hooks: Hooks,
    /// The AI pre-review of finished work.
    pub review: Review,
    /// The Claude Code program volee runs itself (`review`, fix requests).
    pub claude: Claude,
}

/// The AI pre-review of finished work: Claude Code's native `/code-review`,
/// run in the agent's worktree. Its findings are an opinion, never a gate.
/// The model and the number of reviewers come from the Lua rules
/// (`review.model`, `review.reviewers`), which know the risk of the change.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Review {
    /// Whether the daemon reviews at all.
    pub enabled: bool,
    /// Whether a finished-work card is reviewed as soon as it appears
    /// (`true`) or only when asked (`POST /api/agents/{id}/review`).
    pub auto: bool,
    /// How long, in seconds, one reviewer may run.
    pub timeout: u64,
    /// How many reviews run at once.
    pub concurrency: usize,
}

impl Default for Review {
    fn default() -> Self {
        Self {
            enabled: true,
            auto: true,
            timeout: 300,
            concurrency: 1,
        }
    }
}

/// The Claude Code program volee runs itself: the pre-review and the fix
/// requests that resume a stopped session. One setting for both, so that a
/// `claude` outside the daemon's `PATH` is named once.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Claude {
    /// The program, a name found on `PATH` or a path.
    pub command: String,
}

impl Default for Claude {
    fn default() -> Self {
        Self {
            command: "claude".to_owned(),
        }
    }
}

/// Where the daemon receives Claude Code's `http` hooks. It listens on the
/// loopback interface only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hooks {
    /// The TCP port on 127.0.0.1.
    pub port: u16,
    /// How long, in seconds, a permission request is held for the person to
    /// decide in the browser. `volee doctor` writes it as the hook's timeout;
    /// when it runs out, Claude Code asks in the terminal as usual.
    pub permission_timeout: u64,
}

/// The default hold of a permission request, in seconds.
pub const DEFAULT_PERMISSION_TIMEOUT: u64 = 3600;

/// The default hook port.
pub const DEFAULT_HOOK_PORT: u16 = 47835;

impl Default for Hooks {
    fn default() -> Self {
        Self {
            port: DEFAULT_HOOK_PORT,
            permission_timeout: DEFAULT_PERMISSION_TIMEOUT,
        }
    }
}

/// What the daemon does with git in the background.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GitSettings {
    /// Which repositories the daemon fetches.
    pub fetch: Fetch,
}

/// Which repositories the daemon fetches in the background (`git fetch
/// --prune origin`, at most every five minutes), so that push states and
/// merges reflect the remote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Fetch {
    /// The repositories of agents volee launched: their remote is part of
    /// the work volee started.
    #[default]
    Launched,
    /// Also the repositories of sessions volee discovered but did not
    /// launch.
    All,
    /// None: evidence compares with what your own fetches brought.
    Never,
}

impl Fetch {
    /// Whether the repository of an agent may be fetched, given whether
    /// volee launched the agent.
    pub const fn allows(self, launched_by_volee: bool) -> bool {
        match self {
            Self::Launched => launched_by_volee,
            Self::All => true,
            Self::Never => false,
        }
    }
}

impl fmt::Display for Fetch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Launched => "launched",
            Self::All => "all",
            Self::Never => "never",
        })
    }
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
    /// How often the daemon lists Claude Code's sessions for `volee agents`,
    /// in seconds (at least 1).
    pub poll_interval: u64,
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self { poll_interval: 5 }
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
    /// How volee merges this repository's pull requests once you approved a
    /// mission (ADR 0032); the first method the repository allows when
    /// unset.
    #[serde(default)]
    pub merge: Option<MergeMethod>,
}

/// How a pull request is merged into its base branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MergeMethod {
    /// A merge commit: the branch's commits stay in the history.
    Merge,
    /// One commit that holds the whole change.
    Squash,
    /// The branch's commits, replayed on the base branch.
    Rebase,
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

    /// Runs configuration text. `path` only names the file in errors.
    pub fn parse(text: &str, path: &Path, home: &Path) -> Result<Self, ConfigError> {
        let source = volee_rules::Source::new(path.display().to_string(), text);
        let mut config: Self = volee_rules::load_data(&source, volee_rules::Limits::default())
            .map_err(|error| ConfigError::Parse {
                path: path.to_owned(),
                message: error.to_string(),
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
        if self.agents.poll_interval == 0 {
            return Err(invalid(
                "agents.poll_interval must be at least 1 second".to_owned(),
            ));
        }
        if self.hooks.permission_timeout == 0 {
            return Err(invalid(
                "hooks.permission_timeout must be at least 1 second".to_owned(),
            ));
        }
        if self.review.timeout == 0 || self.review.concurrency == 0 {
            return Err(invalid(
                "review.timeout and review.concurrency must be at least 1".to_owned(),
            ));
        }
        Ok(())
    }

    /// The repository whose main checkout is `path`, if one is configured.
    pub fn repo_at(&self, path: &Path) -> Option<(&str, &Repo)> {
        self.repos
            .iter()
            .find(|(_, repo)| repo.path == path)
            .map(|(name, repo)| (name.as_str(), repo))
    }
}

/// The TOML file of volee 0.x in `config_dir`, if one is still there. volee
/// never reads it and never converts it.
pub fn legacy_file(config_dir: &Path) -> Option<PathBuf> {
    let path = config_dir.join(LEGACY_CONFIG_FILE);
    path.is_file().then_some(path)
}

/// The one-line warning for a leftover TOML file.
pub fn legacy_warning(legacy: &Path) -> String {
    format!(
        "{} is ignored: volee reads {CONFIG_FILE} now (see `volee config init`)",
        legacy.display()
    )
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
        Config::parse(text, Path::new("config.lua"), Path::new("/home/ada"))
    }

    #[test]
    fn the_example_runs_to_the_defaults() {
        let config = parse(EXAMPLE).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn the_agents_poll_interval_has_a_default_and_a_floor() {
        assert_eq!(Config::default().agents.poll_interval, 5);
        assert_eq!(
            parse("return { agents = { poll_interval = 2 } }")
                .unwrap()
                .agents
                .poll_interval,
            2
        );
        assert!(parse("return { agents = { poll_interval = 0 } }").is_err());
    }

    #[test]
    fn the_hook_port_has_a_default_and_can_be_set() {
        assert_eq!(Config::default().hooks.port, DEFAULT_HOOK_PORT);
        assert_eq!(
            parse("return { hooks = { port = 4000 } }")
                .unwrap()
                .hooks
                .port,
            4000
        );
        assert!(parse("return { hooks = { port = 70000 } }").is_err());
        assert!(parse("return { hooks = { permission_timeout = 0 } }").is_err());
        let held = parse("return { hooks = { permission_timeout = 30 } }").unwrap();
        assert_eq!(held.hooks.permission_timeout, 30);
        assert_eq!(Config::default().hooks.permission_timeout, 3600);
        assert!(
            parse("return { hooks = { host = '0.0.0.0' } }").is_err(),
            "loopback only"
        );
        assert!(
            parse("return { hooks = { question_delay = 9 } }").is_err(),
            "the held question delay is a rule now (held_question_timeout)"
        );
    }

    #[test]
    fn an_empty_file_is_the_defaults() {
        assert_eq!(parse("return {}").unwrap(), Config::default());
        assert_eq!(Config::default().git.fetch, Fetch::Launched);
    }

    #[test]
    fn the_review_is_on_by_default_and_can_be_tuned_or_switched_off() {
        let defaults = Config::default().review;
        assert!(defaults.enabled && defaults.auto);
        assert_eq!(defaults.concurrency, 1);
        let tuned =
            parse("return { review = { enabled = false, auto = false, timeout = 9 } }").unwrap();
        assert!(!tuned.review.enabled && !tuned.review.auto);
        assert_eq!(tuned.review.timeout, 9);
        assert!(parse("return { review = { concurrency = 0 } }").is_err());
        assert!(parse("return { review = { timeout = 0 } }").is_err());
    }

    #[test]
    fn background_fetches_follow_the_setting() {
        let fetch = |word: &str| {
            parse(&format!("return {{ git = {{ fetch = '{word}' }} }}"))
                .unwrap()
                .git
                .fetch
        };
        assert_eq!(fetch("all"), Fetch::All);
        assert_eq!(fetch("never"), Fetch::Never);
        assert_eq!(fetch("launched"), Fetch::Launched);
        // Repositories of agents volee launched, and only those, by default.
        assert!(Fetch::Launched.allows(true));
        assert!(!Fetch::Launched.allows(false));
        assert!(Fetch::All.allows(false));
        assert!(!Fetch::Never.allows(true));
        let error = parse("return { git = { fetch = 'sometimes' } }")
            .unwrap_err()
            .to_string();
        assert!(error.contains("sometimes"), "{error}");
        assert!(parse("return { git = { fetch = true } }").is_err());
    }

    #[test]
    fn repositories_expand_the_home_directory() {
        let config = parse(
            r#"return { repos = { billing = {
                path = "~/code/billing", base = "develop",
            } } }"#,
        )
        .unwrap();
        let repo = &config.repos["billing"];
        assert_eq!(repo.path, PathBuf::from("/home/ada/code/billing"));
        assert_eq!(repo.base.as_deref(), Some("develop"));
        assert_eq!(
            config
                .repo_at(Path::new("/home/ada/code/billing"))
                .unwrap()
                .0,
            "billing"
        );
    }

    #[test]
    fn lua_can_compute_a_setting() {
        let config = parse(
            r#"local root = "/srv"
            local repos = {}
            for _, name in ipairs({ "a", "b" }) do
              repos[name] = { path = root .. "/" .. name }
            end
            return { repos = repos }"#,
        )
        .unwrap();
        assert_eq!(config.repos["b"].path, PathBuf::from("/srv/b"));
    }

    #[test]
    fn unknown_keys_and_broken_files_are_errors_that_say_where() {
        let error = parse("return { agents = { unknown_setting = 'opus' } }").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("unknown_setting"), "{message}");
        let error = parse("return {\n agents = nil .. 'x'\n}").unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("config.lua") && message.contains('2'),
            "{message}"
        );
        assert!(parse("return 3").is_err());
        assert!(parse("return { agents = function() end }").is_err());
    }

    #[test]
    fn keys_of_volee_0_x_that_nothing_reads_are_unknown_keys() {
        for old in [
            "agents = { model = 'opus' }",
            "agents = { permission_mode = 'auto' }",
            "agents = { wake_timeout = 60 }",
            "agents = { resume_after_reboot = false }",
            "missions = { max_workers = 4 }",
            "repos = { a = { path = '/srv/a', model = 'opus' } }",
            "repos = { a = { path = '/srv/a', setup = { 'bun install' } } }",
        ] {
            let error = parse(&format!("return {{ {old} }}"))
                .unwrap_err()
                .to_string();
            assert!(error.contains("unknown field"), "{old}: {error}");
        }
    }

    #[test]
    fn a_repository_says_how_its_pull_requests_are_merged() {
        let method = |written: &str| {
            parse(&format!(
                "return {{ repos = {{ shop = {{ path = '/srv/shop', merge = '{written}' }} }} }}"
            ))
            .map(|config| config.repos["shop"].merge)
        };
        assert_eq!(method("merge").unwrap(), Some(MergeMethod::Merge));
        assert_eq!(method("squash").unwrap(), Some(MergeMethod::Squash));
        assert_eq!(method("rebase").unwrap(), Some(MergeMethod::Rebase));
        // Unset: the forge is asked what the repository allows.
        let unset = parse("return { repos = { shop = { path = '/srv/shop' } } }").unwrap();
        assert_eq!(unset.repos["shop"].merge, None);
        // Anything else is a mistake said at once, not a default.
        for wrong in ["fast-forward", "Squash", ""] {
            let error = method(wrong).unwrap_err();
            assert!(error.to_string().contains(wrong), "{wrong}: {error}");
        }
    }

    #[test]
    fn relative_repository_paths_are_refused() {
        let error = parse("return { repos = { app = { path = 'code/app' } } }").unwrap_err();
        assert!(error.to_string().contains("must be absolute"), "{error}");
    }

    #[test]
    fn repository_names_are_checked() {
        let error = parse("return { repos = { ['my app'] = { path = '/srv/app' } } }").unwrap_err();
        assert!(error.to_string().contains("my app"), "{error}");
    }

    #[test]
    fn worktrees_cannot_be_moved_out_of_the_repository() {
        // Claude Code never trusts a worktree started outside its repository
        // (ADR 0013), so a `worktrees` table is an error, not a no-op.
        let error = parse("return { worktrees = { root = '~/wt' } }").unwrap_err();
        assert!(error.to_string().contains("worktrees"), "{error}");
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
        let config = Config::load(&dir.path().join("none.lua"), dir.path()).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn a_leftover_toml_file_is_noticed_and_never_read() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(legacy_file(dir.path()), None);
        std::fs::write(dir.path().join(LEGACY_CONFIG_FILE), "[agents]\n").unwrap();
        let found = legacy_file(dir.path()).unwrap();
        assert!(found.ends_with(LEGACY_CONFIG_FILE));
        let warning = legacy_warning(&found);
        assert!(
            warning.contains("config.lua") && warning.contains("ignored"),
            "{warning}"
        );
        assert!(!warning.contains('\n'), "one line");
        let config = Config::load(&dir.path().join(CONFIG_FILE), dir.path()).unwrap();
        assert_eq!(config, Config::default());
    }
}
