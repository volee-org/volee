//! Sample events shared by the integration tests.

use ulid::Ulid;
use volee_core::delivery::{ChecksState, Delivery, DiffStat, PrState, PullRequest, PushState};
use volee_core::event::{Event, EventKind};
use volee_core::harness::{HarnessKind, SessionKey, SessionOrigin};
use volee_core::ids::{AgentId, MessageId, MissionId, ThreadId};
use volee_core::message::{Message, MessageKind, Party, Receipt, Reference, Thread};
use volee_core::mission::{Mission, MissionKind, MissionStatus};
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::AgentState;
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};

pub(crate) fn agent() -> AgentId {
    AgentId::from_ulid(Ulid::from_parts(1_000, 1))
}

fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis)
}

/// One event of every kind, in an order that applies cleanly to an empty
/// registry. Keep it exhaustive: the store test checks the count.
#[allow(clippy::too_many_lines)] // One literal per event kind reads best in one place.
pub(crate) fn one_of_each_kind() -> Vec<Event> {
    let agent = agent();
    let thread_id = ThreadId::from_ulid(Ulid::from_parts(1_000, 2));
    let mission_id = MissionId::from_ulid(Ulid::from_parts(1_000, 3));
    let message_id = MessageId::from_ulid(Ulid::from_parts(1_000, 4));
    let mut workspace = Workspace::at("/work/volee");
    workspace.repo = Some("volee".into());
    workspace.branch = Some("feat/store".into());
    let kinds = vec![
        EventKind::ThreadOpened {
            thread: Thread {
                id: thread_id,
                title: "mission".into(),
                mission: Some(mission_id),
                participants: vec![Party::Human, Party::Agent(agent)],
                opened_at: at(1),
            },
        },
        EventKind::MissionCreated {
            mission: Mission {
                id: mission_id,
                title: "Add the store".into(),
                kind: MissionKind::Change,
                goal: "Persist events".into(),
                repos: vec!["volee".into()],
                requested_by: None,
                coordinator: None,
                thread: thread_id,
                status: MissionStatus::Planning,
                report: None,
                created_at: at(1),
                ended_at: None,
                assessments: std::collections::BTreeMap::new(),
            },
        },
        EventKind::AgentRegistered {
            agent,
            name: "store worker".into(),
            role: Role::Worker,
            harness: HarnessKind::ClaudeCode,
            launched_by: LaunchedBy::Human,
            parent: None,
            mission: Some(mission_id),
            workspace: Workspace::at("/work/volee"),
            location: Location::Background,
        },
        EventKind::SessionLinked {
            agent,
            session: SessionKey::claude("3be7fdc8-3d9c-4ff8-b917-8089d57780b2"),
            origin: SessionOrigin::Started,
        },
        EventKind::StateChanged {
            agent,
            state: AgentState::Working,
            detail: Some("writing tests".into()),
        },
        EventKind::ActivityRecorded {
            agent,
            summary: "Edit src/lib.rs".into(),
        },
        EventKind::WorkspaceUpdated { agent, workspace },
        EventKind::LocationChanged {
            agent,
            location: Location::Pane {
                multiplexer: "tuios".into(),
                target: "3".into(),
            },
        },
        EventKind::ModelChanged {
            agent,
            model: "opus".into(),
        },
        EventKind::DeliveryUpdated {
            agent,
            delivery: Delivery {
                commits: 2,
                dirty: false,
                diff: DiffStat {
                    files: 3,
                    insertions: 40,
                    deletions: 2,
                },
                push: PushState::Ahead { commits: 1 },
                merged_into_base: false,
                pull_request: Some(PullRequest {
                    number: 7,
                    url: "https://github.com/volee-org/volee/pull/7".into(),
                    state: PrState::Open,
                    checks: ChecksState::Pending,
                }),
                report_delivered: false,
                tip: Some("6adc11fa9619748617663ded520ce631e9292269".into()),
                unmerged_tip: Some("6adc11fa9619748617663ded520ce631e9292269".into()),
                checked_at: Some(Timestamp::from_millis(1_234)),
            },
        },
        EventKind::MessagePosted {
            message: Message {
                id: message_id,
                thread: thread_id,
                from: Party::Human,
                kind: MessageKind::Question,
                body: "How is it going?".into(),
                refs: vec![Reference::File {
                    path: "src/lib.rs".into(),
                    line: Some(3),
                }],
                in_reply_to: None,
                at: at(10),
                receipts: vec![Receipt {
                    to: Party::Agent(agent),
                    delivered_at: None,
                    read_at: None,
                }],
            },
        },
        EventKind::MessageDelivered {
            message: message_id,
            to: Party::Agent(agent),
        },
        EventKind::MessageRead {
            message: message_id,
            by: Party::Agent(agent),
        },
        EventKind::MissionUpdated {
            mission: mission_id,
            status: MissionStatus::Running,
            coordinator: Some(agent),
            report: None,
        },
        EventKind::WorkerAssessed {
            mission: mission_id,
            worker: agent,
            verdict: volee_core::mission::Verdict::Delivered,
            evidence: "3 commits, pushed, PR #42 open".to_owned(),
        },
        EventKind::AgentForgotten { agent },
    ];
    kinds
        .into_iter()
        .zip(1..)
        .map(|(kind, millis)| Event::new(at(millis), kind))
        .collect()
}
