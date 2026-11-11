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
use volee_core::harness::HarnessKind;
use volee_core::message::{MessageKind, Party, Thread};
use volee_core::mission::{Mission, MissionKind, MissionStatus};
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::AgentState;
use volee_core::time::Timestamp;
use volee_core::workspace::{Location, Workspace};
use volee_core::{AgentId, Event, EventKind, MissionId, Registry, ThreadId};
use volee_daemon::{Config, Daemon, EvidenceSources, ManualClock, MemoryLog};
use volee_git::GitCli;
use volee_harness::{Harness, PermissionMode};
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{
    AbortMission, AbortParams, AgentParams, CreateMission, CreateMissionParams, EventsSince,
    Preview, RefreshDiscovery, RefreshParams, SinceParams, Spawn, SpawnParams,
};
use volee_proto::paths::Paths;
use volee_proto::rpc;

struct Running {
    socket: PathBuf,
    clock: Arc<ManualClock>,
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
    start_with("")
}

/// A daemon whose configuration also holds `extra`.
fn start_with(extra: &str) -> Running {
    start_prepared(extra, |_| vec![])
}

/// A daemon whose configuration also holds `extra`. `prepare` gets the
/// repository before the daemon starts, and returns events for its log.
fn start_prepared(extra: &str, prepare: impl FnOnce(&Path) -> Vec<Event>) -> Running {
    let dir = tempfile::tempdir().unwrap();
    // Canonical, so that paths compare equal to what git reports.
    let root = dir.path().canonicalize().unwrap();
    let repo = root.join("alpha");
    repository(&repo);
    let events = prepare(&repo);
    let paths = Paths::rooted(root.join("volee"));
    std::fs::create_dir_all(&paths.state_dir).unwrap();
    let socket = paths.socket.clone();
    let listener = UnixListener::bind(&socket).unwrap();
    let harness = Arc::new(FakeHarness::default());
    let clock = Arc::new(ManualClock::at(10_000));
    let settings = volee_config::Config::parse(
        &format!(
            "[repos.alpha]\npath = \"{}\"\n{}",
            repo.display(),
            extra.replace("{repo}", &repo.display().to_string())
        ),
        Path::new("config.toml"),
        Path::new("/home/nobody"),
    )
    .unwrap();
    let git = GitCli::new()
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let config = Config {
        paths: paths.clone(),
        log: Box::new(MemoryLog::with([mission_elsewhere(), events].concat())),
        clock: Arc::clone(&clock) as Arc<dyn volee_daemon::Clock>,
        harnesses: vec![],
        spool: None,
        harness: Some(Arc::clone(&harness) as Arc<dyn Harness>),
        // Discovery is not under test: list rarely.
        discovery_interval: std::time::Duration::from_secs(3_600),
        max_discovery_interval: std::time::Duration::from_secs(3_600),
        idle_discovery_interval: std::time::Duration::from_secs(3_600),
        evidence: EvidenceSources {
            git: Some(Arc::new(git)),
            settings,
            ..EvidenceSources::default()
        },
        volee_bin: Some(PathBuf::from("/opt/volee/bin/volee")),
        multiplexers: Vec::new(),
        hosts: false,
        processes: None,
    };
    tokio::spawn(Daemon::serve(listener, config));
    Running {
        socket,
        clock,
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
        harness: None,
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
async fn agents_launch_in_manual_mode_unless_another_is_configured() {
    // Never the person's own Claude Code default, which may skip every check.
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    client.call::<Spawn>(worker("feat/invoices")).await.unwrap();
    assert_eq!(
        daemon.harness.launches()[0].permission_mode,
        PermissionMode::Manual
    );

    let daemon = start_with("[agents]\npermission_mode = \"acceptEdits\"\n");
    let client = Client::connect(&daemon.socket).await.unwrap();
    client.call::<Spawn>(worker("feat/invoices")).await.unwrap();
    assert_eq!(
        daemon.harness.launches()[0].permission_mode,
        PermissionMode::AcceptEdits
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
    // It says what stays, and that the same command can simply run again.
    assert!(
        error.message.contains("kept the worktree")
            && error.message.contains("feat/broken")
            && error.message.contains("run the same command again"),
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
async fn a_configured_folder_inside_a_repository_is_not_that_repository() {
    // A plain folder inside `alpha`, configured as a repository of its own.
    let daemon = start_with("[repos.docs]\npath = \"{repo}/docs\"\n");
    std::fs::create_dir_all(daemon.repo.join("docs")).unwrap();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let error = rpc_error(
        client
            .call::<Spawn>(SpawnParams {
                repo: "docs".into(),
                ..worker("feat/docs")
            })
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code, rpc::INVALID_PARAMS);
    assert!(
        error.message.contains("not the main checkout"),
        "{}",
        error.message
    );
    assert!(!daemon.repo.join(".claude/worktrees/feat-docs").exists());
    assert_eq!(daemon.harness.launches().len(), 0);
}

/// Launches `branch` while the harness refuses: the worktree and the branch
/// stay, and the agent failed before any session started. Returns it.
async fn failed_launch(daemon: &Running, client: &Client, branch: &str) -> AgentId {
    daemon.harness.refuse_launches(true);
    let error = rpc_error(client.call::<Spawn>(worker(branch)).await.unwrap_err());
    assert!(
        error.message.contains("run the same command again"),
        "{}",
        error.message
    );
    daemon.harness.refuse_launches(false);
    let registry = registry(&daemon.socket).await;
    let failed = registry
        .agents()
        .find(|agent| agent.workspace.branch.as_deref() == Some(branch))
        .unwrap();
    assert_eq!(failed.state, AgentState::Failed);
    assert_eq!(failed.sessions, [], "no session ever started");
    failed.id
}

#[tokio::test]
async fn the_same_launch_runs_again_after_a_failure() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let failed = failed_launch(&daemon, &client, "feat/again").await;
    let worktree = daemon.repo.join(".claude/worktrees/feat-again");

    // The cause is fixed (say, the repository is trusted now): the same
    // command again reuses what the failed launch left.
    let spawned = client.call::<Spawn>(worker("feat/again")).await.unwrap();
    assert_ne!(spawned.agent, failed);
    assert_eq!(spawned.worktree, worktree.display().to_string());
    assert_eq!(daemon.harness.launches().len(), 1);
    let registry = registry(&daemon.socket).await;
    assert!(
        registry.agent(failed).is_none(),
        "the failed agent is forgotten"
    );
    let agent = registry.agent(spawned.agent).unwrap();
    assert_eq!(agent.state, AgentState::Launching);
    assert_eq!(
        agent.workspace.worktree.as_deref(),
        Some(worktree.as_path())
    );
    assert_eq!(agent.workspace.branch.as_deref(), Some("feat/again"));
    let tip = git(&daemon.repo, &["rev-parse", "feat/again"]);
    assert_eq!(agent.workspace.start_commit.as_deref(), Some(tip.trim()));
}

#[tokio::test]
async fn what_a_failed_launch_left_is_never_reused_once_something_was_done_in_it() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();

    // A commit on the branch.
    let committed = failed_launch(&daemon, &client, "feat/committed").await;
    let tree = daemon.repo.join(".claude/worktrees/feat-committed");
    std::fs::write(tree.join("invoice.rs"), "pub struct Invoice;\n").unwrap();
    git(&tree, &["add", "invoice.rs"]);
    git(&tree, &["commit", "--quiet", "-m", "Add invoices"]);
    let error = rpc_error(
        client
            .call::<Spawn>(worker("feat/committed"))
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code, rpc::REFUSED, "{}", error.message);

    // A file nobody committed.
    let untracked = failed_launch(&daemon, &client, "feat/untracked").await;
    let tree = daemon.repo.join(".claude/worktrees/feat-untracked");
    std::fs::write(tree.join("notes.md"), "draft\n").unwrap();
    let error = rpc_error(
        client
            .call::<Spawn>(worker("feat/untracked"))
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code, rpc::REFUSED, "{}", error.message);
    assert!(tree.join("notes.md").is_file(), "nothing was touched");

    // Forgotten by the person: nothing says the worktree is volee's leftover.
    failed_launch(&daemon, &client, "feat/forgotten").await;
    let forgotten = registry(&daemon.socket)
        .await
        .agents()
        .find(|agent| agent.workspace.branch.as_deref() == Some("feat/forgotten"))
        .unwrap()
        .id;
    client
        .call::<volee_proto::methods::Forget>(AgentParams { agent: forgotten })
        .await
        .unwrap();
    let error = rpc_error(
        client
            .call::<Spawn>(worker("feat/forgotten"))
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code, rpc::REFUSED, "{}", error.message);

    // The failed agents stay, and nothing launched.
    let registry = registry(&daemon.socket).await;
    assert!(registry.agent(committed).is_some());
    assert!(registry.agent(untracked).is_some());
    assert_eq!(daemon.harness.launches().len(), 0);
}

#[tokio::test]
async fn an_agent_that_had_a_session_never_leaves_its_worktree_to_another() {
    // A worker that ran (a session linked), then failed: its worktree is not
    // a launch's leftover, even clean and without commits.
    let ran = AgentId::from_ulid(Ulid::from_parts(7_000, 1));
    let daemon = start_prepared("", |repo| {
        let worktree = repo.join(".claude/worktrees/feat-ran");
        git(
            repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "feat/ran",
                worktree.to_str().unwrap(),
                "main",
            ],
        );
        let at = Timestamp::from_millis;
        vec![
            Event::new(
                at(1),
                EventKind::AgentRegistered {
                    agent: ran,
                    name: "alpha-feat-ran".into(),
                    role: Role::Worker,
                    harness: HarnessKind::ClaudeCode,
                    launched_by: LaunchedBy::Human,
                    parent: None,
                    mission: None,
                    workspace: Workspace {
                        repo: Some("alpha".into()),
                        repo_root: Some(repo.to_path_buf()),
                        worktree: Some(worktree.clone()),
                        branch: Some("feat/ran".into()),
                        base: Some("main".into()),
                        ..Workspace::at(&worktree)
                    },
                    location: Location::Background,
                },
            ),
            Event::new(
                at(2),
                EventKind::SessionLinked {
                    agent: ran,
                    session: volee_core::harness::SessionKey::claude("aaaa1111-0000"),
                    origin: volee_core::harness::SessionOrigin::Started,
                },
            ),
            Event::new(
                at(3),
                EventKind::StateChanged {
                    agent: ran,
                    state: AgentState::Failed,
                    detail: None,
                },
            ),
        ]
    });
    let client = Client::connect(&daemon.socket).await.unwrap();
    let error = rpc_error(client.call::<Spawn>(worker("feat/ran")).await.unwrap_err());
    assert_eq!(error.code, rpc::REFUSED, "{}", error.message);
    assert!(registry(&daemon.socket).await.agent(ran).is_some());
}

#[tokio::test]
async fn tidy_removes_what_a_failed_launch_left_untouched_and_keeps_the_branch() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let failed = failed_launch(&daemon, &client, "feat/tidy").await;
    let worktree = daemon.repo.join(".claude/worktrees/feat-tidy");
    // Wait for evidence read after the failure: it proves nothing was done.
    for _ in 0..1_000 {
        let registry = registry(&daemon.socket).await;
        let agent = registry.agent(failed).unwrap();
        if agent.delivery.checked_at.is_some() && !agent.delivery.is_stale_at(agent.state_since) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let outcome = client
        .call::<volee_proto::methods::Tidy>(volee_proto::methods::TidyParams {
            agent: failed,
            dry_run: false,
        })
        .await
        .unwrap();
    assert!(
        matches!(outcome, volee_proto::methods::TidyOutcome::Removed { .. }),
        "{outcome:?}"
    );
    assert!(!worktree.exists());
    assert_ne!(
        git(&daemon.repo, &["branch", "--list", "feat/tidy"]).trim(),
        "",
        "the branch is kept"
    );
    assert!(registry(&daemon.socket).await.agent(failed).is_none());
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

#[tokio::test]
async fn a_directory_named_like_a_mission_repository_is_not_part_of_it() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let created = client.call::<CreateMission>(invoicing()).await.unwrap();
    // Another repository, whose directory is also called `alpha`.
    let other = daemon
        .repo
        .parent()
        .unwrap()
        .join("elsewhere")
        .join("alpha");
    repository(&other);
    let error = rpc_error(
        client
            .call::<Spawn>(SpawnParams {
                repo: other.display().to_string(),
                parent: created.coordinator,
                mission: Some(created.mission),
                ..worker("feat/impostor")
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
    assert!(!other.join(".claude/worktrees/feat-impostor").exists());

    // The mission's own repository, named by its path, is part of it.
    client
        .call::<Spawn>(SpawnParams {
            repo: daemon.repo.display().to_string(),
            parent: created.coordinator,
            mission: Some(created.mission),
            ..worker("feat/by-path")
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn a_mission_launches_at_most_its_limit_even_all_at_once() {
    let daemon = start_with("[missions]\nmax_workers = 2\n");
    let client = Client::connect(&daemon.socket).await.unwrap();
    let created = client.call::<CreateMission>(invoicing()).await.unwrap();
    let launch = |n: usize| {
        let socket = daemon.socket.clone();
        let (parent, mission) = (created.coordinator, created.mission);
        tokio::spawn(async move {
            let client = Client::connect(&socket).await.unwrap();
            client
                .call::<Spawn>(SpawnParams {
                    parent,
                    mission: Some(mission),
                    ..worker(&format!("feat/part-{n}"))
                })
                .await
        })
    };
    // Three at once, as a coordinator calling its tools in parallel would.
    let handles: Vec<_> = (0..3).map(launch).collect();
    let mut launched = 0;
    let mut refused = Vec::new();
    for handle in handles {
        match handle.await.unwrap() {
            Ok(_) => launched += 1,
            Err(error) => refused.push(rpc_error(error)),
        }
    }
    assert_eq!(launched, 2, "{refused:?}");
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].code, rpc::REFUSED);
    assert!(
        refused[0].message.contains("max_workers"),
        "{}",
        refused[0].message
    );
    // Later, too: every agent ever launched counts, not only running ones.
    let late = rpc_error(launch(9).await.unwrap().unwrap_err());
    assert_eq!(late.code, rpc::REFUSED);
    assert_eq!(
        registry(&daemon.socket).await.agents().count(),
        3,
        "two and the coordinator"
    );
}

#[tokio::test]
async fn setup_commands_prepare_the_worktree_before_the_session_starts() {
    let daemon = start_with(concat!(
        "setup = [\n",
        "  'printf \"%s|%s|%s\" \"$VOLEE_REPO\" \"$VOLEE_BRANCH\" \"$VOLEE_BASE\" > env.txt',\n",
        "  'echo one >> order.txt',\n",
        "  'echo two >> order.txt; echo installed',\n",
        "  'printf \"%s\\n%s\" \"$VOLEE_WORKTREE\" \"$VOLEE_REPO_ROOT\" > paths.txt',\n",
        "]\n",
    ));
    let client = Client::connect(&daemon.socket).await.unwrap();
    let spawned = client.call::<Spawn>(worker("feat/setup")).await.unwrap();
    let worktree = daemon.repo.join(".claude/worktrees/feat-setup");
    // In the new worktree, in order, with what they need to know.
    let read = |name: &str| std::fs::read_to_string(worktree.join(name)).unwrap();
    assert_eq!(read("env.txt"), "alpha|feat/setup|main");
    assert_eq!(read("order.txt"), "one\ntwo\n");
    assert_eq!(
        read("paths.txt"),
        format!("{}\n{}", worktree.display(), daemon.repo.display())
    );
    // Then the session starts.
    assert_eq!(daemon.harness.launches().len(), 1);
    // What they printed is in the log, which the answer names.
    let log = spawned.setup_log.expect("a setup log");
    assert!(log.ends_with("setup.log"), "{log}");
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("$ echo one >> order.txt\n"), "{text}");
    assert!(text.contains("installed\n"), "{text}");
    // The agent no longer says it is setting up.
    let registry = registry(&daemon.socket).await;
    let agent = registry.agent(spawned.agent).unwrap();
    assert_eq!(agent.state, AgentState::Launching);
    assert_eq!(agent.detail, None);
}

#[tokio::test]
async fn a_failing_setup_keeps_the_worktree_and_runs_again_with_the_launch() {
    // The second command fails until a file appears next to the repository.
    let daemon = start_with(concat!(
        "setup = [\n",
        "  'echo preparing',\n",
        "  'test -f \"$VOLEE_REPO_ROOT/../go\" || { echo \"the lockfile is stale\" >&2; exit 3; }',\n",
        "  'echo done > ../setup-ran-to-the-end',\n",
        "]\n",
    ));
    let client = Client::connect(&daemon.socket).await.unwrap();
    let error = rpc_error(
        client
            .call::<Spawn>(worker("feat/setup"))
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code, rpc::REFUSED);
    assert!(
        error
            .message
            .contains("exited with status 3: the lockfile is stale")
            && error.message.contains("setup.log")
            && error.message.contains("kept the worktree")
            && error.message.contains("run the same command again"),
        "{}",
        error.message
    );
    // No session, the rest of the setup skipped, the worktree kept.
    assert_eq!(daemon.harness.launches().len(), 0);
    let worktree = daemon.repo.join(".claude/worktrees/feat-setup");
    assert!(worktree.is_dir());
    assert!(!worktree.join("../setup-ran-to-the-end").exists());
    let registry = registry(&daemon.socket).await;
    let agent = registry.agents().next().unwrap();
    assert_eq!(agent.state, AgentState::Failed);
    assert_eq!(
        agent.detail.as_deref(),
        Some(
            "setup failed: `test -f \"$VOLEE_REPO_ROOT/../go\" || { echo \"the lockfile is \
             stale\" >&2; exit 3; }` exited with status 3: the lockfile is stale"
        )
    );

    // Once fixed, the same command reuses the untouched worktree and sets
    // it up again.
    std::fs::write(daemon.repo.join("../go"), "").unwrap();
    let spawned = client.call::<Spawn>(worker("feat/setup")).await.unwrap();
    assert_eq!(PathBuf::from(&spawned.worktree), worktree);
    assert!(worktree.join("../setup-ran-to-the-end").exists());
    assert_eq!(daemon.harness.launches().len(), 1);
}

#[tokio::test]
async fn a_repository_without_setup_starts_at_once() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    let spawned = client.call::<Spawn>(worker("feat/plain")).await.unwrap();
    assert_eq!(spawned.setup_log, None);
    assert_eq!(daemon.harness.launches().len(), 1);
}

/// A mission on `alpha`, so that its workers may be launched there.
fn mission_on_alpha() -> (MissionId, Vec<Event>) {
    let id = MissionId::from_ulid(Ulid::from_parts(4_000, 2));
    let thread = ThreadId::from_ulid(Ulid::from_parts(2_000, 2));
    let at = |kind| Event::new(Timestamp::from_millis(1), kind);
    let events = vec![
        at(EventKind::ThreadOpened {
            thread: Thread {
                id: thread,
                title: "Mission: Invoices".into(),
                mission: Some(id),
                participants: vec![Party::Human],
                opened_at: Timestamp::from_millis(1),
            },
        }),
        at(EventKind::MissionCreated {
            mission: Mission {
                id,
                title: "Invoices".into(),
                kind: MissionKind::Change,
                goal: "Add invoices".into(),
                repos: vec!["alpha".into()],
                requested_by: None,
                coordinator: None,
                thread,
                status: MissionStatus::Running,
                report: None,
                created_at: Timestamp::from_millis(1),
                ended_at: None,
                assessments: BTreeMap::new(),
            },
        }),
    ];
    (id, events)
}

#[tokio::test]
async fn a_mission_that_ends_during_the_setup_stops_the_launch() {
    let (mission, events) = mission_on_alpha();
    // The setup waits for a file next to the repository.
    let daemon = start_prepared(
        "setup = ['while [ ! -f \"$VOLEE_REPO_ROOT/../release\" ]; do sleep 0.05; done']\n",
        move |_| events,
    );
    let launch = {
        let socket = daemon.socket.clone();
        tokio::spawn(async move {
            let client = Client::connect(&socket).await.unwrap();
            client
                .call::<Spawn>(SpawnParams {
                    mission: Some(mission),
                    ..worker("feat/late")
                })
                .await
        })
    };
    // Wait for the setup to run, then abort the mission and let it finish.
    let agent = loop {
        let registry = registry(&daemon.socket).await;
        let setting_up = registry.agents().find(|agent| {
            agent
                .detail
                .as_deref()
                .is_some_and(|detail| detail.starts_with("setting up"))
        });
        if let Some(agent) = setting_up {
            break agent.id;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    let client = Client::connect(&daemon.socket).await.unwrap();
    client
        .call::<AbortMission>(AbortParams {
            mission,
            reason: None,
        })
        .await
        .unwrap();
    std::fs::write(daemon.repo.join("../release"), "").unwrap();
    let error = rpc_error(launch.await.unwrap().unwrap_err());
    assert_eq!(error.code, rpc::REFUSED);
    assert!(
        error.message.contains("its mission ended during its setup"),
        "{}",
        error.message
    );
    // No session started for an aborted mission, and the agent says why.
    assert_eq!(daemon.harness.launches().len(), 0);
    let registry = registry(&daemon.socket).await;
    let record = registry.agent(agent).unwrap();
    assert_eq!(record.state, AgentState::Stopped);
    assert_eq!(
        record.detail.as_deref(),
        Some("no session started: its mission ended during its setup")
    );
    assert!(daemon.repo.join(".claude/worktrees/feat-late").is_dir());
}

#[tokio::test]
async fn the_preview_of_a_failed_setup_is_its_log() {
    let daemon = start_with(concat!(
        "setup = [\n",
        "  'echo resolving packages',\n",
        "  'echo \"error: lockfile had changes, but lockfile is frozen\" >&2; exit 1',\n",
        "]\n",
    ));
    let client = Client::connect(&daemon.socket).await.unwrap();
    client
        .call::<Spawn>(worker("feat/frozen"))
        .await
        .unwrap_err();
    let agent = registry(&daemon.socket).await.agents().next().unwrap().id;
    let preview = client.call::<Preview>(AgentParams { agent }).await.unwrap();
    assert!(!preview.exact, "a log is not the agent's screen");
    assert!(
        preview
            .screen
            .starts_with("volee set up this worktree; no session has started. setup.log:\r\n\r\n"),
        "{}",
        preview.screen
    );
    assert!(
        preview
            .screen
            .contains("$ echo resolving packages\r\nresolving packages\r\n")
            && preview.screen.contains(
                "error: lockfile had changes, but lockfile is frozen\r\n[exited with status 1]\r\n"
            ),
        "{}",
        preview.screen
    );
}

/// Asks the daemon for a listing now, and waits until it is applied.
async fn refresh(socket: &Path) {
    let client = Client::connect(socket).await.unwrap();
    let refreshed = client
        .call::<RefreshDiscovery>(RefreshParams::default())
        .await
        .unwrap();
    assert!(refreshed.listed);
}

#[tokio::test]
async fn a_launch_whose_session_never_appears_fails_after_two_minutes() {
    let daemon = start();
    let client = Client::connect(&daemon.socket).await.unwrap();
    // The fake harness reports session abcd1234 but never lists it.
    let spawned = client.call::<Spawn>(worker("feat/ghost")).await.unwrap();
    refresh(&daemon.socket).await;
    let waiting = registry(&daemon.socket).await;
    assert_eq!(
        waiting.agent(spawned.agent).unwrap().state,
        AgentState::Launching
    );
    daemon.clock.advance(121_000);
    refresh(&daemon.socket).await;
    let registry = registry(&daemon.socket).await;
    let agent = registry.agent(spawned.agent).unwrap();
    assert_eq!(agent.state, AgentState::Failed);
    assert_eq!(
        agent.detail.as_deref(),
        Some("Claude Code started session abcd1234 but never listed it: run the same launch again")
    );
    // A failed launch: the same command reuses what it left (ADR 0016).
    let again = client.call::<Spawn>(worker("feat/ghost")).await.unwrap();
    assert_eq!(again.worktree, spawned.worktree);
}

#[tokio::test]
async fn a_launch_cut_short_by_a_restart_fails_at_the_first_listing() {
    // The log of a daemon that stopped while it launched an agent.
    let agent = AgentId::from_ulid(Ulid::from_parts(3_000, 1));
    let daemon = start_prepared("", move |repo| {
        vec![Event::new(
            Timestamp::from_millis(1),
            EventKind::AgentRegistered {
                agent,
                name: "alpha-feat-cut".into(),
                role: Role::Worker,
                harness: HarnessKind::ClaudeCode,
                launched_by: LaunchedBy::Human,
                parent: None,
                mission: None,
                workspace: Workspace::at(repo),
                location: Location::Background,
            },
        )]
    });
    refresh(&daemon.socket).await;
    let registry = registry(&daemon.socket).await;
    let record = registry.agent(agent).unwrap();
    assert_eq!(record.state, AgentState::Failed);
    assert_eq!(
        record.detail.as_deref(),
        Some(
            "the launch stopped before its session started, as when the daemon stops: run the \
             same launch again"
        )
    );
}
