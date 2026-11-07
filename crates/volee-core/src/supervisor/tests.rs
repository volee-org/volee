use std::collections::BTreeMap;

use ulid::Ulid;

use super::*;
use crate::delivery::{ChecksState, Delivery, PullRequest};
use crate::event::Event;
use crate::harness::HarnessKind;
use crate::ids::{AgentId, MessageId, ThreadId};
use crate::message::{Message, Receipt, Thread};
use crate::mission::Mission;
use crate::role::LaunchedBy;
use crate::time::Timestamp;
use crate::workspace::{Location, Workspace};

const MISSION: u128 = 1;
const COORDINATOR: u128 = 10;

fn mission_id() -> MissionId {
    MissionId::from_ulid(Ulid::from_parts(4_000, MISSION))
}

fn agent(n: u128) -> AgentId {
    AgentId::from_ulid(Ulid::from_parts(1_000, n))
}

fn thread_id() -> ThreadId {
    ThreadId::from_ulid(Ulid::from_parts(2_000, 1))
}

struct World {
    registry: Registry,
    clock: i64,
}

impl World {
    fn new(kind: MissionKind) -> Self {
        let mut world = Self {
            registry: Registry::new(),
            clock: 0,
        };
        world.apply(EventKind::ThreadOpened {
            thread: Thread {
                id: thread_id(),
                title: "Invoicing".into(),
                mission: Some(mission_id()),
                participants: vec![],
                opened_at: Timestamp::from_millis(0),
            },
        });
        world.apply(EventKind::MissionCreated {
            mission: Mission {
                id: mission_id(),
                title: "Invoicing".into(),
                kind,
                goal: "Add invoices".into(),
                repos: vec!["billing".into(), "api".into()],
                requested_by: None,
                coordinator: Some(agent(COORDINATOR)),
                thread: thread_id(),
                status: MissionStatus::Planning,
                report: None,
                created_at: Timestamp::from_millis(0),
                ended_at: None,
                assessments: BTreeMap::new(),
            },
        });
        world.register(COORDINATOR, Role::Coordinator, "coordinator");
        world
    }

    fn apply(&mut self, kind: EventKind) {
        self.clock += 1;
        self.registry
            .apply(&Event::new(Timestamp::from_millis(self.clock), kind))
            .unwrap();
    }

    fn register(&mut self, n: u128, role: Role, name: &str) {
        self.apply(EventKind::AgentRegistered {
            agent: agent(n),
            name: name.into(),
            role,
            harness: HarnessKind::ClaudeCode,
            launched_by: LaunchedBy::Agent(agent(COORDINATOR)),
            parent: Some(agent(COORDINATOR)),
            mission: Some(mission_id()),
            workspace: Workspace {
                repo: Some(name.into()),
                branch: Some("feat/invoices".into()),
                ..Workspace::at("/work")
            },
            location: Location::Background,
        });
    }

    fn set_state(&mut self, n: u128, state: AgentState) {
        self.apply(EventKind::StateChanged {
            agent: agent(n),
            state,
            detail: None,
        });
    }

    fn set_delivery(&mut self, n: u128, delivery: Delivery) {
        self.apply(EventKind::DeliveryUpdated {
            agent: agent(n),
            delivery,
        });
    }

    /// Supervises, applies the decision's events, and returns the decision.
    fn step(&mut self) -> Decision {
        let decision = supervise(&self.registry, mission_id());
        for event in decision.events.clone() {
            self.apply(event);
        }
        decision
    }

    fn status(&self) -> MissionStatus {
        self.registry.mission(mission_id()).unwrap().status
    }

    /// The time of the last applied event.
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.clock)
    }

    fn assessments(&self) -> &BTreeMap<AgentId, crate::mission::Assessment> {
        &self.registry.mission(mission_id()).unwrap().assessments
    }
}

/// The tip of a shipped branch, and the head of its pull request.
const TIP: &str = "c0ffee";

fn shipped() -> Delivery {
    Delivery {
        commits: 3,
        push: PushState::UpToDate,
        pull_request: Some(PullRequest {
            number: 42,
            url: "https://example.com/pull/42".into(),
            state: PrState::Open,
            checks: ChecksState::Passing,
            head: Some(TIP.into()),
        }),
        tip: Some(TIP.into()),
        ..Delivery::default()
    }
}

#[test]
fn a_mission_with_workers_starts_running() {
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.set_state(1, AgentState::Working);
    let decision = world.step();
    assert_eq!(world.status(), MissionStatus::Running);
    assert_eq!(decision.notices, []);
}

