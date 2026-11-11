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
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use volee_core::message::{Message, MessageKind, Party, Receipt};
use volee_core::role::Role;
use volee_core::supervisor::{Notice, supervise};
use volee_core::{AgentId, Event, EventKind, MissionId, Registry};
use volee_proto::methods::{AgentParams, SeqEvent, TidyOutcome, TidyParams};

use crate::error::DaemonError;
use crate::server::Shared;
use crate::state::State;

/// How long the wind-down waits for the delivery evidence of the workers.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the wind-down lets the coordinator finish the turn in which it
/// closed its mission.
const LAST_TURN: Duration = Duration::from_secs(90);

/// How long a stopped coordinator has to be seen stopped.
const STOPPING: Duration = Duration::from_secs(20);

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
                // The person's answer to the report a coordinator presented
                // is what its mission waits for.
                Party::Human => message.in_reply_to.and_then(|question| {
                    registry
                        .missions()
                        .find(|mission| mission.report == Some(question))
                        .map(|mission| mission.id)
                }),
                Party::System => None,
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

/// What closing a mission leaves to do, in one gesture: every worker whose
/// work landed is tidied (its worktree removed, its branch kept, the agent
/// forgotten), the coordinator is stopped once its turn ended and forgotten,
/// and the person is told what was tidied and what was kept, with the
/// reason. Nothing is forced: a worker that is kept stays as it is.
///
/// `by_coordinator` says that the coordinator itself closed the mission: it
/// is in the middle of the turn that did, and is left to end it.
pub(crate) async fn wind_down(shared: Arc<Shared>, mission: MissionId, by_coordinator: bool) {
    let (workers, coordinator) = {
        let state = shared.state.lock().await;
        let registry = state.registry();
        let agents = registry.mission_agents(mission);
        let workers: Vec<(AgentId, String)> = agents
            .iter()
            .filter(|agent| agent.role != Role::Coordinator)
            .map(|agent| (agent.id, agent.name.clone()))
            .collect();
        let coordinator = agents
            .iter()
            .find(|agent| agent.role == Role::Coordinator)
            .map(|agent| (agent.id, agent.name.clone()));
        (workers, coordinator)
    };
    let ids: Vec<AgentId> = workers.iter().map(|(id, _)| *id).collect();
    crate::evidence::check_now(&shared, &ids, CHECK_TIMEOUT).await;

    let mut tidied = Vec::new();
    let mut kept = Vec::new();
    for (agent, name) in workers {
        let params = TidyParams {
            agent,
            dry_run: false,
        };
        match crate::tidy::tidy(&shared, params).await {
            Ok(TidyOutcome::Removed { .. }) => tidied.push(name),
            Ok(TidyOutcome::Kept { reasons }) => {
                let why: Vec<String> = reasons.iter().map(ToString::to_string).collect();
                kept.push(format!("{name}: {}", why.join("; ")));
            }
            Ok(TidyOutcome::WouldRemove { .. }) => {}
            Err(error) => kept.push(format!("{name}: {error}")),
        }
    }
    if let Some((agent, name)) = coordinator
        && !retire(&shared, agent, by_coordinator).await
    {
        kept.push(format!(
            "{name}: it still runs; stop it with `volee stop {name}`"
        ));
    }
    if let Err(error) = say_what_was_tidied(&shared, mission, &tidied, &kept).await {
        tracing::warn!(%error, %mission, "cannot say what closing the mission tidied");
    }
}

/// Stops the coordinator of a closed mission, then forgets it. Returns
/// whether it is gone. When it closed the mission itself (`finishing`), the
/// turn in which it did is left to end first: its last words are its own.
async fn retire(shared: &Arc<Shared>, agent: AgentId, finishing: bool) -> bool {
    if finishing {
        settled(shared, agent, |state| !state.is_busy(), LAST_TURN).await;
    }
    match crate::control::stop(shared, AgentParams { agent }).await {
        // Stopped: its state follows within a few seconds.
        Ok(_) => {
            settled(shared, agent, |state| !state.is_live(), STOPPING).await;
        }
        // An agent volee cannot stop is not waited for.
        Err(error) => {
            tracing::debug!(%error, %agent, "cannot stop the coordinator of a closed mission");
        }
    }
    let mut state = shared.state.lock().await;
    let registry = state.registry();
    if registry.agent(agent).is_none() {
        return true;
    }
    if registry.forget_refusal(agent).is_some() {
        return false;
    }
    let now = state.now();
    state
        .commit(vec![Event::new(now, EventKind::AgentForgotten { agent })])
        .is_ok()
}

/// Waits until `agent` is gone or its state satisfies `done`, for at most
/// `longest`. Returns whether it did.
async fn settled(
    shared: &Arc<Shared>,
    agent: AgentId,
    done: fn(&volee_core::AgentState) -> bool,
    longest: Duration,
) -> bool {
    let end = tokio::time::Instant::now() + longest;
    loop {
        let reached = {
            let state = shared.state.lock().await;
            state
                .registry()
                .agent(agent)
                .is_none_or(|record| done(&record.state))
        };
        if reached {
            return true;
        }
        if tokio::time::Instant::now() >= end {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Tells the person, and the lead that asked for the mission, what closing
/// it tidied and what it kept.
async fn say_what_was_tidied(
    shared: &Arc<Shared>,
    mission: MissionId,
    tidied: &[String],
    kept: &[String],
) -> Result<(), DaemonError> {
    let mut state = shared.state.lock().await;
    let Some(record) = state.registry().mission(mission) else {
        return Ok(());
    };
    let mut body = format!("Mission \"{}\" is closed.", record.title);
    // Writing to a String cannot fail.
    if !tidied.is_empty() {
        let _ = write!(
            body,
            " Tidied: {} (worktrees removed, branches kept).",
            tidied.join(", ")
        );
    }
    if !kept.is_empty() {
        let _ = write!(body, " Kept: {}.", kept.join(". "));
    }
    let mut to = vec![Party::Human];
    if let Some(lead) = record
        .requested_by
        .filter(|lead| state.registry().agent(*lead).is_some())
    {
        to.push(Party::Agent(lead));
    }
    let message = Message {
        id: state.new_message_id(),
        thread: record.thread,
        from: Party::System,
        kind: if kept.is_empty() {
            MessageKind::Note
        } else {
            MessageKind::Alert
        },
        body,
        refs: Vec::new(),
        in_reply_to: None,
        options: Vec::new(),
        at: state.now(),
        receipts: to
            .into_iter()
            .map(|to| Receipt {
                to,
                delivered_at: None,
                read_at: None,
            })
            .collect(),
    };
    let now = state.now();
    state.commit(vec![Event::new(now, EventKind::MessagePosted { message })])?;
    Ok(())
}

/// A supervisor notice as a message from volee in the mission's thread.
pub(crate) fn notice_message(state: &State, mission: MissionId, notice: Notice) -> Option<Message> {
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
        options: Vec::new(),
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
