//! Delivery evidence and the cleanup rule (ADR 0011).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::state::AgentState;
use crate::time::Timestamp;

/// Lines and files changed against the merge base with the base branch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffStat {
    /// Files changed.
    pub files: u32,
    /// Lines added.
    pub insertions: u32,
    /// Lines removed.
    pub deletions: u32,
}

impl fmt::Display for DiffStat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "+{} \u{2212}{}", self.insertions, self.deletions)
    }
}

/// Whether the agent's commits reached the remote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "push")]
pub enum PushState {
    /// The branch has no upstream.
    #[default]
    NoUpstream,
    /// Every local commit is on the remote.
    UpToDate,
    /// Some commits are only local.
    Ahead {
        /// Commits not pushed yet.
        commits: u32,
    },
}

/// The state of a pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrState {
    /// Opened as a draft.
    Draft,
    /// Open for review.
    Open,
    /// Merged.
    Merged,
    /// Closed without merging.
    Closed,
}

/// The combined state of a pull request's checks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChecksState {
    /// No checks ran.
    #[default]
    None,
    /// Some checks still run.
    Pending,
    /// Every check passed.
    Passing,
    /// At least one check failed.
    Failing,
}

/// A pull request opened from the agent's branch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    /// The forge's number.
    pub number: u64,
    /// The web address.
    pub url: String,
    /// Its state.
    pub state: PrState,
    /// Its checks.
    pub checks: ChecksState,
}

/// What volee can prove about an agent's work.
///
/// The evidence is about the agent's branch, not about wherever its worktree
/// happens to point. Proving a merge needs memory: a branch whose commits
/// were merged has no commit left that its base branch lacks, which looks
/// exactly like a branch that never had any. `unmerged_tip` is that memory,
/// carried from one check to the next through
/// [`DeliveryUpdated`](crate::event::EventKind::DeliveryUpdated) events.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    /// Commits on the agent's branch that its base branch does not have.
    pub commits: u32,
    /// Uncommitted changes in the worktree.
    pub dirty: bool,
    /// Changes against the merge base with the base branch.
    pub diff: DiffStat,
    /// Push state of the branch.
    pub push: PushState,
    /// The branch is merged into its base branch: a tip once seen with
    /// commits of its own (`unmerged_tip`) is now part of the base branch.
    pub merged_into_base: bool,
    /// The pull request opened from the branch, if any.
    pub pull_request: Option<PullRequest>,
    /// A report message delivered for analysis work.
    pub report_delivered: bool,
    /// The tip of the agent's branch this evidence was computed from;
    /// `None` when the branch has no commit or no longer exists.
    #[serde(default)]
    pub tip: Option<String>,
    /// The last tip of the agent's branch seen with commits its base branch
    /// did not have. Once this commit is part of the base branch, the work is
    /// merged, even if the branch moved or was deleted afterwards.
    #[serde(default)]
    pub unmerged_tip: Option<String>,
    /// When the daemon started reading this evidence from git and the forge;
    /// `None` when it was never checked (no git source configured).
    #[serde(default)]
    pub checked_at: Option<Timestamp>,
}

impl Delivery {
    /// Whether this evidence was checked before `at`, typically before the
    /// agent's current state began: it may not show what the agent did last.
    /// Evidence that was never checked is not stale; it is all there is.
    pub fn is_stale_at(&self, at: Timestamp) -> bool {
        self.checked_at.is_some_and(|checked| checked < at)
    }

    /// The same evidence without the time it was checked, to tell whether a
    /// new check found anything new.
    #[must_use]
    pub fn without_check_time(&self) -> Self {
        Self {
            checked_at: None,
            ..self.clone()
        }
    }

    /// Whether the work has landed: merged, merged through a pull request, or
    /// reported.
    pub fn is_delivered(&self) -> bool {
        self.merged_into_base
            || self.report_delivered
            || self
                .pull_request
                .as_ref()
                .is_some_and(|pr| pr.state == PrState::Merged)
    }

    /// A one-line summary such as `3 commits · pushed · PR #42 open, checks passing`.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.commits > 0 {
            let plural = if self.commits == 1 { "" } else { "s" };
            parts.push(format!("{} commit{plural}", self.commits));
        }
        parts.push(match self.push {
            PushState::NoUpstream => "not pushed".to_owned(),
            PushState::UpToDate => "pushed".to_owned(),
            PushState::Ahead { commits } => format!("{commits} to push"),
        });
        if let Some(pr) = &self.pull_request {
            let state = match pr.state {
                PrState::Draft => "draft",
                PrState::Open => "open",
                PrState::Merged => "merged",
                PrState::Closed => "closed",
            };
            let checks = match pr.checks {
                ChecksState::None => "",
                ChecksState::Pending => ", checks pending",
                ChecksState::Passing => ", checks passing",
                ChecksState::Failing => ", checks failing",
            };
            parts.push(format!("PR #{} {state}{checks}", pr.number));
        } else if self.merged_into_base {
            parts.push("merged".to_owned());
        }
        if self.dirty {
            parts.push("uncommitted changes".to_owned());
        }
        parts.join(" \u{b7} ")
    }
}

/// Why volee refuses to tidy an agent's worktree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "reason")]
pub enum TidyRefusal {
    /// The agent is still working or waits for a human.
    Busy,
    /// The worktree has uncommitted changes.
    UncommittedChanges,
    /// Nothing proves the work landed.
    NotDelivered,
}

