//! The daemon's state: the registry, its log, subscribers and waiters.
//!
//! Every change goes through [`State::commit`]: the events are applied to a
//! copy of the registry first, so a batch is all or nothing; then they are
//! persisted, then the copy replaces the registry, then subscribers are
//! notified. Callers hold the state lock for the whole commit, so
//! subscribers see events in log order without gaps.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, oneshot};
use ulid::Ulid;
use volee_core::harness::SessionOrigin;
use volee_core::ids::{MessageId, MissionId, ThreadId};
use volee_core::message::Party;
use volee_core::time::Timestamp;
use volee_core::{AgentId, Event, EventKind, Registry};
use volee_harness::hooks::format_messages_for_agent;
use volee_proto::methods::SeqEvent;

use crate::clock::Clock;
use crate::error::DaemonError;
use crate::log::EventLog;

/// How many events a slow subscriber may lag behind before it is dropped.
const BUS_CAPACITY: usize = 4_096;

struct Waiter {
    token: u64,
    sender: oneshot::Sender<String>,
}

/// Mutable daemon state, behind one lock.
pub(crate) struct State {
    registry: Registry,
    log: Box<dyn EventLog>,
    bus: broadcast::Sender<SeqEvent>,
    waiters: HashMap<AgentId, Waiter>,
    next_token: u64,
    clock: Arc<dyn Clock>,
    started_at: Timestamp,
    /// The last identifier handed out, so that the next one sorts after it.
    last_id: Cell<Ulid>,
    /// Sessions the daemon started or resumed and no event linked yet, by
    /// the harness's handle, with the agent they belong to.
    expected: HashMap<String, (AgentId, SessionOrigin)>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("agents", &self.registry.agents().count())
            .field("waiters", &self.waiters.len())
            .finish_non_exhaustive()
    }
}

impl State {
    /// Replays the log into a fresh registry.
    pub(crate) fn load(log: Box<dyn EventLog>, clock: Arc<dyn Clock>) -> Result<Self, DaemonError> {
        let mut registry = Registry::new();
        for (seq, event) in log.since(None)? {
            registry
                .apply(&event)
                .map_err(|source| DaemonError::Corrupted { seq, source })?;
        }
        let (bus, _) = broadcast::channel(BUS_CAPACITY);
        let started_at = clock.now();
        Ok(Self {
            registry,
            log,
            bus,
            waiters: HashMap::new(),
            next_token: 0,
            last_id: Cell::new(Ulid::nil()),
            expected: HashMap::new(),
            clock,
            started_at,
        })
    }

    pub(crate) fn registry(&self) -> &Registry {
        &self.registry
    }

    pub(crate) fn now(&self) -> Timestamp {
        self.clock.now()
    }

    pub(crate) fn started_at(&self) -> Timestamp {
        self.started_at
    }

    pub(crate) fn last_seq(&self) -> Result<Option<u64>, DaemonError> {
        Ok(self.log.last_seq()?)
    }

    pub(crate) fn since(&self, after: Option<u64>) -> Result<Vec<SeqEvent>, DaemonError> {
        Ok(self
            .log
            .since(after)?
            .into_iter()
            .map(|(seq, event)| SeqEvent { seq, event })
            .collect())
    }

