//! Launching agents (ADR 0002, ADR 0013): a worktree on a new branch, the
//! agent's hook settings and MCP configuration, then a background session of
//! the harness.
//!
//! The worktree lives in the repository, at `.claude/worktrees/<branch>`, and
//! the session starts from the repository's main checkout with
//! `--worktree=<branch>`: Claude Code checks its trust on the main checkout
//! and reuses the worktree volee created. Started directly in a fresh
//! worktree, a background session would be refused as untrusted.
//!
//! Every slow step (git, the harness) runs off the state lock. The agent is
//! registered before its session starts, so that the session's first hooks
//! find it; a launch that fails marks it failed and deletes nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use volee_core::harness::HarnessKind;
use volee_core::mission::{Mission, MissionKind};
use volee_core::role::{LaunchedBy, Role};
use volee_core::state::AgentState;
use volee_core::workspace::{Location, Workspace};
use volee_core::{AgentId, Event, EventKind, MissionId};
use volee_git::Git;
use volee_harness::{Harness, PermissionMode, SpawnSpec};
use volee_proto::methods::{SpawnParams, Spawned};

use crate::error::DaemonError;
use crate::server::Shared;

/// The directory, inside a repository, where volee puts its worktrees: the
/// one Claude Code looks into for `--worktree`.
pub(crate) const IN_REPO_WORKTREES: &str = ".claude/worktrees";

/// A repository resolved for a launch.
#[derive(Clone, Debug)]
pub(crate) struct Target {
    /// The configured name, else the directory name.
    pub(crate) name: String,
    /// The main checkout.
    pub(crate) root: PathBuf,
    /// The branch new work starts from.
    pub(crate) base: String,
}

/// What a launch needs to know about the mission it serves.
#[derive(Clone, Debug)]
pub(crate) struct MissionContext {
    id: MissionId,
    title: String,
    kind: MissionKind,
    repos: Vec<String>,
}

/// `agents.spawn`: creates a worktree and launches an agent in it.
pub(crate) async fn spawn(
    shared: &Arc<Shared>,
    params: SpawnParams,
) -> Result<Spawned, DaemonError> {
    let harness = harness(shared)?;
    let git = git(shared)?;
    let task = params.task.trim().to_owned();
    if task.is_empty() {
        return Err(DaemonError::InvalidParams("an agent needs a task".into()));
    }
    if matches!(params.role, Role::Coordinator | Role::Subagent) {
        return Err(DaemonError::InvalidParams(format!(
            "agents.spawn launches workers, leads and independent agents, not a {}",
            params.role.label()
        )));
    }
    let (mission, slot) = context(shared, params.parent, params.mission).await?;
    let target = resolve(shared, &git, &params.repo, params.base.as_deref()).await?;
    if let Some(mission) = &mission
        && !in_mission(shared, &git, mission, &target.root).await
    {
        return Err(DaemonError::Refused(format!(
            "repository {} is not part of mission \"{}\" ({})",
            target.name,
            mission.title,
            mission.repos.join(", ")
        )));
    }

    let agent = shared.state.lock().await.new_agent_id();
    let worktree_name = slug(&params.branch);
    let worktree = target.root.join(IN_REPO_WORKTREES).join(&worktree_name);
    let created = {
        let git = Arc::clone(&git);
        let (root, path, branch, base) = (
            target.root.clone(),
            worktree.clone(),
            params.branch.clone(),
            target.base.clone(),
        );
        blocking(move || git.add_worktree(&root, &path, &branch, &base)).await??
    };
    exclude_worktrees(&target.root);
    let (settings_file, mcp_config_file) = write_agent_files(shared, &harness, agent)?;

    let name = params
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("{}-{}", target.name, slug(&params.branch)));
    let workspace = Workspace {
        repo: Some(target.name.clone()),
        repo_root: Some(target.root.clone()),
        worktree: Some(worktree.clone()),
        branch: Some(params.branch.clone()),
        base: Some(target.base.clone()),
        start_commit: Some(created.start_commit.clone()),
        ..Workspace::at(&worktree)
    };
    register(
        shared,
        agent,
        &name,
        params.role,
        params.parent,
        mission.as_ref().map(|m| m.id),
        workspace,
        slot,
    )
    .await?;

    let model = params.model.clone().or_else(|| {
        shared
            .settings
            .model_for(Some(&target.name))
            .map(str::to_owned)
    });
    let spec = SpawnSpec {
        cwd: target.root.clone(),
        name: name.clone(),
        model,
        prompt: task,
        settings_file,
        mcp_config_file,
        permission_mode: permission_mode(shared),
        allowed_tools: Vec::new(),
        disallowed_tools: Vec::new(),
        append_system_prompt: Some(worker_instructions(
            params.role,
            mission.as_ref(),
            &target,
            &params.branch,
        )),
        worktree: Some(worktree_name),
    };
    start_session(shared, &harness, agent, spec).await?;
    Ok(Spawned {
        agent,
        worktree: worktree.display().to_string(),
    })
}

