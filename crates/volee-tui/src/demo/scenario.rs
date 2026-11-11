//! The demo scenario: a fictional team of agents, as events.
//!
//! Everything is a pure function of the start time, so a given start always
//! produces the same events. The history (before the start) sets the scene;
//! the beats (after it) loop every [`PERIOD_SECS`] seconds and change a state
//! or an activity every two seconds.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ulid::Ulid;
use volee_core::delivery::{ChecksState, Delivery, DiffStat, PrState, PullRequest, PushState};
use volee_core::event::{Event, EventKind};
use volee_core::harness::{HarnessKind, SessionKey, SessionOrigin};
use volee_core::message::{Message, MessageKind, Party, Receipt, Thread};
use volee_core::mission::{Mission, MissionKind, MissionStatus, Verdict};
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::{AgentState, InputRequest};
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};
use volee_core::{AgentId, MessageId, MissionId, ThreadId};

/// The length of one loop of beats.
pub(crate) const PERIOD_SECS: i64 = 40;

/// Seeds the random part of the demo's identifiers, so they are stable.
const SEED: u128 = 0x00de_0000_0000_0000_0000;

/// The fictional home directory of the demo.
pub(crate) const HOME: &str = "/home/dev";

// The tips of the demo's pushed branches. A pull request counts as evidence
// only at its branch's tip, so the demo's pull requests have them as heads.
const INVOICE_PAGE_TIP: &str = "9c41d07e2b5a8f63c0e1d4b7a2f95e8c6d3b1a04";
const INFRA_TIP: &str = "5e7a1c93d0b24f86a1c3e5d7f9b2a4c6e8d0f1a3";
const WEBSITE_TIP: &str = "b3d5f7a9c1e3052749e6a8c0d2f4b6a8c0e2d4f6";

// Members of the cast, by index.
pub(crate) const TOWER: usize = 0;
pub(crate) const INVOICING: usize = 1;
pub(crate) const INVOICE_SCHEMA: usize = 2;
pub(crate) const INVOICE_TOOLS: usize = 3;
pub(crate) const INVOICE_PAGE: usize = 4;
pub(crate) const LATENCY_AUDIT: usize = 5;
pub(crate) const TRACE_GATEWAY: usize = 6;
pub(crate) const TRACE_SEARCH: usize = 7;
pub(crate) const EXPLORE_HANDLERS: usize = 8;
pub(crate) const EXPLORE_TRACES: usize = 9;
pub(crate) const BROKEN_LINKS: usize = 10;
pub(crate) const INFRA: usize = 11;

/// One agent of the demo.
pub(crate) struct Member {
    pub(crate) name: &'static str,
    pub(crate) role: Role,
    pub(crate) parent: Option<usize>,
    /// 0 for Invoicing, 1 for Latency audit.
    pub(crate) mission: Option<usize>,
    pub(crate) repo: Option<&'static str>,
    pub(crate) branch: Option<&'static str>,
    /// Runs in Claude Code's background supervisor rather than a terminal.
    pub(crate) background: bool,
    pub(crate) model: &'static str,
    /// Seconds before the start when the agent appeared.
    pub(crate) born: i64,
    /// Seconds before the start when its activity stopped (0: still active).
    pub(crate) quiet: i64,
    /// Average seconds between two activities.
    pub(crate) pace: i64,
    pub(crate) activities: &'static [&'static str],
}

