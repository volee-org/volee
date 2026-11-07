use ulid::Ulid;

use super::*;
use crate::harness::{HarnessKind, SessionOrigin};
use crate::ids::ThreadId;
use crate::message::Receipt;
use crate::mission::{MissionKind, MissionStatus};
use crate::role::{LaunchedBy, Role};
use crate::state::InputRequest;
use crate::time::Timestamp;
use crate::workspace::{Location, Workspace};

fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis)
}

fn agent_id(n: u128) -> AgentId {
    AgentId::from_ulid(Ulid::from_parts(1_000, n))
}

fn register(registry: &mut Registry, id: AgentId, role: Role, parent: Option<AgentId>) {
    registry
        .apply(&Event::new(
            at(1),
            EventKind::AgentRegistered {
                agent: id,
                name: format!("agent {}", id.short()),
                role,
                harness: HarnessKind::ClaudeCode,
                launched_by: LaunchedBy::Human,
                parent,
                mission: None,
                workspace: Workspace::at("/work"),
                location: Location::Background,
            },
        ))
        .unwrap();
}

fn set_state(registry: &mut Registry, id: AgentId, state: AgentState, millis: i64) {
    registry
        .apply(&Event::new(
            at(millis),
            EventKind::StateChanged {
                agent: id,
                state,
                detail: None,
            },
        ))
        .unwrap();
}

#[test]
fn a_registered_agent_starts_launching() {
    let mut registry = Registry::new();
    register(&mut registry, agent_id(1), Role::Worker, None);
    let agent = registry.agent(agent_id(1)).unwrap();
    assert_eq!(agent.state, AgentState::Launching);
    assert_eq!(agent.created_at, at(1));
}

#[test]
fn registering_twice_is_an_error_and_changes_nothing() {
    let mut registry = Registry::new();
    register(&mut registry, agent_id(1), Role::Worker, None);
    let event = Event::new(
        at(2),
        EventKind::AgentRegistered {
            agent: agent_id(1),
            name: "other".into(),
            role: Role::Lead,
            harness: HarnessKind::ClaudeCode,
            launched_by: LaunchedBy::Human,
            parent: None,
            mission: None,
            workspace: Workspace::default(),
            location: Location::Unknown,
        },
    );
    assert_eq!(
        registry.apply(&event),
        Err(ApplyError::DuplicateAgent(agent_id(1)))
    );
    assert_eq!(registry.agent(agent_id(1)).unwrap().role, Role::Worker);
}

#[test]
fn a_session_cannot_belong_to_two_agents() {
    let mut registry = Registry::new();
    register(&mut registry, agent_id(1), Role::Worker, None);
    register(&mut registry, agent_id(2), Role::Worker, None);
    let link = |agent| {
        Event::new(
            at(3),
            EventKind::SessionLinked {
                agent,
                session: SessionKey::claude("s1"),
                origin: SessionOrigin::Started,
            },
        )
    };
    registry.apply(&link(agent_id(1))).unwrap();
    registry.apply(&link(agent_id(1))).unwrap();
    assert_eq!(registry.agent(agent_id(1)).unwrap().sessions.len(), 1);
    assert!(matches!(
        registry.apply(&link(agent_id(2))),
        Err(ApplyError::SessionTaken { .. })
    ));
    assert_eq!(registry.agent(agent_id(2)).unwrap().sessions, []);
}

#[test]
fn a_fork_keeps_the_agent_and_its_history() {
    let mut registry = Registry::new();
    register(&mut registry, agent_id(1), Role::Worker, None);
    for (session, origin) in [
        ("old", SessionOrigin::Started),
        ("new", SessionOrigin::Forked),
    ] {
        registry
            .apply(&Event::new(
                at(2),
                EventKind::SessionLinked {
                    agent: agent_id(1),
                    session: SessionKey::claude(session),
                    origin,
                },
            ))
            .unwrap();
    }
    let agent = registry.agent(agent_id(1)).unwrap();
    assert_eq!(
        agent.current_session().unwrap().key,
        SessionKey::claude("new")
    );
    assert_eq!(
        registry
            .agent_for_session(&SessionKey::claude("old"))
            .unwrap()
            .id,
        agent_id(1)
    );
}

#[test]
fn state_since_moves_only_when_the_state_changes() {
    let mut registry = Registry::new();
    register(&mut registry, agent_id(1), Role::Worker, None);
    set_state(&mut registry, agent_id(1), AgentState::Working, 10);
    set_state(&mut registry, agent_id(1), AgentState::Working, 20);
    let agent = registry.agent(agent_id(1)).unwrap();
    assert_eq!(agent.state_since, at(10));
    assert_eq!(agent.updated_at, at(20));
}

