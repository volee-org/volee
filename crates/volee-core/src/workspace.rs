//! Where an agent works and where it can be seen.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The repository, worktree and branch an agent works in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    /// The agent's working directory.
    pub cwd: PathBuf,
    /// The repository's display name, when the directory is in a repository.
    pub repo: Option<String>,
    /// The main checkout of the repository.
    pub repo_root: Option<PathBuf>,
    /// The linked worktree, when the agent does not work in the main checkout.
    pub worktree: Option<PathBuf>,
    /// The checked-out branch; `None` when detached or unknown.
    pub branch: Option<String>,
    /// The branch the work started from, for example `main` or `develop`.
    pub base: Option<String>,
    /// The commit the agent started from, used to tell its commits apart.
    pub start_commit: Option<String>,
}

impl Workspace {
    /// A workspace known only by its directory.
    pub fn at(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            ..Self::default()
        }
    }

    /// A short label: `repo › branch`, or the directory name when no
    /// repository is known.
    pub fn label(&self) -> String {
        match (&self.repo, &self.branch) {
            (Some(repo), Some(branch)) => format!("{repo} \u{203a} {branch}"),
            (Some(repo), None) => repo.clone(),
            (None, _) => dir_name(&self.cwd),
        }
    }
}

fn dir_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Where an agent can be shown. Never an identity (ADR 0001).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Location {
    /// In a volee host (ADR 0019): volee holds its terminal, shows its exact
    /// screen and lets any terminal take it over.
    Host {
        /// The host's id: its socket is `<state>/hosts/<id>.sock`.
        id: String,
    },
    /// A background session of its harness, started by an earlier volee or
    /// by hand. volee shows it and can adopt it into a host (ADR 0023).
    Background,
    /// Inside a terminal multiplexer pane.
    Pane {
        /// The multiplexer, for example `tmux` or `tuios`.
        multiplexer: String,
        /// The multiplexer's own address of the pane, written
        /// `<session>:<pane>` (ADR 0015).
        target: String,
    },
    /// In a terminal volee cannot address.
    Terminal,
    /// Not known yet.
    #[default]
    Unknown,
}

impl Location {
    /// The id of the host the agent runs in, if it runs in one.
    pub fn host(&self) -> Option<&str> {
        match self {
            Self::Host { id } => Some(id),
            _ => None,
        }
    }

    /// Where the agent runs, said for people: `volee host`, `background`, the multiplexer
    /// and the session that holds its pane (`tuios · work`), `a terminal`
    /// or `unknown`.
    pub fn label(&self) -> String {
        match self {
            Self::Host { .. } => "volee host".to_owned(),
            Self::Background => "background".to_owned(),
            Self::Pane {
                multiplexer,
                target,
            } => {
                let session = target
                    .rsplit_once(':')
                    .map_or(target.as_str(), |(session, _)| session);
                format!("{multiplexer} · {session}")
            }
            Self::Terminal => "a terminal".to_owned(),
            Self::Unknown => "unknown".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_prefers_repo_and_branch() {
        let mut ws = Workspace::at("/work/slapeo/.worktrees/feat-x");
        assert_eq!(ws.label(), "feat-x");
        ws.repo = Some("slapeo".into());
        assert_eq!(ws.label(), "slapeo");
        ws.branch = Some("feat/x".into());
        assert_eq!(ws.label(), "slapeo \u{203a} feat/x");
    }

    #[test]
    fn location_defaults_to_unknown() {
        assert_eq!(Location::default(), Location::Unknown);
    }

    #[test]
    fn a_pane_is_named_by_its_multiplexer_and_session() {
        let pane = |target: &str| Location::Pane {
            multiplexer: "tuios".into(),
            target: target.into(),
        };
        assert_eq!(pane("work:0a1b2c3d").label(), "tuios · work");
        assert_eq!(pane("build:api:3f2a").label(), "tuios · build:api");
        assert_eq!(pane("plain").label(), "tuios · plain");
        assert_eq!(Location::Terminal.label(), "a terminal");
        assert_eq!(Location::Background.label(), "background");
    }
}