#[test]
fn working_workers_are_not_judged() {
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.set_state(1, AgentState::Working);
    world.step();
    assert_eq!(world.step(), Decision::default());
}

#[test]
fn a_worker_that_stops_without_pushing_is_reported_to_the_coordinator() {
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.set_delivery(
        1,
        Delivery {
            commits: 2,
            push: PushState::Ahead { commits: 2 },
            ..Delivery::default()
        },
    );
    world.set_state(1, AgentState::Done);
    let decision = world.step();
    let notice = &decision.notices[0];
    assert_eq!(notice.to, [Party::Agent(agent(COORDINATOR))]);
    assert_eq!(notice.kind, MessageKind::Alert);
    assert!(
        notice.body.contains("commits not pushed"),
        "{}",
        notice.body
    );
    assert!(notice.body.contains("no open pull request"));
    let assessment = &world.registry.mission(mission_id()).unwrap().assessments[&agent(1)];
    assert_eq!(assessment.verdict, Verdict::NotDelivered);
}

#[test]
fn supervising_twice_says_nothing_new() {
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.set_state(1, AgentState::Done);
    assert!(!world.step().is_empty());
    assert_eq!(world.step(), Decision::default());
}

#[test]
fn new_evidence_brings_a_new_verdict() {
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.register(2, Role::Worker, "api");
    world.set_state(1, AgentState::Done);
    world.set_state(2, AgentState::Working);
    world.step();
    world.set_delivery(1, shipped());
    let decision = world.step();
    assert_eq!(decision.notices.len(), 1);
    assert_eq!(decision.notices[0].kind, MessageKind::Report);
    assert!(decision.notices[0].body.contains("PR #42 open"));
    assert_eq!(world.status(), MissionStatus::Running);
}

#[test]
fn when_every_worker_delivered_the_coordinator_is_asked_for_the_report() {
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.register(2, Role::Worker, "api");
    for n in [1, 2] {
        world.set_delivery(n, shipped());
        world.set_state(n, AgentState::Idle);
    }
    let decision = world.step();
    assert_eq!(world.status(), MissionStatus::Reporting);
    let last = decision.notices.last().unwrap();
    assert!(last.body.contains("All 2 workers"), "{}", last.body);
    assert!(last.to.contains(&Party::Human));
    assert!(last.to.contains(&Party::Agent(agent(COORDINATOR))));
    assert_eq!(world.step(), Decision::default());
}

#[test]
fn a_delivered_worker_working_again_keeps_the_mission_reporting() {
    // Seen in a real mission: a worker that delivered was woken to answer
    // its coordinator, and the mission bounced back to running, then asked
    // for the final report a second time.
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.set_delivery(1, shipped());
    world.set_state(1, AgentState::Idle);
    world.step();
    assert_eq!(world.status(), MissionStatus::Reporting);
    world.set_state(1, AgentState::Working);
    assert_eq!(world.step(), Decision::default());
    assert_eq!(world.status(), MissionStatus::Reporting);
    world.set_state(1, AgentState::Idle);
    assert_eq!(world.step(), Decision::default(), "no second request");
}

#[test]
fn a_regression_moves_the_mission_back_to_running() {
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.set_delivery(1, shipped());
    world.set_state(1, AgentState::Idle);
    world.step();
    assert_eq!(world.status(), MissionStatus::Reporting);
    // It works again and stops with changes it did not commit.
    world.set_state(1, AgentState::Working);
    world.step();
    let mut dirty = shipped();
    dirty.dirty = true;
    world.set_delivery(1, dirty);
    world.set_state(1, AgentState::Idle);
    world.step();
    assert_eq!(world.status(), MissionStatus::Running);
    assert_eq!(
        world.assessments()[&agent(1)].verdict,
        Verdict::NotDelivered
    );
}

#[test]
fn a_failed_worker_alerts_the_human_too() {
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.set_state(1, AgentState::Failed);
    let decision = world.step();
    assert!(decision.notices[0].to.contains(&Party::Human));
    assert!(decision.notices[0].body.contains("failed"));
}

#[test]
fn a_merged_pull_request_is_delivery_even_without_new_commits() {
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    let mut delivery = shipped();
    delivery.commits = 0;
    delivery.pull_request.as_mut().unwrap().state = PrState::Merged;
    world.set_delivery(1, delivery);
    world.set_state(1, AgentState::Stopped);
    world.step();
    let assessment = &world.registry.mission(mission_id()).unwrap().assessments[&agent(1)];
    assert_eq!(assessment.verdict, Verdict::Delivered);
}

