//! The decision inbox: what the web interface shows, built from the agents
//! view.
//!
//! A card is an agent that needs the person: either it **waits** (for a
//! permission, an answer, a dialog) or it has **finished** work to read. The
//! cards come sorted by urgency, so the interface does not decide anything.
//! Every other agent is only counted.

use crate::decisions::DecisionCard;
use serde::Serialize;
use volee_core::state::{AgentState, InputRequest, StateCategory};
use volee_proto::methods::{AgentCard, AgentsView};

/// What a card asks of the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Kind {
    /// The agent is blocked until the person answers.
    Waiting,
    /// The agent stopped and left commits to read.
    Finished,
}

/// The files and lines a branch changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Diff {
    pub(crate) files: u32,
    pub(crate) insertions: u32,
    pub(crate) deletions: u32,
}

/// One card of the inbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct InboxCard {
    /// The Claude Code session id.
    pub(crate) id: String,
    pub(crate) kind: Kind,
    pub(crate) name: Option<String>,
    /// Claude Code's state, in words (`needs you`, `idle`...).
    pub(crate) state: &'static str,
    pub(crate) waiting_for: Option<String>,
    /// Since when the agent waits or has been finished, in milliseconds since
    /// the Unix epoch, when known.
    pub(crate) since: Option<i64>,
    pub(crate) repo: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) base: Option<String>,
    /// How `base` is known: `upstream`, `reflog`, `nearest` or `default` (a
    /// guess, the weakest).
    pub(crate) base_source: Option<String>,
    pub(crate) cwd: String,
    pub(crate) diff: Option<Diff>,
    pub(crate) ahead: u32,
    pub(crate) dirty: bool,
    /// What the AI pre-review says of the branch's current tip, in short.
    /// Filled by the web layer, which knows the reviews; `None` when nobody
    /// asked for one or reviews are off.
    pub(crate) review: Option<ReviewBrief>,
    /// How much care the rules say the change needs, and what the person
    /// set by hand. Filled by the web layer; finished work only.
    pub(crate) risk: Option<crate::assess::RiskView>,
}

/// The pre-review of a card in a few words: enough for a badge, and for the
/// interface to know when to read the whole review again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ReviewBrief {
    /// `pending`, `running`, `done` or `failed`.
    pub(crate) status: String,
    /// How many findings it has.
    pub(crate) findings: usize,
    /// The reviewed commit.
    pub(crate) tip: String,
}

/// The inbox as the interface receives it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct Inbox {
    pub(crate) updated_at: Option<i64>,
    pub(crate) error: Option<String>,
    /// The terminal adapter that can open an agent's terminal, when there is
    /// one. The interface hides the button without it.
    pub(crate) terminal: Option<&'static str>,
    /// Running agents that need no decision now.
    pub(crate) others: usize,
    /// Held permission requests and questions, oldest first: the interface
    /// puts them at the top of "Waiting for you".
    pub(crate) decisions: Vec<DecisionView>,
    pub(crate) cards: Vec<InboxCard>,
    /// What is wrong with the rules files, shown as a discreet banner.
    pub(crate) rules: RulesStatus,
    /// What was done on finished changes (merges, fix requests, comments,
    /// editor opens), newest first.
    pub(crate) journal: Vec<volee_store::JournalEntry>,
}

/// The state of the rules files, for the banner.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct RulesStatus {
    /// Files that fail: each says file, line and message; while any does,
    /// every answer is the safest one.
    pub(crate) errors: Vec<serde_json::Value>,
    /// Probable typos.
    pub(crate) warnings: Vec<String>,
}

/// A held request with the agent it comes from.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct DecisionView {
    #[serde(flatten)]
    pub(crate) card: DecisionCard,
    pub(crate) name: Option<String>,
    pub(crate) repo: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) base: Option<String>,
    /// How the base is known (`upstream`, `reflog`, `nearest`, `default`).
    pub(crate) base_source: Option<String>,
    pub(crate) cwd: Option<String>,
}

/// How much a waiting card is worth acting on first: a permission blocks a
/// tool, a question blocks the plan, the rest is vaguer.
fn urgency(request: &InputRequest) -> u8 {
    match request {
        InputRequest::Permission { .. } => 0,
        InputRequest::Question => 1,
        InputRequest::Dialog => 2,
        InputRequest::Other { .. } => 3,
    }
}

/// Builds the inbox from the agents view. `terminal` names the adapter that
/// can open a terminal, if any.
#[cfg(test)]
pub(crate) fn build(view: &AgentsView, terminal: Option<&'static str>) -> Inbox {
    build_with(view, terminal, &[])
}