#[test]
fn agents_sort_by_urgency_then_recency() {
    let mut registry = Registry::new();
    for n in 1..=4 {
        register(&mut registry, agent_id(n), Role::Worker, None);
    }
    set_state(&mut registry, agent_id(1), AgentState::Idle, 10);
    set_state(&mut registry, agent_id(2), AgentState::Working, 11);
    set_state(&mut registry, agent_id(3), AgentState::Working, 12);
    set_state(
        &mut registry,
        agent_id(4),
        AgentState::NeedsInput {
            request: InputRequest::Question,
        },
        5,
    );
    let order: Vec<AgentId> = registry.agents_by_urgency().iter().map(|a| a.id).collect();
    assert_eq!(order, [agent_id(4), agent_id(3), agent_id(2), agent_id(1)]);
}

#[test]
fn forgetting_an_agent_makes_its_children_roots() {
    let mut registry = Registry::new();
    register(&mut registry, agent_id(1), Role::Lead, None);
    register(&mut registry, agent_id(2), Role::Worker, Some(agent_id(1)));
    assert_eq!(registry.children(agent_id(1)).len(), 1);
    let roots: Vec<AgentId> = registry.roots().iter().map(|a| a.id).collect();
    assert_eq!(roots, [agent_id(1)]);
    registry
        .apply(&Event::new(
            at(5),
            EventKind::AgentForgotten { agent: agent_id(1) },
        ))
        .unwrap();
    let roots: Vec<AgentId> = registry.roots().iter().map(|a| a.id).collect();
    assert_eq!(roots, [agent_id(2)]);
}

fn open_thread(
    registry: &mut Registry,
    n: u128,
    mission: Option<MissionId>,
    participants: Vec<Party>,
) -> ThreadId {
    let id = ThreadId::from_ulid(Ulid::from_parts(2_000, n));
    registry
        .apply(&Event::new(
            at(1),
            EventKind::ThreadOpened {
                thread: Thread {
                    id,
                    title: format!("thread {n}"),
                    mission,
                    participants,
                    opened_at: at(1),
                },
            },
        ))
        .unwrap();
    id
}

fn post(
    registry: &mut Registry,
    n: u128,
    thread: ThreadId,
    from: Party,
    to: &[Party],
    kind: MessageKind,
    in_reply_to: Option<MessageId>,
) -> MessageId {
    let id = MessageId::from_ulid(Ulid::from_parts(3_000, n));
    registry
        .apply(&Event::new(
            at(10),
            EventKind::MessagePosted {
                message: Message {
                    id,
                    thread,
                    from,
                    kind,
                    body: format!("message {n}"),
                    refs: vec![],
                    in_reply_to,
                    at: at(10),
                    receipts: to
                        .iter()
                        .map(|party| Receipt {
                            to: *party,
                            delivered_at: None,
                            read_at: None,
                        })
                        .collect(),
                },
            },
        ))
        .unwrap();
    id
}

#[test]
fn pending_messages_disappear_once_delivered() {
    let mut registry = Registry::new();
    register(&mut registry, agent_id(1), Role::Worker, None);
    let worker = Party::Agent(agent_id(1));
    let thread = open_thread(&mut registry, 1, None, vec![Party::Human, worker]);
    let message = post(
        &mut registry,
        1,
        thread,
        Party::Human,
        &[worker],
        MessageKind::Note,
        None,
    );
    assert_eq!(registry.pending_for(agent_id(1)).len(), 1);
    registry
        .apply(&Event::new(
            at(20),
            EventKind::MessageDelivered {
                message,
                to: worker,
            },
        ))
        .unwrap();
    assert_eq!(registry.pending_for(agent_id(1)), Vec::<&Message>::new());
    let receipt = *registry.message(message).unwrap().receipt(worker).unwrap();
    assert_eq!(receipt.delivered_at, Some(at(20)));
    assert_eq!(receipt.read_at, None);
}

#[test]
fn a_question_is_answered_by_a_reply_from_the_human() {
    let mut registry = Registry::new();
    register(&mut registry, agent_id(1), Role::Worker, None);
    let worker = Party::Agent(agent_id(1));
    let thread = open_thread(&mut registry, 1, None, vec![Party::Human, worker]);
    let question = post(
        &mut registry,
        1,
        thread,
        worker,
        &[Party::Human],
        MessageKind::Question,
        None,
    );
    assert_eq!(registry.unanswered_questions().len(), 1);
    post(
        &mut registry,
        2,
        thread,
        Party::Human,
        &[worker],
        MessageKind::Answer,
        Some(question),
    );
    assert_eq!(registry.unanswered_questions(), Vec::<&Message>::new());
}

