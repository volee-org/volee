//! The agents the daemon sees (v2, step 2): `claude agents --json` joined to
//! the hook events stored for each session and to what git says of each
//! session's branch.
//!
//! A poller keeps one reading in memory; a request that finds it too old
//! reads again first. Every source can fail or change shape without harm:
//! a failing listing keeps the previous cards and records why, a repository
//! git cannot read leaves its card without git facts, and a store that
//! cannot be opened leaves the hook facts empty. Nothing here panics or
//! stops the poller.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::{Notify, watch};
use volee_core::delivery::PrState;
use volee_core::finished::{Activity, BranchWork, HookKind, classify_hook, is_finished_work};
use volee_rules::FinishedFacts;

use crate::rules::RulesService;
use volee_core::reconcile::{ListedSession, Listing};
use volee_core::state::{AgentState, InputRequest, StateCategory};
use volee_core::time::Timestamp;
use volee_git::{BaseGuess, BaseSource, BranchHead, DeliveryQuery, Fingerprint, Forge, Git};
use volee_harness::Harness;
use volee_proto::methods::{
    AgentCard, AgentsView, GitFacts, HookFacts, PullRequestFacts, RepoFacts,
};
use volee_store::{CachedPullRequest, HookFilter, Store};

/// How many of a session's newest hook events are read to find its last
/// activity and its last stop.
const HOOK_WINDOW: u32 = 500;

/// How long a nudged poller waits before reading, so that the burst of hook
/// events around a stop is read once.
const NUDGE_DELAY: Duration = Duration::from_millis(300);

/// How the agents view is set up.
#[derive(Clone, Debug)]
pub struct AgentsViewConfig {
    /// How often the sessions are listed.
    pub poll_interval: Duration,
    /// The SQLite database the hook events are in.
    pub database: PathBuf,
}

/// How long the forge's answer that a branch has no merged pull request is
/// kept: a pull request can be opened and merged at any time. A merged one is
/// kept for good, and an error for [`ERROR_TTL`].
const PULL_REQUEST_TTL_MS: i64 = 5 * 60 * 1_000;

/// How long a failed question to the forge is not asked again.
const ERROR_TTL_MS: i64 = 60 * 1_000;

/// How long the git facts of a worktree are kept while nothing about it
/// moves. What can change without moving the worktree itself (the base
/// branch gaining commits, the remote) is picked up at the latest then.
const FACTS_TTL: Duration = Duration::from_secs(60);

/// What the poller keeps between two readings.
#[derive(Default)]
struct Memory {
    /// For each branch, the last tip seen with commits of its own: the
    /// memory that proves a merge once that commit is part of the base
    /// branch (also kept in SQLite, which outlives the daemon).
    tips: HashMap<(PathBuf, String), String>,
    /// The git facts of each session's directory, while its fingerprint
    /// holds.
    facts: HashMap<PathBuf, CachedFacts>,
}

/// The git facts of a directory, and what they were read under.
struct CachedFacts {
    key: FactsKey,
    /// When the refs of the base branch last changed, as read with the
    /// facts: a merge into the base does not touch the agent's worktree.
    base_refs: Vec<Option<SystemTime>>,
    read_at: Instant,
    repo: RepoFacts,
    facts: Option<GitFacts>,
}

/// What must stay the same for cached facts to be reused: the tree's branch,
/// commit and uncommitted state (one cheap `git status`) and when its index
/// last changed.
#[derive(PartialEq, Eq)]
struct FactsKey {
    fingerprint: Fingerprint,
    index_modified: Option<SystemTime>,
}

/// Where the readings of a card come from.
struct Sources {
    git: Option<Arc<dyn Git>>,
    /// The forge, for squash merges, which git cannot see.
    forge: Option<Arc<dyn Forge>>,
    store: Option<Arc<Mutex<Store>>>,
    /// Whether the forge can be used, asked once: it may reach the network.
    forge_usable: OnceLock<bool>,
    /// The rules, which decide what finished work is. `None`: the built-in
    /// rule ([`is_finished_work`]).
    rules: Option<Arc<RulesService>>,
}

