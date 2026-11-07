//! Launching agents end to end: a real repository, real git worktrees, and a
//! fake harness that records what it was asked to start.

// In tests, a failed unwrap is a failed test with a clear location.
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::{FakeHarness, git, repository};
use tokio::net::UnixListener;
use ulid::Ulid;
use volee_core::message::{MessageKind, Party, Thread};
use volee_core::mission::{Mission, MissionKind, MissionStatus};
use volee_core::role::Role;
use volee_core::state::AgentState;
use volee_core::time::Timestamp;
use volee_core::{Event, EventKind, MissionId, Registry, ThreadId};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_git::GitCli;
use volee_harness::Harness;
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{
    CreateMission, CreateMissionParams, EventsSince, SinceParams, Spawn, SpawnParams,
};
use volee_proto::paths::Paths;
use volee_proto::rpc;

struct Running {
    socket: PathBuf,
    harness: Arc<FakeHarness>,
    paths: Paths,
    repo: PathBuf,
    _dir: tempfile::TempDir,
}

fn mission_id() -> MissionId {
    MissionId::from_ulid(Ulid::from_parts(4_000, 1))
}

/// A mission on another repository than `alpha`.
fn mission_elsewhere() -> Vec<Event> {
    let thread = ThreadId::from_ulid(Ulid::from_parts(2_000, 1));
    let at = |kind| Event::new(Timestamp::from_millis(1), kind);
    vec![
        at(EventKind::ThreadOpened {
            thread: Thread {
                id: thread,
                title: "Mission: Docs".into(),
                mission: Some(mission_id()),
                participants: vec![Party::Human],
                opened_at: Timestamp::from_millis(1),
            },
        }),
        at(EventKind::MissionCreated {
            mission: Mission {
                id: mission_id(),
                title: "Docs".into(),
                kind: MissionKind::Change,
                goal: "Write docs".into(),
                repos: vec!["docs".into()],
                requested_by: None,
                coordinator: None,
                thread,
                status: MissionStatus::Planning,
                report: None,
                created_at: Timestamp::from_millis(1),
                ended_at: None,
                assessments: BTreeMap::new(),
            },
        }),
    ]
}

fn start() -> Running {
    let dir = tempfile::tempdir().unwrap();
    // Canonical, so that paths compare equal to what git reports.
    let root = dir.path().canonicalize().unwrap();
    let repo = root.join("alpha");
    repository(&repo);
    let paths = Paths::rooted(root.join("volee"));
    std::fs::create_dir_all(&paths.state_dir).unwrap();
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let harness = Arc::new(FakeHarness::default());
    let settings = volee_config::Config::parse(
        &format!("[repos.alpha]\npath = \"{}\"\n", repo.display()),
        Path::new("config.toml"),
        Path::new("/home/nobody"),
    )
    .unwrap();
    let git = GitCli::new()
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let config = Config {
        paths: paths.clone(),
        log: Box::new(MemoryLog::with(mission_elsewhere())),
        clock: Arc::new(ManualClock::at(10_000)),
        harnesses: vec![],
        spool: None,
        harness: Some(Arc::clone(&harness) as Arc<dyn Harness>),
        // Discovery is not under test: list rarely.
        discovery_interval: std::time::Duration::from_secs(3_600),
        idle_discovery_interval: std::time::Duration::from_secs(3_600),
        evidence: EvidenceSources {
            git: Some(Arc::new(git)),
            settings,
            ..EvidenceSources::default()
        },
        volee_bin: Some(PathBuf::from("/opt/volee/bin/volee")),
    };
    tokio::spawn(Daemon::serve(listener, config));
    Running {
        socket,
        harness,
        paths,
        repo,
        _dir: dir,
    }
}

fn worker(branch: &str) -> SpawnParams {
    SpawnParams {
        repo: "alpha".into(),
        branch: branch.into(),
        base: None,
        task: "Add invoices to the billing module.".into(),
        role: Role::Worker,
        parent: None,
        mission: None,
        model: None,
        name: None,
    }
}

async fn registry(socket: &Path) -> Registry {
    let client = Client::connect(socket).await.unwrap();
    let batch = client
        .call::<EventsSince>(SinceParams::default())
        .await
        .unwrap();
    let events: Vec<Event> = batch.events.into_iter().map(|e| e.event).collect();
    Registry::from_events(&events).unwrap()
}