/// Builds the inbox with the held requests. An agent that has one is not
/// listed again as a plain waiting card: the held request says more.
pub(crate) fn build_with(
    view: &AgentsView,
    terminal: Option<&'static str>,
    held: &[DecisionCard],
) -> Inbox {
    let mut keyed: Vec<((u8, i64), InboxCard)> = Vec::new();
    let mut others = 0;
    for agent in &view.agents {
        let has_held = held.iter().any(|card| card.session_id == agent.session_id);
        match classify(agent).filter(|(kind, _)| !(has_held && *kind == Kind::Waiting)) {
            Some((kind, rank)) => {
                let card = card(agent, kind);
                // The agent that waited longest comes first; an unknown time
                // goes last, since nothing says it waits long.
                let since = card.since.unwrap_or(i64::MAX);
                keyed.push(((rank, since), card));
            }
            None if has_held => {}
            None if agent.alive && agent.state.category() != StateCategory::Stopped => others += 1,
            None => {}
        }
    }
    keyed.sort_by(|(a, x), (b, y)| a.cmp(b).then_with(|| x.id.cmp(&y.id)));
    Inbox {
        updated_at: view.updated_at.map(volee_core::time::Timestamp::as_millis),
        error: view.error.clone(),
        terminal,
        others,
        decisions: held.iter().map(|card| decision_view(view, card)).collect(),
        cards: keyed.into_iter().map(|(_, card)| card).collect(),
        rules: RulesStatus::default(),
        journal: Vec::new(),
    }
}

fn decision_view(view: &AgentsView, card: &DecisionCard) -> DecisionView {
    let agent = view
        .agents
        .iter()
        .find(|agent| agent.session_id == card.session_id);
    let repo = agent.and_then(|agent| agent.repo.as_ref());
    DecisionView {
        card: card.clone(),
        name: agent.and_then(|agent| agent.name.clone()),
        repo: repo.map(|repo| repo.name.clone()),
        branch: repo.and_then(|repo| repo.branch.clone()),
        base: repo.and_then(|repo| repo.base.clone()),
        base_source: repo.and_then(|repo| repo.base_source.clone()),
        cwd: agent.map(|agent| agent.cwd.display().to_string()),
    }
}

/// Whether the agent is a card, and how urgent.
fn classify(agent: &AgentCard) -> Option<(Kind, u8)> {
    if !agent.alive {
        return None;
    }
    if let AgentState::NeedsInput { request } = &agent.state {
        return Some((Kind::Waiting, urgency(request)));
    }
    // Waiting cards use ranks 0 to 3, finished work comes after all of them.
    agent.finished_work.then_some((Kind::Finished, 4))
}

fn card(agent: &AgentCard, kind: Kind) -> InboxCard {
    let since = match kind {
        Kind::Waiting => agent.hooks.last_event_at,
        Kind::Finished => agent.hooks.activity.last_stop_at,
    };
    InboxCard {
        id: agent.session_id.clone(),
        kind,
        name: agent.name.clone(),
        state: agent.state.label(),
        waiting_for: agent.waiting_for.clone(),
        since,
        repo: agent.repo.as_ref().map(|repo| repo.name.clone()),
        branch: agent.repo.as_ref().and_then(|repo| repo.branch.clone()),
        base: agent.repo.as_ref().and_then(|repo| repo.base.clone()),
        base_source: agent
            .repo
            .as_ref()
            .and_then(|repo| repo.base_source.clone()),
        cwd: agent.cwd.display().to_string(),
        diff: agent.git.as_ref().map(|git| Diff {
            files: git.diff.files,
            insertions: git.diff.insertions,
            deletions: git.diff.deletions,
        }),
        ahead: agent.git.as_ref().map_or(0, |git| git.ahead),
        dirty: agent.git.as_ref().is_some_and(|git| git.dirty),
        review: None,
        risk: None,
    }
}

#[cfg(test)]
mod tests {
    use volee_core::delivery::{DiffStat, PushState};
    use volee_core::finished::Activity;
    use volee_proto::methods::{GitFacts, HookFacts, RepoFacts};

    use super::*;

    fn agent(id: &str, state: AgentState) -> AgentCard {
        AgentCard {
            session_id: id.to_owned(),
            name: Some(id.to_owned()),
            background: false,
            alive: true,
            pid: Some(1),
            cwd: "/work/app".into(),
            started_at: None,
            waiting_for: None,
            state,
            repo: Some(RepoFacts {
                name: "app".to_owned(),
                root: "/work/app".into(),
                worktree: None,
                branch: Some("feat/x".to_owned()),
                base: Some("main".to_owned()),
                base_source: None,
            }),
            git: Some(GitFacts {
                ahead: 2,
                unique: 2,
                behind: 0,
                dirty: false,
                merged: false,
                diff: DiffStat {
                    files: 3,
                    insertions: 40,
                    deletions: 5,
                },
                push: PushState::default(),
                merge_source: None,
                merge_unknown: None,
                pull_request: None,
            }),
            git_error: None,
            hooks: HookFacts::default(),
            finished_work: false,
        }
    }