/// The poller and its last reading.
pub(crate) struct Agents {
    harness: Arc<dyn Harness>,
    sources: Arc<Sources>,
    inner: tokio::sync::Mutex<Inner>,
    /// Counts the readings that changed what a person sees.
    revision: watch::Sender<u64>,
    /// Asks the poller to read now, because a hook event just arrived.
    nudge: Notify,
    /// The last reading, readable at any time: a reading can take seconds
    /// (the listing and git), and whoever only wants the last one must not
    /// wait for it.
    published: Mutex<Option<AgentsView>>,
    /// The last successful listing, for the agents a mission knows: they
    /// follow it (see [`follow_listings`]), so Claude Code is asked once.
    listings: watch::Sender<Option<Arc<Listing>>>,
}

impl std::fmt::Debug for Agents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agents").finish_non_exhaustive()
    }
}

#[derive(Default)]
struct Inner {
    view: AgentsView,
    read_at: Option<Instant>,
    /// Whether the last listing failed, so that a failure is logged once.
    failing: bool,
    memory: Memory,
    /// How many readings succeeded, so that the first one always counts.
    read_count: u64,
}

impl Agents {
    pub(crate) fn new(
        harness: Arc<dyn Harness>,
        git: Option<Arc<dyn Git>>,
        forge: Option<Arc<dyn Forge>>,
        database: &Path,
        rules: Option<Arc<RulesService>>,
    ) -> Self {
        let store = match Store::open(database) {
            Ok(store) => Some(Arc::new(Mutex::new(store))),
            Err(error) => {
                tracing::warn!(%error, "the agents view cannot read hook events");
                None
            }
        };
        Self {
            harness,
            sources: Arc::new(Sources {
                git,
                forge,
                store,
                forge_usable: OnceLock::new(),
                rules,
            }),
            inner: tokio::sync::Mutex::default(),
            revision: watch::channel(0).0,
            nudge: Notify::new(),
            published: Mutex::new(None),
            listings: watch::channel(None).0,
        }
    }

    /// A receiver of the listings the poller reads.
    pub(crate) fn listings(&self) -> watch::Receiver<Option<Arc<Listing>>> {
        self.listings.subscribe()
    }

    /// A receiver that wakes each time a reading changed the cards or the
    /// error. The value is a counter: only the change matters.
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }

    /// Asks the poller for a reading as soon as it can (hook events often
    /// mean that a session just changed state).
    pub(crate) fn nudge(&self) {
        self.nudge.notify_one();
    }

    /// The last reading, if there is one, without waiting for a reading in
    /// progress.
    pub(crate) fn peek(&self) -> Option<AgentsView> {
        self.published
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn publish(&self, view: &AgentsView) {
        *self
            .published
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(view.clone());
    }

    /// The last reading, without reading again unless there is none yet.
    pub(crate) async fn latest(&self) -> AgentsView {
        self.get(Duration::MAX).await
    }

    /// The agents, read again first when the last reading is older than
    /// `max_age`.
    pub(crate) async fn get(&self, max_age: Duration) -> AgentsView {
        let mut inner = self.inner.lock().await;
        if inner.read_at.is_none_or(|at| at.elapsed() > max_age) {
            self.read(&mut inner).await;
        }
        inner.view.clone()
    }

    async fn read(&self, inner: &mut Inner) {
        let harness = Arc::clone(&self.harness);
        let listed = tokio::task::spawn_blocking(move || harness.list()).await;
        let listing = match listed {
            Ok(Ok(listing)) => listing,
            Ok(Err(error)) => {
                self.fail(inner, format!("cannot list sessions: {error}"));
                return;
            }
            Err(error) => {
                self.fail(inner, format!("the listing task failed: {error}"));
                return;
            }
        };
        if inner.failing {
            tracing::info!("listing Claude Code's sessions works again");
        }
        inner.failing = false;
        self.listings.send_replace(Some(Arc::new(listing.clone())));
        let sources = Arc::clone(&self.sources);
        let memory = std::mem::take(&mut inner.memory);
        let built = tokio::task::spawn_blocking(move || {
            let mut memory = memory;
            // The daemon's own review sessions are not agents.
            let cards: Vec<AgentCard> = listing
                .sessions
                .iter()
                .filter(|session| !crate::review::is_review_session(session.name.as_deref()))
                .map(|session| card(session, &sources, &mut memory))
                .collect();
            // A directory no session is in any more costs nothing to forget.
            memory
                .facts
                .retain(|cwd, _| listing.sessions.iter().any(|s| &s.cwd == cwd));
            (cards, memory)
        })
        .await;
        match built {
            Ok((mut cards, memory)) => {
                sort(&mut cards);
                inner.memory = memory;
                let changed = inner.view.agents != cards || inner.view.error.is_some();
                inner.view = AgentsView {
                    updated_at: Some(now()),
                    error: None,
                    agents: cards,
                };
                inner.read_at = Some(Instant::now());
                self.publish(&inner.view);
                if changed || inner.read_count == 0 {
                    self.revision.send_modify(|revision| *revision += 1);
                }
                inner.read_count += 1;
            }
            Err(error) => self.fail(inner, format!("reading the agents failed: {error}")),
        }
    }

    /// Records a failed reading: the previous cards stay, with the reason.
    fn fail(&self, inner: &mut Inner, why: String) {
        if inner.failing {
            tracing::debug!(%why, "listing Claude Code's sessions still fails");
        } else {
            tracing::warn!(%why, "listing Claude Code's sessions fails");
        }
        inner.failing = true;
        if inner.view.error.as_deref() != Some(why.as_str()) {
            self.revision.send_modify(|revision| *revision += 1);
        }
        inner.view.error = Some(why);
        self.publish(&inner.view);
        // Try again at the next request or tick, not at every request.
        inner.read_at = Some(Instant::now());
    }
}

