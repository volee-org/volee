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

/// A session someone else started, as discovery registers it.
fn watch(registry: &mut Registry, id: AgentId, session: &str) {
    for kind in [
        EventKind::AgentRegistered {
            agent: id,
            name: format!("session {session}"),
            role: Role::Independent,
            harness: HarnessKind::ClaudeCode,
            launched_by: LaunchedBy::Outside,
            parent: None,
            mission: None,
            workspace: Workspace::at("/work"),
            location: Location::Background,
        },
        EventKind::SessionLinked {
            agent: id,
            session: SessionKey::claude(session),
            origin: SessionOrigin::Discovered,
        },
    ] {
        registry.apply(&Event::new(at(1), kind)).unwrap();
    }
}

#[test]
fn a_watched_failure_sorts_after_the_agents_that_run() {
    let mut registry = Registry::new();
    register(&mut registry, agent_id(1), Role::Worker, None);
    register(&mut registry, agent_id(2), Role::Worker, None);
    watch(&mut registry, agent_id(3), "s3");
    watch(&mut registry, agent_id(4), "s4");
    set_state(&mut registry, agent_id(1), AgentState::Idle, 10);
    // A launch that failed calls for the person; a session started
    // elsewhere that failed is history, below the stopped ones that ended
    // after it.
    set_state(&mut registry, agent_id(2), AgentState::Failed, 11);
    set_state(&mut registry, agent_id(3), AgentState::Failed, 12);
    set_state(&mut registry, agent_id(4), AgentState::Stopped, 13);
    let order: Vec<AgentId> = registry.agents_by_urgency().iter().map(|a| a.id).collect();
    assert_eq!(order, [agent_id(2), agent_id(1), agent_id(4), agent_id(3)]);
}

#[test]
fn a_renamed_agent_keeps_everything_but_its_name() {
    let mut registry = Registry::new();
    watch(&mut registry, agent_id(1), "s1");
    registry
        .apply(&Event::new(
            at(7),
            EventKind::AgentRenamed {
                agent: agent_id(1),
                name: "fix the login flow".into(),
            },
        ))
        .unwrap();
    let agent = registry.agent(agent_id(1)).unwrap();
    assert_eq!(agent.name, "fix the login flow");
    assert_eq!(agent.updated_at, at(7));
    assert!(agent.owns_session(&SessionKey::claude("s1")));
    let unknown = Event::new(
        at(8),
        EventKind::AgentRenamed {
            agent: agent_id(9),
            name: "x".into(),
        },
    );
    assert_eq!(
        registry.apply(&unknown),
        Err(ApplyError::UnknownAgent(agent_id(9)))
    );
}

#[test]
fn a_released_session_is_nobodys_and_can_be_linked_again() {
    let mut registry = Registry::new();
    watch(&mut registry, agent_id(1), "s1");
    watch(&mut registry, agent_id(2), "s2");
    let link = |agent, session: &str| {
        Event::new(
            at(3),
            EventKind::SessionLinked {
                agent,
                session: SessionKey::claude(session),
                origin: SessionOrigin::Forked,
            },
        )
    };
    registry.apply(&link(agent_id(1), "copy")).unwrap();
    let release = |agent, session: &str| {
        Event::new(
            at(4),
            EventKind::SessionReleased {
                agent,
                session: SessionKey::claude(session),
            },
        )
    };
    registry.apply(&release(agent_id(1), "copy")).unwrap();
    let first = registry.agent(agent_id(1)).unwrap();
    assert_eq!(first.sessions.len(), 1);
    assert_eq!(
        first.current_session().unwrap().key,
        SessionKey::claude("s1")
    );
    assert!(
        registry
            .agent_for_session(&SessionKey::claude("copy"))
            .is_none()
    );
    assert!(!registry.is_ignored(&SessionKey::claude("copy")));
    // Another agent may own it now.
    registry.apply(&link(agent_id(2), "copy")).unwrap();
    assert_eq!(
        registry
            .agent_for_session(&SessionKey::claude("copy"))
            .unwrap()
            .id,
        agent_id(2)
    );
    // Releasing a session the agent does not hold takes nothing from the
    // agent that does.
    registry.apply(&release(agent_id(1), "copy")).unwrap();
    assert_eq!(
        registry
            .agent_for_session(&SessionKey::claude("copy"))
            .unwrap()
            .id,
        agent_id(2)
    );
}