fn rpc_error(error: ClientError) -> rpc::RpcError {
    match error {
        ClientError::Rpc(error) => error,
        other => panic!("expected an RPC error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_worker_gets_its_own_worktree_and_session() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let spawned = client.call::<Spawn>(worker("feat/invoices")).await.unwrap();

    let worktree = daemon.repo.join(".claude/worktrees/feat-invoices");
    assert_eq!(spawned.worktree, worktree.display().to_string());
    assert_eq!(
        git(&worktree, &["branch", "--show-current"]),
        "feat/invoices"
    );
    let exclude = std::fs::read_to_string(daemon.repo.join(".git/info/exclude")).unwrap();
    assert!(exclude.lines().any(|line| line == ".claude/worktrees/"));
    assert_eq!(
        git(&daemon.repo, &["status", "--porcelain"]),
        "",
        "main checkout stays clean"
    );

    let registry = registry(&daemon.socket).await;
    let agent = registry.agent(spawned.agent).unwrap();
    assert_eq!(agent.name, "alpha-feat-invoices");
    assert_eq!(agent.role, Role::Worker);
    assert_eq!(agent.workspace.repo.as_deref(), Some("alpha"));
    assert_eq!(agent.workspace.branch.as_deref(), Some("feat/invoices"));
    assert_eq!(agent.workspace.base.as_deref(), Some("main"));
    assert_eq!(
        agent.workspace.start_commit.as_deref(),
        Some(git(&daemon.repo, &["rev-parse", "main"]).as_str())
    );

    let launches = daemon.harness.launches();
    assert_eq!(launches.len(), 1);
    let spec = &launches[0];
    // Started from the trusted main checkout, in volee's worktree (ADR 0013).
    assert_eq!(spec.cwd, daemon.repo);
    assert_eq!(spec.worktree.as_deref(), Some("feat-invoices"));
    assert_eq!(spec.prompt, "Add invoices to the billing module.");
    assert!(
        spec.append_system_prompt
            .as_deref()
            .unwrap()
            .contains("worktree of alpha")
    );
    let agent_dir = daemon.paths.agent_dir(spawned.agent);
    assert_eq!(spec.settings_file, agent_dir.join("settings.json"));
    assert_eq!(spec.mcp_config_file, agent_dir.join("mcp.json"));
    for file in [&spec.settings_file, &spec.mcp_config_file] {
        let mode = std::fs::metadata(file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", file.display());
    }
    let settings = std::fs::read_to_string(&spec.settings_file).unwrap();
    assert!(settings.contains(&spawned.agent.to_string()));
    // The session runs under Claude Code's supervisor: it learns where this
    // daemon is from its settings and its MCP configuration.
    let socket = daemon.paths.socket.display().to_string();
    let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
    assert_eq!(settings["env"]["VOLEE_SOCKET"], socket.as_str());
    let mcp: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&spec.mcp_config_file).unwrap()).unwrap();
    assert_eq!(
        mcp["mcpServers"]["volee"]["env"]["VOLEE_SOCKET"],
        socket.as_str()
    );
}

#[tokio::test]
async fn a_failed_launch_marks_the_agent_failed_and_keeps_the_worktree() {
    let daemon = start();
    daemon.harness.refuse_launches(true);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let error = rpc_error(
        client
            .call::<Spawn>(worker("feat/broken"))
            .await
            .unwrap_err(),
    );
    assert!(
        error.message.contains("refused by the fake harness"),
        "{}",
        error.message
    );
    assert!(daemon.repo.join(".claude/worktrees/feat-broken").is_dir());
    let registry = registry(&daemon.socket).await;
    let agent = registry.agents().next().unwrap();
    assert_eq!(agent.state, AgentState::Failed);
    assert!(
        agent
            .detail
            .as_deref()
            .unwrap()
            .starts_with("launch failed")
    );
}

#[tokio::test]
async fn an_existing_branch_is_refused_before_anything_starts() {
    let daemon = start();
    git(&daemon.repo, &["branch", "feat/taken"]);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let error = rpc_error(
        client
            .call::<Spawn>(worker("feat/taken"))
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code, rpc::REFUSED, "{}", error.message);
    assert_eq!(daemon.harness.launches().len(), 0);
    assert_eq!(registry(&daemon.socket).await.agents().count(), 0);
}

#[tokio::test]
async fn a_mission_worker_stays_in_the_mission_repositories() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let error = rpc_error(
        client
            .call::<Spawn>(SpawnParams {
                mission: Some(mission_id()),
                ..worker("feat/docs")
            })
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code, rpc::REFUSED);
    assert!(
        error.message.contains("not part of mission"),
        "{}",
        error.message
    );
    assert!(!daemon.repo.join(".claude/worktrees/feat-docs").exists());
}