    /// A receiver of every event committed from now on. Take it while holding
    /// the state lock, together with a backlog from [`State::since`], and the
    /// subscriber misses nothing.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<SeqEvent> {
        self.bus.subscribe()
    }

    /// A new message identifier, timestamped by the daemon's clock so that
    /// identifiers sort like the events that create them.
    pub(crate) fn new_message_id(&self) -> MessageId {
        MessageId::from_ulid(self.new_ulid())
    }

    /// A new thread identifier.
    pub(crate) fn new_thread_id(&self) -> ThreadId {
        ThreadId::from_ulid(self.new_ulid())
    }

    /// Remembers that the session the harness calls `handle` belongs to
    /// `agent`, until a hook or a listing links it.
    pub(crate) fn expect_session(&mut self, handle: String, agent: AgentId, origin: SessionOrigin) {
        self.expected.insert(handle, (agent, origin));
    }

    /// The agent waiting for the session the harness calls `handle`.
    pub(crate) fn take_expected(&mut self, handle: &str) -> Option<(AgentId, SessionOrigin)> {
        self.expected.remove(handle)
    }

    /// A new agent identifier.
    pub(crate) fn new_agent_id(&self) -> AgentId {
        AgentId::from_ulid(self.new_ulid())
    }

    /// A new mission identifier.
    pub(crate) fn new_mission_id(&self) -> MissionId {
        MissionId::from_ulid(self.new_ulid())
    }

    /// A new identifier, timestamped by the daemon's clock and strictly
    /// greater than every identifier handed out before, even within one
    /// millisecond: identifiers sort in the order the daemon created them.
    fn new_ulid(&self) -> Ulid {
        let millis = u64::try_from(self.now().as_millis()).unwrap_or(0);
        let candidate = Ulid::from_parts(millis, Ulid::generate().random());
        let last = self.last_id.get();
        let next = if candidate > last {
            candidate
        } else {
            // The clock did not move (or went back): count up from the last
            // one. An overflow carries into the next millisecond, still up.
            last.increment().unwrap_or_else(|carried| carried)
        };
        self.last_id.set(next);
        next
    }

    /// Validates, persists, applies and publishes events, runs the mission
    /// supervisor on what they changed, then hands new messages to agents
    /// that wait for them.
    pub(crate) fn commit(&mut self, events: Vec<Event>) -> Result<Vec<SeqEvent>, DaemonError> {
        let mut committed = self.commit_only(events)?;
        let follow_ups = crate::missions::follow_up(self, &committed);
        committed.extend(follow_ups);
        committed.extend(self.wake_waiters()?);
        Ok(committed)
    }

    /// Like [`State::commit`], without follow-ups or wake-ups.
    pub(crate) fn commit_only(&mut self, events: Vec<Event>) -> Result<Vec<SeqEvent>, DaemonError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let mut next = self.registry.clone();
        for event in &events {
            next.apply(event)?;
        }
        let seqs = self.log.append(&events)?;
        self.registry = next;
        let committed: Vec<SeqEvent> = seqs
            .into_iter()
            .zip(events)
            .map(|(seq, event)| SeqEvent { seq, event })
            .collect();
        for event in &committed {
            // No subscriber is not an error.
            let _ = self.bus.send(event.clone());
        }
        Ok(committed)
    }

    /// Renders an agent's pending messages and marks them delivered. Returns
    /// `None` when nothing is pending.
    pub(crate) fn take_pending(&mut self, agent: AgentId) -> Result<Option<String>, DaemonError> {
        let Some((text, receipts)) = self.render_pending(agent) else {
            return Ok(None);
        };
        self.commit_only(receipts)?;
        Ok(Some(text))
    }

    /// The text that hands `agent` its pending messages, and the receipts of
    /// the messages it shows. Messages past the text's size limit stay
    /// pending; the text tells the agent to read them with `read_messages`.
    fn render_pending(&self, agent: AgentId) -> Option<(String, Vec<Event>)> {
        let pending = self.registry.pending_for(agent);
        if pending.is_empty() {
            return None;
        }
        let rendered = format_messages_for_agent(pending.iter().copied(), |id| {
            self.registry.agent(id).map(|sender| sender.name.clone())
        });
        let now = self.now();
        let receipts = pending
            .iter()
            .take(rendered.shown)
            .map(|message| {
                Event::new(
                    now,
                    EventKind::MessageDelivered {
                        message: message.id,
                        to: Party::Agent(agent),
                    },
                )
            })
            .collect();
        Some((rendered.text, receipts))
    }

    /// Registers the single waiter of an idle agent. A previous waiter of the
    /// same agent is dropped, which tells it that it was replaced.
    pub(crate) fn register_waiter(&mut self, agent: AgentId) -> (u64, oneshot::Receiver<String>) {
        self.next_token += 1;
        let (sender, receiver) = oneshot::channel();
        self.waiters.insert(
            agent,
            Waiter {
                token: self.next_token,
                sender,
            },
        );
        (self.next_token, receiver)
    }

    /// Removes a waiter that gave up, unless another one replaced it.
    pub(crate) fn cancel_waiter(&mut self, agent: AgentId, token: u64) {
        if self.waiters.get(&agent).is_some_and(|w| w.token == token) {
            self.waiters.remove(&agent);
        }
    }

    fn wake_waiters(&mut self) -> Result<Vec<SeqEvent>, DaemonError> {
        let ready: Vec<AgentId> = self
            .waiters
            .keys()
            .copied()
            .filter(|agent| !self.registry.pending_for(*agent).is_empty())
            .collect();
        let mut committed = Vec::new();
        for agent in ready {
            let Some(waiter) = self.waiters.remove(&agent) else {
                continue;
            };
            if waiter.sender.is_closed() {
                continue;
            }
            let Some((text, receipts)) = self.render_pending(agent) else {
                continue;
            };
            if waiter.sender.send(text).is_ok() {
                committed.extend(self.commit_only(receipts)?);
            }
        }
        Ok(committed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use crate::log::MemoryLog;

    #[test]
    fn identifiers_sort_in_creation_order_within_a_millisecond() {
        let state = State::load(
            Box::new(MemoryLog::with(vec![])),
            Arc::new(ManualClock::at(1_000)),
        )
        .unwrap();
        let ids: Vec<MessageId> = (0..1_000).map(|_| state.new_message_id()).collect();
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        let thread = state.new_thread_id().to_string();
        let last = ids.last().unwrap().to_string();
        // Different kinds share one sequence, after their prefix.
        assert!(thread["th_".len()..] > last["msg_".len()..]);
    }
}