/// Launches the coordinator of a new mission, in the main checkout of its
/// first repository. It plans and delegates; it is told never to change files
/// there, and Claude Code's editing tools are disabled for it.
pub(crate) async fn coordinator(
    shared: &Arc<Shared>,
    mission: MissionId,
) -> Result<AgentId, DaemonError> {
    let harness = harness(shared)?;
    let git = git(shared)?;
    let record = shared
        .state
        .lock()
        .await
        .registry()
        .mission(mission)
        .cloned()
        .ok_or_else(|| DaemonError::NotFound(format!("unknown mission {mission}")))?;
    let first = record.repos.first().ok_or_else(|| {
        DaemonError::InvalidParams("a mission needs at least one repository".into())
    })?;
    let target = resolve(shared, &git, first, None).await?;
    let agent = shared.state.lock().await.new_agent_id();
    let (settings_file, mcp_config_file) = write_agent_files(shared, &harness, agent)?;
    let name = format!("coordinator-{}", slug(&record.title.to_lowercase()));
    let workspace = Workspace {
        repo: Some(target.name.clone()),
        repo_root: Some(target.root.clone()),
        ..Workspace::at(&target.root)
    };
    register(
        shared,
        agent,
        &name,
        Role::Coordinator,
        None,
        Some(mission),
        workspace,
        None,
    )
    .await?;
    let spec = SpawnSpec {
        cwd: target.root.clone(),
        name,
        model: shared
            .settings
            .model_for(Some(&target.name))
            .map(str::to_owned),
        prompt: coordinator_prompt(&record),
        settings_file,
        mcp_config_file,
        permission_mode: permission_mode(shared),
        allowed_tools: Vec::new(),
        disallowed_tools: ["Edit", "Write", "NotebookEdit"]
            .map(str::to_owned)
            .to_vec(),
        append_system_prompt: Some(coordinator_instructions(&record)),
        worktree: None,
    };
    start_session(shared, &harness, agent, spec).await?;
    Ok(agent)
}

/// The coordinator's first prompt: the mission itself.
fn coordinator_prompt(mission: &Mission) -> String {
    format!(
        "Mission \"{}\" ({} mission).\n\nGoal:\n{}\n\nRepositories: {}.\n\nPlan the work, then \
         launch the workers with `spawn_worker`.",
        mission.title,
        mission.kind.label(),
        mission.goal.trim(),
        mission.repos.join(", ")
    )
}

/// How a coordinator works, appended to its system prompt so that it
/// survives compaction.
pub(crate) fn coordinator_instructions(mission: &Mission) -> String {
    let delivery = match mission.kind {
        MissionKind::Change => {
            "This is a change mission: each worker commits on its own branch, pushes it and \
             opens a draft pull request, then reports. volee counts a worker as delivered only \
             when its commits are pushed and its pull request is open (or merged)."
        }
        MissionKind::Analysis => {
            "This is an analysis mission: workers must not change code. Each one investigates \
             and sends its findings with `report`; that report is what volee counts as \
             delivered."
        }
    };
    format!(
        "You are the coordinator of the volee mission \"{title}\". volee shows every coding agent \
         on this machine to the human and checks their work. You plan and coordinate; workers do \
         the work.\n\n\
         1. Read what you need to plan. You are in the main checkout of one of the mission's \
         repositories: never change files here.\n\
         2. Split the goal into a few independent tasks and launch one worker per task with \
         `spawn_worker` (repository, a new branch such as `feat/<topic>`, and the task). A worker \
         knows only what you write: give it the context, what to change or find, the acceptance \
         criteria and how to check its work. Prefer few workers.\n\
         3. Then wait. Do not poll and do not sleep: volee wakes you with a message when a worker \
         stops, with the evidence it checked. Answer workers' questions with `send_message`. When \
         a worker stopped without delivering, tell it what is missing.\n\
         4. When volee tells you every worker delivered, send the final report to the human with \
         `report`: what changed, a link to every pull request or report, and what the human must \
         review. Then call `close_mission`.\n\n\
         {delivery}\n\n\
         Write to the human (`send_message` to \"human\") only for decisions that are theirs. Keep \
         every message short.",
        title = mission.title
    )
}