/// The cast, in order of appearance in [`CAST`] (not of birth).
pub(crate) const CAST: [Member; 12] = [
    Member {
        name: "tower",
        role: Role::Lead,
        parent: None,
        mission: None,
        repo: Some("platform"),
        branch: Some("main"),
        background: true,
        model: "opus",
        born: 1_500,
        quiet: 0,
        pace: 70,
        activities: &[
            "Read docs/roadmap.md",
            "Bash volee ls",
            "Write missions/latency-audit/goal.md",
            "Read missions/invoicing/plan.md",
        ],
    },
    Member {
        name: "invoicing",
        role: Role::Coordinator,
        parent: Some(TOWER),
        mission: Some(0),
        repo: None,
        branch: None,
        background: true,
        model: "sonnet",
        born: 1_080,
        quiet: 840,
        pace: 35,
        activities: &[
            "Write tasks/billing.md",
            "Write tasks/mcp-server.md",
            "Write tasks/dashboard.md",
            "Bash volee spawn --repo billing",
        ],
    },
    Member {
        name: "invoice-schema",
        role: Role::Worker,
        parent: Some(INVOICING),
        mission: Some(0),
        repo: Some("billing"),
        branch: Some("feat/invoices"),
        background: true,
        model: "sonnet",
        born: 960,
        quiet: 125,
        pace: 30,
        activities: &[
            "Read src/db/schema.ts",
            "Edit src/db/schema.ts",
            "Edit src/invoices/totals.ts",
            "Bash bun test invoices",
            "Write migrations/0042_invoices.sql",
        ],
    },
    Member {
        name: "invoice-tools",
        role: Role::Worker,
        parent: Some(INVOICING),
        mission: Some(0),
        repo: Some("mcp-server"),
        branch: Some("feat/invoice-tools"),
        background: true,
        model: "sonnet",
        born: 955,
        quiet: 65,
        pace: 45,
        activities: &[
            "Read src/tools/index.ts",
            "Edit src/tools/list_invoices.ts",
            "Bash bun test tools",
            "Read docs/contract.md",
        ],
    },
    Member {
        name: "invoice-page",
        role: Role::Worker,
        parent: Some(INVOICING),
        mission: Some(0),
        repo: Some("dashboard"),
        branch: Some("feat/invoices-page"),
        background: true,
        model: "sonnet",
        born: 950,
        quiet: 365,
        pace: 28,
        activities: &[
            "Edit app/invoices/page.tsx",
            "Edit app/invoices/[id]/page.tsx",
            "Bash bun run build",
            "Bash gh pr create",
        ],
    },
    Member {
        name: "latency-audit",
        role: Role::Coordinator,
        parent: Some(TOWER),
        mission: Some(1),
        repo: None,
        branch: None,
        background: true,
        model: "sonnet",
        born: 780,
        quiet: 720,
        pace: 20,
        activities: &["Write tasks/gateway.md", "Write tasks/search.md"],
    },
    Member {
        name: "trace-gateway",
        role: Role::Worker,
        parent: Some(LATENCY_AUDIT),
        mission: Some(1),
        repo: Some("gateway"),
        branch: Some("audit/p95-latency"),
        background: true,
        model: "sonnet",
        born: 700,
        quiet: 0,
        pace: 40,
        activities: &[
            "Grep \"p95\" src/",
            "Read config/limits.toml",
            "Read src/middleware/auth.rs",
        ],
    },
    Member {
        name: "trace-search",
        role: Role::Worker,
        parent: Some(LATENCY_AUDIT),
        mission: Some(1),
        repo: Some("search"),
        branch: Some("audit/p95-latency"),
        background: true,
        model: "haiku",
        born: 695,
        quiet: 0,
        pace: 50,
        activities: &[
            "Read src/query/plan.rs",
            "Read benches/results.json",
            "Bash cargo bench --bench query",
        ],
    },
    Member {
        name: "explore-handlers",
        role: Role::Subagent,
        parent: Some(TRACE_GATEWAY),
        mission: Some(1),
        repo: Some("gateway"),
        branch: Some("audit/p95-latency"),
        background: true,
        model: "haiku",
        born: 420,
        quiet: 0,
        pace: 22,
        activities: &[
            "Read src/handlers/search.rs",
            "Grep \"timeout\" src/handlers",
            "Grep \"p95\" src/handlers",
        ],
    },
    Member {
        name: "explore-traces",
        role: Role::Subagent,
        parent: Some(TRACE_GATEWAY),
        mission: Some(1),
        repo: Some("gateway"),
        branch: Some("audit/p95-latency"),
        background: true,
        model: "haiku",
        born: 40,
        quiet: 0,
        pace: 12,
        activities: &["Grep \"auth\" traces/", "Read traces/2026-11-05.jsonl"],
    },
    Member {
        name: "broken-links",
        role: Role::Independent,
        parent: None,
        mission: None,
        repo: Some("website"),
        branch: Some("fix/broken-links"),
        background: true,
        model: "sonnet",
        born: 1_140,
        quiet: 0,
        pace: 85,
        activities: &[
            "Bash npm run check-links",
            "Read docs/guide/install.md",
            "Edit docs/guide/links.md",
        ],
    },
    Member {
        name: "infra",
        role: Role::Independent,
        parent: None,
        mission: None,
        repo: Some("infra"),
        branch: Some("chore/load-balancer"),
        background: false,
        model: "opus",
        born: 1_800,
        quiet: 1_500,
        pace: 60,
        activities: &["Read main.tf", "Bash terraform plan"],
    },
];

/// One mission of the demo.
struct MissionSpec {
    title: &'static str,
    kind: MissionKind,
    goal: &'static str,
    repos: &'static [&'static str],
    coordinator: usize,
    /// Seconds before the start when the mission was created.
    born: i64,
}

/// The missions of the demo.
const MISSIONS: [MissionSpec; 2] = [
    MissionSpec {
        title: "Invoicing",
        kind: MissionKind::Change,
        goal: "Add invoices: the model and migration in billing, MCP tools in mcp-server, \
               and a page in dashboard.",
        repos: &["billing", "mcp-server", "dashboard"],
        coordinator: INVOICING,
        born: 1_085,
    },
    MissionSpec {
        title: "Latency audit",
        kind: MissionKind::Analysis,
        goal: "Find why the p95 latency of search requests doubled since last week. Read only.",
        repos: &["gateway", "search"],
        coordinator: LATENCY_AUDIT,
        born: 785,
    },
];

/// The identifier of a cast member, stable for a given start.
pub(crate) fn agent_id(start: Timestamp, member: usize) -> AgentId {
    let born = start.plus_millis(-CAST[member].born * 1_000);
    AgentId::from_ulid(ulid_at(born, 0x0a00 + member as u128))
}

/// The identifier of a mission, stable for a given start.
pub(crate) fn mission_id(start: Timestamp, mission: usize) -> MissionId {
    let born = start.plus_millis(-MISSIONS[mission].born * 1_000);
    MissionId::from_ulid(ulid_at(born, 0x0b00 + mission as u128))
}

fn thread_id(start: Timestamp, thread: usize) -> ThreadId {
    ThreadId::from_ulid(ulid_at(start, 0x0c00 + thread as u128))
}

/// The identifier of the message numbered `id` and sent `seconds` after
/// the start, as [`Script::message`] makes it.
fn message_id(start: Timestamp, seconds: i64, id: u128) -> MessageId {
    MessageId::from_ulid(ulid_at(start.plus_millis(seconds * 1_000), 0x0d00 + id))
}

/// The inbox thread, where questions to the person land.
const INBOX: usize = 2;

/// Questions answered after the start, as (seconds, number): the question
/// to the person about a draft flag, and the latency audit's question about
/// token calls, which the person relays to Invoicing.
const DRAFT_QUESTION: (i64, u128) = (-60, 6);
const TOKEN_QUESTION: (i64, u128) = (-30, 13);

/// The harness of a member. Every agent runs in Claude Code but infra, a
/// Codex CLI session in another terminal: agents of two AIs talk to each
/// other through volee.
fn harness_of(member: usize) -> HarnessKind {
    if member == INFRA {
        HarnessKind::Other("codex".to_owned())
    } else {
        HarnessKind::ClaudeCode
    }
}

/// The thread of volee's alerts to the person.
const VOLEE: usize = 7;

fn ulid_at(at: Timestamp, random: u128) -> Ulid {
    Ulid::from_parts(u64::try_from(at.as_millis()).unwrap_or(0), SEED | random)
}