impl fmt::Display for TidyRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Busy => "the agent is still busy",
            Self::UncommittedChanges => "the worktree has uncommitted changes",
            Self::NotDelivered => {
                "no evidence that the work landed (merge, merged pull request or report)"
            }
        })
    }
}

/// The verdict of the cleanup rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "verdict")]
pub enum TidyVerdict {
    /// The worktree can be removed. The branch is always kept.
    Allowed,
    /// The worktree must stay, for these reasons.
    Refused {
        /// Every reason, so the person sees all of them at once.
        reasons: Vec<TidyRefusal>,
    },
}

/// Decides whether an agent's worktree may be removed (ADR 0011).
///
/// Absence is never evidence: a lost agent with no delivery evidence is kept.
pub fn tidy_verdict(state: &AgentState, delivery: &Delivery) -> TidyVerdict {
    let mut reasons = Vec::new();
    if state.is_busy() {
        reasons.push(TidyRefusal::Busy);
    }
    if delivery.dirty {
        reasons.push(TidyRefusal::UncommittedChanges);
    }
    if !delivery.is_delivered() {
        reasons.push(TidyRefusal::NotDelivered);
    }
    if reasons.is_empty() {
        TidyVerdict::Allowed
    } else {
        TidyVerdict::Refused { reasons }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn merged_pr() -> PullRequest {
        PullRequest {
            number: 42,
            url: "https://example.com/pr/42".into(),
            state: PrState::Merged,
            checks: ChecksState::Passing,
        }
    }

    #[test]
    fn a_lost_agent_without_evidence_is_kept() {
        let verdict = tidy_verdict(&AgentState::Lost, &Delivery::default());
        assert_eq!(
            verdict,
            TidyVerdict::Refused {
                reasons: vec![TidyRefusal::NotDelivered]
            }
        );
    }

    #[test]
    fn a_merged_pull_request_allows_tidying() {
        let delivery = Delivery {
            pull_request: Some(merged_pr()),
            ..Delivery::default()
        };
        assert_eq!(
            tidy_verdict(&AgentState::Done, &delivery),
            TidyVerdict::Allowed
        );
    }

    #[test]
    fn every_reason_is_reported() {
        let delivery = Delivery {
            dirty: true,
            ..Delivery::default()
        };
        let TidyVerdict::Refused { reasons } = tidy_verdict(&AgentState::Working, &delivery) else {
            panic!("expected a refusal");
        };
        assert_eq!(
            reasons,
            [
                TidyRefusal::Busy,
                TidyRefusal::UncommittedChanges,
                TidyRefusal::NotDelivered
            ]
        );
    }

    #[test]
    fn a_report_delivers_analysis_work() {
        let delivery = Delivery {
            report_delivered: true,
            ..Delivery::default()
        };
        assert_eq!(
            tidy_verdict(&AgentState::Idle, &delivery),
            TidyVerdict::Allowed
        );
    }

    #[test]
    fn summary_reads_like_a_sentence() {
        let delivery = Delivery {
            commits: 3,
            push: PushState::UpToDate,
            pull_request: Some(PullRequest {
                state: PrState::Open,
                ..merged_pr()
            }),
            ..Delivery::default()
        };
        assert_eq!(
            delivery.summary(),
            "3 commits \u{b7} pushed \u{b7} PR #42 open, checks passing"
        );
        let local = Delivery {
            commits: 1,
            push: PushState::Ahead { commits: 1 },
            dirty: true,
            ..Delivery::default()
        };
        assert_eq!(
            local.summary(),
            "1 commit \u{b7} 1 to push \u{b7} uncommitted changes"
        );
    }

    #[test]
    fn evidence_recorded_before_the_branch_tips_still_reads() {
        // The event log keeps evidence written before `tip` and
        // `unmerged_tip` existed.
        let old = r#"{"commits":2,"dirty":false,"diff":{"files":1,"insertions":3,"deletions":0},
            "push":{"push":"up-to-date"},"merged_into_base":false,"pull_request":null,
            "report_delivered":false}"#;
        let delivery: Delivery = serde_json::from_str(old).unwrap();
        assert_eq!(delivery.commits, 2);
        assert_eq!(delivery.tip, None);
        assert_eq!(delivery.unmerged_tip, None);

        let recorded = Delivery {
            tip: Some("6adc11fa9619748617663ded520ce631e9292269".into()),
            unmerged_tip: Some("6adc11fa9619748617663ded520ce631e9292269".into()),
            ..delivery
        };
        let json = serde_json::to_string(&recorded).unwrap();
        assert_eq!(serde_json::from_str::<Delivery>(&json).unwrap(), recorded);
    }

    #[test]
    fn evidence_checked_before_a_moment_is_stale_then() {
        let checked = Delivery {
            checked_at: Some(Timestamp::from_millis(1_000)),
            ..Delivery::default()
        };
        assert!(checked.is_stale_at(Timestamp::from_millis(1_001)));
        assert!(!checked.is_stale_at(Timestamp::from_millis(1_000)));
        assert!(!Delivery::default().is_stale_at(Timestamp::from_millis(1_001)));
        assert_eq!(checked.without_check_time(), Delivery::default());
    }
}
