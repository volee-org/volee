//! Workspaces and delivery evidence, kept fresh from git and the forge
//! (ADR 0004, ADR 0010, ADR 0011).
//!
//! The refresher follows the events the daemon commits, like any
//! subscriber, and acts on what they say about agents:
//!
//! - an agent registers, or its workspace changes: its repository,
//!   worktree, branch and base branch are read with [`Git::repo_info`] and
//!   recorded with a `workspace-updated` event when they differ;
//! - an agent finishes a turn, stops or fails, the moment the mission
//!   supervisor judges it: its delivery evidence and its pull request are
//!   read again at once;
//! - every [`refresh_interval`](EvidenceSources::refresh_interval), the
//!   evidence of live agents is read again, and each repository is fetched
//!   at most every [`fetch_interval`](EvidenceSources::fetch_interval), when
//!   the person's `[git] fetch` setting allows it for the agent (by default,
//!   only for agents volee launched).
//!
//! git and the forge run on blocking threads, never under the state lock,
//! for a few agents at a time and one job per agent; triggers that arrive
//! meanwhile are merged into the next job. A `delivery-updated` event is
//! committed only when the evidence changed, or when it is the first check
//! since the agent's state changed, which the supervisor waits for.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::{Semaphore, broadcast};
use tokio::task::JoinHandle;
use volee_core::delivery::{Delivery, PullRequest};
use volee_core::role::LaunchedBy;
use volee_core::state::AgentState;
use volee_core::time::Timestamp;
use volee_core::workspace::Workspace;
use volee_core::{Agent, AgentId, Event, EventKind};
use volee_git::{DeliveryQuery, Forge, Git, GitError, RepoInfo};
use volee_proto::methods::ForgeStatus;

use crate::server::Shared;

/// How many agents are refreshed at the same time. Each job runs its git
/// commands one after the other.
const MAX_JOBS: usize = 4;

/// Where the daemon reads workspaces and delivery evidence from, and how
/// often. The default reads nothing: workspaces and evidence then change
/// only through the events clients send.
pub struct EvidenceSources {
    /// git, for repositories, worktrees and delivery evidence. Nothing is
    /// refreshed without it.
    pub git: Option<Arc<dyn Git>>,
    /// The forge, for pull requests and their checks.
    pub forge: Option<Arc<dyn Forge>>,
    /// The person's configuration: the names and base branches of the
    /// repositories it lists.
    pub settings: volee_config::Config,
    /// How often the evidence of live agents is read again.
    pub refresh_interval: Duration,
    /// How often each repository is fetched, at most.
    pub fetch_interval: Duration,
    /// How often each branch's pull request is looked up, at most, except
    /// right after its agent stopped.
    pub forge_interval: Duration,
}

impl EvidenceSources {
    /// The default [`refresh_interval`](Self::refresh_interval): git runs at
    /// most once a minute per worktree unless an event asks for it.
    pub const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

    /// The default [`fetch_interval`](Self::fetch_interval).
    pub const DEFAULT_FETCH_INTERVAL: Duration = Duration::from_secs(300);

    /// The default [`forge_interval`](Self::forge_interval).
    pub const DEFAULT_FORGE_INTERVAL: Duration = Duration::from_secs(60);

    /// Reads from `git`, with no forge, no configured repository and the
    /// default intervals.
    pub fn git(git: Arc<dyn Git>) -> Self {
        Self {
            git: Some(git),
            ..Self::default()
        }
    }
}

impl Default for EvidenceSources {
    fn default() -> Self {
        Self {
            git: None,
            forge: None,
            settings: volee_config::Config::default(),
            refresh_interval: Self::DEFAULT_REFRESH_INTERVAL,
            fetch_interval: Self::DEFAULT_FETCH_INTERVAL,
            forge_interval: Self::DEFAULT_FORGE_INTERVAL,
        }
    }
}

impl std::fmt::Debug for EvidenceSources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvidenceSources")
            .field("git", &self.git.is_some())
            .field("forge", &self.forge.is_some())
            .field("repos", &self.settings.repos.len())
            .field("refresh_interval", &self.refresh_interval)
            .field("fetch_interval", &self.fetch_interval)
            .field("forge_interval", &self.forge_interval)
            .finish()
    }
}