/// The harness session of a member, shaped like a session id.
pub(crate) fn session_of(member: usize) -> SessionKey {
    let n = member as u64;
    SessionKey {
        harness: harness_of(member),
        id: format!(
            "{:08x}-7a1c-4e2b-9d3f-{:012x}",
            0x5eed_0a00 + n,
            0x00c0_ffee_0000 + n * 0x1111
        ),
    }
}

/// The cast member an agent identifier belongs to.
pub(crate) fn member_of(start: Timestamp, agent: AgentId) -> Option<usize> {
    (0..CAST.len()).find(|member| agent_id(start, *member) == agent)
}

fn workspace(index: usize) -> Workspace {
    let member = &CAST[index];
    match (member.repo, member.branch) {
        (Some(repo), Some(branch)) if member.role == Role::Lead => Workspace {
            cwd: PathBuf::from(format!("{HOME}/code/{repo}")),
            repo: Some(repo.to_owned()),
            repo_root: Some(PathBuf::from(format!("{HOME}/code/{repo}"))),
            worktree: None,
            branch: Some(branch.to_owned()),
            base: None,
            start_commit: None,
        },
        (Some(repo), Some(branch)) => {
            // Worktrees live in their repository, under .claude/worktrees
            // (ADR 0013); the infra session works in one its person made by
            // hand, next to the repository.
            let worktree = if index == INFRA {
                PathBuf::from(format!("{HOME}/code/{repo}-lb"))
            } else {
                PathBuf::from(format!(
                    "{HOME}/code/{repo}/.claude/worktrees/{}",
                    branch.replace('/', "-")
                ))
            };
            Workspace {
                cwd: worktree.clone(),
                repo: Some(repo.to_owned()),
                repo_root: Some(PathBuf::from(format!("{HOME}/code/{repo}"))),
                worktree: Some(worktree),
                branch: Some(branch.to_owned()),
                base: Some("main".to_owned()),
                start_commit: Some(format!("{:07x}", 0x3a9_f00 + repo.len() * 0x1d)),
            }
        }
        _ => Workspace::at(format!(
            "{HOME}/.local/state/volee/missions/{}",
            member.name
        )),
    }
}

/// Builds events at offsets from the start.
struct Script {
    start: Timestamp,
    events: Vec<Event>,
}

impl Script {
    fn at(&mut self, seconds: i64, kind: EventKind) {
        self.at_millis(seconds * 1_000, kind);
    }

    fn at_millis(&mut self, millis: i64, kind: EventKind) {
        self.events
            .push(Event::new(self.start.plus_millis(millis), kind));
    }

    fn id(&self, member: usize) -> AgentId {
        agent_id(self.start, member)
    }

    fn state(&mut self, seconds: i64, member: usize, state: AgentState, detail: Option<&str>) {
        let agent = self.id(member);
        self.at(
            seconds,
            EventKind::StateChanged {
                agent,
                state,
                detail: detail.map(str::to_owned),
            },
        );
    }

    fn activity(&mut self, millis: i64, member: usize, summary: &str) {
        let agent = self.id(member);
        self.at_millis(
            millis,
            EventKind::ActivityRecorded {
                agent,
                summary: summary.to_owned(),
            },
        );
    }

    /// New delivery evidence, checked from git and the forge as it is
    /// recorded.
    fn delivery(&mut self, seconds: i64, member: usize, mut delivery: Delivery) {
        let agent = self.id(member);
        delivery
            .checked_at
            .get_or_insert(self.start.plus_millis(seconds * 1_000));
        self.at(seconds, EventKind::DeliveryUpdated { agent, delivery });
    }

    /// The mission supervisor's verdict on a worker.
    fn verdict(&mut self, seconds: i64, member: usize, verdict: Verdict, evidence: &str) {
        let Some(mission) = CAST[member].mission else {
            return;
        };
        let worker = self.id(member);
        self.at(
            seconds,
            EventKind::WorkerAssessed {
                mission: mission_id(self.start, mission),
                worker,
                verdict,
                evidence: evidence.to_owned(),
            },
        );
    }