#[tokio::test]
async fn unknown_repositories_and_empty_tasks_are_rejected() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    for params in [
        SpawnParams {
            repo: "nowhere".into(),
            ..worker("feat/x")
        },
        SpawnParams {
            task: "  ".into(),
            ..worker("feat/y")
        },
    ] {
        let error = rpc_error(client.call::<Spawn>(params).await.unwrap_err());
        assert_eq!(error.code, rpc::INVALID_PARAMS, "{}", error.message);
    }
}

fn invoicing() -> CreateMissionParams {
    CreateMissionParams {
        title: "Invoicing".into(),
        kind: MissionKind::Change,
        goal: "Add invoices to the billing module.".into(),
        repos: vec!["alpha".into()],
        requested_by: None,
    }
}

#[tokio::test]
async fn a_new_mission_gets_a_coordinator_that_cannot_edit() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let created = client.call::<CreateMission>(invoicing()).await.unwrap();
    let coordinator = created.coordinator.expect("a coordinator was launched");

    let registry = registry(&daemon.socket).await;
    let agent = registry.agent(coordinator).unwrap();
    assert_eq!(agent.role, Role::Coordinator);
    assert_eq!(agent.mission, Some(created.mission));
    assert_eq!(
        registry.mission(created.mission).unwrap().coordinator,
        Some(coordinator),
        "linked to its mission"
    );

    let spec = daemon.harness.launches().pop().unwrap();
    assert_eq!(spec.cwd, daemon.repo, "plans from the main checkout");
    assert_eq!(spec.worktree, None);
    for tool in ["Edit", "Write", "NotebookEdit"] {
        assert!(spec.disallowed_tools.iter().any(|t| t == tool), "{tool}");
    }
    assert!(spec.prompt.contains("Add invoices to the billing module."));
    let instructions = spec.append_system_prompt.unwrap();
    assert!(instructions.contains("coordinator"));
    assert!(instructions.contains("spawn_worker"));
    assert!(instructions.contains("close_mission"));
}

#[tokio::test]
async fn a_coordinator_that_cannot_start_is_reported_to_the_human() {
    let daemon = start();
    daemon.harness.refuse_launches(true);
    let client = Client::connect(&daemon.socket).await.unwrap();
    let created = client.call::<CreateMission>(invoicing()).await.unwrap();
    assert_eq!(created.coordinator, None);
    let registry = registry(&daemon.socket).await;
    let mission = registry.mission(created.mission).unwrap();
    assert_eq!(mission.status, MissionStatus::Planning);
    let alerts: Vec<_> = registry
        .thread_messages(created.thread)
        .into_iter()
        .filter(|message| message.kind == MessageKind::Alert)
        .collect();
    assert_eq!(alerts.len(), 1);
    assert!(alerts[0].body.contains("could not launch the coordinator"));
}

#[tokio::test]
async fn a_mission_worker_reports_to_its_coordinator() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let created = client.call::<CreateMission>(invoicing()).await.unwrap();
    let coordinator = created.coordinator.unwrap();
    let spawned = client
        .call::<Spawn>(SpawnParams {
            parent: Some(coordinator),
            mission: Some(created.mission),
            ..worker("feat/invoices")
        })
        .await
        .unwrap();
    let registry = registry(&daemon.socket).await;
    let agent = registry.agent(spawned.agent).unwrap();
    assert_eq!(agent.parent, Some(coordinator));
    assert_eq!(agent.mission, Some(created.mission));
    let spec = daemon.harness.launches().pop().unwrap();
    let instructions = spec.append_system_prompt.unwrap();
    assert!(instructions.contains("worker of the volee mission \"Invoicing\""));
    assert!(instructions.contains("pull request"));
    // The mission now runs: it has a worker.
    assert_eq!(
        registry.mission(created.mission).unwrap().status,
        MissionStatus::Running
    );
}
