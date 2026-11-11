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

    /// Moves an agent to `state`. An agent that comes to rest has worked
    /// first, as an agent does: one that never did is seen working before.
    fn set_state(&mut self, n: u128, state: AgentState) {
        let rested = state == AgentState::Idle
            && self
                .registry
                .agent(agent(n))
                .is_some_and(|agent| !agent.worked);
        if rested {
            self.starts(n, AgentState::Working);
        }
        self.starts(n, state);
    }

    /// Moves an agent to `state`, and nothing more.
    fn starts(&mut self, n: u128, state: AgentState) {
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

    /// The daemon reads an agent's evidence, as it does when the agent
    /// stops: `delivery`, read now.
    fn reads(&mut self, n: u128, delivery: Delivery) {
        let checked_at = Some(self.now());
        self.set_delivery(
            n,
            Delivery {
                checked_at,
                ..delivery
            },
        );
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

    /// Posts message `n` in the mission's thread, and returns its id.
    #[allow(clippy::too_many_arguments)] // Each argument is one field of the message.
    fn post(
        &mut self,
        n: u128,
        from: Party,
        to: &[Party],
        kind: MessageKind,
        body: &str,
        in_reply_to: Option<MessageId>,
        options: &[&str],
    ) -> MessageId {
        let id = MessageId::from_ulid(Ulid::from_parts(3_000, n));
        self.apply(EventKind::MessagePosted {
            message: Message {
                id,
                thread: thread_id(),
                from,
                kind,
                body: body.to_owned(),
                refs: vec![],
                in_reply_to,
                options: options.iter().map(|option| (*option).to_owned()).collect(),
                at: Timestamp::from_millis(self.clock),
                receipts: to
                    .iter()
                    .map(|party| Receipt {
                        to: *party,
                        delivered_at: None,
                        read_at: None,
                    })
                    .collect(),
            },
        });
        id
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
fn a_worker_that_rests_without_having_worked_is_not_judged() {
    // Seen in a real mission: a session that just started rests at its
    // prompt for a moment before its task is submitted, and the coordinator
    // was told that its worker had stopped without a commit.
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.starts(1, AgentState::Idle);
    world.set_delivery(
        1,
        Delivery {
            checked_at: Some(world.now()),
            ..Delivery::default()
        },
    );
    let decision = world.step();
    assert_eq!(decision.notices, []);
    assert!(world.assessments().is_empty());
    assert_eq!(world.status(), MissionStatus::Running);
    // It takes its task and comes to rest with nothing to show: now it
    // stopped without delivering.
    world.starts(1, AgentState::Working);
    world.starts(1, AgentState::Idle);
    world.set_delivery(
        1,
        Delivery {
            checked_at: Some(world.now()),
            ..Delivery::default()
        },
    );
    let decision = world.step();
    assert_eq!(
        world.assessments()[&agent(1)].verdict,
        Verdict::NotDelivered
    );
    assert!(
        decision.notices[0]
            .body
            .contains("stopped without delivering"),
        "{}",
        decision.notices[0].body
    );
}

#[test]
fn a_worker_that_ended_its_session_without_working_is_judged() {
    // Stopped is not resting: its session is over, whatever it did.
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.starts(1, AgentState::Stopped);
    world.reads(1, Delivery::default());
    world.step();
    assert_eq!(
        world.assessments()[&agent(1)].verdict,
        Verdict::NotDelivered
    );
}

#[test]
fn a_worker_is_judged_on_evidence_read_since_it_stopped() {
    // A worker commits, reports and stops before volee ever read its branch
    // (it lived less than a refresh interval): until the read its stop
    // causes, there is nothing to judge, and it is never said to have no
    // commits.
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.set_state(1, AgentState::Idle);
    assert_eq!(world.step().notices, []);
    assert!(world.assessments().is_empty());
    world.reads(
        1,
        Delivery {
            commits: 1,
            push: PushState::NoRemote,
            ..Delivery::default()
        },
    );
    let decision = world.step();
    assert!(
        decision
            .notices
            .iter()
            .all(|notice| !notice.body.contains("stopped without delivering")),
        "{:?}",
        decision.notices
    );
    assert_eq!(decision.notices[0].kind, MessageKind::Report);
    assert!(
        decision.notices[0].body.contains("delivered: 1 commit"),
        "{}",
        decision.notices[0].body
    );
    // Evidence read before the stop is not judged either.
    world.set_state(1, AgentState::Working);
    world.reads(1, Delivery::default());
    world.set_state(1, AgentState::Idle);
    assert_eq!(world.step(), Decision::default());
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
    world.set_state(1, AgentState::Done);
    world.reads(
        1,
        Delivery {
            commits: 2,
            push: PushState::Ahead { commits: 2 },
            ..Delivery::default()
        },
    );
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
    world.reads(1, Delivery::default());
    assert!(!world.step().is_empty());
    assert_eq!(world.step(), Decision::default());
}

#[test]
fn new_evidence_brings_a_new_verdict() {
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.register(2, Role::Worker, "api");
    world.set_state(1, AgentState::Done);
    world.reads(1, Delivery::default());
    world.set_state(2, AgentState::Working);
    world.step();
    world.reads(1, shipped());
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
        world.set_state(n, AgentState::Idle);
        world.reads(n, shipped());
    }
    let decision = world.step();
    assert_eq!(world.status(), MissionStatus::Reporting);
    let last = decision.notices.last().unwrap();
    assert!(last.body.contains("All 2 workers"), "{}", last.body);
    assert!(last.to.contains(&Party::Human));
    assert!(last.to.contains(&Party::Agent(agent(COORDINATOR))));
    assert_eq!(world.step(), Decision::default());
}

/// A change mission whose two workers delivered: the coordinator is asked
/// for its report.
fn ready(kind: MissionKind) -> World {
    let mut world = World::new(kind);
    world.register(1, Role::Worker, "billing");
    world.register(2, Role::Worker, "api");
    for n in [1, 2] {
        world.set_state(n, AgentState::Idle);
        world.reads(n, shipped());
        if kind == MissionKind::Analysis {
            world.post(
                100 + n,
                Party::Agent(agent(n)),
                &[Party::Agent(agent(COORDINATOR))],
                MessageKind::Report,
                "Findings.",
                None,
                &[],
            );
        }
    }
    world.step();
    assert_eq!(world.status(), MissionStatus::Reporting);
    world
}

/// The coordinator presents its report: the question that asks for the
/// person's approval, recorded as the mission's report.
fn present(world: &mut World, n: u128) -> MessageId {
    let kind = world.registry.mission(mission_id()).unwrap().kind;
    let options = kind.approval_options();
    let options: Vec<&str> = options.iter().map(String::as_str).collect();
    let question = world.post(
        n,
        Party::Agent(agent(COORDINATOR)),
        &[Party::Human],
        MessageKind::Question,
        "Mission \"Invoicing\" is ready.\nDone: invoices.\nProof: two pull requests.\nLeft: nothing.",
        None,
        &options,
    );
    world.apply(EventKind::MissionUpdated {
        mission: mission_id(),
        status: MissionStatus::AwaitingApproval,
        coordinator: None,
        report: Some(question),
    });
    question
}

/// The person answers a question.
fn answer(world: &mut World, n: u128, question: MessageId, words: &str) {
    world.post(
        n,
        Party::Human,
        &[Party::Agent(agent(COORDINATOR))],
        MessageKind::Answer,
        words,
        Some(question),
        &[],
    );
}

#[test]
fn the_coordinator_is_asked_to_present_its_report_and_to_merge_nothing() {
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.set_state(1, AgentState::Idle);
    world.reads(1, shipped());
    let decision = world.step();
    let last = decision.notices.last().unwrap();
    assert!(last.body.contains("`request_approval`"), "{}", last.body);
    assert!(
        last.body
            .contains("You merge nothing and close nothing: volee does, once the human approved"),
        "{}",
        last.body
    );
}

#[test]
fn waiting_for_the_person_s_answer_says_nothing() {
    let mut world = ready(MissionKind::Change);
    present(&mut world, 1);
    assert_eq!(world.step(), Decision::default());
    assert_eq!(world.status(), MissionStatus::AwaitingApproval);
}

#[test]
fn ok_merge_approves_and_tells_the_coordinator_that_volee_merges() {
    let mut world = ready(MissionKind::Change);
    let question = present(&mut world, 1);
    answer(&mut world, 2, question, "ok merge");
    let decision = world.step();
    assert_eq!(world.status(), MissionStatus::Approved);
    let [notice] = &decision.notices[..] else {
        panic!("one notice: {:?}", decision.notices);
    };
    assert_eq!(notice.to, [Party::Agent(agent(COORDINATOR))]);
    assert!(
        notice.body.contains("The human approved"),
        "{}",
        notice.body
    );
    assert!(
        notice
            .body
            .contains("volee merges every worker's work itself"),
        "{}",
        notice.body
    );
    assert!(notice.body.contains("merge nothing"), "{}", notice.body);
    // No command to run: the merge is not the coordinator's.
    for command in ["gh pr", "git merge", "`"] {
        assert!(!notice.body.contains(command), "{}", notice.body);
    }
    // Said once.
    assert_eq!(world.step(), Decision::default());
    assert_eq!(world.status(), MissionStatus::Approved);
}

#[test]
fn the_person_s_spelling_of_the_approval_does_not_matter() {
    let mut world = ready(MissionKind::Change);
    let question = present(&mut world, 1);
    answer(&mut world, 2, question, "  OK Merge ");
    world.step();
    assert_eq!(world.status(), MissionStatus::Approved);
}

#[test]
fn no_keeps_the_mission_open_and_nothing_is_merged() {
    let mut world = ready(MissionKind::Change);
    let question = present(&mut world, 1);
    answer(&mut world, 2, question, "no");
    let decision = world.step();
    assert_eq!(world.status(), MissionStatus::Reporting);
    let [notice] = &decision.notices[..] else {
        panic!("one notice: {:?}", decision.notices);
    };
    assert_eq!(notice.to, [Party::Agent(agent(COORDINATOR))]);
    assert!(
        notice.body.contains("did not approve") && notice.body.contains("they answered \"no\""),
        "{}",
        notice.body
    );
    assert!(notice.body.contains("Nothing is merged"), "{}", notice.body);
    assert!(!notice.body.contains("gh pr merge"), "{}", notice.body);
    assert_eq!(world.step(), Decision::default());
}

#[test]
fn a_sentence_that_contains_the_approval_does_not_approve() {
    for words in [
        "ok merge, but only billing",
        "not ok merge",
        "ok",
        "yes",
        "ok merge\nand then deploy",
    ] {
        let mut world = ready(MissionKind::Change);
        let question = present(&mut world, 1);
        answer(&mut world, 2, question, words);
        let decision = world.step();
        assert_eq!(world.status(), MissionStatus::Reporting, "{words:?}");
        assert!(
            decision.notices[0].body.contains("did not approve"),
            "{words:?}: {}",
            decision.notices[0].body
        );
        // The person's words reach the coordinator on one line.
        assert!(!decision.notices[0].body.contains('\n'), "{words:?}");
    }
}

#[test]
fn only_the_person_s_answer_approves() {
    let mut world = ready(MissionKind::Change);
    let question = present(&mut world, 1);
    // A worker, or the coordinator itself, replies "ok merge".
    for (n, from) in [(2, agent(1)), (3, agent(COORDINATOR))] {
        world.post(
            n,
            Party::Agent(from),
            &[Party::Human],
            MessageKind::Answer,
            "ok merge",
            Some(question),
            &[],
        );
    }
    assert_eq!(world.step(), Decision::default());
    assert_eq!(world.status(), MissionStatus::AwaitingApproval);
}

#[test]
fn an_answer_to_an_older_report_approves_nothing() {
    let mut world = ready(MissionKind::Change);
    let first = present(&mut world, 1);
    answer(&mut world, 2, first, "no");
    world.step();
    // A second report; the person then says "ok merge" to the first one.
    let second = present(&mut world, 3);
    world.post(
        4,
        Party::Human,
        &[Party::Agent(agent(COORDINATOR))],
        MessageKind::Answer,
        "ok merge",
        Some(first),
        &[],
    );
    // The first answer to the first report was "no": it stays its answer,
    // and the second report still waits.
    assert_eq!(world.step(), Decision::default());
    assert_eq!(world.status(), MissionStatus::AwaitingApproval);
    answer(&mut world, 5, second, "ok merge");
    world.step();
    assert_eq!(world.status(), MissionStatus::Approved);
}

#[test]
fn an_analysis_mission_is_approved_with_ok_close_and_has_nothing_to_merge() {
    let mut world = ready(MissionKind::Analysis);
    let question = present(&mut world, 1);
    // The approval of a change mission is not this question's.
    answer(&mut world, 2, question, "ok close");
    let decision = world.step();
    assert_eq!(world.status(), MissionStatus::Approved);
    let body = &decision.notices[0].body;
    assert!(
        body.contains("volee closes the mission and stops you"),
        "{body}"
    );
    assert!(!body.contains("merge"), "{body}");
}

#[test]
fn work_that_changes_under_a_presented_report_voids_it() {
    let mut world = ready(MissionKind::Change);
    let question = present(&mut world, 1);
    // A worker works again and stops with changes it did not commit.
    world.set_state(1, AgentState::Working);
    world.step();
    let mut dirty = shipped();
    dirty.dirty = true;
    world.set_state(1, AgentState::Idle);
    world.reads(1, dirty);
    let decision = world.step();
    assert_eq!(world.status(), MissionStatus::Running);
    let alert = decision.notices.last().unwrap();
    assert_eq!(alert.kind, MessageKind::Alert);
    assert!(alert.to.contains(&Party::Human));
    assert!(
        alert.body.contains("an answer to it approves nothing"),
        "{}",
        alert.body
    );
    // The person answers the old question anyway: nothing is approved.
    answer(&mut world, 2, question, "ok merge");
    world.step();
    assert_eq!(world.status(), MissionStatus::Running);
    // Delivered again: the coordinator is asked for a new report.
    world.reads(1, shipped());
    let again = world.step();
    assert_eq!(world.status(), MissionStatus::Reporting);
    assert!(
        again
            .notices
            .last()
            .unwrap()
            .body
            .contains("`request_approval`"),
        "{:?}",
        again.notices
    );
    // And the old answer still approves nothing.
    assert_eq!(world.step(), Decision::default());
}

#[test]
fn merges_seen_after_the_approval_keep_the_mission_approved() {
    let mut world = ready(MissionKind::Change);
    let question = present(&mut world, 1);
    answer(&mut world, 2, question, "ok merge");
    world.step();
    let mut merged = shipped();
    merged.commits = 0;
    merged.pull_request.as_mut().unwrap().state = PrState::Merged;
    world.reads(1, merged);
    let decision = world.step();
    assert_eq!(world.status(), MissionStatus::Approved);
    // The coordinator learns that the merge is seen.
    assert_eq!(decision.notices.len(), 1);
    assert!(
        decision.notices[0].body.contains("PR #42 merged"),
        "{}",
        decision.notices[0].body
    );
}

#[test]
fn in_a_repository_without_a_remote_committed_work_is_delivered() {
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    let local = |commits, dirty| Delivery {
        commits,
        dirty,
        push: PushState::NoRemote,
        tip: Some(TIP.into()),
        ..Delivery::default()
    };
    // Nothing committed yet.
    world.set_state(1, AgentState::Idle);
    world.reads(1, local(0, false));
    world.step();
    assert_eq!(
        world.assessments()[&agent(1)].verdict,
        Verdict::NotDelivered
    );
    assert_eq!(world.assessments()[&agent(1)].evidence, "no commits");
    // Committed, with changes left.
    world.reads(1, local(2, true));
    world.step();
    assert_eq!(
        world.assessments()[&agent(1)].evidence,
        "uncommitted changes"
    );
    // Committed and clean: no push and no pull request are asked for.
    world.reads(1, local(2, false));
    world.step();
    assert_eq!(world.assessments()[&agent(1)].verdict, Verdict::Delivered);
    assert_eq!(
        world.assessments()[&agent(1)].evidence,
        "2 commits \u{b7} no remote"
    );
    assert_eq!(world.status(), MissionStatus::Reporting);
    // The same evidence with a remote is not delivered.
    let mut with_remote = local(2, false);
    with_remote.push = PushState::NoUpstream;
    world.reads(1, with_remote);
    world.step();
    assert_eq!(
        world.assessments()[&agent(1)].evidence,
        "commits not pushed, no open pull request"
    );
}

#[test]
fn a_delivered_worker_working_again_keeps_the_mission_reporting() {
    // Seen in a real mission: a worker that delivered was woken to answer
    // its coordinator, and the mission bounced back to running, then asked
    // for the final report a second time.
    let mut world = World::new(MissionKind::Change);
    world.register(1, Role::Worker, "billing");
    world.set_state(1, AgentState::Idle);
    world.reads(1, shipped());
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
    world.set_state(1, AgentState::Idle);
    world.reads(1, shipped());
    world.step();
    assert_eq!(world.status(), MissionStatus::Reporting);
    // It works again and stops with changes it did not commit.
    world.set_state(1, AgentState::Working);
    world.step();
    let mut dirty = shipped();
    dirty.dirty = true;
    world.set_state(1, AgentState::Idle);
    world.reads(1, dirty);
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
    world.set_state(1, AgentState::Stopped);
    world.reads(1, delivery);
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
    world.set_state(1, AgentState::Stopped);
    world.reads(1, delivery);
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
            options: Vec::new(),
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
