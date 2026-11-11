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

use volee_core::delivery::{PrState, PullRequest};
use volee_core::message::{Message, MessageKind, Party, Receipt};
use volee_core::mission::{MissionKind, MissionStatus};
use volee_core::role::Role;
use volee_core::supervisor::{Notice, supervise};
use volee_core::{AgentId, Event, EventKind, MissionId, Registry};
use volee_git::MergeMethod;
use volee_proto::methods::{AgentParams, SeqEvent, TidyOutcome, TidyParams};

use crate::error::DaemonError;
use crate::server::Shared;
use crate::state::State;

/// How long the merge and the wind-down wait for the delivery evidence of
/// the workers to be read again: a few git commands and one forge lookup
/// each.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the wind-down lets the coordinator finish the turn the person's
/// approval woke.
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

/// One worker's work, as the merge needs it.
struct Work {
    agent: AgentId,
    name: String,
    /// The main checkout of its repository.
    repo: std::path::PathBuf,
    /// Its repository's name in the configuration, if it has one.
    configured: Option<String>,
    branch: String,
    base: String,
    /// Its repository has no remote: the merge is a local one.
    no_remote: bool,
    /// The pull request at the tip of its branch, when it is open.
    pull_request: Option<PullRequest>,
}

/// A merge that did not go through.
struct Failure {
    worker: AgentId,
    /// The worker's name and the reason, as the person reads them.
    why: String,
}

/// Carries out the person's approval of a mission (ADR 0032): merges the
/// work of every worker of a change mission, closes the mission, and winds
/// it down. volee does this itself, as a program does what a person agreed
/// to: an agent's merge depends on what its harness lets it run, and the
/// person's word given here is not a word its harness heard.
///
/// A branch in a repository without a remote is merged into its base branch
/// in the main checkout; a pull request is merged on the forge. Nothing is
/// forced. When a merge does not go through, what was merged stays merged,
/// the mission is open again (`reporting`), and the person, the coordinator
/// and the lead are told why.
pub(crate) async fn carry_out(shared: Arc<Shared>, mission: MissionId) {
    // One answer is carried out once, whoever notices it.
    if !lock_concluding(&shared).insert(mission) {
        return;
    }
    let failures = merge_all(&shared, mission).await;
    let closed = match failures {
        Some(failures) => conclude(&shared, mission, &failures).await,
        None => false,
    };
    lock_concluding(&shared).remove(&mission);
    if closed {
        // The person's answer woke the coordinator: its turn is its own.
        wind_down(shared, mission, true).await;
    }
}

fn lock_concluding(
    shared: &Shared,
) -> std::sync::MutexGuard<'_, std::collections::HashSet<MissionId>> {
    // A set of identifiers stays consistent whatever a panic interrupted.
    shared
        .concluding
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Carries out the missions that are approved when the daemon starts: the
/// person's answer came while it was stopping, or a merge was cut short.
pub(crate) async fn resume_approved(shared: &Arc<Shared>) {
    let approved: Vec<MissionId> = {
        let state = shared.state.lock().await;
        state
            .registry()
            .missions()
            .filter(|mission| mission.status == MissionStatus::Approved)
            .map(|mission| mission.id)
            .collect()
    };
    for mission in approved {
        tokio::spawn(carry_out(Arc::clone(shared), mission));
    }
}

/// Merges what is left to merge of an approved mission, and returns why
/// each merge that failed did; `None` when the mission is not approved any
/// more.
async fn merge_all(shared: &Arc<Shared>, mission: MissionId) -> Option<Vec<Failure>> {
    // What git and the forge say now decides what is left to merge and how:
    // the evidence in the registry may be as old as the worker's last stop,
    // and the person may have merged a pull request themselves since.
    let workers: Vec<AgentId> = {
        let state = shared.state.lock().await;
        let registry = state.registry();
        if registry.mission(mission)?.status != MissionStatus::Approved {
            return None;
        }
        registry
            .mission_agents(mission)
            .into_iter()
            .filter(|agent| agent.role == Role::Worker)
            .map(|agent| agent.id)
            .collect()
    };
    crate::evidence::check_now(shared, &workers, CHECK_TIMEOUT).await;
    let (kind, work) = {
        let state = shared.state.lock().await;
        let registry = state.registry();
        let record = registry.mission(mission)?;
        if record.status != MissionStatus::Approved {
            return None;
        }
        let work: Vec<Work> = registry
            .mission_agents(mission)
            .into_iter()
            .filter(|agent| agent.role == Role::Worker && !agent.delivery.is_delivered())
            .map(|agent| Work {
                agent: agent.id,
                name: agent.name.clone(),
                repo: agent
                    .workspace
                    .repo_root
                    .clone()
                    .unwrap_or_else(|| agent.workspace.cwd.clone()),
                configured: agent.workspace.repo.clone(),
                branch: agent.workspace.branch.clone().unwrap_or_default(),
                base: agent.workspace.base.clone().unwrap_or_default(),
                no_remote: agent.delivery.no_remote(),
                pull_request: agent
                    .delivery
                    .pull_request_at_tip()
                    .filter(|pr| matches!(pr.state, PrState::Open | PrState::Draft))
                    .cloned(),
            })
            .collect();
        (record.kind, work)
    };
    let mut failures = Vec::new();
    if kind == MissionKind::Change {
        for work in &work {
            if let Err(reason) = merge(shared, work).await {
                tracing::warn!(%mission, worker = %work.name, %reason, "cannot merge");
                failures.push(Failure {
                    worker: work.agent,
                    why: format!("{}: {reason}", work.name),
                });
            }
        }
        // The same again, the merges included.
        let ids: Vec<AgentId> = work.iter().map(|work| work.agent).collect();
        crate::evidence::check_now(shared, &ids, CHECK_TIMEOUT).await;
    }
    Some(failures)
}