/// Starts the refresher; `None` without git.
pub(crate) fn spawn(shared: &Arc<Shared>, sources: EvidenceSources) -> Option<JoinHandle<()>> {
    let git = sources.git?;
    let refresher = Arc::new(Refresher {
        shared: Arc::clone(shared),
        git,
        forge: sources.forge,
        settings: sources.settings,
        refresh_interval: sources.refresh_interval,
        fetch_interval: sources.fetch_interval,
        forge_interval: sources.forge_interval,
        jobs: Semaphore::new(MAX_JOBS),
        book: Mutex::new(Book {
            // Optimistic until the forge's status says otherwise: the check
            // may take a network round trip.
            forge_usable: true,
            ..Book::default()
        }),
    });
    tokio::spawn(Arc::clone(&refresher).check_forge());
    Some(tokio::spawn(refresher.run()))
}

/// What an agent needs refreshed. Triggers for an agent that is being
/// refreshed are merged into one more job.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Wanted {
    /// Read the repository, worktree, branch and base again.
    workspace: bool,
    /// Read the delivery evidence again.
    delivery: bool,
    /// Look the pull request up even if it was looked up recently.
    forge_now: bool,
}

impl Wanted {
    /// The workspace, then the evidence of the workspace found.
    const WORKSPACE: Self = Self {
        workspace: true,
        delivery: true,
        forge_now: false,
    };
    /// The evidence.
    const DELIVERY: Self = Self {
        workspace: false,
        delivery: true,
        forge_now: false,
    };
    /// Everything the supervisor judges, at once.
    const FINISHED: Self = Self {
        workspace: false,
        delivery: true,
        forge_now: true,
    };

    fn merge(&mut self, other: Self) {
        self.workspace |= other.workspace;
        self.delivery |= other.delivery;
        self.forge_now |= other.forge_now;
    }
}

/// The refresher's memory, behind a lock never held across an `await`.
#[derive(Default)]
struct Book {
    /// Agents waiting for a job, with what they need.
    queue: BTreeMap<AgentId, Wanted>,
    /// Agents with a job running.
    running: BTreeSet<AgentId>,
    /// The workspace last recorded for each agent, so that the refresher
    /// does not take its own `workspace-updated` events for news.
    recorded: HashMap<AgentId, Workspace>,
    /// What `repo_info` said about each directory, and when.
    layouts: HashMap<PathBuf, (Instant, Option<RepoInfo>)>,
    /// The default branch of each repository, and when it was read.
    default_branches: HashMap<PathBuf, (Instant, Option<String>)>,
    /// When each repository was last fetched.
    fetched: HashMap<PathBuf, Instant>,
    /// When each branch's pull request was last looked up.
    looked_up: HashMap<(PathBuf, String), Instant>,
    /// Whether the forge can be used.
    forge_usable: bool,
}

struct Refresher {
    shared: Arc<Shared>,
    git: Arc<dyn Git>,
    forge: Option<Arc<dyn Forge>>,
    settings: volee_config::Config,
    refresh_interval: Duration,
    fetch_interval: Duration,
    forge_interval: Duration,
    jobs: Semaphore,
    book: Mutex<Book>,
}

