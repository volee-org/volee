//! What happened when: the timeline's memory.
//!
//! The registry keeps only the current state of each agent. As the interface
//! applies events, it records here when each agent changed state, sent or
//! received a message, saw its delivery evidence move or got a verdict from
//! its mission's supervisor, so the timeline can draw the last minutes or
//! hours. Activities are left out: the working state already shows them.

use std::collections::HashMap;

use volee_core::delivery::{ChecksState, Delivery, PrState, PushState};
use volee_core::event::{Event, EventKind};
use volee_core::message::{MessageKind, Party};
use volee_core::mission::Verdict;
use volee_core::state::StateCategory;
use volee_core::time::Timestamp;
use volee_core::{AgentId, MessageId};

/// How many state changes, and how many marks, a lane keeps. Older ones are
/// dropped by tenths, so memory stays bounded however long volee runs.
const KEEP: usize = 2_000;

/// What moved in an agent's delivery evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeliveryChange {
    /// The branch now has this many commits of its own.
    Commits(u32),
    /// Every commit reached the remote.
    Pushed,
    /// A pull request was opened, or changed state.
    PullRequest {
        /// Its number.
        number: u64,
        /// Its new state.
        state: PrState,
    },
    /// The pull request's checks changed.
    Checks(ChecksState),
    /// The work is merged into the base branch.
    Merged,
    /// A report was delivered.
    Report,
}

impl DeliveryChange {
    /// What changed between two pieces of evidence, the most important first,
    /// or `None` when nothing a person cares about moved.
    pub(crate) fn between(before: &Delivery, after: &Delivery) -> Option<Self> {
        let merged = |delivery: &Delivery| {
            delivery.merged_into_base
                || delivery
                    .pull_request
                    .as_ref()
                    .is_some_and(|pr| pr.state == PrState::Merged)
        };
        if merged(after) && !merged(before) {
            return Some(Self::Merged);
        }
        let pr = |delivery: &Delivery| {
            delivery
                .pull_request
                .as_ref()
                .map(|pr| (pr.number, pr.state))
        };
        if let Some((number, state)) = pr(after)
            && pr(before) != Some((number, state))
        {
            return Some(Self::PullRequest { number, state });
        }
        let checks = |delivery: &Delivery| delivery.pull_request.as_ref().map(|pr| pr.checks);
        if let Some(state) = checks(after)
            && checks(before) != Some(state)
            && state != ChecksState::None
        {
            return Some(Self::Checks(state));
        }
        if after.push == PushState::UpToDate
            && before.push != PushState::UpToDate
            && after.commits > 0
        {
            return Some(Self::Pushed);
        }
        if after.commits != before.commits && after.commits > 0 {
            return Some(Self::Commits(after.commits));
        }
        if after.report_delivered && !before.report_delivered {
            return Some(Self::Report);
        }
        None
    }

    /// A few words for people: `3 commits`, `PR #42 merged`.
    pub(crate) fn describe(self) -> String {
        match self {
            Self::Commits(1) => "1 commit".to_owned(),
            Self::Commits(commits) => format!("{commits} commits"),
            Self::Pushed => "pushed".to_owned(),
            Self::PullRequest { number, state } => match state {
                PrState::Draft => format!("opened draft PR #{number}"),
                PrState::Open => format!("PR #{number} open"),
                PrState::Merged => format!("PR #{number} merged"),
                PrState::Closed => format!("PR #{number} closed"),
            },
            Self::Checks(state) => match state {
                ChecksState::Passing => "checks passing".to_owned(),
                ChecksState::Failing => "checks failing".to_owned(),
                ChecksState::Pending | ChecksState::None => "checks running".to_owned(),
            },
            Self::Merged => "merged".to_owned(),
            Self::Report => "report delivered".to_owned(),
        }
    }
}

/// Something that happened to an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarkKind {
    /// The agent entered a state.
    State(StateCategory),
    /// The agent sent a message.
    Sent {
        /// The message, to read it in the registry.
        message: MessageId,
        /// What it is for.
        kind: MessageKind,
    },
    /// The agent received a message.
    Received {
        /// The message.
        message: MessageId,
        /// What it is for.
        kind: MessageKind,
    },
    /// Its delivery evidence moved.
    Delivery(DeliveryChange),
    /// Its mission's supervisor judged its work.
    Verdict(Verdict),
}