/// The agents view, when it is configured and Claude Code is available.
pub(crate) fn build(
    config: Option<&AgentsViewConfig>,
    harness: Option<Arc<dyn Harness>>,
    git: Option<Arc<dyn Git>>,
    forge: Option<Arc<dyn Forge>>,
    rules: Option<Arc<RulesService>>,
) -> Option<Arc<Agents>> {
    let (config, harness) = config.zip(harness)?;
    Some(Arc::new(Agents::new(
        harness,
        git,
        forge,
        &config.database,
        rules,
    )))
}

/// Starts the poller of `agents`, when there is a view to poll.
pub(crate) fn spawn_poll(
    config: Option<&AgentsViewConfig>,
    agents: Option<Arc<Agents>>,
    stop: &watch::Receiver<bool>,
) -> Option<tokio::task::JoinHandle<()>> {
    let (config, agents) = config.zip(agents)?;
    Some(tokio::spawn(poll(
        agents,
        config.poll_interval,
        stop.clone(),
    )))
}

/// Reads the agents at every `interval` until `stop` says so.
pub(crate) async fn poll(agents: Arc<Agents>, interval: Duration, mut stop: watch::Receiver<bool>) {
    loop {
        agents.get(Duration::ZERO).await;
        tokio::select! {
            () = tokio::time::sleep(interval) => {}
            () = agents.nudge.notified() => {
                // A burst of hook events is read once.
                tokio::time::sleep(NUDGE_DELAY).await;
            }
            _ = stop.changed() => return,
        }
    }
}

/// Applies each listing the agents view reads to the agents a mission knows
/// (the registry), until `stop` says so. It creates no agent: the view is
/// the one list of sessions.
pub(crate) async fn follow_listings(
    shared: Arc<crate::server::Shared>,
    agents: Arc<Agents>,
    mut stop: watch::Receiver<bool>,
) {
    let mut listings = agents.listings();
    loop {
        tokio::select! {
            changed = listings.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            _ = stop.changed() => return,
        }
        let Some(listing) = listings.borrow_and_update().clone() else {
            continue;
        };
        let mut state = shared.state.lock().await;
        let events = volee_core::reconcile::reconcile(state.registry(), &listing, state.now());
        if events.is_empty() {
            continue;
        }
        let count = events.len();
        match state.commit(events) {
            Ok(_) => tracing::debug!(events = count, "the listing moved the agents of missions"),
            Err(error) => tracing::error!(%error, "cannot apply the listing to the agents"),
        }
    }
}