impl Refresher {
    async fn run(self: Arc<Self>) {
        // Subscribe and look at every agent under one lock, so no event
        // falls in between.
        let mut events = {
            let state = self.shared.state.lock().await;
            for agent in state.registry().agents() {
                if let Some(wanted) = wanted_at_start(agent) {
                    self.schedule(agent.id, wanted);
                }
            }
            state.subscribe()
        };
        let mut ticks = tokio::time::interval_at(
            tokio::time::Instant::now() + self.refresh_interval,
            self.refresh_interval,
        );
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                received = events.recv() => match received {
                    Ok(event) => self.on_event(&event.event.kind),
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(
                            skipped,
                            "the evidence refresher lagged behind; looking at every agent"
                        );
                        self.rescan(wanted_at_start).await;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                _ = ticks.tick() => self.rescan(wanted_on_tick).await,
            }
        }
    }

    fn book(&self) -> MutexGuard<'_, Book> {
        // The book holds caches and queues that stay consistent between
        // statements, so a poisoned lock is still safe to use.
        self.book.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn on_event(self: &Arc<Self>, kind: &EventKind) {
        if let EventKind::AgentForgotten { agent } = kind {
            let mut book = self.book();
            book.recorded.remove(agent);
            book.queue.remove(agent);
            return;
        }
        let wanted = {
            let book = self.book();
            wanted_after(kind, |agent| book.recorded.get(&agent))
        };
        if let Some((agent, wanted)) = wanted {
            self.schedule(agent, wanted);
        }
    }

    async fn rescan(self: &Arc<Self>, wanted: fn(&Agent) -> Option<Wanted>) {
        let found: Vec<(AgentId, Wanted)> = {
            let state = self.shared.state.lock().await;
            state
                .registry()
                .agents()
                .filter_map(|agent| wanted(agent).map(|wanted| (agent.id, wanted)))
                .collect()
        };
        for (agent, wanted) in found {
            self.schedule(agent, wanted);
        }
    }

    /// Queues work for `agent`, and starts its job unless one runs.
    fn schedule(self: &Arc<Self>, agent: AgentId, wanted: Wanted) {
        let start = {
            let mut book = self.book();
            book.queue.entry(agent).or_default().merge(wanted);
            book.running.insert(agent)
        };
        if start {
            tokio::spawn(Arc::clone(self).drain(agent));
        }
    }

    /// Runs `agent`'s queued work until none is left.
    async fn drain(self: Arc<Self>, agent: AgentId) {
        loop {
            let wanted = {
                let mut book = self.book();
                let Some(wanted) = book.queue.remove(&agent) else {
                    book.running.remove(&agent);
                    return;
                };
                wanted
            };
            // The semaphore is never closed.
            let Ok(_permit) = self.jobs.acquire().await else {
                self.book().running.remove(&agent);
                return;
            };
            if wanted.workspace {
                self.refresh_workspace(agent).await;
            }
            if wanted.delivery {
                self.refresh_delivery(agent, wanted.forge_now).await;
            }
        }
    }

    /// Reads `agent` under the state lock, with the time.
    async fn read<T>(
        &self,
        agent: AgentId,
        read: impl FnOnce(&Agent) -> T,
    ) -> Option<(T, Timestamp)> {
        let state = self.shared.state.lock().await;
        let found = state.registry().agent(agent)?;
        Some((read(found), state.now()))
    }

    /// Runs a git query on a blocking thread. Failures are logged and
    /// return `None`: the next trigger tries again.
    async fn ask<T: Send + 'static>(
        &self,
        query: &'static str,
        run: impl FnOnce(&dyn Git) -> Result<T, GitError> + Send + 'static,
    ) -> Option<T> {
        let git = Arc::clone(&self.git);
        match tokio::task::spawn_blocking(move || run(git.as_ref())).await {
            Ok(Ok(value)) => Some(value),
            Ok(Err(error)) => {
                tracing::debug!(%error, query, "git query failed");
                None
            }
            Err(error) => {
                tracing::warn!(%error, query, "git query panicked");
                None
            }
        }
    }

    async fn refresh_workspace(&self, agent: AgentId) {
        let Some((current, _)) = self.read(agent, |found| found.workspace.clone()).await else {
            return;
        };
        let Some(layout) = self.layout(&current.cwd).await else {
            return;
        };
        let (name, base) = match &layout {
            Some(info) => {
                let needs_base = !same_repository(&current, info) || current.base.is_none();
                let base = if needs_base {
                    self.base_for(info).await
                } else {
                    None
                };
                (Some(self.repo_name(info)), base)
            }
            None => (None, None),
        };
        let next = enriched(&current, layout.as_ref(), name, base);
        self.book().recorded.insert(agent, next.clone());
        if next == current {
            return;
        }
        let mut state = self.shared.state.lock().await;
        // The workspace changed while git ran: the job its event queued
        // reads it again.
        if state.registry().agent(agent).map(|found| &found.workspace) != Some(&current) {
            return;
        }
        let now = state.now();
        let event = Event::new(
            now,
            EventKind::WorkspaceUpdated {
                agent,
                workspace: next,
            },
        );
        if let Err(error) = state.commit(vec![event]) {
            tracing::warn!(%error, %agent, "cannot record the workspace");
        }
    }

    /// What `repo_info` says about `cwd`, read at most once per refresh
    /// interval. `None` when git failed.
    async fn layout(&self, cwd: &Path) -> Option<Option<RepoInfo>> {
        if let Some((read_at, layout)) = self.book().layouts.get(cwd)
            && read_at.elapsed() < self.refresh_interval
        {
            return Some(layout.clone());
        }
        let dir = cwd.to_owned();
        let layout = self
            .ask("repo_info", move |git| git.repo_info(&dir))
            .await?;
        self.book()
            .layouts
            .insert(cwd.to_owned(), (Instant::now(), layout.clone()));
        Some(layout)
    }

    /// The display name of a repository: its name in the configuration,
    /// else the name git gives it.
    fn repo_name(&self, info: &RepoInfo) -> String {
        self.settings
            .repo_at(&info.main_checkout)
            .map_or_else(|| info.name.clone(), |(name, _)| name.to_owned())
    }

    /// The base branch of a repository: the configured one, else its
    /// default branch.
    async fn base_for(&self, info: &RepoInfo) -> Option<String> {
        if let Some(base) = self
            .settings
            .repo_at(&info.main_checkout)
            .and_then(|(_, repo)| repo.base.clone())
        {
            return Some(base);
        }
        let root = info.main_checkout.clone();
        if let Some((read_at, branch)) = self.book().default_branches.get(&root)
            && read_at.elapsed() < self.refresh_interval
        {
            return branch.clone();
        }
        let dir = root.clone();
        let branch = self
            .ask("default_branch", move |git| git.default_branch(&dir))
            .await?;
        self.book()
            .default_branches
            .insert(root, (Instant::now(), branch.clone()));
        branch
    }

    async fn refresh_delivery(&self, agent: AgentId, forge_now: bool) {
        let Some(((workspace, unmerged_tip, launched_by_volee), checked_at)) = self
            .read(agent, |found| {
                (
                    found.workspace.clone(),
                    found.delivery.unmerged_tip.clone(),
                    found.launched_by != LaunchedBy::Outside,
                )
            })
            .await
        else {
            return;
        };
        let (Some(branch), Some(base)) = (workspace.branch.clone(), workspace.base.clone()) else {
            return;
        };
        // The person decides whose remotes volee may fetch (`[git] fetch`):
        // by default, only those of the work volee started.
        if let Some(root) = &workspace.repo_root
            && self.settings.git.fetch.allows(launched_by_volee)
        {
            self.fetch_if_due(root).await;
        }
        let dir = workspace.cwd.clone();
        let query_branch = branch.clone();
        let Some(report) = self
            .ask("delivery", move |git| {
                let query = DeliveryQuery {
                    branch: &query_branch,
                    base: &base,
                    unmerged_tip: unmerged_tip.as_deref(),
                };
                git.delivery(&dir, &query)
            })
            .await
        else {
            return;
        };
        let looked_up = self.pull_request(&workspace, &branch, forge_now).await;

        let mut state = self.shared.state.lock().await;
        let Some(current) = state.registry().agent(agent) else {
            return;
        };
        // The agent moved on while git ran: the job its event queued reads
        // the new workspace.
        if current.workspace.cwd != workspace.cwd || current.workspace.branch != workspace.branch {
            return;
        }
        let fresh = Delivery {
            pull_request: looked_up.unwrap_or_else(|| current.delivery.pull_request.clone()),
            report_delivered: current.delivery.report_delivered,
            checked_at: Some(checked_at),
            ..report.delivery
        };
        if !worth_recording(&current.delivery, &fresh, current.state_since) {
            return;
        }
        let now = state.now();
        let event = Event::new(
            now,
            EventKind::DeliveryUpdated {
                agent,
                delivery: fresh,
            },
        );
        if let Err(error) = state.commit(vec![event]) {
            tracing::warn!(%error, %agent, "cannot record the delivery evidence");
        }
    }

    /// Fetches `root` unless it was fetched within the fetch interval.
    async fn fetch_if_due(&self, root: &Path) {
        let due = {
            let mut book = self.book();
            let due = book
                .fetched
                .get(root)
                .is_none_or(|at| at.elapsed() >= self.fetch_interval);
            if due {
                book.fetched.insert(root.to_owned(), Instant::now());
            }
            due
        };
        if due {
            let dir = root.to_owned();
            self.ask("fetch", move |git| git.fetch(&dir)).await;
        }
    }

    /// The pull request of `branch`, looked up unless it was within the
    /// forge interval (or `forge_now`). `None` when it was not looked up or
    /// the lookup failed: the previous one stays.
    async fn pull_request(
        &self,
        workspace: &Workspace,
        branch: &str,
        forge_now: bool,
    ) -> Option<Option<PullRequest>> {
        let forge = Arc::clone(self.forge.as_ref()?);
        let key = (
            workspace
                .repo_root
                .clone()
                .unwrap_or_else(|| workspace.cwd.clone()),
            branch.to_owned(),
        );
        {
            let mut book = self.book();
            if !book.forge_usable {
                return None;
            }
            let due = forge_now
                || book
                    .looked_up
                    .get(&key)
                    .is_none_or(|at| at.elapsed() >= self.forge_interval);
            if !due {
                return None;
            }
            book.looked_up.insert(key, Instant::now());
        }
        let dir = workspace.cwd.clone();
        let branch = branch.to_owned();
        match tokio::task::spawn_blocking(move || forge.pull_request(&dir, &branch)).await {
            Ok(Ok(found)) => Some(found),
            Ok(Err(error)) => {
                tracing::debug!(%error, "pull request lookup failed");
                None
            }
            Err(error) => {
                tracing::warn!(%error, "pull request lookup panicked");
                None
            }
        }
    }

    /// Asks the forge whether it can be used, once, and tells
    /// `volee daemon status`.
    async fn check_forge(self: Arc<Self>) {
        let Some(forge) = self.forge.clone() else {
            return;
        };
        let Ok(status) = tokio::task::spawn_blocking(move || forge.status()).await else {
            return;
        };
        if status.available {
            tracing::info!(
                program = %status.program,
                version = ?status.version,
                "pull requests come from the forge"
            );
        } else {
            tracing::info!(
                program = %status.program,
                detail = ?status.detail,
                "the forge is unavailable; pull requests are not looked up"
            );
        }
        self.book().forge_usable = status.available;
        self.shared.set_forge_status(ForgeStatus {
            program: status.program,
            available: status.available,
            version: status.version,
            detail: status.detail,
        });
    }
}