impl MarkKind {
    /// Which mark wins when several fall in the same column of the
    /// timeline: the higher, the more a person needs to see it.
    pub(crate) const fn priority(self) -> u8 {
        match self {
            Self::State(StateCategory::NeedsInput) => 9,
            Self::State(StateCategory::Failed | StateCategory::Lost) => 8,
            Self::Sent {
                kind: MessageKind::Question,
                ..
            } => 7,
            Self::Delivery(_) => 6,
            Self::Verdict(_) => 5,
            Self::Sent { .. } => 4,
            Self::Received { .. } => 3,
            Self::State(StateCategory::Done) => 2,
            Self::State(_) => 1,
        }
    }
}

/// One thing that happened, and when.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Mark {
    /// When.
    pub(crate) at: Timestamp,
    /// What.
    pub(crate) kind: MarkKind,
}

/// The past of one agent.
#[derive(Clone, Debug, Default)]
pub(crate) struct Lane {
    /// When the agent appeared.
    pub(crate) born: Option<Timestamp>,
    /// When the agent entered each state, oldest first.
    pub(crate) states: Vec<(Timestamp, StateCategory)>,
    /// Everything that happened, oldest first.
    pub(crate) marks: Vec<Mark>,
    /// The last evidence seen, to tell what the next one changes.
    delivery: Delivery,
    /// The last verdict seen.
    verdict: Option<Verdict>,
}

impl Lane {
    /// The state the agent was in at `at`, if it existed then and the
    /// change is still remembered.
    pub(crate) fn state_at(&self, at: Timestamp) -> Option<StateCategory> {
        let index = self.states.partition_point(|(since, _)| *since <= at);
        index.checked_sub(1).map(|index| self.states[index].1)
    }

    fn mark(&mut self, at: Timestamp, kind: MarkKind) {
        self.marks.push(Mark { at, kind });
        trim(&mut self.marks);
    }
}

/// Drops the oldest tenth of `items` once there are too many.
fn trim<T>(items: &mut Vec<T>) {
    if items.len() > KEEP {
        items.drain(..KEEP / 10);
    }
}

/// The past of every agent.
#[derive(Clone, Debug, Default)]
pub(crate) struct History {
    lanes: HashMap<AgentId, Lane>,
    /// How many messages were posted: a cheap way for views to tell that
    /// conversations changed.
    messages: u64,
}

impl History {
    /// Remembers what `event` changed. Called for every event the registry
    /// accepted, in order.
    pub(crate) fn record(&mut self, event: &Event) {
        let at = event.at;
        match &event.kind {
            EventKind::AgentRegistered { agent, .. } => {
                let lane = self.lanes.entry(*agent).or_default();
                lane.born = Some(at);
                lane.states.push((at, StateCategory::Launching));
            }
            EventKind::StateChanged { agent, state, .. } => {
                let lane = self.lanes.entry(*agent).or_default();
                let category = state.category();
                if lane.states.last().map(|(_, last)| *last) != Some(category) {
                    lane.states.push((at, category));
                    trim(&mut lane.states);
                    lane.mark(at, MarkKind::State(category));
                }
            }
            EventKind::MessagePosted { message } => {
                self.messages += 1;
                if let Party::Agent(from) = message.from {
                    self.lanes.entry(from).or_default().mark(
                        at,
                        MarkKind::Sent {
                            message: message.id,
                            kind: message.kind,
                        },
                    );
                }
                for to in message.recipients() {
                    if let Party::Agent(to) = to {
                        self.lanes.entry(to).or_default().mark(
                            at,
                            MarkKind::Received {
                                message: message.id,
                                kind: message.kind,
                            },
                        );
                    }
                }
            }
            EventKind::DeliveryUpdated { agent, delivery } => {
                let lane = self.lanes.entry(*agent).or_default();
                if let Some(change) = DeliveryChange::between(&lane.delivery, delivery) {
                    lane.mark(at, MarkKind::Delivery(change));
                }
                lane.delivery.clone_from(delivery);
            }
            EventKind::WorkerAssessed {
                worker, verdict, ..
            } => {
                let lane = self.lanes.entry(*worker).or_default();
                if lane.verdict != Some(*verdict) {
                    lane.verdict = Some(*verdict);
                    lane.mark(at, MarkKind::Verdict(*verdict));
                }
            }
            kind => {
                if let Some(agent) = kind.removed_agent() {
                    self.lanes.remove(&agent);
                }
            }
        }
    }