#[test]
fn a_pull_request_at_another_commit_proves_nothing() {
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    // A merged pull request whose head is not the branch's tip: an old one
    // of a reused branch name, or commits made after it.
    let mut delivery = shipped();
    delivery.commits = 0;
    let pr = delivery.pull_request.as_mut().unwrap();
    pr.state = PrState::Merged;
    pr.head = Some("0dd5ed".into());
    world.set_delivery(1, delivery);
    world.set_state(1, AgentState::Stopped);
    world.step();
    let assessment = &world.registry.mission(mission_id()).unwrap().assessments[&agent(1)];
    assert_eq!(assessment.verdict, Verdict::NotDelivered);
    assert!(
        assessment.evidence.contains("no open pull request"),
        "{}",
        assessment.evidence
    );
}

#[test]
fn an_analysis_worker_delivers_with_a_report() {
    let mut world = World::new(MissionKind::Analysis);
    world.register(1, Role::Worker, "billing");
    world.set_state(1, AgentState::Done);
    world.step();
    assert_eq!(
        world.registry.mission(mission_id()).unwrap().assessments[&agent(1)].verdict,
        Verdict::NotDelivered
    );
    world.apply(EventKind::MessagePosted {
        message: Message {
            id: MessageId::from_ulid(Ulid::from_parts(3_000, 1)),
            thread: thread_id(),
            from: Party::Agent(agent(1)),
            kind: MessageKind::Report,
            body: "Three high severity advisories.".into(),
            refs: vec![],
            in_reply_to: None,
            at: Timestamp::from_millis(99),
            receipts: vec![Receipt {
                to: Party::Agent(agent(COORDINATOR)),
                delivered_at: None,
                read_at: None,
            }],
        },
    });
    world.step();
    assert_eq!(world.status(), MissionStatus::Reporting);
}

#[test]
fn closed_missions_are_left_alone() {
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.set_state(1, AgentState::Failed);
    world.apply(EventKind::MissionUpdated {
        mission: mission_id(),
        status: MissionStatus::Aborted,
        coordinator: None,
        report: None,
    });
    assert_eq!(world.step(), Decision::default());
}

#[test]
fn the_coordinator_itself_is_never_judged() {
    let mut world = World::new(MissionKind::Change);
    world.set_state(COORDINATOR, AgentState::Done);
    let decision = world.step();
    assert_eq!(decision, Decision::default());
    assert_eq!(world.status(), MissionStatus::Planning);
}

#[test]
fn a_stopped_worker_is_judged_on_evidence_checked_after_it_stopped() {
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.set_state(1, AgentState::Working);
    let while_working = world.now();
    world.set_delivery(
        1,
        Delivery {
            checked_at: Some(while_working),
            ..Delivery::default()
        },
    );
    world.set_state(1, AgentState::Done);
    // Evidence from before the stop would say "no commits": wait.
    world.step();
    assert!(world.assessments().is_empty());

    let after_stop = world.now();
    world.set_delivery(
        1,
        Delivery {
            checked_at: Some(after_stop),
            ..shipped()
        },
    );
    let decision = world.step();
    assert_eq!(decision.notices.len(), 2);
    assert_eq!(decision.notices[0].kind, MessageKind::Report);
    assert_eq!(world.assessments()[&agent(1)].verdict, Verdict::Delivered);
    assert_eq!(world.status(), MissionStatus::Reporting);
}

#[test]
fn a_stale_check_holds_the_mission_back_from_reporting() {
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.set_delivery(
        1,
        Delivery {
            checked_at: Some(world.now()),
            ..shipped()
        },
    );
    world.set_state(1, AgentState::Stopped);
    world.step();
    // Even good evidence waits for a check after the stop.
    assert!(world.assessments().is_empty());
    assert_ne!(world.status(), MissionStatus::Reporting);
}

#[test]
fn analysis_workers_do_not_wait_for_git_evidence() {
    let mut world = World::new(MissionKind::Analysis);
    world.register(1, Role::Worker, "billing");
    world.set_delivery(
        1,
        Delivery {
            checked_at: Some(world.now()),
            ..Delivery::default()
        },
    );
    world.set_state(1, AgentState::Done);
    world.step();
    assert_eq!(
        world.assessments()[&agent(1)].verdict,
        Verdict::NotDelivered
    );
}
