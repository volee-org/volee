//! What the demo does when the person reads, summarizes or launches: the
//! events a daemon would write, built from the scenario's state.

use std::collections::BTreeMap;
use std::path::PathBuf;

use volee_core::event::{Event, EventKind};
use volee_core::harness::HarnessKind;
use volee_core::message::{Message, MessageKind, Party, Receipt, Thread};
use volee_core::mission::{Mission, MissionStatus};
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::AgentState;
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};
use volee_core::{AgentId, MessageId, MissionId, Registry, ThreadId};

use super::scenario::{CAST, HOME};
use crate::source::{MissionRequest, NewMission, SourceError, SpawnRequest, Summary};

/// The repositories the demo's configuration would list: those of the
/// agents volee launched.
pub(crate) fn repos() -> Vec<String> {
    let mut repos: Vec<String> = CAST
        .iter()
        .filter(|member| member.role != Role::Independent)
        .filter_map(|member| member.repo.map(str::to_owned))
        .collect();
    repos.sort();
    repos.dedup();
    repos
}

/// The event that marks a message read by the person.
pub(crate) fn read(
    registry: &Registry,
    message: MessageId,
    now: Timestamp,
) -> Result<Event, SourceError> {
    let found = registry
        .message(message)
        .ok_or_else(|| SourceError::Failed(format!("unknown message {message}")))?;
    if !found.is_for(Party::Human) {
        return Err(SourceError::Unsupported(format!(
            "you are not a recipient of message {message}"
        )));
    }
    Ok(Event::new(
        now,
        EventKind::MessageRead {
            message,
            by: Party::Human,
        },
    ))
}

/// A summary written without a model: who said what last, which is enough
/// to show where summaries appear.
pub(crate) fn summary(registry: &Registry, thread: ThreadId) -> Result<Summary, SourceError> {
    let found = registry
        .thread(thread)
        .ok_or_else(|| SourceError::Failed(format!("unknown thread {thread}")))?;
    let messages = registry.thread_messages(thread);
    let text = match messages.last() {
        None => format!("{} has no messages yet.", found.title),
        Some(last) => {
            let sender = match last.from {
                Party::Human => "you".to_owned(),
                Party::System => "volee".to_owned(),
                Party::Agent(agent) => registry
                    .agent(agent)
                    .map_or_else(|| agent.short(), |agent| agent.name.clone()),
            };
            let line = last.body.lines().next().unwrap_or_default();
            let kind = last.kind.label();
            let article = if kind.starts_with(['a', 'e', 'i', 'o', 'u']) {
                "an"
            } else {
                "a"
            };
            format!(
                "{} message{} in {}. The latest, {article} {kind} from {sender}: {line}",
                messages.len(),
                if messages.len() == 1 { "" } else { "s" },
                found.title,
            )
        }
    };
    Ok(Summary {
        text,
        messages: messages.len(),
        model: "demo".to_owned(),
    })
}

/// Identifiers for what the person creates, in creation order.
pub(crate) struct Ids {
    pub(crate) at: Timestamp,
    pub(crate) random: u128,
}

impl Ids {
    fn ulid(&mut self) -> ulid::Ulid {
        self.random += 1;
        ulid::Ulid::from_parts(u64::try_from(self.at.as_millis()).unwrap_or(0), self.random)
    }
}

/// The events of an agent the person launches: registered, then at work.
pub(crate) fn spawn(
    registry: &Registry,
    request: &SpawnRequest,
    ids: &mut Ids,
) -> Result<(AgentId, Vec<Event>), SourceError> {
    let repo = request.repo.trim();
    let branch = request.branch.trim();
    if !repos().iter().any(|known| known == repo) {
        return Err(SourceError::Failed(format!(
            "unknown repository `{repo}`: use a [repos.<name>] of the configuration or an \
             absolute path"
        )));
    }
    if registry.agents().any(|agent| {
        agent.workspace.repo.as_deref() == Some(repo)
            && agent.workspace.branch.as_deref() == Some(branch)
    }) {
        return Err(SourceError::Failed(format!(
            "branch `{branch}` already exists"
        )));
    }
    let now = ids.at;
    let agent = AgentId::from_ulid(ids.ulid());
    let slug = branch.replace('/', "-");
    let worktree = PathBuf::from(format!("{HOME}/code/{repo}/.claude/worktrees/{slug}"));
    let mut events = vec![Event::new(
        now,
        EventKind::AgentRegistered {
            agent,
            name: format!("{repo}-{slug}"),
            role: Role::Lead,
            harness: HarnessKind::ClaudeCode,
            launched_by: LaunchedBy::Human,
            parent: None,
            mission: None,
            workspace: Workspace {
                cwd: worktree.clone(),
                repo: Some(repo.to_owned()),
                repo_root: Some(PathBuf::from(format!("{HOME}/code/{repo}"))),
                worktree: Some(worktree),
                branch: Some(branch.to_owned()),
                base: Some("main".to_owned()),
                start_commit: None,
            },
            location: Location::Background,
        },
    )];
    if let Some(model) = request
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        events.push(Event::new(
            now,
            EventKind::ModelChanged {
                agent,
                model: model.to_owned(),
            },
        ));
    }
    let task = request.task.lines().next().unwrap_or_default().to_owned();
    events.push(Event::new(
        now,
        EventKind::StateChanged {
            agent,
            state: AgentState::Working,
            detail: Some(task),
        },
    ));
    Ok((agent, events))
}

