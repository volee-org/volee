//! The readable settings table of a rules file, and its defaults.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::editor::Editor;
use crate::types::Layer;

/// A duration written as seconds (`600`) or text (`"30s"`, `"10m"`, `"2h"`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DurationSetting {
    /// Seconds.
    Seconds(f64),
    /// Text with a unit suffix: `s`, `m` or `h`.
    Text(String),
}

impl DurationSetting {
    /// The duration, or why the setting is not one.
    pub fn to_duration(&self) -> Result<Duration, String> {
        match self {
            Self::Seconds(s) if s.is_finite() && *s >= 0.0 => Ok(Duration::from_secs_f64(*s)),
            Self::Seconds(s) => Err(format!("{s} is not a valid number of seconds")),
            Self::Text(text) => {
                let text = text.trim();
                let (number, unit) = text.split_at(text.len().saturating_sub(1));
                let factor = match unit {
                    "s" => 1.0,
                    "m" => 60.0,
                    "h" => 3600.0,
                    _ => return Err(format!("{text:?} is not a duration (try \"30s\", \"10m\")")),
                };
                number
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite() && *n >= 0.0)
                    .map(|n| Duration::from_secs_f64(n * factor))
                    .ok_or_else(|| format!("{text:?} is not a duration (try \"30s\", \"10m\")"))
            }
        }
    }
}

/// Size thresholds, in lines changed (added plus removed) and files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SizeSetting {
    /// At or above this many lines the risk is at least medium.
    pub medium_lines: u32,
    /// At or above this many lines the risk is high.
    pub high_lines: u32,
    /// At or above this many files the risk is at least medium.
    pub medium_files: u32,
    /// At or above this many files the risk is high.
    pub high_files: u32,
    /// A change with no test touched and at least this many lines is at
    /// least medium.
    pub untested_lines: u32,
}

impl Default for SizeSetting {
    fn default() -> Self {
        Self {
            medium_lines: 200,
            high_lines: 800,
            medium_files: 10,
            high_files: 30,
            untested_lines: 50,
        }
    }
}

/// The pre-review.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewSetting {
    /// The reviewer model.
    pub model: String,
    /// Reviewers for a low or medium risk change; 0 skips the review.
    pub reviewers: u8,
    /// Reviewers for a high risk change.
    pub high_reviewers: u8,
}

impl Default for ReviewSetting {
    fn default() -> Self {
        Self {
            model: "haiku".into(),
            reviewers: 1,
            high_reviewers: 2,
        }
    }
}

/// The layers of the card shown at each risk level.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayersSetting {
    /// Layers for a low risk change.
    pub low: Vec<Layer>,
    /// Layers for a medium risk change.
    pub medium: Vec<Layer>,
    /// Layers for a high risk change.
    pub high: Vec<Layer>,
}

impl Default for LayersSetting {
    fn default() -> Self {
        Self {
            low: vec![Layer::Summary, Layer::Diff],
            medium: vec![Layer::Summary, Layer::Review, Layer::Diff],
            high: Layer::ALL.to_vec(),
        }
    }
}

/// Everything a rules file can set without writing a function. Missing keys
/// take the defaults below.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Globs of paths that make a change high risk.
    pub sensitive_paths: Vec<String>,
    /// Globs of database migrations (high risk).
    pub migration_paths: Vec<String>,
    /// Globs of dependency files (at least medium).
    pub dependency_paths: Vec<String>,
    /// Size thresholds.
    pub size: SizeSetting,
    /// The pre-review.
    pub review: ReviewSetting,
    /// The card layers per risk level.
    pub layers: LayersSetting,
    /// Whether a low risk change may merge without asking. Off by default.
    pub auto_merge: bool,
    /// How long a held question waits before it gives up.
    pub held_question_timeout: DurationSetting,
    /// The person's editors.
    pub editors: Vec<Editor>,
    /// Trust the rules file of every repository. Only the personal file can
    /// say it: without it a repository's file can raise a risk but never
    /// lower it, and never turn automatic merging on.
    pub trust_repo_rules: bool,
    /// Repository roots whose rules file is trusted (see `trust_repo_rules`).
    pub trusted_repos: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        let strings = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect();
        Self {
            sensitive_paths: Vec::new(),
            migration_paths: strings(&["**/migrations/**", "**/migrate/**"]),
            dependency_paths: strings(&[
                "Cargo.toml",
                "Cargo.lock",
                "package.json",
                "package-lock.json",
                "pnpm-lock.yaml",
                "bun.lock",
                "yarn.lock",
                "go.mod",
                "go.sum",
                "pyproject.toml",
                "requirements.txt",
                "composer.json",
                "composer.lock",
                "Gemfile",
                "Gemfile.lock",
            ]),
            size: SizeSetting::default(),
            review: ReviewSetting::default(),
            layers: LayersSetting::default(),
            auto_merge: false,
            held_question_timeout: DurationSetting::Seconds(600.0),
            editors: Vec::new(),
            trust_repo_rules: false,
            trusted_repos: Vec::new(),
        }
    }
}

impl Settings {
    /// How long a held question waits. Valid once [`Settings::validate`]
    /// passed (rules loading checks it); falls back to ten minutes.
    pub fn held_question_timeout(&self) -> Duration {
        self.held_question_timeout
            .to_duration()
            .unwrap_or(Duration::from_secs(600))
    }

    /// Whether the rules file of the repository at `root` is trusted: the
    /// person said so for every repository or for this one.
    pub fn trusts(&self, root: &std::path::Path) -> bool {
        if self.trust_repo_rules {
            return true;
        }
        let same = |listed: &str| {
            let listed = std::path::Path::new(listed);
            listed == root
                || matches!(
                    (listed.canonicalize(), root.canonicalize()),
                    (Ok(a), Ok(b)) if a == b
                )
        };
        self.trusted_repos.iter().any(|listed| same(listed))
    }

    /// Checks what a type cannot: durations and editor commands.
    pub fn validate(&self) -> Result<(), String> {
        self.held_question_timeout
            .to_duration()
            .map_err(|e| format!("held_question_timeout: {e}"))?;
        for editor in &self.editors {
            editor.validate().map_err(|e| format!("editors: {e}"))?;
        }
        Ok(())
    }
}

/// The top-level keys a rules file may hold.
pub(crate) const SETTING_KEYS: [&str; 11] = [
    "sensitive_paths",
    "migration_paths",
    "dependency_paths",
    "size",
    "review",
    "layers",
    "auto_merge",
    "held_question_timeout",
    "editors",
    "trust_repo_rules",
    "trusted_repos",
];

/// The rule functions a rules file may define.
pub(crate) const FUNCTION_KEYS: [&str; 5] = ["risk", "layers", "review", "auto_merge", "finished"];