/// Checks the parent and the mission a launch names, and returns what the
/// launch needs to know about the mission.
/// The mission a launch serves, and a slot among the agents it may launch,
/// taken under the state lock so that launches running at once all count.
async fn context(
    shared: &Arc<Shared>,
    parent: Option<AgentId>,
    mission: Option<MissionId>,
) -> Result<(Option<MissionContext>, Option<Slot>), DaemonError> {
    let state = shared.state.lock().await;
    let registry = state.registry();
    if let Some(parent) = parent
        && registry.agent(parent).is_none()
    {
        return Err(DaemonError::NotFound(format!("unknown agent {parent}")));
    }
    let Some(id) = mission else {
        return Ok((None, None));
    };
    let record = registry
        .mission(id)
        .ok_or_else(|| DaemonError::NotFound(format!("unknown mission {id}")))?;
    if record.status.is_over() {
        return Err(DaemonError::Refused(format!(
            "mission \"{}\" is {}",
            record.title,
            record.status.label()
        )));
    }
    let launched = registry
        .mission_agents(id)
        .iter()
        .filter(|agent| agent.role != Role::Coordinator)
        .count();
    let max = shared.settings.missions.max_workers;
    let slot = shared.slots.take(id, launched, max).ok_or_else(|| {
        DaemonError::Refused(format!(
            "mission \"{}\" already launched {launched} agents, the most `[missions] \
             max_workers` allows ({max}): ask the human to raise it in volee's configuration",
            record.title
        ))
    })?;
    Ok((
        Some(MissionContext {
            id,
            title: record.title.clone(),
            kind: record.kind,
            repos: record.repos.clone(),
        }),
        Some(slot),
    ))
}

/// Agents being launched for each mission, not registered yet.
#[derive(Debug, Default)]
pub(crate) struct Slots(Arc<Mutex<HashMap<MissionId, usize>>>);

impl Slots {
    /// A slot for one more agent of `mission`, unless the `launched` agents
    /// and those being launched already reach `max`.
    fn take(&self, mission: MissionId, launched: usize, max: usize) -> Option<Slot> {
        let mut pending = lock_slots(&self.0);
        let count = pending.entry(mission).or_default();
        if launched + *count >= max {
            return None;
        }
        *count += 1;
        Some(Slot {
            pending: Arc::clone(&self.0),
            mission,
        })
    }
}

fn lock_slots(
    pending: &Mutex<HashMap<MissionId, usize>>,
) -> MutexGuard<'_, HashMap<MissionId, usize>> {
    // The map only holds counts; a poisoned lock is still consistent.
    pending.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One agent being launched for a mission. It stops counting as pending
/// when dropped, which [`register`] does under the state lock, right after
/// the agent counts as launched.
#[derive(Debug)]
pub(crate) struct Slot {
    pending: Arc<Mutex<HashMap<MissionId, usize>>>,
    mission: MissionId,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut pending = lock_slots(&self.pending);
        if let Some(count) = pending.get_mut(&self.mission) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                pending.remove(&self.mission);
            }
        }
    }
}

/// Whether `root` is the main checkout of one of the mission's repositories,
/// each given as a configured name or a path. Directory names never decide.
async fn in_mission(
    shared: &Arc<Shared>,
    git: &Arc<dyn Git>,
    mission: &MissionContext,
    root: &Path,
) -> bool {
    for repo in &mission.repos {
        if let Ok((main, _)) = locate(shared, git, repo).await
            && main == root
        {
            return true;
        }
    }
    false
}

/// Records the agent before its session exists. Its mission `slot`, if
/// any, is released under the same lock, once the agent counts as launched.
#[allow(clippy::too_many_arguments)] // Each is one fact of the registration.
async fn register(
    shared: &Arc<Shared>,
    agent: AgentId,
    name: &str,
    role: Role,
    parent: Option<AgentId>,
    mission: Option<MissionId>,
    workspace: Workspace,
    slot: Option<Slot>,
) -> Result<(), DaemonError> {
    let mut state = shared.state.lock().await;
    let now = state.now();
    state.commit(vec![Event::new(
        now,
        EventKind::AgentRegistered {
            agent,
            name: name.to_owned(),
            role,
            harness: HarnessKind::ClaudeCode,
            launched_by: parent.map_or(LaunchedBy::Human, LaunchedBy::Agent),
            parent,
            mission,
            workspace,
            location: Location::Background,
        },
    )])?;
    drop(slot);
    Ok(())
}