    #[allow(clippy::too_many_arguments)] // Each argument is one field of the message.
    fn message(
        &mut self,
        seconds: i64,
        id: u128,
        thread: usize,
        from: Party,
        to: &[Party],
        kind: MessageKind,
        body: &str,
        in_reply_to: Option<MessageId>,
    ) -> MessageId {
        let at = self.start.plus_millis(seconds * 1_000);
        let message = MessageId::from_ulid(ulid_at(at, 0x0d00 + id));
        self.at(
            seconds,
            EventKind::MessagePosted {
                message: Message {
                    id: message,
                    thread: thread_id(self.start, thread),
                    from,
                    kind,
                    body: body.to_owned(),
                    refs: Vec::new(),
                    in_reply_to,
                    at,
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
        );
        // Agents receive their messages a second later, through hooks: before
        // the next beat, so events stay in time order.
        for party in to.iter().filter(|party| matches!(party, Party::Agent(_))) {
            self.at_millis(
                seconds * 1_000 + 1_000,
                EventKind::MessageDelivered {
                    message,
                    to: *party,
                },
            );
        }
        message
    }
}

/// A small deterministic pseudo-random sequence (xorshift), so activity
/// rhythms look organic but never change between runs.
fn jitter(seed: u64, step: u64) -> u64 {
    let mut x = seed
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(step.wrapping_mul(0xbf58_476d_1ce4_e5b9))
        | 1;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

/// Everything that happened before the start, oldest first.
#[allow(clippy::too_many_lines)] // A scenario reads best as one script.
pub(crate) fn history(start: Timestamp) -> Vec<Event> {
    let mut script = Script {
        start,
        events: Vec::new(),
    };
    let party = |script: &Script, member: usize| Party::Agent(script.id(member));

    // Threads and missions.
    script.at(
        -1_800,
        EventKind::ThreadOpened {
            thread: Thread {
                id: thread_id(start, INBOX),
                title: "Inbox".to_owned(),
                mission: None,
                participants: vec![Party::Human],
                opened_at: start.plus_millis(-1_800_000),
            },
        },
    );
    for (index, spec) in MISSIONS.iter().enumerate() {
        let born = spec.born;
        let members: Vec<Party> = (0..CAST.len())
            .filter(|member| CAST[*member].mission == Some(index))
            .map(|member| party(&script, member))
            .chain([party(&script, TOWER), Party::Human])
            .collect();
        script.at(
            -born,
            EventKind::ThreadOpened {
                thread: Thread {
                    id: thread_id(start, index),
                    title: spec.title.to_owned(),
                    mission: Some(mission_id(start, index)),
                    participants: members,
                    opened_at: start.plus_millis(-born * 1_000),
                },
            },
        );
        script.at(
            -born,
            EventKind::MissionCreated {
                mission: Mission {
                    id: mission_id(start, index),
                    title: spec.title.to_owned(),
                    kind: spec.kind,
                    goal: spec.goal.to_owned(),
                    repos: spec.repos.iter().map(|repo| (*repo).to_owned()).collect(),
                    requested_by: Some(script.id(TOWER)),
                    coordinator: None,
                    thread: thread_id(start, index),
                    status: MissionStatus::Planning,
                    report: None,
                    created_at: start.plus_millis(-born * 1_000),
                    ended_at: None,
                    assessments: BTreeMap::new(),
                },
            },
        );
        let coordinator = script.id(spec.coordinator);
        script.at(
            -born + 7,
            EventKind::MissionUpdated {
                mission: mission_id(start, index),
                status: MissionStatus::Running,
                coordinator: Some(coordinator),
                report: None,
            },
        );
    }

    // Every agent appears, gets a session and starts working.
    for (index, member) in CAST.iter().enumerate() {
        let agent = script.id(index);
        let outside = member.role == Role::Independent;
        let launched_by = match member.parent {
            Some(parent) => LaunchedBy::Agent(script.id(parent)),
            None if outside => LaunchedBy::Outside,
            None => LaunchedBy::Human,
        };
        let full = workspace(index);
        script.at(
            -member.born,
            EventKind::AgentRegistered {
                agent,
                name: member.name.to_owned(),
                role: member.role,
                harness: harness_of(index),
                launched_by,
                parent: member.parent.map(|parent| script.id(parent)),
                mission: member.mission.map(|mission| mission_id(start, mission)),
                // Discovered sessions are first known by their directory only.
                workspace: if outside {
                    Workspace::at(&full.cwd)
                } else {
                    full.clone()
                },
                location: if member.background {
                    Location::Background
                } else {
                    Location::Terminal
                },
            },
        );
        // The agent of another harness joined through volee's MCP server
        // (ADR 0014): volee knows it by no session.
        if member.role != Role::Subagent && index != INFRA {
            script.at(
                -member.born,
                EventKind::SessionLinked {
                    agent,
                    session: session_of(index),
                    origin: if outside {
                        SessionOrigin::Discovered
                    } else {
                        SessionOrigin::Started
                    },
                },
            );
        }
        if outside {
            script.at(
                -member.born + 2,
                EventKind::WorkspaceUpdated {
                    agent,
                    workspace: full,
                },
            );
        }
        script.at(
            -member.born + 1,
            EventKind::ModelChanged {
                agent,
                model: member.model.to_owned(),
            },
        );
        script.state(-member.born + 1, index, AgentState::Working, None);

        // A rhythm of activities until the agent went quiet.
        let mut t = (-member.born + 3) * 1_000;
        let mut step = 0;
        while t < -member.quiet * 1_000 {
            let summary = member.activities[step % member.activities.len()];
            script.activity(t, index, summary);
            let spread = jitter(index as u64 + 1, step as u64) % 1_000;
            let pace = member.pace * 1_000;
            t += pace / 2 + pace * i64::try_from(spread).unwrap_or(0) / 1_000;
            step += 1;
        }
    }

    // How things stand at the start.
    script.state(-1_500, INFRA, AgentState::Idle, None);
    script.state(
        -840,
        INVOICING,
        AgentState::Idle,
        Some("Waiting for 3 workers"),
    );
    script.state(
        -720,
        LATENCY_AUDIT,
        AgentState::Idle,
        Some("Waiting for 2 workers"),
    );
    script.activity(-365_000, INVOICE_PAGE, "Bash gh pr create");
    script.state(
        -360,
        INVOICE_PAGE,
        AgentState::Done,
        Some("PR #42 is ready for review"),
    );
    script.activity(
        -125_000,
        INVOICE_SCHEMA,
        "Write migrations/0042_invoices.sql",
    );
    script.state(
        -120,
        INVOICE_SCHEMA,
        AgentState::NeedsInput {
            request: InputRequest::Permission {
                tool: Some("Bash".to_owned()),
            },
        },
        Some("Bash: bun run db:migrate"),
    );
    script.state(
        -60,
        INVOICE_TOOLS,
        AgentState::NeedsInput {
            request: InputRequest::Question,
        },
        Some("Should create_invoice accept a draft flag?"),
    );

    // Delivery evidence from git and the forge.
    script.delivery(
        -300,
        INVOICE_SCHEMA,
        Delivery {
            commits: 2,
            dirty: true,
            diff: DiffStat {
                files: 6,
                insertions: 214,
                deletions: 12,
            },
            push: PushState::NoUpstream,
            ..Delivery::default()
        },
    );
    script.delivery(
        -200,
        INVOICE_TOOLS,
        Delivery {
            commits: 1,
            diff: DiffStat {
                files: 3,
                insertions: 88,
                deletions: 4,
            },
            push: PushState::UpToDate,
            ..Delivery::default()
        },
    );
    let pr = |checks| PullRequest {
        number: 42,
        url: "https://git.example.com/dashboard/pull/42".to_owned(),
        state: PrState::Open,
        checks,
        head: Some(INVOICE_PAGE_TIP.to_owned()),
    };
    let page = |checks| Delivery {
        commits: 3,
        diff: DiffStat {
            files: 9,
            insertions: 163,
            deletions: 43,
        },
        push: PushState::UpToDate,
        pull_request: Some(pr(checks)),
        tip: Some(INVOICE_PAGE_TIP.to_owned()),
        ..Delivery::default()
    };
    script.delivery(-400, INVOICE_PAGE, page(ChecksState::Pending));
    script.delivery(-330, INVOICE_PAGE, page(ChecksState::Passing));

    // What the mission supervisor concluded from that evidence.
    script.verdict(
        -329,
        INVOICE_PAGE,
        Verdict::Delivered,
        "3 commits · pushed · PR #42 open, checks passing",
    );
    script.verdict(
        -119,
        INVOICE_SCHEMA,
        Verdict::NeedsHuman,
        "Bash: bun run db:migrate",
    );
    script.verdict(
        -59,
        INVOICE_TOOLS,
        Verdict::NeedsHuman,
        "Should create_invoice accept a draft flag?",
    );
    script.delivery(
        -500,
        BROKEN_LINKS,
        Delivery {
            commits: 1,
            diff: DiffStat {
                files: 4,
                insertions: 12,
                deletions: 9,
            },
            push: PushState::Ahead { commits: 1 },
            ..Delivery::default()
        },
    );
    // The infra session's load balancer change went through pull request
    // #12, which a person merged: its worktree is safe to tidy.
    let lb = |state, checks| PullRequest {
        number: 12,
        url: "https://git.example.com/infra/pull/12".to_owned(),
        state,
        checks,
        head: Some(INFRA_TIP.to_owned()),
    };
    let opened = |checks| Delivery {
        commits: 2,
        diff: DiffStat {
            files: 3,
            insertions: 58,
            deletions: 21,
        },
        push: PushState::UpToDate,
        pull_request: Some(lb(PrState::Open, checks)),
        tip: Some(INFRA_TIP.to_owned()),
        ..Delivery::default()
    };
    script.delivery(-1_620, INFRA, opened(ChecksState::Pending));
    script.delivery(-1_590, INFRA, opened(ChecksState::Passing));
    script.delivery(
        -1_200,
        INFRA,
        Delivery {
            push: PushState::UpToDate,
            merged_into_base: true,
            pull_request: Some(lb(PrState::Merged, ChecksState::Passing)),
            tip: Some(INFRA_TIP.to_owned()),
            ..Delivery::default()
        },
    );

    // Conversations.
    let workers = [INVOICE_SCHEMA, INVOICE_TOOLS, INVOICE_PAGE].map(|m| party(&script, m));
    script.message(
        -900,
        1,
        0,
        party(&script, INVOICING),
        &workers,
        MessageKind::Contract,
        "Invoice API contract: POST /invoices takes {customer_id, lines[], currency} and \
         returns {id, number, total_cents}. Amounts are integer cents; numbers look like \
         INV-2026-0042.",
        None,
    );
    let auditors = [TRACE_GATEWAY, TRACE_SEARCH].map(|m| party(&script, m));
    script.message(
        -690,
        2,
        1,
        party(&script, LATENCY_AUDIT),
        &auditors,
        MessageKind::Contract,
        "Read-only audit: find where search p95 grew since 2026-10-30. No commits; report \
         with evidence (traces, file and line).",
        None,
    );
    let question = script.message(
        -600,
        3,
        0,
        party(&script, INVOICE_PAGE),
        &[party(&script, INVOICE_SCHEMA)],
        MessageKind::Question,
        "Is total_cents computed by the API or stored by the database?",
        None,
    );
    script.message(
        -560,
        4,
        0,
        party(&script, INVOICE_SCHEMA),
        &[party(&script, INVOICE_PAGE)],
        MessageKind::Answer,
        "Stored: a generated column on invoices, recomputed when lines change.",
        Some(question),
    );
    script.message(
        -350,
        5,
        0,
        party(&script, INVOICE_PAGE),
        &[party(&script, INVOICING)],
        MessageKind::Report,
        "Invoices page done: list, detail and PDF download behind the invoices flag. \
         PR #42, checks passing.",
        None,
    );
    script.message(
        DRAFT_QUESTION.0,
        DRAFT_QUESTION.1,
        INBOX,
        party(&script, INVOICE_TOOLS),
        &[Party::Human],
        MessageKind::Question,
        "Should create_invoice accept a draft flag, or always issue a final invoice?",
        None,
    );

    // Outside a mission, an agent writes to its family, its coordinator and
    // you (ADR 0006): the sessions volee did not launch, one of them a Codex
    // CLI agent, ask you; a worker of one mission asks you about the other
    // one, and waits for the answer at the start.
    let merge = script.message(
        -1_530,
        7,
        INBOX,
        party(&script, INFRA),
        &[Party::Human],
        MessageKind::Question,
        "PR #12 is green: it moves the gateway behind the new load balancer. Shall I merge it, \
         or will you after a review?",
        None,
    );
    script.message(
        -1_210,
        8,
        INBOX,
        Party::Human,
        &[party(&script, INFRA)],
        MessageKind::Answer,
        "Reviewed: I am merging it now.",
        Some(merge),
    );
    let url = script.message(
        -450,
        9,
        INBOX,
        party(&script, BROKEN_LINKS),
        &[Party::Human],
        MessageKind::Question,
        "The docs link to /billing/invoices. Is that the final URL of the new invoices page?",
        None,
    );
    script.message(
        -420,
        12,
        INBOX,
        Party::Human,
        &[party(&script, BROKEN_LINKS)],
        MessageKind::Answer,
        "It is /invoices; /billing/invoices redirects there.",
        Some(url),
    );
    script.message(
        TOKEN_QUESTION.0,
        TOKEN_QUESTION.1,
        INBOX,
        party(&script, TRACE_GATEWAY),
        &[Party::Human],
        MessageKind::Question,
        "Can you ask Invoicing whether create_invoice fetches a token from /auth/token on every \
         call? That call is where search p95 doubled.",
        None,
    );

    // The supervisor tells the coordinator what it concluded, from volee.
    let invoicing = [party(&script, INVOICING)];
    for (seconds, id, kind, body) in [
        (
            -329,
            14,
            MessageKind::Report,
            "invoice-page delivered: 3 commits · pushed · PR #42 open, checks passing.",
        ),
        (
            -119,
            15,
            MessageKind::Alert,
            "invoice-schema waits for a person: Bash: bun run db:migrate.",
        ),
        (
            -59,
            16,
            MessageKind::Alert,
            "invoice-tools waits for a person: should create_invoice accept a draft flag?",
        ),
    ] {
        script.message(seconds, id, 0, Party::System, &invoicing, kind, body, None);
    }

    // volee's own alerts land in a thread of their own.
    script.at(
        -420,
        EventKind::ThreadOpened {
            thread: Thread {
                id: thread_id(start, VOLEE),
                title: "volee".to_owned(),
                mission: None,
                participants: vec![Party::System, Party::Human],
                opened_at: start.plus_millis(-420_000),
            },
        },
    );
    script.message(
        -420,
        40,
        VOLEE,
        Party::System,
        &[Party::Human],
        MessageKind::Alert,
        "explore-traces stopped with a message waiting; volee resumed its session to deliver it.",
        None,
    );

    let mut events = script.events;
    // A stable sort keeps the order of events written at the same second.
    events.sort_by_key(|event| event.at);
    events
}

/// The offsets of the beats within one period, in seconds.
const BEATS: [i64; 19] = [
    2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30, 32, 34, 36, 38,
];

/// The events of one beat. `cycle` counts the periods since the start.
#[allow(clippy::too_many_lines)] // One arm per beat reads best as one table.
fn beat(start: Timestamp, cycle: i64, offset: i64) -> Vec<Event> {
    let base = cycle * PERIOD_SECS + offset;
    let mut script = Script {
        start,
        events: Vec::new(),
    };
    let first = cycle == 0;
    match offset {
        2 => script.activity(
            base * 1_000,
            EXPLORE_HANDLERS,
            "Read src/handlers/search.rs",
        ),
        4 => script.activity(base * 1_000, TOWER, "Read missions/latency-audit/goal.md"),
        6 => script.activity(base * 1_000, BROKEN_LINKS, "Bash npm run check-links"),
        8 => {
            script.state(base, INVOICE_SCHEMA, AgentState::Working, None);
            script.activity(base * 1_000, INVOICE_SCHEMA, "Bash bun run db:migrate");
        }
        10 => script.activity(base * 1_000, TRACE_SEARCH, "Read benches/results.json"),
        12 => {
            script.activity(base * 1_000, EXPLORE_TRACES, "Grep \"auth\" traces/");
            script.state(
                base,
                EXPLORE_TRACES,
                AgentState::Done,
                Some("Found 3 slow spans in the auth middleware"),
            );
        }
        14 => {
            script.activity(base * 1_000, INVOICE_SCHEMA, "Edit src/invoices/totals.ts");
            let commits = 3 + u32::try_from(cycle).unwrap_or(0);
            script.delivery(
                base,
                INVOICE_SCHEMA,
                Delivery {
                    commits,
                    dirty: true,
                    diff: DiffStat {
                        files: 7,
                        insertions: 240 + 11 * commits,
                        deletions: 15,
                    },
                    push: PushState::NoUpstream,
                    ..Delivery::default()
                },
            );
        }
        16 => script.activity(base * 1_000, TRACE_GATEWAY, "Read src/middleware/auth.rs"),
        18 => {
            if first {
                let to = Party::Agent(script.id(INVOICE_TOOLS));
                script.message(
                    base,
                    10,
                    INBOX,
                    Party::Human,
                    &[to],
                    MessageKind::Answer,
                    "Accept a draft flag, false by default.",
                    Some(message_id(start, DRAFT_QUESTION.0, DRAFT_QUESTION.1)),
                );
            }
            script.state(base, INVOICE_TOOLS, AgentState::Working, None);
            script.activity(
                base * 1_000,
                INVOICE_TOOLS,
                "Edit src/tools/create_invoice.ts",
            );
        }
        20 => {
            script.state(
                base,
                TRACE_SEARCH,
                AgentState::Done,
                Some("Report sent to latency-audit"),
            );
            script.delivery(
                base,
                TRACE_SEARCH,
                Delivery {
                    report_delivered: true,
                    ..Delivery::default()
                },
            );
            if first {
                let from = Party::Agent(script.id(TRACE_SEARCH));
                let to = Party::Agent(script.id(LATENCY_AUDIT));
                script.message(
                    base,
                    11,
                    1,
                    from,
                    &[to],
                    MessageKind::Report,
                    "Search p95 is stable on its own (12 ms). The regression is upstream: \
                     the gateway auth middleware calls the token service once per request.",
                    None,
                );
                script.verdict(base, TRACE_SEARCH, Verdict::Delivered, "report sent");
            }
        }
        22 => script.activity(
            base * 1_000,
            EXPLORE_HANDLERS,
            "Grep \"timeout\" src/handlers",
        ),
        24 => script.activity(base * 1_000, INVOICE_TOOLS, "Bash bun test tools"),
        26 => script.activity(base * 1_000, BROKEN_LINKS, "Edit docs/guide/links.md"),
        28 => script.state(
            base,
            INVOICE_SCHEMA,
            AgentState::NeedsInput {
                request: InputRequest::Permission {
                    tool: Some("Bash".to_owned()),
                },
            },
            Some("Bash: bun run db:migrate"),
        ),
        30 => {
            script.state(base, TRACE_SEARCH, AgentState::Working, None);
            script.activity(base * 1_000, TRACE_SEARCH, "Bash cargo bench --bench query");
        }
        32 => {
            script.state(base, EXPLORE_TRACES, AgentState::Working, None);
            script.activity(base * 1_000, EXPLORE_TRACES, "Read traces/2026-11-05.jsonl");
        }
        34 => script.state(
            base,
            INVOICE_TOOLS,
            AgentState::NeedsInput {
                request: InputRequest::Question,
            },
            Some("Should create_invoice accept a draft flag?"),
        ),
        36 => script.activity(base * 1_000, TOWER, "Bash volee ls"),
        _ => script.activity(base * 1_000, TRACE_GATEWAY, "Grep \"p95\" src/"),
    }
    conversations(&mut script, cycle, offset);
    // Receipts come a second after their message, after the other events of
    // the beat; a stable sort keeps the rest in writing order.
    let mut events = script.events;
    events.sort_by_key(|event| event.at);
    events
}

/// The number of the person's question to invoice-tools, relaying the
/// latency audit's question 20 seconds into the first loop.
const RELAYED_QUESTION: u128 = 28;

/// What agents tell each other at a beat, so messages keep moving on the
/// network in every loop, and what the first loop delivers. Every message
/// takes a route volee allows (ADR 0006): across missions, you relay the
/// latency audit's question to Invoicing, and the audit's coordinator warns
/// the lead, who warns Invoicing's coordinator. Pull request #42 merges and
/// the session started by hand opens pull request #7.
#[allow(clippy::too_many_lines)] // One arm per beat reads best as one table.
fn conversations(script: &mut Script, cycle: i64, offset: i64) {
    let base = cycle * PERIOD_SECS + offset;
    let first = cycle == 0;
    let start = script.start;
    let party = |script: &Script, member| Party::Agent(script.id(member));
    match offset {
        16 => {
            let (from, to) = (party(script, TRACE_GATEWAY), party(script, LATENCY_AUDIT));
            script.message(
                base,
                20,
                1,
                from,
                &[to],
                MessageKind::Note,
                "Three slow spans in the auth middleware so far; writing them up with traces.",
                None,
            );
        }
        20 if !first => {
            let (from, to) = (party(script, TRACE_SEARCH), party(script, LATENCY_AUDIT));
            script.message(
                base,
                21,
                1,
                from,
                &[to],
                MessageKind::Note,
                "Re-ran the query benchmark: search p95 stays at 12 ms on its own.",
                None,
            );
        }
        20 => {
            // You relay the latency audit's question to Invoicing.
            let to = party(script, INVOICE_TOOLS);
            script.message(
                base,
                RELAYED_QUESTION,
                INBOX,
                Party::Human,
                &[to],
                MessageKind::Question,
                "The latency audit asks: does create_invoice fetch a token from /auth/token on \
                 every call?",
                None,
            );
        }
        22 => {
            let (from, to) = (party(script, INVOICE_TOOLS), party(script, INVOICE_SCHEMA));
            script.message(
                base,
                22,
                0,
                from,
                &[to],
                MessageKind::Question,
                "Is invoices.number unique per customer, or across the whole company?",
                None,
            );
            if first {
                let from = party(script, INVOICE_TOOLS);
                script.message(
                    base,
                    29,
                    INBOX,
                    from,
                    &[Party::Human],
                    MessageKind::Answer,
                    "No: one token per session, cached for 10 minutes. create_invoice never \
                     calls /auth/token itself.",
                    Some(message_id(start, base - 2, RELAYED_QUESTION)),
                );
                // Woken by the search report, the latency audit's coordinator
                // warns the lead who asked for both missions.
                script.state(
                    base,
                    LATENCY_AUDIT,
                    AgentState::Working,
                    Some("Reading the search report"),
                );
                let (from, to) = (party(script, LATENCY_AUDIT), party(script, TOWER));
                script.message(
                    base,
                    23,
                    1,
                    from,
                    &[to],
                    MessageKind::Note,
                    "Heads-up: the gateway's auth middleware calls /auth/token on every request, \
                     and Invoicing's MCP tools go through the gateway.",
                    None,
                );
            }
        }
        24 if first => {
            let to = party(script, TRACE_GATEWAY);
            script.message(
                base,
                24,
                INBOX,
                Party::Human,
                &[to],
                MessageKind::Answer,
                "Invoicing says: one token per session, cached for 10 minutes; create_invoice \
                 never calls /auth/token itself.",
                Some(message_id(start, TOKEN_QUESTION.0, TOKEN_QUESTION.1)),
            );
            // The lead passes the heads-up on to Invoicing's coordinator.
            let (from, to) = (party(script, TOWER), party(script, INVOICING));
            script.message(
                base,
                30,
                0,
                from,
                &[to],
                MessageKind::Note,
                "From the latency audit: the gateway calls /auth/token on every request. Check \
                 that the invoice tools keep their token.",
                None,
            );
            script.state(
                base,
                LATENCY_AUDIT,
                AgentState::Idle,
                Some("Waiting for 1 worker"),
            );
        }
        26 => {
            let (from, to) = (party(script, INVOICE_SCHEMA), party(script, INVOICE_TOOLS));
            script.message(
                base,
                25,
                0,
                from,
                &[to],
                MessageKind::Answer,
                "Across the company: numbers look like INV-2026-0042, with a unique index.",
                Some(message_id(start, base - 4, 22)),
            );
            if first {
                script.delivery(
                    base,
                    INVOICE_PAGE,
                    Delivery {
                        push: PushState::UpToDate,
                        merged_into_base: true,
                        pull_request: Some(PullRequest {
                            number: 42,
                            url: "https://git.example.com/dashboard/pull/42".to_owned(),
                            state: PrState::Merged,
                            checks: ChecksState::Passing,
                            head: Some(INVOICE_PAGE_TIP.to_owned()),
                        }),
                        tip: Some(INVOICE_PAGE_TIP.to_owned()),
                        ..Delivery::default()
                    },
                );
                script.verdict(base, INVOICE_PAGE, Verdict::Delivered, "PR #42 merged");
                let to = party(script, INVOICING);
                script.message(
                    base,
                    26,
                    0,
                    Party::System,
                    &[to],
                    MessageKind::Report,
                    "invoice-page delivered: PR #42 merged.",
                    None,
                );
            }
        }
        30 | 38 if first => {
            let checks = if offset == 30 {
                ChecksState::Pending
            } else {
                ChecksState::Passing
            };
            script.delivery(
                base,
                BROKEN_LINKS,
                Delivery {
                    commits: 1,
                    diff: DiffStat {
                        files: 4,
                        insertions: 12,
                        deletions: 9,
                    },
                    push: PushState::UpToDate,
                    pull_request: Some(PullRequest {
                        number: 7,
                        url: "https://git.example.com/website/pull/7".to_owned(),
                        state: PrState::Open,
                        checks,
                        head: Some(WEBSITE_TIP.to_owned()),
                    }),
                    tip: Some(WEBSITE_TIP.to_owned()),
                    ..Delivery::default()
                },
            );
        }
        36 => {
            let from = party(script, TOWER);
            script.message(
                base,
                27,
                INBOX,
                from,
                &[Party::Human],
                MessageKind::Note,
                "Status: invoice-page delivered; invoice-schema waits for your permission to \
                 migrate; the latency audit points at the gateway's token calls.",
                None,
            );
        }
        _ => {}
    }
}

/// Every event after the start, in order, forever.
pub(crate) fn beats(start: Timestamp) -> impl Iterator<Item = Event> {
    (0..).flat_map(move |cycle| {
        BEATS
            .iter()
            .flat_map(move |offset| beat(start, cycle, *offset))
    })
}

/// Every event up to `until`, included.
pub(crate) fn events_until(start: Timestamp, until: Timestamp) -> Vec<Event> {
    let mut events: Vec<Event> = history(start)
        .into_iter()
        .filter(|event| event.at <= until)
        .collect();
    events.extend(beats(start).take_while(|event| event.at <= until));
    events
}

#[cfg(test)]
mod tests {
    use volee_core::Registry;
    use volee_core::state::StateCategory;

    use super::*;

    fn start() -> Timestamp {
        Timestamp::from_millis(1_793_986_617_000)
    }

    #[test]
    fn the_scenario_applies_cleanly_for_many_loops() {
        let until = start().plus_millis(PERIOD_SECS * 1_000 * 5);
        let events = events_until(start(), until);
        let registry = Registry::from_events(&events).unwrap();
        assert_eq!(registry.agents().count(), CAST.len());
        assert!(events.windows(2).all(|pair| pair[0].at <= pair[1].at));
    }

    #[test]
    fn the_scenario_is_deterministic() {
        let until = start().plus_millis(90_000);
        assert_eq!(events_until(start(), until), events_until(start(), until));
        assert_ne!(
            events_until(start(), until),
            events_until(start().plus_millis(1), until)
        );
    }

    #[test]
    fn the_opening_scene_has_everything_the_demo_promises() {
        let registry = Registry::from_events(&history(start())).unwrap();
        let state = |member| &registry.agent(agent_id(start(), member)).unwrap().state;
        let count = |category| {
            registry
                .agents()
                .filter(|agent| agent.state.category() == category)
                .count()
        };
        assert_eq!(count(StateCategory::NeedsInput), 2);
        assert_eq!(count(StateCategory::Working), 6);
        assert!(matches!(
            state(INVOICE_SCHEMA),
            AgentState::NeedsInput {
                request: InputRequest::Permission { .. }
            }
        ));
        let page = registry.agent(agent_id(start(), INVOICE_PAGE)).unwrap();
        assert_eq!(
            page.delivery.summary(),
            "3 commits \u{b7} pushed \u{b7} PR #42 open, checks passing"
        );
        assert_eq!(registry.children(agent_id(start(), TRACE_GATEWAY)).len(), 2);
        assert_eq!(registry.missions().count(), 2);
        // invoice-tools' draft flag, and the latency audit's question about
        // Invoicing's token calls.
        assert_eq!(registry.unanswered_questions().len(), 2);
        let independents: Vec<_> = registry
            .agents()
            .filter(|agent| agent.role == Role::Independent)
            .map(|agent| agent.location.clone())
            .collect();
        assert_eq!(independents.len(), 2);
        assert!(independents.contains(&Location::Background));
        assert!(independents.contains(&Location::Terminal));
    }

    #[test]
    fn every_message_takes_a_route_volee_allows() {
        // The demo replays through the routing the daemon enforces
        // (ADR 0006), over a few loops: a message the daemon would refuse,
        // between two agents of different missions for example, fails here.
        let until = start().plus_millis(PERIOD_SECS * 1_000 * 3);
        let mut registry = Registry::new();
        let mut routes = 0;
        for event in events_until(start(), until) {
            if let EventKind::MessagePosted { message } = &event.kind {
                for to in message.recipients() {
                    if let Err(error) = registry.check_route(message.from, to) {
                        panic!("the daemon would refuse \"{}\": {error}", message.body);
                    }
                    routes += 1;
                }
            }
            registry.apply(&event).unwrap();
        }
        // The replay did check the history and three loops of messages.
        assert!(routes >= 30, "only {routes} routes checked");
        // The second harness stays in the conversation: infra, a Codex CLI
        // session, asks you, and you answer it.
        let infra = Party::Agent(agent_id(start(), INFRA));
        assert!(
            registry
                .messages()
                .any(|m| m.from == infra && m.is_for(Party::Human))
        );
        assert!(
            registry
                .messages()
                .any(|m| m.from == Party::Human && m.is_for(infra))
        );
        // What an earlier demo did, and the daemon refuses: infra writing
        // straight to a worker of the latency audit.
        let gateway = Party::Agent(agent_id(start(), TRACE_GATEWAY));
        assert!(registry.check_route(infra, gateway).is_err());
    }

    #[test]
    fn states_change_every_few_seconds() {
        let events: Vec<Event> = beats(start())
            .take_while(|event| event.at <= start().plus_millis(PERIOD_SECS * 1_000))
            .collect();
        let changes = events
            .iter()
            .filter(|event| matches!(event.kind, EventKind::StateChanged { .. }))
            .count();
        assert!(changes >= 8, "{changes}");
        assert!(events.iter().all(|event| event.at > start()));
    }
}