fn now() -> Timestamp {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });
    Timestamp::from_millis(millis)
}

/// The card of one session.
fn card(session: &ListedSession, sources: &Sources, memory: &mut Memory) -> AgentCard {
    let session_id = session.key.id.clone();
    let hooks = sources
        .store
        .as_deref()
        .map_or_else(HookFacts::default, |store| hook_facts(store, &session_id));
    let mut card = AgentCard {
        waiting_for: waiting_for(&session.state),
        session_id,
        name: session.name.clone(),
        background: session.background,
        alive: session.alive,
        pid: session.pid,
        cwd: session.cwd.clone(),
        started_at: session.started_at,
        state: session.state.clone(),
        repo: None,
        git: None,
        git_error: None,
        hooks,
        finished_work: false,
    };
    // Only a directory inside a git repository is worth a git command.
    if let Some(git) = sources.git.as_deref()
        && let Some(top) = git_root(&session.cwd)
    {
        match cached_git_facts(sources, git, &top, &session.cwd, memory) {
            Ok(Some((repo, facts))) => {
                card.repo = Some(repo);
                card.git = facts;
            }
            Ok(None) => {}
            Err(error) => card.git_error = Some(error),
        }
    }
    if let Some(facts) = &card.git {
        let work = BranchWork {
            commits: facts.ahead,
            unique_commits: facts.unique,
            merged: facts.merged,
        };
        card.finished_work = match (&sources.rules, &card.repo) {
            (Some(rules), repo) => rules.finished(
                repo.as_ref().map(|repo| repo.root.as_path()),
                &FinishedFacts {
                    stopped_last: card.hooks.activity.stopped_last(),
                    busy: matches!(
                        card.state,
                        AgentState::Working | AgentState::NeedsInput { .. } | AgentState::Launching
                    ),
                    commits: work.commits,
                    unique_commits: work.unique_commits,
                    merged: work.merged,
                },
            ),
            (None, _) => is_finished_work(&card.hooks.activity, &card.state, &work),
        };
    }
    card
}

/// What a session waits for, in words.
fn waiting_for(state: &AgentState) -> Option<String> {
    let AgentState::NeedsInput { request } = state else {
        return None;
    };
    Some(match request {
        InputRequest::Permission { tool: Some(tool) } => format!("permission for {tool}"),
        InputRequest::Permission { tool: None } => "a permission".to_owned(),
        InputRequest::Question => "an answer to a question".to_owned(),
        InputRequest::Dialog => "a dialog".to_owned(),
        InputRequest::Other { detail } => detail.clone(),
    })
}

/// The hook events of `session_id`, reduced to when it last acted and when
/// it last stopped.
fn hook_facts(store: &Mutex<Store>, session_id: &str) -> HookFacts {
    let events = {
        let store = store.lock().unwrap_or_else(PoisonError::into_inner);
        store.hook_events(&HookFilter {
            session_id: Some(session_id.to_owned()),
            limit: Some(HOOK_WINDOW),
        })
    };
    let events = match events {
        Ok(events) => events,
        Err(error) => {
            tracing::warn!(%error, session_id, "cannot read the hook events of a session");
            return HookFacts::default();
        }
    };
    let mut activity = Activity::default();
    for event in &events {
        let notification_type = (event.event == "Notification")
            .then(|| notification_type(&event.payload))
            .flatten();
        match classify_hook(&event.event, notification_type.as_deref()) {
            HookKind::Activity => activity.last_activity_at = Some(event.received_at),
            HookKind::Completion => activity.last_stop_at = Some(event.received_at),
            HookKind::Other => {}
        }
    }
    HookFacts {
        events: events.len() as u64,
        last_event: events.last().map(|event| event.event.clone()),
        last_event_at: events.last().map(|event| event.received_at),
        activity,
    }
}

fn notification_type(payload: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()?
        .get("notification_type")?
        .as_str()
        .map(str::to_owned)
}

