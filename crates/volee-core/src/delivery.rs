//! Delivery evidence and the cleanup rule (ADR 0011).

use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent::Agent;
use crate::role::LaunchedBy;
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
    /// The repository has no remote: there is nowhere to push and no pull
    /// request to open. Work is ready once it is committed, and lands when
    /// its branch is merged into its base branch.
    NoRemote,
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
    /// The commit at the head of the pull request when volee read it. volee
    /// keeps only a pull request opened from `origin` whose head is the tip
    /// of the agent's branch, so this is the tip the pull request proves
    /// something about; `None` in evidence recorded before volee read heads.
    #[serde(default)]
    pub head: Option<String>,
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

    /// The pull request, when its head is the branch's current tip. A pull
    /// request read for an older tip says nothing about the commits made
    /// since, and one recorded without its head proves nothing: neither
    /// counts as evidence.
    pub fn pull_request_at_tip(&self) -> Option<&PullRequest> {
        self.pull_request
            .as_ref()
            .filter(|pr| pr.head.is_some() && pr.head == self.tip)
    }

    /// Whether the agent's repository has no remote: nothing can be pushed
    /// and no pull request opened.
    pub fn no_remote(&self) -> bool {
        self.push == PushState::NoRemote
    }

    /// Whether the work has landed: merged, merged through a pull request
    /// whose head is the branch's tip, or reported.
    pub fn is_delivered(&self) -> bool {
        self.merged_into_base
            || self.report_delivered
            || self
                .pull_request_at_tip()
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
            PushState::NoRemote => "no remote".to_owned(),
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

/// Why volee refuses to tidy an agent's worktree. Each reason reads as a
/// sentence for the person who asked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "reason")]
pub enum TidyRefusal {
    /// The agent is still running a turn, starting, or waiting for a person.
    Busy,
    /// The worktree has uncommitted changes.
    UncommittedChanges,
    /// Nothing proves the work landed.
    NotDelivered,
    /// volee did not launch the agent, so its worktree is not volee's.
    NotLaunchedByVolee,
    /// The agent does not work in a worktree volee created
    /// (`<repository>/.claude/worktrees/<name>`).
    NotAVoleeWorktree,
    /// The delivery evidence was never checked.
    EvidenceNotChecked,
    /// The delivery evidence was checked before the agent's last change, so
    /// it may not show what the agent did last.
    EvidenceStale,
    /// The harness kept the agent's session instead of deleting it.
    KeptByHarness {
        /// The harness's explanation.
        detail: String,
    },
    /// git refused to remove the worktree.
    RefusedByGit {
        /// git's explanation, as volee-git reports it.
        detail: String,
    },
}

impl fmt::Display for TidyRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("it is still running (working, starting or waiting for you)"),
            Self::UncommittedChanges => f.write_str("its worktree has uncommitted changes"),
            Self::NotDelivered => f.write_str(
                "its work has not landed: not merged into its base branch, no merged pull \
                 request, no report",
            ),
            Self::NotLaunchedByVolee => f.write_str("volee did not launch it"),
            Self::NotAVoleeWorktree => {
                f.write_str("it does not work in a worktree volee created (.claude/worktrees)")
            }
            Self::EvidenceNotChecked => f.write_str("volee has not checked its work yet"),
            Self::EvidenceStale => f.write_str(
                "volee has not checked its work since its last change; try again in a minute",
            ),
            Self::KeptByHarness { detail } => write!(f, "Claude Code kept its session: {detail}"),
            Self::RefusedByGit { detail } => write!(f, "git kept its worktree: {detail}"),
        }
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

/// Where volee creates the worktrees of the agents it launches, inside their
/// repository ([ADR 0013](https://volee-org.github.io/volee/adr/0013-worktrees-live-in-their-repository.html)).
pub const VOLEE_WORKTREES: &str = ".claude/worktrees";

/// Decides whether volee may remove `agent`'s worktree, and which one
/// (ADR 0011).
///
/// volee removes only a worktree it created (inside
/// `<repository>/.claude/worktrees/`) for an agent it launched, and only
/// when delivery evidence checked since the agent's last change satisfies
/// [`tidy_verdict`]. Stale or missing evidence proves nothing, so only the
/// agent's state is judged then. Every reason to keep the worktree is
/// reported at once. The branch is never part of the decision: it is always
/// kept.
pub fn tidy_check(agent: &Agent) -> Result<PathBuf, Vec<TidyRefusal>> {
    let mut reasons = Vec::new();
    if agent.launched_by == LaunchedBy::Outside {
        reasons.push(TidyRefusal::NotLaunchedByVolee);
    }
    let worktree = agent
        .workspace
        .worktree
        .as_ref()
        .filter(|worktree| is_volee_worktree(worktree, agent.workspace.repo_root.as_deref()));
    if worktree.is_none() {
        reasons.push(TidyRefusal::NotAVoleeWorktree);
    }
    let evidence = match agent.delivery.checked_at {
        None => Some(TidyRefusal::EvidenceNotChecked),
        Some(_) if agent.delivery.is_stale_at(agent.state_since) => {
            Some(TidyRefusal::EvidenceStale)
        }
        Some(_) => None,
    };
    match evidence {
        Some(refusal) => {
            if agent.state.is_busy() {
                reasons.push(TidyRefusal::Busy);
            }
            reasons.push(refusal);
        }
        // A launch that failed before any session started left a worktree
        // in which nothing was ever done: no work can be lost (ADR 0016).
        None if never_used(agent) => {}
        None => {
            if let TidyVerdict::Refused { reasons: more } =
                tidy_verdict(&agent.state, &agent.delivery)
            {
                reasons.extend(more);
            }
        }
    }
    match worktree {
        Some(worktree) if reasons.is_empty() => Ok(worktree.clone()),
        _ => Err(reasons),
    }
}