/// Starts the harness session; a failure marks the agent failed.
async fn start_session(
    shared: &Arc<Shared>,
    harness: &Arc<dyn Harness>,
    agent: AgentId,
    spec: SpawnSpec,
) -> Result<(), DaemonError> {
    let started = {
        let harness = Arc::clone(harness);
        blocking(move || harness.spawn(&spec)).await?
    };
    match started {
        Ok(spawned) => {
            tracing::info!(%agent, handle = %spawned.handle, "agent launched");
            shared
                .state
                .lock()
                .await
                .expect_session(spawned.handle, agent, spawned.origin);
            Ok(())
        }
        Err(error) => {
            let mut state = shared.state.lock().await;
            let now = state.now();
            state.commit(vec![Event::new(
                now,
                EventKind::StateChanged {
                    agent,
                    state: AgentState::Failed,
                    detail: Some(format!("launch failed: {error}")),
                },
            )])?;
            Err(error.into())
        }
    }
}

/// What volee tells an agent it launched, rebuilt from the registry: a
/// resumed session needs its instructions again, since they travel on the
/// command line and not in the conversation.
pub(crate) fn instructions_for(
    registry: &volee_core::Registry,
    agent: &volee_core::Agent,
) -> String {
    let mission = agent.mission.and_then(|id| registry.mission(id));
    if agent.role == Role::Coordinator
        && let Some(mission) = mission
    {
        return coordinator_instructions(mission);
    }
    let workspace = &agent.workspace;
    let target = Target {
        name: workspace.repo.clone().unwrap_or_else(|| workspace.label()),
        root: workspace
            .repo_root
            .clone()
            .unwrap_or_else(|| workspace.cwd.clone()),
        base: workspace.base.clone().unwrap_or_else(|| "main".to_owned()),
    };
    let context = mission.map(|mission| MissionContext {
        id: mission.id,
        title: mission.title.clone(),
        kind: mission.kind,
        repos: mission.repos.clone(),
    });
    let branch = workspace.branch.clone().unwrap_or_default();
    worker_instructions(agent.role, context.as_ref(), &target, &branch)
}

/// The name under `.claude/worktrees/` of a worktree volee created in
/// `root`, which `--worktree` takes.
pub(crate) fn worktree_name(root: &Path, worktree: &Path) -> Option<String> {
    let rest = worktree.strip_prefix(root.join(IN_REPO_WORKTREES)).ok()?;
    let mut parts = rest.components();
    let name = parts.next()?.as_os_str().to_str()?.to_owned();
    parts.next().is_none().then_some(name)
}

/// The repository a launch names: a configured name, or a directory inside
/// a repository.
pub(crate) async fn resolve(
    shared: &Arc<Shared>,
    git: &Arc<dyn Git>,
    input: &str,
    base: Option<&str>,
) -> Result<Target, DaemonError> {
    let configured = shared.settings.repos.get(input);
    let (root, name) = locate(shared, git, input).await?;
    let chosen = base
        .map(str::to_owned)
        .or_else(|| configured.and_then(|repo| repo.base.clone()));
    let base = if let Some(base) = chosen {
        base
    } else {
        let git = Arc::clone(git);
        let root = root.clone();
        blocking(move || git.default_branch(&root))
            .await??
            .ok_or_else(|| {
                DaemonError::InvalidParams(format!(
                    "{name} has no main, master or develop branch: say which base to start from"
                ))
            })?
    };
    Ok(Target { name, root, base })
}

/// The main checkout of the repository `input` names (a configured name, or
/// a directory inside a repository), and the repository's name: the
/// configured one, else its directory's.
async fn locate(
    shared: &Arc<Shared>,
    git: &Arc<dyn Git>,
    input: &str,
) -> Result<(PathBuf, String), DaemonError> {
    let dir = if let Some(repo) = shared.settings.repos.get(input) {
        repo.path.clone()
    } else {
        let path = PathBuf::from(input);
        if !path.is_absolute() {
            return Err(DaemonError::InvalidParams(format!(
                "unknown repository `{input}`: use a [repos.<name>] of the configuration or an \
                 absolute path"
            )));
        }
        path
    };
    let info = {
        let git = Arc::clone(git);
        let dir = dir.clone();
        blocking(move || git.repo_info(&dir)).await??
    }
    .ok_or_else(|| {
        DaemonError::InvalidParams(format!("{} is not a git repository", dir.display()))
    })?;
    let root = info.main_checkout;
    let name = shared
        .settings
        .repo_at(&root)
        .map_or(info.name, |(name, _)| name.to_owned());
    Ok((root, name))
}