/// Merges one worker's work: its branch into its base branch in a
/// repository without a remote, its pull request on the forge otherwise.
async fn merge(shared: &Arc<Shared>, work: &Work) -> Result<(), String> {
    if work.no_remote {
        if work.branch.is_empty() || work.base.is_empty() {
            return Err("volee does not know its branch or its base branch".to_owned());
        }
        let git = crate::launch::git(shared).map_err(|error| error.to_string())?;
        let (repo, branch, base) = (work.repo.clone(), work.branch.clone(), work.base.clone());
        return crate::launch::blocking(move || git.merge(&repo, &branch, &base))
            .await
            .map_err(|error| error.to_string())?
            .map(drop)
            .map_err(|error| error.to_string());
    }
    let Some(pull_request) = work.pull_request.clone() else {
        return Err("it has no open pull request at the tip of its branch".to_owned());
    };
    let Some(forge) = shared.forge_client.clone() else {
        return Err(format!(
            "volee has no forge client to merge {} with (the GitHub CLI, signed in)",
            pull_request.url
        ));
    };
    // The method the person set for this repository, else the first one it
    // allows.
    let method = work
        .configured
        .as_deref()
        .and_then(|name| shared.settings.repos.get(name))
        .and_then(|repo| repo.merge)
        .map(|method| match method {
            volee_config::MergeMethod::Merge => MergeMethod::Merge,
            volee_config::MergeMethod::Squash => MergeMethod::Squash,
            volee_config::MergeMethod::Rebase => MergeMethod::Rebase,
        });
    let repo = work.repo.clone();
    crate::launch::blocking(move || forge.merge(&repo, &pull_request, method))
        .await
        .map_err(|error| error.to_string())?
        .map(drop)
        .map_err(|error| error.to_string())
}

/// Closes an approved mission whose work is all merged; else opens it again
/// and says which merges failed. Returns whether it closed.
async fn conclude(shared: &Arc<Shared>, mission: MissionId, failures: &[Failure]) -> bool {
    let mut state = shared.state.lock().await;
    let registry = state.registry();
    let Some(record) = registry.mission(mission) else {
        return false;
    };
    if record.status != MissionStatus::Approved {
        return false;
    }
    let mut left: Vec<String> = failures.iter().map(|failure| failure.why.clone()).collect();
    if record.kind == MissionKind::Change {
        // A merge that reported no failure and that the evidence does not
        // show is not a merge.
        left.extend(
            registry
                .mission_agents(mission)
                .into_iter()
                .filter(|agent| agent.role == Role::Worker && !agent.delivery.is_delivered())
                .filter(|agent| !failures.iter().any(|failure| failure.worker == agent.id))
                .map(|agent| format!("{}: volee sees no merge of its branch", agent.name)),
        );
    }
    let now = state.now();
    if left.is_empty() {
        let closed = state.commit(vec![Event::new(
            now,
            EventKind::MissionUpdated {
                mission,
                status: MissionStatus::Closed,
                coordinator: None,
                report: None,
            },
        )]);
        return match closed {
            Ok(_) => true,
            Err(error) => {
                tracing::error!(%error, %mission, "cannot close the approved mission");
                false
            }
        };
    }
    let mut to = vec![Party::Human];
    to.extend(
        [record.coordinator, record.requested_by]
            .into_iter()
            .flatten()
            .filter(|agent| registry.agent(*agent).is_some())
            .map(Party::Agent),
    );
    let message = Message {
        id: state.new_message_id(),
        thread: record.thread,
        from: Party::System,
        kind: MessageKind::Alert,
        body: format!(
            "volee could not merge all the work of \"{}\". {}. What was merged stays merged, \
             and nothing was forced. The mission is open again: once what blocks is fixed, \
             the coordinator presents its report again with `request_approval`.",
            record.title,
            left.join(". ")
        ),
        refs: Vec::new(),
        in_reply_to: None,
        options: Vec::new(),
        at: now,
        receipts: to
            .into_iter()
            .map(|to| Receipt {
                to,
                delivered_at: None,
                read_at: None,
            })
            .collect(),
    };
    let reopened = state.commit(vec![
        Event::new(
            now,
            EventKind::MissionUpdated {
                mission,
                status: MissionStatus::Reporting,
                coordinator: None,
                report: None,
            },
        ),
        Event::new(now, EventKind::MessagePosted { message }),
    ]);
    if let Err(error) = reopened {
        tracing::error!(%error, %mission, "cannot say that the merge failed");
    }
    false
}

/// What closing a mission leaves to do, in one gesture: every worker whose
/// work landed is tidied (its worktree removed, its branch kept, the agent
/// forgotten), the coordinator is stopped once its turn ended and forgotten,
/// and the person is told what was tidied and what was kept, with the
/// reason. Nothing is forced: a worker that is kept stays as it is.
///
/// `last_turn` says that the coordinator may be in the turn the person's
/// approval woke: it is left to end it. A mission the person closes by hand
/// stops its coordinator at once.
pub(crate) async fn wind_down(shared: Arc<Shared>, mission: MissionId, last_turn: bool) {
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
        && !retire(&shared, agent, last_turn).await
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
/// whether it is gone. With `last_turn`, a turn it is in the middle of (it
/// was just told that the person approved) is left to end first: its last
/// words are its own.
async fn retire(shared: &Arc<Shared>, agent: AgentId, last_turn: bool) -> bool {
    if last_turn {
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