/// Whether a state is one the mission supervisor judges an agent in.
fn is_finished(state: &AgentState) -> bool {
    matches!(
        state,
        AgentState::Idle | AgentState::Done | AgentState::Stopped | AgentState::Failed
    )
}

/// What an event asks of the refresher, given the workspaces it recorded.
fn wanted_after<'a>(
    kind: &EventKind,
    recorded: impl Fn(AgentId) -> Option<&'a Workspace>,
) -> Option<(AgentId, Wanted)> {
    match kind {
        EventKind::AgentRegistered { agent, .. } => Some((*agent, Wanted::WORKSPACE)),
        EventKind::WorkspaceUpdated { agent, workspace } if recorded(*agent) != Some(workspace) => {
            Some((*agent, Wanted::WORKSPACE))
        }
        EventKind::StateChanged { agent, state, .. } if is_finished(state) => {
            Some((*agent, Wanted::FINISHED))
        }
        _ => None,
    }
}

/// What an agent needs when the refresher starts, or after it lagged: live
/// agents need everything, and an agent the supervisor waits for (evidence
/// older than its state) needs its evidence.
fn wanted_at_start(agent: &Agent) -> Option<Wanted> {
    if agent.state.is_live() {
        Some(Wanted::WORKSPACE)
    } else if agent.delivery.is_stale_at(agent.state_since) {
        Some(Wanted::FINISHED)
    } else {
        None
    }
}