#[test]
fn a_message_needs_an_existing_thread() {
    let mut registry = Registry::new();
    let missing = ThreadId::from_ulid(Ulid::from_parts(9, 9));
    let event = Event::new(
        at(1),
        EventKind::MessagePosted {
            message: Message {
                id: MessageId::from_ulid(Ulid::from_parts(9, 10)),
                thread: missing,
                from: Party::Human,
                kind: MessageKind::Note,
                body: String::new(),
                refs: vec![],
                in_reply_to: None,
                at: at(1),
                receipts: vec![],
            },
        },
    );
    assert_eq!(
        registry.apply(&event),
        Err(ApplyError::UnknownThread(missing))
    );
}

#[test]
fn direct_threads_are_found_by_their_two_participants() {
    let mut registry = Registry::new();
    let a = Party::Agent(agent_id(1));
    let b = Party::Agent(agent_id(2));
    let thread = open_thread(&mut registry, 1, None, vec![a, b]);
    assert_eq!(registry.thread_between(b, a).unwrap().id, thread);
    assert!(registry.thread_between(a, Party::Human).is_none());
}

fn create_mission(registry: &mut Registry, coordinator: Option<AgentId>) -> MissionId {
    let id = MissionId::from_ulid(Ulid::from_parts(4_000, 1));
    let thread = open_thread(registry, 99, Some(id), vec![]);
    registry
        .apply(&Event::new(
            at(1),
            EventKind::MissionCreated {
                mission: Mission {
                    id,
                    title: "Invoices".into(),
                    kind: MissionKind::Change,
                    goal: "Add invoices".into(),
                    repos: vec!["billing".into(), "mcp-server".into()],
                    requested_by: None,
                    coordinator,
                    thread,
                    status: MissionStatus::Planning,
                    report: None,
                    created_at: at(1),
                    ended_at: None,
                    assessments: BTreeMap::new(),
                },
            },
        ))
        .unwrap();
    id
}

fn join_mission(registry: &mut Registry, agent: AgentId, mission: MissionId, role: Role) {
    registry
        .apply(&Event::new(
            at(1),
            EventKind::AgentRegistered {
                agent,
                name: agent.short(),
                role,
                harness: HarnessKind::ClaudeCode,
                launched_by: LaunchedBy::Human,
                parent: None,
                mission: Some(mission),
                workspace: Workspace::default(),
                location: Location::Background,
            },
        ))
        .unwrap();
}

#[test]
fn members_of_a_mission_write_to_each_other() {
    let mut registry = Registry::new();
    let mission = create_mission(&mut registry, Some(agent_id(10)));
    join_mission(&mut registry, agent_id(10), mission, Role::Coordinator);
    join_mission(&mut registry, agent_id(11), mission, Role::Worker);
    join_mission(&mut registry, agent_id(12), mission, Role::Worker);
    let worker_a = Party::Agent(agent_id(11));
    let worker_b = Party::Agent(agent_id(12));
    assert_eq!(registry.check_route(worker_a, worker_b), Ok(()));
    assert_eq!(
        registry.check_route(worker_a, Party::Agent(agent_id(10))),
        Ok(())
    );
    assert_eq!(registry.mission_agents(mission).len(), 3);
}

#[test]
fn strangers_may_not_write_to_each_other() {
    let mut registry = Registry::new();
    register(&mut registry, agent_id(1), Role::Worker, None);
    register(&mut registry, agent_id(2), Role::Worker, None);
    let a = Party::Agent(agent_id(1));
    let b = Party::Agent(agent_id(2));
    assert_eq!(
        registry.check_route(a, b),
        Err(RouteError::NotPermitted {
            from: agent_id(1),
            to: agent_id(2)
        })
    );
    assert_eq!(registry.check_route(a, Party::Human), Ok(()));
    assert_eq!(registry.check_route(Party::Human, b), Ok(()));
    assert_eq!(registry.check_route(a, a), Err(RouteError::ToSelf));
}

#[test]
fn parents_and_children_write_to_each_other() {
    let mut registry = Registry::new();
    register(&mut registry, agent_id(1), Role::Lead, None);
    register(&mut registry, agent_id(2), Role::Worker, Some(agent_id(1)));
    let lead = Party::Agent(agent_id(1));
    let worker = Party::Agent(agent_id(2));
    assert_eq!(registry.check_route(lead, worker), Ok(()));
    assert_eq!(registry.check_route(worker, lead), Ok(()));
}

