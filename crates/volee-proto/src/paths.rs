//! Where volee keeps its files, following the XDG base directories on
//! every platform (a CLI audience expects `~/.config`, also on macOS).

use std::path::PathBuf;

use volee_core::AgentId;

/// Every location volee uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// Configuration: `config.toml`, themes.
    pub config_dir: PathBuf,
    /// State: database, logs, spool, per-agent files.
    pub state_dir: PathBuf,
    /// Data: worktrees created by volee.
    pub data_dir: PathBuf,
    /// The daemon socket.
    pub socket: PathBuf,
}

/// The environment variable that names the agent a shell belongs to: volee
/// sets it, to the agent's identifier, in the sessions of the agents it
/// launches. The commands that speak for the person (answering a question,
/// closing a mission) refuse to run where it is set.
pub const AGENT_ENV: &str = "VOLEE_AGENT";

/// The home directory could not be determined.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("cannot find the home directory: set HOME, or VOLEE_CONFIG_DIR and VOLEE_STATE_DIR")]
pub struct NoHome;

impl Paths {
    /// Resolves the paths from environment variables, read through `env` so
    /// that tests never touch the real environment.
    ///
    /// | Path | Variables, in order |
    /// | --- | --- |
    /// | config | `VOLEE_CONFIG_DIR`, `$XDG_CONFIG_HOME/volee`, `~/.config/volee` |
    /// | state | `VOLEE_STATE_DIR`, `$XDG_STATE_HOME/volee`, `~/.local/state/volee` |
    /// | data | `VOLEE_DATA_DIR`, `$XDG_DATA_HOME/volee`, `~/.local/share/volee` |
    /// | socket | `VOLEE_SOCKET`, `$XDG_RUNTIME_DIR/volee.sock`, `<state>/volee.sock` |
    pub fn resolve(env: impl Fn(&str) -> Option<String>) -> Result<Self, NoHome> {
        let var = |name: &str| {
            env(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let home = var("HOME");
        let under_home = |relative: &str| home.as_ref().map(|home| home.join(relative));
        let pick = |own: &str, xdg: &str, fallback: &str| {
            var(own)
                .or_else(|| var(xdg).map(|dir| dir.join("volee")))
                .or_else(|| under_home(fallback))
                .ok_or(NoHome)
        };
        let config_dir = pick("VOLEE_CONFIG_DIR", "XDG_CONFIG_HOME", ".config/volee")?;
        let state_dir = pick("VOLEE_STATE_DIR", "XDG_STATE_HOME", ".local/state/volee")?;
        let data_dir = pick("VOLEE_DATA_DIR", "XDG_DATA_HOME", ".local/share/volee")?;
        let socket = var("VOLEE_SOCKET")
            .or_else(|| var("XDG_RUNTIME_DIR").map(|dir| dir.join("volee.sock")))
            .unwrap_or_else(|| state_dir.join("volee.sock"));
        Ok(Self {
            config_dir,
            state_dir,
            data_dir,
            socket,
        })
    }

    /// Resolves the paths from the process environment.
    pub fn from_env() -> Result<Self, NoHome> {
        Self::resolve(|name| std::env::var(name).ok())
    }

    /// Paths rooted in one directory, for tests and isolated runs.
    pub fn rooted(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            config_dir: root.join("config"),
            state_dir: root.join("state"),
            data_dir: root.join("data"),
            socket: root.join("volee.sock"),
        }
    }

    /// The configuration file.
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// The event database.
    pub fn database(&self) -> PathBuf {
        self.state_dir.join("volee.db")
    }

    /// The spool used by hooks when the daemon is unreachable.
    pub fn spool_dir(&self) -> PathBuf {
        self.state_dir.join("spool")
    }

    /// The daemon's logs.
    pub fn log_dir(&self) -> PathBuf {
        self.state_dir.join("logs")
    }

    /// The lock that keeps a single daemon running.
    pub fn lock_file(&self) -> PathBuf {
        self.state_dir.join("daemon.lock")
    }

    /// Per-agent files: hook settings, MCP configuration.
    pub fn agent_dir(&self, agent: AgentId) -> PathBuf {
        self.state_dir.join("agents").join(agent.to_string())
    }

    /// Where volee creates worktrees for a repository.
    pub fn worktrees_dir(&self, repo: &str) -> PathBuf {
        self.data_dir.join("worktrees").join(repo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn resolve(vars: &[(&str, &str)]) -> Result<Paths, NoHome> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Paths::resolve(|name| map.get(name).cloned())
    }

    #[test]
    fn defaults_follow_xdg_under_home() {
        let paths = resolve(&[("HOME", "/home/ada")]).unwrap();
        assert_eq!(paths.config_dir, PathBuf::from("/home/ada/.config/volee"));
        assert_eq!(
            paths.state_dir,
            PathBuf::from("/home/ada/.local/state/volee")
        );
        assert_eq!(
            paths.data_dir,
            PathBuf::from("/home/ada/.local/share/volee")
        );
        assert_eq!(
            paths.socket,
            PathBuf::from("/home/ada/.local/state/volee/volee.sock")
        );
        assert_eq!(
            paths.database(),
            PathBuf::from("/home/ada/.local/state/volee/volee.db")
        );
    }

    #[test]
    fn xdg_variables_win_over_home() {
        let paths = resolve(&[
            ("HOME", "/home/ada"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("XDG_STATE_HOME", "/st"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ])
        .unwrap();
        assert_eq!(paths.config_dir, PathBuf::from("/cfg/volee"));
        assert_eq!(paths.state_dir, PathBuf::from("/st/volee"));
        assert_eq!(paths.socket, PathBuf::from("/run/user/1000/volee.sock"));
    }

    #[test]
    fn volee_variables_win_over_everything() {
        let paths = resolve(&[
            ("HOME", "/home/ada"),
            ("XDG_STATE_HOME", "/st"),
            ("VOLEE_STATE_DIR", "/tmp/volee-state"),
            ("VOLEE_SOCKET", "/tmp/v.sock"),
        ])
        .unwrap();
        assert_eq!(paths.state_dir, PathBuf::from("/tmp/volee-state"));
        assert_eq!(paths.socket, PathBuf::from("/tmp/v.sock"));
    }

    #[test]
    fn empty_variables_are_ignored() {
        let paths = resolve(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "")]).unwrap();
        assert_eq!(paths.config_dir, PathBuf::from("/h/.config/volee"));
    }

    #[test]
    fn no_home_is_an_error() {
        assert_eq!(resolve(&[]), Err(NoHome));
    }
}
