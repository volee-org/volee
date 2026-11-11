//! The journal: what happened lately, one line each, for the column at the
//! right of the screen. It is built from the events the interface applies:
//! who launched whom, who wrote to whom, who needs you, who finished, who
//! left.

use std::collections::VecDeque;

use volee_core::event::{Event, EventKind};
use volee_core::message::{MessageKind, Party};
use volee_core::state::StateCategory;
use volee_core::time::Timestamp;
use volee_core::{AgentId, AgentState};

/// How many lines the journal keeps.
const KEPT: usize = 200;

/// What a line of the journal says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum What {
    /// `parent` launched `agent`.
    Launched {
        /// Who launched it.
        parent: AgentId,
        /// The agent.
        agent: AgentId,
    },
    /// A session started elsewhere showed up.
    Appeared(AgentId),
    /// A message, from one party to another.
    Message {
        /// Who wrote it.
        from: Party,
        /// Its first recipient.
        to: Party,
        /// A question or anything else.
        question: bool,
    },
    /// An agent changed to a state worth telling.
    State {
        /// The agent.
        agent: AgentId,
        /// Its new state.
        state: AgentState,
    },
    /// An agent left the list.
    Left(AgentId),
}

/// One line of the journal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Line {
    /// When it happened.
    pub(crate) at: Timestamp,
    /// What happened.
    pub(crate) what: What,
}

/// The last lines of the journal, oldest first.
#[derive(Clone, Debug, Default)]
pub(crate) struct Journal {
    lines: VecDeque<Line>,
}

impl Journal {
    /// Takes in an event the registry applied.
    pub(crate) fn record(&mut self, event: &Event) {
        let what = match &event.kind {
            EventKind::AgentRegistered {
                agent,
                parent: Some(parent),
                ..
            } => What::Launched {
                parent: *parent,
                agent: *agent,
            },
            EventKind::AgentRegistered { agent, .. } => What::Appeared(*agent),
            EventKind::MessagePosted { message } => {
                let Some(to) = message.recipients().next() else {
                    return;
                };
                What::Message {
                    from: message.from,
                    to,
                    question: message.kind == MessageKind::Question,
                }
            }
            EventKind::StateChanged { agent, state, .. } if worth_telling(state) => What::State {
                agent: *agent,
                state: state.clone(),
            },
            EventKind::AgentExpired { agent } | EventKind::AgentForgotten { agent } => {
                What::Left(*agent)
            }
            _ => return,
        };
        // Two lines alike in a row say nothing more than one.
        if self
            .lines
            .back()
            .is_some_and(|last| last.what == what && last.at == event.at)
        {
            return;
        }
        self.lines.push_back(Line { at: event.at, what });
        while self.lines.len() > KEPT {
            self.lines.pop_front();
        }
    }

    /// The last `count` lines, oldest first.
    pub(crate) fn last(&self, count: usize) -> impl Iterator<Item = &Line> {
        self.lines
            .iter()
            .skip(self.lines.len().saturating_sub(count))
    }

    /// Whether nothing happened yet.
    pub(crate) fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// The states a person wants to hear of: it needs them, it finished, it
/// failed. Working and resting come and go too often to tell.
fn worth_telling(state: &AgentState) -> bool {
    matches!(
        state.category(),
        StateCategory::NeedsInput | StateCategory::Done | StateCategory::Failed
    )
}

#[cfg(test)]
mod tests {
    use ulid::Ulid;
    use volee_core::harness::HarnessKind;
    use volee_core::role::{LaunchedBy, Role};
    use volee_core::state::InputRequest;
    use volee_core::workspace::{Location, Workspace};

    use super::*;

    fn agent(n: u128) -> AgentId {
        AgentId::from_ulid(Ulid::from_parts(1, n))
    }

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_millis(1_800_000_000_000 + seconds * 1_000)
    }

    fn registered(id: AgentId, parent: Option<AgentId>, seconds: i64) -> Event {
        Event::new(
            at(seconds),
            EventKind::AgentRegistered {
                agent: id,
                name: "a".into(),
                role: Role::Worker,
                harness: HarnessKind::ClaudeCode,
                launched_by: LaunchedBy::Human,
                parent,
                mission: None,
                workspace: Workspace::at("/w"),
                location: Location::Background,
            },
        )
    }

    fn state(id: AgentId, state: AgentState, seconds: i64) -> Event {
        Event::new(
            at(seconds),
            EventKind::StateChanged {
                agent: id,
                state,
                detail: None,
            },
        )
    }

    #[test]
    fn it_tells_launches_needs_ends_and_departures() {
        let mut journal = Journal::default();
        journal.record(&registered(agent(1), None, 0));
        journal.record(&registered(agent(2), Some(agent(1)), 1));
        journal.record(&state(agent(2), AgentState::Working, 2));
        let needs = AgentState::NeedsInput {
            request: InputRequest::Other {
                detail: "a question".into(),
            },
        };
        journal.record(&state(agent(2), needs.clone(), 3));
        journal.record(&state(agent(2), AgentState::Done, 4));
        journal.record(&Event::new(
            at(5),
            EventKind::AgentExpired { agent: agent(2) },
        ));
        let lines: Vec<&What> = journal.last(10).map(|line| &line.what).collect();
        assert_eq!(
            lines,
            [
                &What::Appeared(agent(1)),
                &What::Launched {
                    parent: agent(1),
                    agent: agent(2)
                },
                &What::State {
                    agent: agent(2),
                    state: needs
                },
                &What::State {
                    agent: agent(2),
                    state: AgentState::Done
                },
                &What::Left(agent(2)),
            ],
            "working is not worth a line"
        );
        assert_eq!(journal.last(2).count(), 2);
        assert_eq!(journal.last(2).last().unwrap().what, What::Left(agent(2)));
    }

    #[test]
    fn it_keeps_the_last_lines_only() {
        let mut journal = Journal::default();
        assert!(journal.is_empty());
        for second in 0..250 {
            journal.record(&state(agent(1), AgentState::Done, second));
        }
        assert_eq!(journal.last(usize::MAX).count(), KEPT);
        assert_eq!(journal.last(1).next().unwrap().at, at(249));
    }
}