#[test]
fn closing_a_mission_records_its_end_once() {
    let mut registry = Registry::new();
    let mission = create_mission(&mut registry, None);
    let update = |status, millis| {
        Event::new(
            at(millis),
            EventKind::MissionUpdated {
                mission,
                status,
                coordinator: Some(agent_id(7)),
                report: None,
            },
        )
    };
    registry.apply(&update(MissionStatus::Running, 5)).unwrap();
    assert_eq!(registry.mission(mission).unwrap().ended_at, None);
    registry.apply(&update(MissionStatus::Closed, 9)).unwrap();
    registry.apply(&update(MissionStatus::Closed, 12)).unwrap();
    let record = registry.mission(mission).unwrap();
    assert_eq!(record.ended_at, Some(at(9)));
    assert_eq!(record.coordinator, Some(agent_id(7)));
}

fn stop(registry: &mut Registry, agent: AgentId, millis: i64) {
    registry
        .apply(&Event::new(at(millis), EventKind::StopRequested { agent }))
        .unwrap();
}

#[test]
fn a_stop_holds_the_agent_until_the_human_writes_to_it() {
    let mut registry = Registry::new();
    register(&mut registry, agent_id(1), Role::Worker, None);
    register(&mut registry, agent_id(2), Role::Lead, None);
    let worker = Party::Agent(agent_id(1));
    let thread = open_thread(&mut registry, 1, None, vec![Party::Human, worker]);
    stop(&mut registry, agent_id(1), 5);
    let held = || Some(Hold::StoppedByHuman { at: at(5) });
    assert_eq!(registry.agent(agent_id(1)).unwrap().hold, held());

    // Messages from volee and from agents leave the hold in place.
    post(
        &mut registry,
        1,
        thread,
        Party::System,
        &[worker],
        MessageKind::Note,
        None,
    );
    post(
        &mut registry,
        2,
        thread,
        Party::Agent(agent_id(2)),
        &[worker],
        MessageKind::Note,
        None,
    );
    assert_eq!(registry.agent(agent_id(1)).unwrap().hold, held());

    // A message from the human lifts it.
    post(
        &mut registry,
        3,
        thread,
        Party::Human,
        &[worker],
        MessageKind::Note,
        None,
    );
    assert_eq!(registry.agent(agent_id(1)).unwrap().hold, None);
}

#[test]
fn a_finished_mission_holds_its_agents_for_good() {
    let mut registry = Registry::new();
    let mission = create_mission(&mut registry, Some(agent_id(10)));
    join_mission(&mut registry, agent_id(10), mission, Role::Coordinator);
    join_mission(&mut registry, agent_id(11), mission, Role::Worker);
    register(&mut registry, agent_id(12), Role::Worker, None);
    registry
        .apply(&Event::new(
            at(9),
            EventKind::MissionUpdated {
                mission,
                status: MissionStatus::Aborted,
                coordinator: None,
                report: None,
            },
        ))
        .unwrap();
    let over = Some(Hold::MissionOver { at: at(9) });
    assert_eq!(registry.agent(agent_id(10)).unwrap().hold, over);
    assert_eq!(registry.agent(agent_id(11)).unwrap().hold, over);
    assert_eq!(
        registry.agent(agent_id(12)).unwrap().hold,
        None,
        "not a member"
    );

    // Neither a message from the human nor a stop replaces it.
    let worker = Party::Agent(agent_id(11));
    let thread = open_thread(&mut registry, 2, None, vec![Party::Human, worker]);
    post(
        &mut registry,
        4,
        thread,
        Party::Human,
        &[worker],
        MessageKind::Note,
        None,
    );
    stop(&mut registry, agent_id(11), 20);
    assert_eq!(registry.agent(agent_id(11)).unwrap().hold, over);
}

#[test]
fn replaying_the_same_events_gives_the_same_registry() {
    let events = vec![
        Event::new(
            at(1),
            EventKind::AgentRegistered {
                agent: agent_id(1),
                name: "a".into(),
                role: Role::Worker,
                harness: HarnessKind::ClaudeCode,
                launched_by: LaunchedBy::Human,
                parent: None,
                mission: None,
                workspace: Workspace::at("/w"),
                location: Location::Background,
            },
        ),
        Event::new(
            at(2),
            EventKind::StateChanged {
                agent: agent_id(1),
                state: AgentState::Working,
                detail: Some("Edit src/lib.rs".into()),
            },
        ),
    ];
    let first = Registry::from_events(&events).unwrap();
    let second = Registry::from_events(&events).unwrap();
    let ids = |r: &Registry| {
        r.agents()
            .map(|a| (a.id, a.state.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&first), ids(&second));
    assert_eq!(
        first.agent(agent_id(1)).unwrap().detail.as_deref(),
        Some("Edit src/lib.rs")
    );
}