/// The top of the git working tree that holds `dir`, found by looking for a
/// `.git` entry upwards: no process is started to tell a directory outside
/// any repository.
fn git_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .map(Path::to_path_buf)
}

/// The git directory of the working tree at `top`, and the directory it
/// shares with the repository's other worktrees. A linked worktree's `.git`
/// is a file that points to its own git directory, which names the common
/// one in `commondir`.
fn git_dirs(top: &Path) -> Option<(PathBuf, PathBuf)> {
    let dot_git = top.join(".git");
    if dot_git.is_dir() {
        return Some((dot_git.clone(), dot_git));
    }
    let pointer = std::fs::read_to_string(&dot_git).ok()?;
    let git_dir = top.join(pointer.trim().strip_prefix("gitdir:")?.trim());
    let common = std::fs::read_to_string(git_dir.join("commondir"))
        .map_or_else(|_| git_dir.clone(), |text| git_dir.join(text.trim()));
    Some((git_dir, common))
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

/// When the index of the working tree at `top` last changed.
fn index_modified(top: &Path) -> Option<SystemTime> {
    git_dirs(top).and_then(|(git_dir, _)| modified(&git_dir.join("index")))
}

/// When the refs of the base branch last changed: the local branch, its copy
/// on `origin` and the packed refs. Moves when work is merged into the base
/// or the base is fetched, which leaves the agent's worktree untouched.
fn base_refs_modified(top: &Path, base: Option<&str>) -> Vec<Option<SystemTime>> {
    let Some((_, common)) = git_dirs(top) else {
        return Vec::new();
    };
    let mut stamps = vec![modified(&common.join("packed-refs"))];
    if let Some(base) = base {
        stamps.push(modified(&common.join("refs/heads").join(base)));
        stamps.push(modified(&common.join("refs/remotes/origin").join(base)));
    }
    stamps
}

/// The git facts of `cwd`, read again only when the tree moved since they
/// were read (or a minute passed): one `git status` is all an idle agent
/// costs.
fn cached_git_facts(
    sources: &Sources,
    git: &dyn Git,
    top: &Path,
    cwd: &Path,
    memory: &mut Memory,
) -> Result<Option<(RepoFacts, Option<GitFacts>)>, String> {
    let Some(fingerprint) = git.fingerprint(top).map_err(|error| error.to_string())? else {
        return Ok(None);
    };
    let key = FactsKey {
        fingerprint,
        index_modified: index_modified(top),
    };
    if let Some(cached) = memory.facts.get(cwd)
        && cached.key == key
        && cached.read_at.elapsed() < FACTS_TTL
        && cached.base_refs == base_refs_modified(top, cached.repo.base.as_deref())
    {
        return Ok(Some((cached.repo.clone(), cached.facts.clone())));
    }
    let read = git_facts(sources, git, cwd, &mut memory.tips)?;
    match &read {
        Some((repo, facts)) => {
            memory.facts.insert(
                cwd.to_path_buf(),
                CachedFacts {
                    key,
                    base_refs: base_refs_modified(top, repo.base.as_deref()),
                    read_at: Instant::now(),
                    repo: repo.clone(),
                    facts: facts.clone(),
                },
            );
        }
        None => {
            memory.facts.remove(cwd);
        }
    }
    Ok(read)
}

/// Where `cwd` sits and what git says of its branch. `Ok(None)` outside a
/// repository; the error is already a sentence.
fn git_facts(
    sources: &Sources,
    git: &dyn Git,
    cwd: &Path,
    memory: &mut HashMap<(PathBuf, String), String>,
) -> Result<Option<(RepoFacts, Option<GitFacts>)>, String> {
    let info = match git.repo_info(cwd) {
        Ok(Some(info)) => info,
        Ok(None) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    // The base of the branch the agent is on, else the default branch.
    let guess = match &info.branch {
        Some(branch) => git.base_of(&info.main_checkout, branch).ok().flatten(),
        None => git
            .default_branch(&info.main_checkout)
            .ok()
            .flatten()
            .map(|branch| BaseGuess {
                branch,
                source: BaseSource::Default,
            }),
    };
    let base = guess.as_ref().map(|guess| guess.branch.clone());
    let repo = RepoFacts {
        name: info.name.clone(),
        root: info.main_checkout.clone(),
        worktree: info.is_linked_worktree.then(|| info.top_level.clone()),
        branch: info.branch.clone(),
        base: base.clone(),
        base_source: guess.map(|guess| guess.source.label().to_owned()),
    };
    // The default branch has no base; what it holds beyond its remote is
    // still told, so it is compared with itself (its remote copy).
    let compare = base.clone().or_else(|| {
        let branch = info.branch.as_ref()?;
        let default = git.default_branch(&info.main_checkout).ok().flatten()?;
        (&default == branch).then_some(default)
    });
    let (Some(branch), Some(base)) = (&info.branch, &compare) else {
        return Ok(Some((repo, None)));
    };
    let key = (info.main_checkout.clone(), branch.clone());
    let remembered = remembered_tip(sources, memory, &key);
    let query = DeliveryQuery {
        branch,
        base,
        unmerged_tip: remembered.as_deref(),
    };
    let report = git
        .delivery(&info.top_level, &query)
        .map_err(|error| error.to_string())?;
    let delivery = &report.delivery;
    remember_tip(
        sources,
        memory,
        &key,
        remembered.as_deref(),
        delivery.unmerged_tip.as_deref(),
    );
    let mut facts = GitFacts {
        ahead: delivery.commits,
        unique: report.unique_commits,
        behind: report.behind,
        dirty: delivery.dirty,
        merged: delivery.merged_into_base,
        diff: delivery.diff,
        push: delivery.push,
        merge_source: delivery.merged_into_base.then(|| "history".to_owned()),
        merge_unknown: None,
        pull_request: None,
    };
    // A squash leaves nothing in the history: ask the forge, when there is
    // work whose merge is in doubt.
    if !facts.merged
        && facts.ahead > 0
        && facts.unique > 0
        && let Some(tip) = &delivery.tip
    {
        match pull_request_state(sources, git, &info.main_checkout, branch, tip) {
            PrAnswer::Merged(pull_request) => {
                facts.merged = true;
                facts.merge_source = Some("pull-request".to_owned());
                facts.pull_request = Some(pull_request);
            }
            PrAnswer::NotMerged(pull_request) => facts.pull_request = pull_request,
            PrAnswer::Unknown(why) => facts.merge_unknown = Some(why),
        }
    }
    Ok(Some((repo, Some(facts))))
}

/// The tip remembered for a branch: from this run's memory, else from the
/// database, where a previous run left it.
fn remembered_tip(
    sources: &Sources,
    memory: &HashMap<(PathBuf, String), String>,
    key: &(PathBuf, String),
) -> Option<String> {
    if let Some(tip) = memory.get(key) {
        return Some(tip.clone());
    }
    let store = sources.store.as_deref()?;
    let store = store.lock().unwrap_or_else(PoisonError::into_inner);
    store
        .unmerged_tip(&key.0.to_string_lossy(), &key.1)
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "cannot read what was remembered of a branch");
            None
        })
}