    fn waiting(id: &str, request: InputRequest, at: i64) -> AgentCard {
        let mut card = agent(id, AgentState::NeedsInput { request });
        card.hooks.last_event_at = Some(at);
        card
    }

    fn finished(id: &str, stopped_at: i64) -> AgentCard {
        let mut card = agent(id, AgentState::Idle);
        card.finished_work = true;
        card.hooks.activity = Activity {
            last_activity_at: Some(stopped_at - 10),
            last_stop_at: Some(stopped_at),
        };
        card
    }

    fn view(agents: Vec<AgentCard>) -> AgentsView {
        AgentsView {
            updated_at: None,
            error: None,
            agents,
        }
    }

    #[test]
    fn waiting_agents_come_before_finished_work_and_the_longest_wait_first() {
        let inbox = build(
            &view(vec![
                finished("done-old", 100),
                waiting("question", InputRequest::Question, 50),
                waiting("perm-new", InputRequest::Permission { tool: None }, 300),
                waiting("perm-old", InputRequest::Permission { tool: None }, 200),
                finished("done-new", 500),
            ]),
            None,
        );
        let ids: Vec<&str> = inbox.cards.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            ["perm-old", "perm-new", "question", "done-old", "done-new"]
        );
        assert_eq!(inbox.cards[0].kind, Kind::Waiting);
        assert_eq!(inbox.cards[3].kind, Kind::Finished);
    }

    #[test]
    fn a_card_carries_the_repository_the_branch_and_the_diff() {
        let inbox = build(&view(vec![finished("a", 100)]), Some("rex"));
        let card = &inbox.cards[0];
        assert_eq!(card.repo.as_deref(), Some("app"));
        assert_eq!(card.branch.as_deref(), Some("feat/x"));
        assert_eq!(card.base.as_deref(), Some("main"));
        assert_eq!(card.since, Some(100));
        assert_eq!(card.ahead, 2);
        assert_eq!(
            card.diff,
            Some(Diff {
                files: 3,
                insertions: 40,
                deletions: 5
            })
        );
        assert_eq!(inbox.terminal, Some("rex"));
    }

    #[test]
    fn a_waiting_agent_says_what_it_waits_for_and_since_when() {
        let mut blocked = waiting("a", InputRequest::Permission { tool: None }, 42);
        blocked.waiting_for = Some("permission for Bash".to_owned());
        let inbox = build(&view(vec![blocked]), None);
        assert_eq!(
            inbox.cards[0].waiting_for.as_deref(),
            Some("permission for Bash")
        );
        assert_eq!(inbox.cards[0].since, Some(42));
        assert_eq!(inbox.cards[0].state, "needs you");
    }

    #[test]
    fn other_running_agents_are_counted_not_listed() {
        let mut ended = agent("ended", AgentState::Idle);
        ended.alive = false;
        let inbox = build(
            &view(vec![
                agent("busy", AgentState::Working),
                agent("idle", AgentState::Idle),
                ended,
                finished("f", 1),
            ]),
            None,
        );
        assert_eq!(inbox.cards.len(), 1);
        assert_eq!(inbox.others, 2);
    }

    #[test]
    fn an_ended_agent_never_makes_a_card() {
        let mut ended = waiting("x", InputRequest::Question, 1);
        ended.alive = false;
        assert_eq!(build(&view(vec![ended]), None).cards.len(), 0);
    }

    #[test]
    fn an_unknown_time_goes_last_within_its_group() {
        let mut unknown = waiting("unknown", InputRequest::Question, 0);
        unknown.hooks.last_event_at = None;
        let inbox = build(
            &view(vec![unknown, waiting("known", InputRequest::Question, 9)]),
            None,
        );
        assert_eq!(inbox.cards[0].id, "known");
    }

    #[test]
    fn the_error_and_the_time_of_the_reading_are_passed_on() {
        let mut v = view(vec![]);
        v.error = Some("cannot list sessions".to_owned());
        v.updated_at = Some(volee_core::time::Timestamp::from_millis(7));
        let inbox = build(&v, None);
        assert_eq!(inbox.error.as_deref(), Some("cannot list sessions"));
        assert_eq!(inbox.updated_at, Some(7));
    }
}