/// What a live agent needs at each refresh interval: its evidence, and its
/// workspace while parts of it are unknown.
fn wanted_on_tick(agent: &Agent) -> Option<Wanted> {
    if !agent.state.is_live() {
        return None;
    }
    let workspace = &agent.workspace;
    if workspace.repo.is_none() || workspace.branch.is_none() || workspace.base.is_none() {
        Some(Wanted::WORKSPACE)
    } else {
        Some(Wanted::DELIVERY)
    }
}

/// Whether `info` is the repository `current` already names (or `current`
/// names none yet).
fn same_repository(current: &Workspace, info: &RepoInfo) -> bool {
    current
        .repo_root
        .as_ref()
        .is_none_or(|root| *root == info.main_checkout)
}

/// `current` completed with what git says about its directory.
///
/// Within the same repository, what is already known wins: the branch the
/// agent was launched on (or first seen on) stays its branch even when
/// `HEAD` moves, and so do its base branch and start commit. In another
/// repository, everything comes from git. Outside any repository, only the
/// directory is left.
fn enriched(
    current: &Workspace,
    layout: Option<&RepoInfo>,
    name: Option<String>,
    base: Option<String>,
) -> Workspace {
    let Some(info) = layout else {
        return Workspace::at(current.cwd.clone());
    };
    let same = same_repository(current, info);
    Workspace {
        cwd: current.cwd.clone(),
        repo: name,
        repo_root: Some(info.main_checkout.clone()),
        worktree: info.is_linked_worktree.then(|| info.top_level.clone()),
        branch: if same {
            current.branch.clone().or_else(|| info.branch.clone())
        } else {
            info.branch.clone()
        },
        base: if same {
            current.base.clone().or(base)
        } else {
            base
        },
        start_commit: if same {
            current.start_commit.clone()
        } else {
            None
        },
    }
}