/// The events of a mission the person creates: its thread, the mission, and
/// its coordinator at work.
pub(crate) fn mission(
    registry: &Registry,
    request: &MissionRequest,
    ids: &mut Ids,
) -> Result<(NewMission, Vec<Event>), SourceError> {
    let known = repos();
    if let Some(unknown) = request.repos.iter().find(|repo| !known.contains(repo)) {
        return Err(SourceError::Failed(format!(
            "unknown repository `{unknown}` in the mission"
        )));
    }
    if registry
        .missions()
        .any(|mission| mission.title.eq_ignore_ascii_case(request.title.trim()))
    {
        return Err(SourceError::Failed(format!(
            "a mission is already called \"{}\"",
            request.title.trim()
        )));
    }
    Ok(mission_events(request, ids))
}

/// A note from volee to the person.
fn note(ids: &mut Ids, thread: ThreadId, body: String) -> Event {
    let now = ids.at;
    Event::new(
        now,
        EventKind::MessagePosted {
            message: Message {
                id: MessageId::from_ulid(ids.ulid()),
                thread,
                from: Party::System,
                kind: MessageKind::Note,
                body,
                refs: Vec::new(),
                in_reply_to: None,
                at: now,
                receipts: vec![Receipt {
                    to: Party::Human,
                    delivered_at: Some(now),
                    read_at: None,
                }],
            },
        },
    )
}

/// The events of a mission that passed the checks.
fn mission_events(request: &MissionRequest, ids: &mut Ids) -> (NewMission, Vec<Event>) {
    let now = ids.at;
    let mission = MissionId::from_ulid(ids.ulid());
    let thread = ThreadId::from_ulid(ids.ulid());
    let coordinator = AgentId::from_ulid(ids.ulid());
    let title = request.title.trim().to_owned();
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let events = vec![
        Event::new(
            now,
            EventKind::ThreadOpened {
                thread: Thread {
                    id: thread,
                    title: format!("Mission: {title}"),
                    mission: Some(mission),
                    participants: vec![Party::Human, Party::Agent(coordinator)],
                    opened_at: now,
                },
            },
        ),
        Event::new(
            now,
            EventKind::MissionCreated {
                mission: Mission {
                    id: mission,
                    title: title.clone(),
                    kind: request.kind,
                    goal: request.goal.trim().to_owned(),
                    repos: request.repos.clone(),
                    requested_by: None,
                    coordinator: None,
                    thread,
                    status: MissionStatus::Planning,
                    report: None,
                    created_at: now,
                    ended_at: None,
                    assessments: BTreeMap::new(),
                },
            },
        ),
        Event::new(
            now,
            EventKind::AgentRegistered {
                agent: coordinator,
                name: format!("coordinator-{slug}"),
                role: Role::Coordinator,
                harness: HarnessKind::ClaudeCode,
                launched_by: LaunchedBy::Human,
                parent: None,
                mission: Some(mission),
                workspace: Workspace::at(format!("{HOME}/.local/state/volee/missions/{slug}")),
                location: Location::Background,
            },
        ),
        Event::new(
            now,
            EventKind::MissionUpdated {
                mission,
                status: MissionStatus::Running,
                coordinator: Some(coordinator),
                report: None,
            },
        ),
        note(
            ids,
            thread,
            format!(
                "Mission \"{title}\" created in {}. Its coordinator plans the work.",
                request.repos.join(", ")
            ),
        ),
        Event::new(
            now,
            EventKind::StateChanged {
                agent: coordinator,
                state: AgentState::Working,
                detail: Some("Plan the mission".to_owned()),
            },
        ),
    ];
    (
        NewMission {
            mission,
            coordinator: Some(coordinator),
        },
        events,
    )
}