#[test]
fn an_expired_agent_is_gone_and_its_sessions_are_not_ignored() {
    let mut registry = Registry::new();
    watch(&mut registry, agent_id(1), "s1");
    watch(&mut registry, agent_id(2), "s2");
    registry
        .apply(&Event::new(
            at(5),
            EventKind::AgentExpired { agent: agent_id(1) },
        ))
        .unwrap();
    assert!(registry.agent(agent_id(1)).is_none());
    assert!(
        registry
            .agent_for_session(&SessionKey::claude("s1"))
            .is_none()
    );
    assert!(!registry.is_ignored(&SessionKey::claude("s1")));
    // A forgotten one is ignored for good.
    registry
        .apply(&Event::new(
            at(6),
            EventKind::AgentForgotten { agent: agent_id(2) },
        ))
        .unwrap();
    assert!(registry.is_ignored(&SessionKey::claude("s2")));
    let again = Event::new(at(7), EventKind::AgentExpired { agent: agent_id(1) });
    assert_eq!(
        registry.apply(&again),
        Err(ApplyError::UnknownAgent(agent_id(1)))
    );
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

#[test]
fn only_agents_that_stopped_or_came_from_outside_are_forgotten() {
    let mut registry = Registry::new();
    // Launched by the person, still starting: stop it first.
    register(&mut registry, agent_id(1), Role::Lead, None);
    let name = registry.agent(agent_id(1)).unwrap().name.clone();
    assert_eq!(
        registry.forget_refusal(agent_id(1)),
        Some(format!(
            "{name} is still running: stop it first (`volee stop {name}`), then forget it"
        ))
    );
    set_state(&mut registry, agent_id(1), AgentState::Failed, 2);
    assert_eq!(registry.forget_refusal(agent_id(1)), None);
    // A member of an open mission waits for the mission to end, even
    // stopped. (Done is still live: its session runs.)
    let mission = create_mission(&mut registry, None);
    join_mission(&mut registry, agent_id(2), mission, Role::Worker);
    set_state(&mut registry, agent_id(2), AgentState::Stopped, 3);
    let refusal = registry.forget_refusal(agent_id(2)).unwrap();
    assert!(
        refusal.ends_with(
            "belongs to the open mission \"Invoices\": close or abort the mission first"
        ),
        "{refusal}"
    );
    registry
        .apply(&Event::new(
            at(4),
            EventKind::MissionUpdated {
                mission,
                status: MissionStatus::Closed,
                coordinator: None,
                report: None,
            },
        ))
        .unwrap();
    assert_eq!(registry.forget_refusal(agent_id(2)), None);
    // A session found outside volee is forgotten even while it runs.
    registry
        .apply(&Event::new(
            at(5),
            EventKind::AgentRegistered {
                agent: agent_id(3),
                name: "notes".into(),
                role: Role::Lead,
                harness: HarnessKind::ClaudeCode,
                launched_by: LaunchedBy::Outside,
                parent: None,
                mission: None,
                workspace: Workspace::at("/work"),
                location: Location::Background,
            },
        ))
        .unwrap();
    set_state(&mut registry, agent_id(3), AgentState::Working, 6);
    assert_eq!(registry.forget_refusal(agent_id(3)), None);
    assert_eq!(registry.forget_refusal(agent_id(9)), None, "unknown");
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
                    options: Vec::new(),
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
    let kinds: Vec<MessageKind> = registry.messages().map(|message| message.kind).collect();
    assert_eq!(kinds, [MessageKind::Question, MessageKind::Answer]);
}

#[test]
fn the_answer_to_a_question_is_the_person_s_first_reply_to_it() {
    let mut registry = Registry::new();
    register(&mut registry, agent_id(1), Role::Worker, None);
    register(&mut registry, agent_id(2), Role::Worker, None);
    let worker = Party::Agent(agent_id(1));
    let other = Party::Agent(agent_id(2));
    let thread = open_thread(&mut registry, 1, None, vec![Party::Human, worker, other]);
    let question = post(
        &mut registry,
        1,
        thread,
        worker,
        &[Party::Human, other],
        MessageKind::Question,
        None,
    );
    assert_eq!(registry.answer_to(question), None);
    // Another agent's reply is not the person's answer.
    post(
        &mut registry,
        2,
        thread,
        other,
        &[worker],
        MessageKind::Answer,
        Some(question),
    );
    assert_eq!(registry.answer_to(question), None);
    assert_eq!(registry.unanswered_questions().len(), 1);
    let first = post(
        &mut registry,
        3,
        thread,
        Party::Human,
        &[worker],
        MessageKind::Answer,
        Some(question),
    );
    post(
        &mut registry,
        4,
        thread,
        Party::Human,
        &[worker],
        MessageKind::Note,
        Some(question),
    );
    assert_eq!(
        registry.answer_to(question).map(|answer| answer.id),
        Some(first)
    );
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
                options: Vec::new(),
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

/// An agent of another harness that joined through volee's MCP server.
fn join_through_mcp(registry: &mut Registry, agent: AgentId) {
    registry
        .apply(&Event::new(
            at(1),
            EventKind::AgentRegistered {
                agent,
                name: "infra".into(),
                role: Role::Independent,
                harness: HarnessKind::Other("codex".into()),
                launched_by: LaunchedBy::Outside,
                parent: None,
                mission: None,
                workspace: Workspace::default(),
                location: Location::Unknown,
            },
        ))
        .unwrap();
}

#[test]
fn an_agent_started_elsewhere_is_managed_once_adopted_into_a_host() {
    let mut registry = Registry::new();
    let agent = agent_id(30);
    registry
        .apply(&Event::new(
            at(1),
            EventKind::AgentRegistered {
                agent,
                name: "by-hand".into(),
                role: Role::Independent,
                harness: HarnessKind::ClaudeCode,
                launched_by: LaunchedBy::Outside,
                parent: None,
                mission: None,
                workspace: Workspace::default(),
                location: Location::Background,
            },
        ))
        .unwrap();
    assert!(!registry.agent(agent).unwrap().managed(), "read only");
    let mut move_to = |location| {
        registry
            .apply(&Event::new(
                at(2),
                EventKind::LocationChanged { agent, location },
            ))
            .unwrap();
        registry.agent(agent).unwrap().managed()
    };
    assert!(move_to(Location::Host { id: "h1".into() }), "adopted");
    assert!(!move_to(Location::Background), "back in the background");
}

#[test]
fn a_joined_agent_takes_part_in_a_mission_and_leaves_it() {
    let mut registry = Registry::new();
    let mission = create_mission(&mut registry, None);
    join_mission(&mut registry, agent_id(11), mission, Role::Worker);
    join_through_mcp(&mut registry, agent_id(20));
    let joined = Party::Agent(agent_id(20));
    let worker = Party::Agent(agent_id(11));
    assert!(registry.agent(agent_id(20)).unwrap().joined());
    assert!(!registry.agent(agent_id(11)).unwrap().joined());
    assert!(registry.check_route(joined, worker).is_err(), "strangers");

    let member = |mission| {
        Event::new(
            at(2),
            EventKind::MembershipChanged {
                agent: agent_id(20),
                mission,
            },
        )
    };
    registry.apply(&member(Some(mission))).unwrap();
    assert_eq!(registry.check_route(joined, worker), Ok(()));
    assert_eq!(registry.check_route(worker, joined), Ok(()));

    registry.apply(&member(None)).unwrap();
    assert_eq!(registry.agent(agent_id(20)).unwrap().mission, None);
    assert!(registry.check_route(joined, worker).is_err());
}

#[test]
fn only_joined_agents_change_missions() {
    let mut registry = Registry::new();
    let mission = create_mission(&mut registry, None);
    join_mission(&mut registry, agent_id(11), mission, Role::Worker);
    join_through_mcp(&mut registry, agent_id(20));
    let change = |agent, mission| {
        Event::new(
            at(2),
            EventKind::MembershipChanged {
                agent: agent_id(agent),
                mission,
            },
        )
    };
    assert_eq!(
        registry.apply(&change(11, None)),
        Err(ApplyError::NotJoined(agent_id(11)))
    );
    let unknown = MissionId::from_ulid(Ulid::from_parts(4_000, 9));
    assert_eq!(
        registry.apply(&change(20, Some(unknown))),
        Err(ApplyError::UnknownMission(unknown))
    );
    assert_eq!(
        registry.agent(agent_id(11)).unwrap().mission,
        Some(mission),
        "unchanged"
    );
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

#[test]
fn an_agent_has_worked_once_it_was_seen_working_using_a_tool_or_asking() {
    let state = |state: AgentState| EventKind::StateChanged {
        agent: agent_id(1),
        state,
        detail: None,
    };
    let after = |events: Vec<EventKind>| {
        let mut registry = Registry::new();
        register(&mut registry, agent_id(1), Role::Worker, None);
        for kind in events {
            registry.apply(&Event::new(at(5), kind)).unwrap();
        }
        registry.agent(agent_id(1)).unwrap().worked
    };
    assert!(!after(vec![]), "launching");
    assert!(
        !after(vec![state(AgentState::Idle)]),
        "resting at its prompt"
    );
    assert!(!after(vec![state(AgentState::Stopped)]));
    assert!(after(vec![state(AgentState::Working)]));
    assert!(after(vec![
        state(AgentState::Working),
        state(AgentState::Idle)
    ]));
    assert!(after(vec![state(AgentState::NeedsInput {
        request: InputRequest::Question
    })]));
    assert!(after(vec![EventKind::ActivityRecorded {
        agent: agent_id(1),
        summary: "Edit src/lib.rs".into(),
    }]));
}

#[test]
fn the_name_of_an_agent_outlives_it() {
    let mut registry = Registry::new();
    register(&mut registry, agent_id(1), Role::Worker, None);
    let name = registry.agent(agent_id(1)).unwrap().name.clone();
    assert_eq!(registry.name_of(agent_id(1)), Some(name.as_str()));
    assert_eq!(registry.name_of(agent_id(9)), None);
    // Renamed, then gone from the list: the threads it wrote in still name
    // it, by its last name.
    registry
        .apply(&Event::new(
            at(5),
            EventKind::AgentRenamed {
                agent: agent_id(1),
                name: "billing-invoices".into(),
            },
        ))
        .unwrap();
    registry
        .apply(&Event::new(
            at(6),
            EventKind::AgentForgotten { agent: agent_id(1) },
        ))
        .unwrap();
    assert!(registry.agent(agent_id(1)).is_none());
    assert_eq!(registry.name_of(agent_id(1)), Some("billing-invoices"));
    // An expired agent keeps its name too.
    register(&mut registry, agent_id(2), Role::Independent, None);
    let name = registry.agent(agent_id(2)).unwrap().name.clone();
    registry
        .apply(&Event::new(
            at(7),
            EventKind::AgentExpired { agent: agent_id(2) },
        ))
        .unwrap();
    assert_eq!(registry.name_of(agent_id(2)), Some(name.as_str()));
}
