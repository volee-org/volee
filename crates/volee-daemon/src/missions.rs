//! Missions in the daemon (ADR 0007).
//!
//! After every commit, the daemon finds the missions the new events concern
//! and runs the supervisor's rules on them. Its decisions are committed in
//! turn: verdicts, status changes, and notices posted by volee in the
//! mission's thread. Notices are ordinary messages, so they wake an idle
//! coordinator like any other message.
//!
//! Follow-ups are derived from events that are already committed, so a
//! failure here never fails the request that caused them: it is logged, and
//! the next commit or the next start catches up.

use std::collections::BTreeSet;

use volee_core::message::{Message, Party, Receipt};
use volee_core::role::Role;
use volee_core::supervisor::{Notice, supervise};
use volee_core::{Event, EventKind, MissionId, Registry};
use volee_proto::methods::SeqEvent;

use crate::error::DaemonError;
use crate::state::State;

/// Supervises the missions concerned by `committed` and commits the result.
pub(crate) fn follow_up(state: &mut State, committed: &[SeqEvent]) -> Vec<SeqEvent> {
    let missions = affected_missions(state.registry(), committed);
    run(state, &missions)
}

/// Supervises every open mission. The daemon calls it once at start, so
/// that nothing is missed if it stopped between a commit and its follow-ups,
/// or if the supervisor's rules changed since.
pub(crate) fn catch_up(state: &mut State) -> Vec<SeqEvent> {
    let missions = state
        .registry()
        .missions()
        .filter(|mission| !mission.status.is_over())
        .map(|mission| mission.id)
        .collect();
    run(state, &missions)
}

fn run(state: &mut State, missions: &BTreeSet<MissionId>) -> Vec<SeqEvent> {
    if missions.is_empty() {
        return Vec::new();
    }
    match supervise_all(state, missions) {
        Ok(committed) => committed,
        Err(error) => {
            tracing::error!(%error, ?missions, "cannot apply the mission supervisor's decision");
            Vec::new()
        }
    }
}

fn supervise_all(
    state: &mut State,
    missions: &BTreeSet<MissionId>,
) -> Result<Vec<SeqEvent>, DaemonError> {
    let mut committed = Vec::new();
    let now = state.now();

    // Coordinators first, so that the notices below already reach them.
    let links: Vec<Event> = missions
        .iter()
        .filter_map(|mission| link_coordinator(state.registry(), *mission))
        .map(|kind| Event::new(now, kind))
        .collect();
    if !links.is_empty() {
        committed.extend(state.commit_only(links)?);
    }

    let mut events = Vec::new();
    for &mission in missions {
        let decision = supervise(state.registry(), mission);
        events.extend(
            decision
                .events
                .into_iter()
                .map(|kind| Event::new(now, kind)),
        );
        for notice in decision.notices {
            if let Some(message) = notice_message(state, mission, notice) {
                events.push(Event::new(now, EventKind::MessagePosted { message }));
            }
        }
    }
    if !events.is_empty() {
        committed.extend(state.commit_only(events)?);
    }
    Ok(committed)
}

/// The missions whose supervision may change after these events. The
/// supervisor's own output (verdicts, status updates, notices from volee) is
/// left out, so its decisions do not feed back into themselves.
fn affected_missions(registry: &Registry, events: &[SeqEvent]) -> BTreeSet<MissionId> {
    let mission_of_agent = |agent| registry.agent(agent).and_then(|a| a.mission);
    let mut missions = BTreeSet::new();
    for SeqEvent { event, .. } in events {
        let mission = match &event.kind {
            EventKind::MissionCreated { mission } => Some(mission.id),
            EventKind::AgentRegistered { mission, .. } => *mission,
            EventKind::StateChanged { agent, .. } | EventKind::DeliveryUpdated { agent, .. } => {
                mission_of_agent(*agent)
            }
            EventKind::MessagePosted { message } => match message.from {
                Party::Agent(agent) => mission_of_agent(agent),
                Party::Human | Party::System => None,
            },
            _ => None,
        };
        missions.extend(mission);
    }
    missions
}

/// Records the agent registered as the mission's coordinator, once.
fn link_coordinator(registry: &Registry, mission: MissionId) -> Option<EventKind> {
    let record = registry.mission(mission)?;
    if record.coordinator.is_some() || record.status.is_over() {
        return None;
    }
    let coordinator = registry
        .mission_agents(mission)
        .into_iter()
        .find(|agent| agent.role == Role::Coordinator)?;
    Some(EventKind::MissionUpdated {
        mission,
        status: record.status,
        coordinator: Some(coordinator.id),
        report: None,
    })
}

/// A supervisor notice as a message from volee in the mission's thread.
fn notice_message(state: &State, mission: MissionId, notice: Notice) -> Option<Message> {
    let thread = state.registry().mission(mission)?.thread;
    if notice.to.is_empty() {
        return None;
    }
    Some(Message {
        id: state.new_message_id(),
        thread,
        from: Party::System,
        kind: notice.kind,
        body: notice.body,
        refs: Vec::new(),
        in_reply_to: None,
        at: state.now(),
        receipts: notice
            .to
            .into_iter()
            .map(|to| Receipt {
                to,
                delivered_at: None,
                read_at: None,
            })
            .collect(),
    })
}