/// A branch name as a directory name: `feat/invoices` becomes
/// `feat-invoices`.
pub(crate) fn slug(branch: &str) -> String {
    let slug: String = branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches(|c| c == '-' || c == '.');
    if slug.is_empty() {
        "agent".to_owned()
    } else {
        slug.to_owned()
    }
}

/// Keeps volee's worktrees out of the main checkout's `git status`, as
/// Claude Code does, through the repository's local exclude file. Best
/// effort: a failure only makes `git status` noisier.
fn exclude_worktrees(root: &Path) {
    let git_dir = root.join(".git");
    if !git_dir.is_dir() {
        return;
    }
    let exclude = git_dir.join("info").join("exclude");
    let rule = format!("{IN_REPO_WORKTREES}/");
    let current = std::fs::read_to_string(&exclude).unwrap_or_default();
    if current.lines().any(|line| line.trim() == rule) {
        return;
    }
    let mut text = current;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&rule);
    text.push('\n');
    let written = exclude
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&exclude, text));
    if let Err(error) = written {
        tracing::warn!(%error, path = %exclude.display(), "cannot update the exclude file");
    }
}

/// Writes the agent's hook settings and MCP configuration, readable only by
/// the user.
pub(crate) fn write_agent_files(
    shared: &Shared,
    harness: &Arc<dyn Harness>,
    agent: AgentId,
) -> Result<(PathBuf, PathBuf), DaemonError> {
    let volee_bin = shared.volee_bin.as_deref().ok_or_else(|| {
        DaemonError::Unavailable("this daemon does not know where the volee program is".into())
    })?;
    let rewake = Duration::from_secs(shared.settings.agents.wake_timeout);
    let env = daemon_env(&shared.paths);
    let mut settings = harness.agent_settings(volee_bin, agent, rewake)?;
    merge_env(&mut settings, &env);
    let mut mcp = harness.mcp_config(volee_bin, agent)?;
    if let Some(servers) = mcp
        .get_mut("mcpServers")
        .and_then(serde_json::Value::as_object_mut)
    {
        for server in servers.values_mut() {
            merge_env(server, &env);
        }
    }
    let dir = shared.paths.agent_dir(agent);
    create_private_dir(&dir)?;
    let settings_file = dir.join("settings.json");
    let mcp_file = dir.join("mcp.json");
    write_json(&settings_file, &settings)?;
    write_json(&mcp_file, &mcp)?;
    Ok((settings_file, mcp_file))
}

/// The daemon's own paths, which an agent's hooks and MCP server need to reach
/// this daemon. A background session runs under Claude Code's supervisor, not
/// as a child of the daemon, so it never inherits the daemon's environment:
/// without these, a daemon with custom paths would never hear from its agents.
fn daemon_env(paths: &volee_proto::paths::Paths) -> serde_json::Map<String, serde_json::Value> {
    [
        ("VOLEE_SOCKET", &paths.socket),
        ("VOLEE_STATE_DIR", &paths.state_dir),
        ("VOLEE_CONFIG_DIR", &paths.config_dir),
        ("VOLEE_DATA_DIR", &paths.data_dir),
    ]
    .into_iter()
    .filter_map(|(name, path)| {
        path.to_str()
            .map(|path| (name.to_owned(), serde_json::Value::String(path.to_owned())))
    })
    .collect()
}

/// Adds `env` to the `env` object of a settings file or of an MCP server,
/// keeping what is already there.
fn merge_env(target: &mut serde_json::Value, env: &serde_json::Map<String, serde_json::Value>) {
    let Some(object) = target.as_object_mut() else {
        return;
    };
    let entry = object
        .entry("env")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if let Some(existing) = entry.as_object_mut() {
        for (name, value) in env {
            existing.insert(name.clone(), value.clone());
        }
    }
}

