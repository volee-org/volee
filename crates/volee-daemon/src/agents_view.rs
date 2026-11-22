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
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::watch;
use volee_core::finished::{Activity, BranchWork, HookKind, classify_hook, is_finished_work};
use volee_core::reconcile::ListedSession;
use volee_core::state::{AgentState, InputRequest, StateCategory};
use volee_core::time::Timestamp;
use volee_git::{BaseGuess, BaseSource, DeliveryQuery, Git};
use volee_harness::Harness;
use volee_proto::methods::{AgentCard, AgentsView, GitFacts, HookFacts, RepoFacts};
use volee_store::{HookFilter, Store};

/// How many of a session's newest hook events are read to find its last
/// activity and its last stop.
const HOOK_WINDOW: u32 = 500;

/// How the agents view is set up.
#[derive(Clone, Debug)]
pub struct AgentsViewConfig {
    /// How often the sessions are listed.
    pub poll_interval: Duration,
    /// The SQLite database the hook events are in.
    pub database: PathBuf,
}

/// The poller and its last reading.
pub(crate) struct Agents {
    harness: Arc<dyn Harness>,
    git: Option<Arc<dyn Git>>,
    store: Option<Arc<Mutex<Store>>>,
    inner: tokio::sync::Mutex<Inner>,
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
    /// For each branch, the last tip seen with commits of its own: the memory
    /// that proves a merge once that commit is part of the base branch.
    unmerged: HashMap<(PathBuf, String), String>,
}

impl Agents {
    pub(crate) fn new(
        harness: Arc<dyn Harness>,
        git: Option<Arc<dyn Git>>,
        database: &Path,
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
            git,
            store,
            inner: tokio::sync::Mutex::default(),
        }
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
                Self::fail(inner, format!("cannot list sessions: {error}"));
                return;
            }
            Err(error) => {
                Self::fail(inner, format!("the listing task failed: {error}"));
                return;
            }
        };
        if inner.failing {
            tracing::info!("listing Claude Code's sessions works again");
        }
        inner.failing = false;
        let git = self.git.clone();
        let store = self.store.clone();
        let memory = std::mem::take(&mut inner.unmerged);
        let built = tokio::task::spawn_blocking(move || {
            let mut memory = memory;
            let cards: Vec<AgentCard> = listing
                .sessions
                .iter()
                .map(|session| card(session, git.as_deref(), store.as_deref(), &mut memory))
                .collect();
            (cards, memory)
        })
        .await;
        match built {
            Ok((mut cards, memory)) => {
                sort(&mut cards);
                inner.unmerged = memory;
                inner.view = AgentsView {
                    updated_at: Some(now()),
                    error: None,
                    agents: cards,
                };
                inner.read_at = Some(Instant::now());
            }
            Err(error) => Self::fail(inner, format!("reading the agents failed: {error}")),
        }
    }

    /// Records a failed reading: the previous cards stay, with the reason.
    fn fail(inner: &mut Inner, why: String) {
        if inner.failing {
            tracing::debug!(%why, "listing Claude Code's sessions still fails");
        } else {
            tracing::warn!(%why, "listing Claude Code's sessions fails");
        }
        inner.failing = true;
        inner.view.error = Some(why);
        // Try again at the next request or tick, not at every request.
        inner.read_at = Some(Instant::now());
    }
}

/// The agents view, when it is configured and Claude Code is available.
pub(crate) fn build(
    config: Option<&AgentsViewConfig>,
    harness: Option<Arc<dyn Harness>>,
    git: Option<Arc<dyn Git>>,
) -> Option<Arc<Agents>> {
    let (config, harness) = config.zip(harness)?;
    Some(Arc::new(Agents::new(harness, git, &config.database)))
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
            _ = stop.changed() => return,
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
fn card(
    session: &ListedSession,
    git: Option<&dyn Git>,
    store: Option<&Mutex<Store>>,
    memory: &mut HashMap<(PathBuf, String), String>,
) -> AgentCard {
    let session_id = session.key.id.clone();
    let hooks = store.map_or_else(HookFacts::default, |store| hook_facts(store, &session_id));
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
    if let Some(git) = git
        && session.cwd.is_dir()
    {
        match git_facts(git, &session.cwd, memory) {
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
        card.finished_work = is_finished_work(&card.hooks.activity, &card.state, &work);
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

/// Where `cwd` sits and what git says of its branch. `Ok(None)` outside a
/// repository; the error is already a sentence.
fn git_facts(
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
    let (Some(branch), Some(base)) = (&info.branch, &base) else {
        return Ok(Some((repo, None)));
    };
    let key = (info.main_checkout.clone(), branch.clone());
    let query = DeliveryQuery {
        branch,
        base,
        unmerged_tip: memory.get(&key).map(String::as_str),
    };
    let report = git
        .delivery(&info.top_level, &query)
        .map_err(|error| error.to_string())?;
    let delivery = &report.delivery;
    match &delivery.unmerged_tip {
        Some(tip) => memory.insert(key, tip.clone()),
        None => memory.remove(&key),
    };
    Ok(Some((
        repo,
        Some(GitFacts {
            ahead: delivery.commits,
            unique: report.unique_commits,
            behind: report.behind,
            dirty: delivery.dirty,
            merged: delivery.merged_into_base,
            diff: delivery.diff,
            push: delivery.push,
        }),
    )))
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
        let mut memory = HashMap::new();
        let git = volee_git::GitCli::new();
        let card = card(
            &session("s1", AgentState::Idle, true),
            Some(&git),
            None,
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
            let mut card = card(&session(id, state, alive), None, None, &mut HashMap::new());
            card.name = Some(id.to_owned());
            card
        })
        .collect();
        sort(&mut cards);
        let order: Vec<&str> = cards.iter().map(|c| c.session_id.as_str()).collect();
        assert_eq!(order, ["waiting", "working", "idle", "ended"]);
    }
}