    /// The past of an agent.
    pub(crate) fn lane(&self, agent: AgentId) -> Option<&Lane> {
        self.lanes.get(&agent)
    }

    /// How many messages were posted so far.
    pub(crate) fn messages(&self) -> u64 {
        self.messages
    }
}

#[cfg(test)]
mod tests {
    use ulid::Ulid;
    use volee_core::delivery::PullRequest;
    use volee_core::state::AgentState;

    use super::*;

    fn agent() -> AgentId {
        AgentId::from_ulid(Ulid::from_parts(1, 1))
    }

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_millis(1_800_000_000_000 + seconds * 1_000)
    }

    fn state(seconds: i64, state: AgentState) -> Event {
        Event::new(
            at(seconds),
            EventKind::StateChanged {
                agent: agent(),
                state,
                detail: None,
            },
        )
    }

    #[test]
    fn states_are_remembered_once_per_change() {
        let mut history = History::default();
        history.record(&state(0, AgentState::Working));
        history.record(&state(5, AgentState::Working));
        history.record(&state(9, AgentState::Idle));
        let lane = history.lane(agent()).unwrap();
        assert_eq!(
            lane.states,
            [
                (at(0), StateCategory::Working),
                (at(9), StateCategory::Idle)
            ]
        );
        assert_eq!(lane.state_at(at(-1)), None);
        assert_eq!(lane.state_at(at(4)), Some(StateCategory::Working));
        assert_eq!(lane.state_at(at(9)), Some(StateCategory::Idle));
        assert_eq!(lane.marks.len(), 2);
    }

    #[test]
    fn delivery_marks_name_what_moved() {
        let pr = |state, checks| PullRequest {
            number: 42,
            url: String::new(),
            state,
            checks,
            head: None,
        };
        let before = Delivery {
            commits: 2,
            ..Delivery::default()
        };
        let pushed = Delivery {
            push: PushState::UpToDate,
            ..before.clone()
        };
        assert_eq!(
            DeliveryChange::between(&before, &pushed),
            Some(DeliveryChange::Pushed)
        );
        let open = Delivery {
            pull_request: Some(pr(PrState::Open, ChecksState::Pending)),
            ..pushed.clone()
        };
        assert_eq!(
            DeliveryChange::between(&pushed, &open),
            Some(DeliveryChange::PullRequest {
                number: 42,
                state: PrState::Open
            })
        );
        let passing = Delivery {
            pull_request: Some(pr(PrState::Open, ChecksState::Passing)),
            ..pushed.clone()
        };
        assert_eq!(
            DeliveryChange::between(&open, &passing),
            Some(DeliveryChange::Checks(ChecksState::Passing))
        );
        let merged = Delivery {
            pull_request: Some(pr(PrState::Merged, ChecksState::Passing)),
            ..pushed
        };
        assert_eq!(
            DeliveryChange::between(&passing, &merged),
            Some(DeliveryChange::Merged)
        );
        assert_eq!(DeliveryChange::between(&merged, &merged), None);
        assert_eq!(DeliveryChange::Merged.describe(), "merged");
        assert_eq!(DeliveryChange::Commits(1).describe(), "1 commit");
    }

    #[test]
    fn lanes_stay_bounded() {
        let mut history = History::default();
        for second in 0..5_000 {
            let next = if second % 2 == 0 {
                AgentState::Working
            } else {
                AgentState::Idle
            };
            history.record(&state(second, next));
        }
        let lane = history.lane(agent()).unwrap();
        assert!(lane.states.len() <= KEEP);
        assert!(lane.marks.len() <= KEEP);
        assert_eq!(lane.states.last().unwrap().0, at(4_999));
        history.record(&Event::new(
            at(5_000),
            EventKind::AgentForgotten { agent: agent() },
        ));
        assert!(history.lane(agent()).is_none());
    }
}