/// Keeps the new tip in memory and, when it changed, in the database.
fn remember_tip(
    sources: &Sources,
    memory: &mut HashMap<(PathBuf, String), String>,
    key: &(PathBuf, String),
    before: Option<&str>,
    after: Option<&str>,
) {
    match after {
        Some(tip) => memory.insert(key.clone(), tip.to_owned()),
        None => memory.remove(key),
    };
    if before == after {
        return;
    }
    let Some(store) = sources.store.as_deref() else {
        return;
    };
    let store = store.lock().unwrap_or_else(PoisonError::into_inner);
    if let Err(error) =
        store.set_unmerged_tip(&key.0.to_string_lossy(), &key.1, after, now().as_millis())
    {
        tracing::warn!(%error, "cannot remember the tip of a branch");
    }
}

/// What the forge says of a branch's pull request.
enum PrAnswer {
    /// Merged: the branch's work is in its base, as a squash or any merge.
    Merged(PullRequestFacts),
    /// Not merged: open, closed, or none was found.
    NotMerged(Option<PullRequestFacts>),
    /// It could not be asked, and why.
    Unknown(String),
}

/// Asks the forge about the pull request of `branch` at `tip`, through the
/// database's cache: a merged answer for good, others for a few minutes.
fn pull_request_state(
    sources: &Sources,
    git: &dyn Git,
    repo: &Path,
    branch: &str,
    tip: &str,
) -> PrAnswer {
    let Some(forge) = sources.forge.as_deref() else {
        return PrAnswer::Unknown("no forge client (gh) is configured".to_owned());
    };
    if !*sources
        .forge_usable
        .get_or_init(|| forge.status().available)
    {
        return PrAnswer::Unknown("gh is missing or not signed in".to_owned());
    }
    let slug = match git.origin(repo) {
        Ok(Some(slug)) => slug,
        Ok(None) => return PrAnswer::Unknown("the repository has no GitHub remote".to_owned()),
        Err(error) => return PrAnswer::Unknown(error.to_string()),
    };
    let key = repo.to_string_lossy();
    let at = now().as_millis();
    let cached = sources.store.as_deref().and_then(|store| {
        let store = store.lock().unwrap_or_else(PoisonError::into_inner);
        store.cached_pull_request(&key, branch, tip).ok().flatten()
    });
    if let Some(cached) = cached.filter(|cached| fresh(cached, at)) {
        return answer_of(&cached);
    }
    let asked = forge.pull_request(
        repo,
        &BranchHead {
            branch,
            origin: Some(&slug),
            tip,
        },
    );
    let cached = match asked {
        Ok(Some(pull_request)) => CachedPullRequest {
            state: pr_state_name(pull_request.state).to_owned(),
            number: i64::try_from(pull_request.number).ok(),
            url: Some(pull_request.url),
            checked_at: at,
        },
        Ok(None) => CachedPullRequest {
            state: "none".to_owned(),
            number: None,
            url: None,
            checked_at: at,
        },
        Err(error) => {
            tracing::debug!(%error, branch, "cannot ask the forge about a pull request");
            CachedPullRequest {
                state: "error".to_owned(),
                number: None,
                url: Some(error.to_string()),
                checked_at: at,
            }
        }
    };
    if let Some(store) = sources.store.as_deref() {
        let store = store.lock().unwrap_or_else(PoisonError::into_inner);
        if let Err(error) = store.cache_pull_request(&key, branch, tip, &cached) {
            tracing::warn!(%error, "cannot cache the forge's answer");
        }
    }
    answer_of(&cached)
}