/// Whether nothing was ever done in the agent's worktree, by the evidence
/// (ADR 0016): its launch failed before any session started, its tree is
/// clean, and its branch holds no commit that its base branch lacks. Checked
/// only on fresh evidence, by [`tidy_check`].
fn never_used(agent: &Agent) -> bool {
    agent.state == AgentState::Failed
        && agent.sessions.is_empty()
        && agent.delivery.commits == 0
        && !agent.delivery.dirty
}

/// Whether `worktree` is strictly inside `<repo_root>/.claude/worktrees/`,
/// written without `.` or `..` that could lead out of it.
fn is_volee_worktree(worktree: &Path, repo_root: Option<&Path>) -> bool {
    let Some(root) = repo_root else {
        return false;
    };
    let dir = root.join(VOLEE_WORKTREES);
    worktree != dir
        && worktree.starts_with(&dir)
        && worktree
            .components()
            .all(|part| !matches!(part, Component::CurDir | Component::ParentDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tip of the branch in these tests, and the head of its pull request.
    const TIP: &str = "c0ffee";

    fn merged_pr() -> PullRequest {
        PullRequest {
            number: 42,
            url: "https://example.com/pr/42".into(),
            state: PrState::Merged,
            checks: ChecksState::Passing,
            head: Some(TIP.into()),
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
            tip: Some(TIP.into()),
            ..Delivery::default()
        };
        assert_eq!(
            tidy_verdict(&AgentState::Done, &delivery),
            TidyVerdict::Allowed
        );
    }

    #[test]
    fn a_pull_request_proves_only_the_tip_it_was_read_at() {
        let at = |tip: Option<&str>, head: Option<&str>| Delivery {
            pull_request: Some(PullRequest {
                head: head.map(str::to_owned),
                ..merged_pr()
            }),
            tip: tip.map(str::to_owned),
            ..Delivery::default()
        };
        assert!(at(Some(TIP), Some(TIP)).is_delivered());
        assert!(at(Some(TIP), Some(TIP)).pull_request_at_tip().is_some());
        // Commits came after the pull request was read: they are not in it.
        let moved = at(Some("beef00"), Some(TIP));
        assert!(!moved.is_delivered());
        assert_eq!(moved.pull_request_at_tip(), None);
        assert_eq!(
            tidy_verdict(&AgentState::Done, &moved),
            TidyVerdict::Refused {
                reasons: vec![TidyRefusal::NotDelivered]
            }
        );
        // Evidence recorded before volee read heads, or a branch that is
        // gone, proves nothing.
        assert!(!at(Some(TIP), None).is_delivered());
        assert!(!at(None, Some(TIP)).is_delivered());
        assert!(!at(None, None).is_delivered());
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
    fn a_pull_request_recorded_before_heads_reads_without_one() {
        let old = r#"{"commits":0,"dirty":false,"diff":{"files":0,"insertions":0,"deletions":0},
            "push":{"push":"up-to-date"},"merged_into_base":false,
            "pull_request":{"number":42,"url":"u","state":"merged","checks":"passing"},
            "report_delivered":false,"tip":"c0ffee"}"#;
        let delivery: Delivery = serde_json::from_str(old).unwrap();
        assert_eq!(delivery.pull_request.as_ref().unwrap().head, None);
        assert!(!delivery.is_delivered(), "no head, no proof");
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

    /// An agent volee launched in its own worktree, done, whose branch was
    /// merged, with evidence checked after its last change.
    fn tidy_candidate() -> Agent {
        use crate::harness::HarnessKind;
        use crate::ids::AgentId;
        use crate::role::Role;
        use crate::workspace::{Location, Workspace};

        let worktree = "/work/alpha/.claude/worktrees/feat-x";
        Agent {
            id: AgentId::from_ulid(ulid::Ulid::from_parts(1, 1)),
            name: "alpha-feat-x".into(),
            role: Role::Worker,
            harness: HarnessKind::ClaudeCode,
            launched_by: LaunchedBy::Human,
            parent: None,
            mission: None,
            sessions: vec![],
            workspace: Workspace {
                repo: Some("alpha".into()),
                repo_root: Some("/work/alpha".into()),
                worktree: Some(worktree.into()),
                branch: Some("feat/x".into()),
                base: Some("main".into()),
                ..Workspace::at(worktree)
            },
            location: Location::Background,
            state: AgentState::Done,
            state_since: Timestamp::from_millis(1_000),
            detail: None,
            last_activity: None,
            worked: true,
            model: None,
            delivery: Delivery {
                merged_into_base: true,
                checked_at: Some(Timestamp::from_millis(2_000)),
                ..Delivery::default()
            },
            hold: None,
            created_at: Timestamp::from_millis(0),
            updated_at: Timestamp::from_millis(2_000),
        }
    }

    #[test]
    fn a_delivered_volee_worktree_may_be_tidied() {
        assert_eq!(
            tidy_check(&tidy_candidate()),
            Ok(PathBuf::from("/work/alpha/.claude/worktrees/feat-x"))
        );
    }

    #[test]
    fn a_worktree_a_failed_launch_left_untouched_may_be_tidied() {
        let untouched = || Agent {
            state: AgentState::Failed,
            delivery: Delivery {
                checked_at: Some(Timestamp::from_millis(2_000)),
                ..Delivery::default()
            },
            ..tidy_candidate()
        };
        assert!(
            tidy_check(&untouched()).is_ok(),
            "nothing was ever done in it"
        );

        // Anything done in it, and the usual rule applies: not delivered.
        let mut committed = untouched();
        committed.delivery.commits = 1;
        let mut dirty = untouched();
        dirty.delivery.dirty = true;
        let mut ran = untouched();
        ran.sessions.push(crate::agent::SessionLink {
            key: crate::harness::SessionKey::claude("c0ffee00-0000"),
            origin: crate::harness::SessionOrigin::Started,
            at: Timestamp::from_millis(500),
        });
        for agent in [committed, ran] {
            assert_eq!(tidy_check(&agent), Err(vec![TidyRefusal::NotDelivered]));
        }
        assert_eq!(
            tidy_check(&dirty),
            Err(vec![
                TidyRefusal::UncommittedChanges,
                TidyRefusal::NotDelivered
            ])
        );
        // Stale evidence proves nothing, untouched or not.
        let mut stale = untouched();
        stale.state_since = Timestamp::from_millis(3_000);
        assert_eq!(tidy_check(&stale), Err(vec![TidyRefusal::EvidenceStale]));
    }

    #[test]
    fn only_worktrees_volee_created_for_agents_it_launched() {
        let mut found = tidy_candidate();
        found.launched_by = LaunchedBy::Outside;
        assert_eq!(
            tidy_check(&found),
            Err(vec![TidyRefusal::NotLaunchedByVolee])
        );
        for worktree in [
            None,
            Some("/elsewhere/feat-x"),
            Some("/work/alpha/.claude/worktrees"),
            Some("/work/alpha/.claude/worktrees/../../../etc"),
            Some("/work/alpha/src"),
        ] {
            let mut agent = tidy_candidate();
            agent.workspace.worktree = worktree.map(PathBuf::from);
            assert_eq!(
                tidy_check(&agent),
                Err(vec![TidyRefusal::NotAVoleeWorktree]),
                "{worktree:?}"
            );
        }
        let mut rootless = tidy_candidate();
        rootless.workspace.repo_root = None;
        assert_eq!(
            tidy_check(&rootless),
            Err(vec![TidyRefusal::NotAVoleeWorktree])
        );
    }

    #[test]
    fn stale_or_missing_evidence_proves_nothing() {
        let mut stale = tidy_candidate();
        stale.state_since = Timestamp::from_millis(3_000);
        assert_eq!(tidy_check(&stale), Err(vec![TidyRefusal::EvidenceStale]));
        let mut never = tidy_candidate();
        never.delivery = Delivery::default();
        assert_eq!(
            tidy_check(&never),
            Err(vec![TidyRefusal::EvidenceNotChecked])
        );
        never.state = AgentState::Working;
        assert_eq!(
            tidy_check(&never),
            Err(vec![TidyRefusal::Busy, TidyRefusal::EvidenceNotChecked])
        );
    }

    #[test]
    fn every_reason_to_keep_a_worktree_is_given_at_once() {
        let mut agent = tidy_candidate();
        agent.state = AgentState::Working;
        agent.state_since = Timestamp::from_millis(1_500);
        agent.delivery = Delivery {
            dirty: true,
            checked_at: Some(Timestamp::from_millis(2_000)),
            ..Delivery::default()
        };
        assert_eq!(
            tidy_check(&agent),
            Err(vec![
                TidyRefusal::Busy,
                TidyRefusal::UncommittedChanges,
                TidyRefusal::NotDelivered
            ])
        );
    }

    #[test]
    fn refusals_read_as_sentences() {
        assert_eq!(
            TidyRefusal::KeptByHarness {
                detail: "2 unpushed commits".into()
            }
            .to_string(),
            "Claude Code kept its session: 2 unpushed commits"
        );
        let json = serde_json::to_value(TidyRefusal::RefusedByGit {
            detail: "it is locked".into(),
        })
        .unwrap();
        assert_eq!(json["reason"], "refused-by-git");
        assert_eq!(json["detail"], "it is locked");
    }
}