/// Whether `fresh` evidence is worth an event: it says something new, or it
/// is the first check since the agent's state changed, which the supervisor
/// waits for. Repeated checks that find nothing new stay silent.
fn worth_recording(current: &Delivery, fresh: &Delivery, state_since: Timestamp) -> bool {
    if current.without_check_time() != fresh.without_check_time() {
        return true;
    }
    !fresh.is_stale_at(state_since) && current.checked_at.is_none_or(|at| at < state_since)
}

#[cfg(test)]
mod tests {
    use volee_core::delivery::PushState;
    use volee_core::harness::HarnessKind;
    use volee_core::role::{LaunchedBy, Role};
    use volee_core::workspace::Location;

    use super::*;

    fn agent_id() -> AgentId {
        AgentId::from_ulid(ulid::Ulid::from_parts(1_000, 1))
    }

    fn info(main: &str, top: &str, branch: Option<&str>) -> RepoInfo {
        RepoInfo {
            top_level: PathBuf::from(top),
            main_checkout: PathBuf::from(main),
            name: "volee".into(),
            is_linked_worktree: main != top,
            branch: branch.map(str::to_owned),
            head: Some("6adc11fa9619748617663ded520ce631e9292269".into()),
        }
    }

    #[test]
    fn registrations_workspaces_and_finished_turns_trigger_work() {
        let agent = agent_id();
        let registered = EventKind::AgentRegistered {
            agent,
            name: "billing".into(),
            role: Role::Worker,
            harness: HarnessKind::ClaudeCode,
            launched_by: LaunchedBy::Human,
            parent: None,
            mission: None,
            workspace: Workspace::at("/work"),
            location: Location::Background,
        };
        assert_eq!(
            wanted_after(&registered, |_| None),
            Some((agent, Wanted::WORKSPACE))
        );

        let ours = Workspace {
            repo: Some("volee".into()),
            ..Workspace::at("/work")
        };
        let updated = EventKind::WorkspaceUpdated {
            agent,
            workspace: ours.clone(),
        };
        // Its own record is not news; anybody else's change is.
        assert_eq!(wanted_after(&updated, |_| Some(&ours)), None);
        assert_eq!(
            wanted_after(&updated, |_| None),
            Some((agent, Wanted::WORKSPACE))
        );

        let state = |state| EventKind::StateChanged {
            agent,
            state,
            detail: None,
        };
        assert_eq!(
            wanted_after(&state(AgentState::Done), |_| None),
            Some((agent, Wanted::FINISHED))
        );
        assert_eq!(wanted_after(&state(AgentState::Working), |_| None), None);
    }