fn create_private_dir(dir: &Path) -> Result<(), DaemonError> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .and_then(|()| std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)))
        .map_err(|source| DaemonError::Write {
            path: dir.to_path_buf(),
            source,
        })
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), DaemonError> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    let text = serde_json::to_vec_pretty(value).map_err(|error| DaemonError::Write {
        path: path.to_path_buf(),
        source: std::io::Error::other(error),
    })?;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| file.write_all(&text))
        .map_err(|source| DaemonError::Write {
            path: path.to_path_buf(),
            source,
        })
}

pub(crate) fn permission_mode(shared: &Shared) -> Option<PermissionMode> {
    use volee_config::PermissionMode as Configured;
    shared
        .settings
        .agents
        .permission_mode
        .map(|mode| match mode {
            Configured::AcceptEdits => PermissionMode::AcceptEdits,
            Configured::Auto => PermissionMode::Auto,
            Configured::Manual => PermissionMode::Manual,
            Configured::DontAsk => PermissionMode::DontAsk,
            Configured::Plan => PermissionMode::Plan,
        })
}

/// What volee tells an agent it launched, appended to its system prompt so
/// that it survives compaction.
pub(crate) fn worker_instructions(
    role: Role,
    mission: Option<&MissionContext>,
    target: &Target,
    branch: &str,
) -> String {
    let place = format!(
        "You work in a git worktree of {}, on branch `{branch}`, created from `{}`. Stay in \
         this worktree.",
        target.name, target.base
    );
    let tools = "You run under volee, which shows every agent on this machine to the human. \
                 Use its MCP tools: `send_message` to write to another agent or to the human \
                 (\"human\"), `read_messages`, and `report` when your task is done, blocked or \
                 failed. Messages for you also arrive on their own at the end of your turn.";
    let Some(mission) = mission else {
        return format!("{place}\n\n{tools}");
    };
    let delivery = match mission.kind {
        MissionKind::Change => {
            "When your work is done: commit it, push the branch, open a draft pull request \
             (for example `gh pr create --draft --fill`), then call `report` with the pull \
             request's link. volee checks that the commits are pushed and the pull request is \
             open before it counts your work as delivered."
        }
        MissionKind::Analysis => {
            "This mission delivers a report, not code: do not change files. When you are done, \
             call `report` with your findings, precise enough to act on (files, lines, \
             commands, numbers)."
        }
    };
    let role = match role {
        Role::Worker => "a worker",
        _ => "an agent",
    };
    format!(
        "You are {role} of the volee mission \"{}\". {place}\n\n{delivery}\n\nIf you are \
         blocked or need a decision, ask your coordinator with `send_message`; it answers or \
         asks the human.\n\n{tools}",
        mission.title
    )
}

/// The harness, or why the daemon has none.
pub(crate) fn harness(shared: &Shared) -> Result<Arc<dyn Harness>, DaemonError> {
    shared.harness.clone().ok_or_else(|| {
        DaemonError::Unavailable(
            "Claude Code is not available to this daemon; see `volee daemon status`".into(),
        )
    })
}

/// git, or why the daemon has none.
pub(crate) fn git(shared: &Shared) -> Result<Arc<dyn Git>, DaemonError> {
    shared
        .git
        .clone()
        .ok_or_else(|| DaemonError::Unavailable("git is not available to this daemon".into()))
}

/// Runs blocking work (git, the harness) on the blocking thread pool.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, DaemonError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| DaemonError::Unavailable(format!("a background task failed: {error}")))
}

#[cfg(test)]
mod tests {
    use super::slug;

    #[test]
    fn worktree_names_come_from_volee_worktrees_only() {
        use std::path::Path;
        let root = Path::new("/work/alpha");
        assert_eq!(
            super::worktree_name(
                root,
                Path::new("/work/alpha/.claude/worktrees/feat-invoices")
            ),
            Some("feat-invoices".to_owned())
        );
        assert_eq!(super::worktree_name(root, Path::new("/work/alpha")), None);
        assert_eq!(
            super::worktree_name(root, Path::new("/elsewhere/.claude/worktrees/x")),
            None
        );
        assert_eq!(
            super::worktree_name(root, Path::new("/work/alpha/.claude/worktrees/a/b")),
            None
        );
    }

    #[test]
    fn branches_become_directory_names() {
        assert_eq!(slug("feat/invoices"), "feat-invoices");
        assert_eq!(slug("fix/a b"), "fix-a-b");
        assert_eq!(slug("v1.2"), "v1.2");
        assert_eq!(slug("/"), "agent");
        assert_eq!(slug(".hidden"), "hidden");
    }
}