/// Whether a cached answer can still be used at `at`.
fn fresh(cached: &CachedPullRequest, at: i64) -> bool {
    let ttl = match cached.state.as_str() {
        "merged" => return true,
        "error" => ERROR_TTL_MS,
        _ => PULL_REQUEST_TTL_MS,
    };
    at.saturating_sub(cached.checked_at) < ttl
}

fn answer_of(cached: &CachedPullRequest) -> PrAnswer {
    let facts = |state| {
        Some(PullRequestFacts {
            number: cached
                .number
                .and_then(|n| u64::try_from(n).ok())
                .unwrap_or(0),
            url: cached.url.clone().unwrap_or_default(),
            state,
        })
    };
    match cached.state.as_str() {
        "merged" => {
            facts(PrState::Merged).map_or_else(|| PrAnswer::NotMerged(None), PrAnswer::Merged)
        }
        "open" => PrAnswer::NotMerged(facts(PrState::Open)),
        "draft" => PrAnswer::NotMerged(facts(PrState::Draft)),
        "closed" => PrAnswer::NotMerged(facts(PrState::Closed)),
        "error" => PrAnswer::Unknown(format!(
            "the forge could not be asked: {}",
            cached.url.as_deref().unwrap_or("unknown error")
        )),
        _ => PrAnswer::NotMerged(None),
    }
}

fn pr_state_name(state: PrState) -> &'static str {
    match state {
        PrState::Draft => "draft",
        PrState::Open => "open",
        PrState::Merged => "merged",
        PrState::Closed => "closed",
    }
}

