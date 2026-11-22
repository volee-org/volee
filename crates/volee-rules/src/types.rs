//! The typed facts Rust gives to the rules and the typed answers it takes
//! back.

use serde::{Deserialize, Serialize};

/// A risk level. Ordered: `Low < Medium < High`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    /// A glance is enough, and the merge may be automatic when a rule says so.
    Low,
    /// A normal read.
    #[default]
    Medium,
    /// A full read; never automatic.
    High,
}

impl Risk {
    /// The word Lua sees and returns.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

/// One file of a change.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    /// Path relative to the repository root, with `/` separators.
    pub path: String,
    /// Lines added.
    pub added: u32,
    /// Lines removed.
    pub removed: u32,
}

/// The facts about a change that Lua decides on. Rust gathers them (git,
/// hooks); Lua never touches the repository.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// Repository name.
    pub repo: String,
    /// The branch holding the change.
    pub branch: String,
    /// The branch it would merge into.
    pub base: String,
    /// The files touched.
    pub files: Vec<FileChange>,
    /// Commits on the branch that the base does not have.
    pub commits: u32,
    /// The change adds or edits tests.
    pub tests_touched: bool,
    /// The change adds or edits a database migration.
    pub migrations: bool,
    /// The agent that did the work (for example `claude`).
    pub agent: String,
    /// The person using volee, so the same change may need different care
    /// for two people.
    pub developer: String,
}

impl Change {
    /// Lines added across all files.
    pub fn lines_added(&self) -> u32 {
        self.files.iter().map(|f| f.added).sum()
    }

    /// Lines removed across all files.
    pub fn lines_removed(&self) -> u32 {
        self.files.iter().map(|f| f.removed).sum()
    }

    /// Lines added plus removed.
    pub fn lines_changed(&self) -> u32 {
        self.lines_added() + self.lines_removed()
    }

    /// A small change on one file, for documentation and tests.
    pub fn example(path: &str, added: u32, removed: u32) -> Self {
        Self {
            repo: "example".into(),
            branch: "feature".into(),
            base: "main".into(),
            files: vec![FileChange {
                path: path.into(),
                added,
                removed,
            }],
            commits: 1,
            tests_touched: true,
            migrations: false,
            agent: "claude".into(),
            developer: "dev".into(),
        }
    }
}

/// What Lua sees: the change plus the numbers computed from it.
#[derive(Serialize)]
pub(crate) struct ChangeView<'a> {
    #[serde(flatten)]
    pub change: &'a Change,
    pub added: u32,
    pub removed: u32,
    pub lines: u32,
    pub file_count: usize,
}

impl<'a> From<&'a Change> for ChangeView<'a> {
    fn from(change: &'a Change) -> Self {
        Self {
            change,
            added: change.lines_added(),
            removed: change.lines_removed(),
            lines: change.lines_changed(),
            file_count: change.files.len(),
        }
    }
}

/// A risk level and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskAssessment {
    /// The level.
    pub level: Risk,
    /// The reasons, shown on the card.
    pub reasons: Vec<String>,
}

/// A part of the card the person is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    /// What was done and why.
    Summary,
    /// The plan the agent followed.
    Plan,
    /// The AI review opinion.
    Review,
    /// The diagram of the change.
    Diagram,
    /// The guided diff.
    Diff,
}

impl Layer {
    /// Every layer, in display order.
    pub const ALL: [Layer; 5] = [
        Layer::Summary,
        Layer::Plan,
        Layer::Review,
        Layer::Diagram,
        Layer::Diff,
    ];

    pub(crate) fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.as_str() == text)
    }

    /// The word Lua sees and returns.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Plan => "plan",
            Self::Review => "review",
            Self::Diagram => "diagram",
            Self::Diff => "diff",
        }
    }
}

/// How the pre-review runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewPlan {
    /// The model name given to the reviewer.
    pub model: String,
    /// How many reviewers; 0 skips the review.
    pub reviewers: u8,
}

/// Whether volee may merge by itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeDecision {
    /// Merge without asking the person.
    pub allowed: bool,
    /// Why, shown on the card.
    pub reason: String,
}

/// The facts the finished-work rule decides on (the same as
/// `volee_core::finished`, as plain data).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishedFacts {
    /// The agent's newest sign of life is a stop.
    pub stopped_last: bool,
    /// The agent works, waits for an answer or launches now.
    pub busy: bool,
    /// Commits on the branch that the base does not have.
    pub commits: u32,
    /// Of those, the commits whose change is not already in the base.
    pub unique_commits: u32,
    /// The branch is merged into its base.
    pub merged: bool,
}