    #[test]
    fn known_branch_base_and_start_commit_survive_head_moving() {
        let current = Workspace {
            repo_root: Some("/work/volee".into()),
            branch: Some("feat/x".into()),
            base: Some("develop".into()),
            start_commit: Some("abc".into()),
            ..Workspace::at("/work/wt/feat")
        };
        // HEAD is detached now: the agent's branch is still `feat/x`.
        let layout = info("/work/volee", "/work/wt/feat", None);
        let next = enriched(
            &current,
            Some(&layout),
            Some("volee".into()),
            Some("main".into()),
        );
        assert_eq!(
            next,
            Workspace {
                cwd: "/work/wt/feat".into(),
                repo: Some("volee".into()),
                repo_root: Some("/work/volee".into()),
                worktree: Some("/work/wt/feat".into()),
                branch: Some("feat/x".into()),
                base: Some("develop".into()),
                start_commit: Some("abc".into()),
            }
        );
    }

    #[test]
    fn a_first_look_fills_everything_and_another_repository_starts_over() {
        let first = enriched(
            &Workspace::at("/work/volee"),
            Some(&info("/work/volee", "/work/volee", Some("main"))),
            Some("volee".into()),
            Some("main".into()),
        );
        assert_eq!(first.branch.as_deref(), Some("main"));
        assert_eq!(first.base.as_deref(), Some("main"));
        assert_eq!(first.worktree, None);

        let moved = Workspace {
            start_commit: Some("abc".into()),
            ..first.clone()
        };
        let elsewhere = enriched(
            &moved,
            Some(&info("/work/other", "/work/other", Some("trunk"))),
            Some("other".into()),
            Some("trunk".into()),
        );
        assert_eq!(elsewhere.branch.as_deref(), Some("trunk"));
        assert_eq!(elsewhere.start_commit, None);

        let outside = enriched(&first, None, None, None);
        assert_eq!(outside, Workspace::at("/work/volee"));
    }

    #[test]
    fn evidence_is_recorded_when_new_or_first_fresh_after_a_state_change() {
        let at = Timestamp::from_millis;
        let checked = |millis| Delivery {
            commits: 1,
            push: PushState::UpToDate,
            checked_at: Some(at(millis)),
            ..Delivery::default()
        };
        let state_since = at(2_000);
        // Same evidence, already fresh: nothing to say.
        assert!(!worth_recording(
            &checked(2_500),
            &checked(3_000),
            state_since
        ));
        // Same evidence, first check since the state changed: the
        // supervisor waits for it.
        assert!(worth_recording(
            &checked(1_000),
            &checked(3_000),
            state_since
        ));
        // A check that began before the state changed is not fresh.
        assert!(!worth_recording(
            &checked(1_000),
            &checked(1_500),
            state_since
        ));
        // New evidence is always worth an event.
        let pushed_more = Delivery {
            commits: 2,
            ..checked(1_500)
        };
        assert!(worth_recording(&checked(1_000), &pushed_more, state_since));
        // Never checked before.
        assert!(worth_recording(
            &Delivery {
                checked_at: None,
                ..checked(0)
            },
            &checked(3_000),
            state_since
        ));
    }

    #[test]
    fn wanted_work_merges() {
        let mut wanted = Wanted::DELIVERY;
        wanted.merge(Wanted::FINISHED);
        wanted.merge(Wanted::WORKSPACE);
        assert_eq!(
            wanted,
            Wanted {
                workspace: true,
                delivery: true,
                forge_now: true,
            }
        );
    }
}