/// The ones that need a person first: waiting, then finished work, then
/// working, then idle, then the rest; running before ended.
fn sort(cards: &mut [AgentCard]) {
    let rank = |card: &AgentCard| -> u8 {
        if !card.alive {
            return 9;
        }
        if card.state.category() == StateCategory::NeedsInput {
            return 0;
        }
        if card.finished_work {
            return 1;
        }
        match card.state.category() {
            StateCategory::Working => 2,
            StateCategory::Launching => 3,
            StateCategory::Idle => 4,
            _ => 5,
        }
    };
    cards.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use volee_core::harness::SessionKey;

    fn no_sources(git: Option<Arc<dyn Git>>) -> Sources {
        Sources {
            git,
            forge: None,
            store: None,
            forge_usable: OnceLock::new(),
            rules: None,
        }
    }

    #[test]
    fn a_cached_pull_request_answer_lives_as_long_as_its_kind_allows() {
        let answer = |state: &str, checked_at| CachedPullRequest {
            state: state.to_owned(),
            number: Some(7),
            url: Some("https://example.com/pull/7".into()),
            checked_at,
        };
        let now = 10_000_000;
        assert!(fresh(&answer("merged", 0), now), "a merged one is for good");
        assert!(fresh(&answer("open", now - 1_000), now));
        assert!(!fresh(&answer("open", now - PULL_REQUEST_TTL_MS - 1), now));
        assert!(fresh(&answer("error", now - 1_000), now));
        assert!(!fresh(&answer("error", now - ERROR_TTL_MS - 1), now));
        match answer_of(&answer("merged", 0)) {
            PrAnswer::Merged(facts) => {
                assert_eq!((facts.number, facts.state), (7, PrState::Merged));
            }
            _ => panic!("merged"),
        }
        assert!(matches!(
            answer_of(&answer("none", 0)),
            PrAnswer::NotMerged(None)
        ));
        assert!(matches!(
            answer_of(&answer("error", 0)),
            PrAnswer::Unknown(_)
        ));
    }

    fn session(id: &str, state: AgentState, alive: bool) -> ListedSession {
        ListedSession {
            alive,
            ..ListedSession::new(SessionKey::claude(id), "/nowhere/at/all", state)
        }
    }

    #[test]
    fn what_a_session_waits_for_is_said_in_words() {
        assert_eq!(waiting_for(&AgentState::Idle), None);
        assert_eq!(
            waiting_for(&AgentState::NeedsInput {
                request: InputRequest::Permission { tool: None }
            })
            .as_deref(),
            Some("a permission")
        );
        assert_eq!(
            waiting_for(&AgentState::NeedsInput {
                request: InputRequest::Other {
                    detail: "sandbox request".into()
                }
            })
            .as_deref(),
            Some("sandbox request")
        );
    }

    #[test]
    fn a_session_whose_directory_is_gone_still_gets_a_card_without_git_facts() {
        let mut memory = Memory::default();
        let sources = no_sources(Some(Arc::new(volee_git::GitCli::new())));
        let card = card(
            &session("s1", AgentState::Idle, true),
            &sources,
            &mut memory,
        );
        assert_eq!(card.session_id, "s1");
        assert!(card.repo.is_none() && card.git.is_none() && card.git_error.is_none());
        assert_eq!(card.hooks, HookFacts::default());
        assert!(!card.finished_work);
    }

    #[test]
    fn the_ones_that_need_a_person_come_first_and_the_ended_last() {
        let mut cards: Vec<AgentCard> = [
            ("idle", AgentState::Idle, true),
            ("ended", AgentState::Stopped, false),
            ("working", AgentState::Working, true),
            (
                "waiting",
                AgentState::NeedsInput {
                    request: InputRequest::Question,
                },
                true,
            ),
        ]
        .into_iter()
        .map(|(id, state, alive)| {
            let mut card = card(
                &session(id, state, alive),
                &no_sources(None),
                &mut Memory::default(),
            );
            card.name = Some(id.to_owned());
            card
        })
        .collect();
        sort(&mut cards);
        let order: Vec<&str> = cards.iter().map(|c| c.session_id.as_str()).collect();
        assert_eq!(order, ["waiting", "working", "idle", "ended"]);
    }
}
