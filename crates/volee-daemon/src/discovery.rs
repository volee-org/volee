//! Discovery (ADR 0004): every session a harness lists becomes an agent, and
//! the state of every known agent follows its sessions.
//!
//! The daemon lists the harness outside the state lock (a listing runs a
//! program and can take a second), then reconciles the listing with the
//! registry and commits the difference. Hooks give agents launched by volee
//! a richer, faster picture; discovery is what makes every other session
//! visible, and what notices sessions that ended without a word.
//!
//! A listing costs CPU (Claude Code starts a Node process for each), so its
//! cadence follows who watches and what changes. Nobody watching: rarely.
//! An interactive client (the interface) subscribed: every two seconds after
//! a listing that changed something, then twice as long after each listing
//! that changed nothing, up to ten seconds. A new interactive client, or a
//! one-off command asking with `discovery.refresh` (`volee ls`), gets a
//! listing at once and the short wait again. Hooks never shorten the wait:
//! agents volee launched report through them whatever discovery does, and
//! resetting on them would keep the short wait exactly while agents work.
//! Each wait starts when the previous listing ends, so a slow listing on a
//! loaded machine never runs back to back.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;
use volee_core::reconcile::{Listing, reconcile};
use volee_core::time::Timestamp;
use volee_core::{Event, EventKind};
use volee_harness::Harness;
use volee_proto::methods::{RefreshParams, Refreshed};

use crate::error::DaemonError;
use crate::server::Shared;
use crate::state::State;
use crate::watchers::Watching;

/// How long a refresh waits for its listing when the request says nothing.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(3);

/// The longest a refresh may wait for its listing.
const MAX_REFRESH_TIMEOUT: Duration = Duration::from_secs(10);

/// Where discovery stands, for requests that wait for a listing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Progress {
    /// Listings started since the daemon started.
    started: u64,
    /// The number of the last listing that finished.
    finished: u64,
    /// Whether that listing worked.
    succeeded: bool,
}

/// Requests for a listing now, and how listings progress.
#[derive(Debug)]
pub(crate) struct Refreshes {
    /// Requests so far; discovery lists when it changes.
    requests: watch::Sender<u64>,
    progress: watch::Sender<Progress>,
}

impl Refreshes {
    pub(crate) fn new() -> Self {
        Self {
            requests: watch::Sender::new(0),
            progress: watch::Sender::new(Progress::default()),
        }
    }
}

/// `discovery.refresh`: has discovery list now, and waits, at most the
/// timeout, for a listing that started after the request. Concurrent
/// requests share one listing.
pub(crate) async fn refresh(
    shared: &Arc<Shared>,
    params: RefreshParams,
) -> Result<Refreshed, DaemonError> {
    if shared.harness.is_none() {
        // No harness, no discovery: nothing to wait for.
        return Ok(Refreshed { listed: false });
    }
    let timeout = params
        .timeout_ms
        .map_or(REFRESH_TIMEOUT, Duration::from_millis)
        .min(MAX_REFRESH_TIMEOUT);
    let mut progress = shared.refreshes.progress.subscribe();
    // A listing that starts from now on.
    let wanted = progress.borrow_and_update().started + 1;
    shared.refreshes.requests.send_modify(|count| *count += 1);
    let done = tokio::time::timeout(
        timeout,
        progress.wait_for(|progress| progress.finished >= wanted),
    )
    .await;
    let listed = matches!(done, Ok(Ok(progress)) if progress.succeeded);
    Ok(Refreshed { listed })
}

/// The shortest wait between two listings, while an interactive client
/// watches and sessions change, when the configuration says nothing.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(2);

/// The longest wait between two listings while an interactive client
/// watches and nothing changes, when the configuration says nothing.
pub const DEFAULT_MAX_INTERVAL: Duration = Duration::from_secs(10);

/// How often the harness is listed while no interactive client watches,
/// when the configuration says nothing.
pub const DEFAULT_IDLE_INTERVAL: Duration = Duration::from_secs(30);

/// How long discovery waits between two listings.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cadence {
    /// While at least one interactive client is subscribed: the wait after
    /// a listing that changed something.
    pub(crate) watched: Duration,
    /// While at least one interactive client is subscribed: the longest
    /// wait, reached while nothing changes.
    pub(crate) watched_max: Duration,
    /// While none is.
    pub(crate) idle: Duration,
}

impl Cadence {
    /// The wait before the next listing: the backoff's while someone
    /// watches, the idle one otherwise.
    fn wait(self, watching: Watching, backoff: Backoff) -> Duration {
        if watching.open > 0 {
            backoff.current
        } else {
            self.idle
        }
    }
}

/// The wait between two listings while someone watches: the shortest after
/// a listing that changed something or answered someone, twice as long
/// after each listing that changed nothing, up to the longest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Backoff {
    shortest: Duration,
    longest: Duration,
    current: Duration,
}

impl Backoff {
    fn new(shortest: Duration, longest: Duration) -> Self {
        Self {
            shortest,
            longest: longest.max(shortest),
            current: shortest,
        }
    }

    /// The wait after a listing: the shortest when it changed something or
    /// answered someone (`reset`), otherwise twice the last one, capped.
    fn after(&mut self, reset: bool) -> Duration {
        self.current = if reset {
            self.shortest
        } else {
            self.current.saturating_mul(2).min(self.longest)
        };
        self.current
    }
}

/// What a listing did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Listed {
    /// The harness answered and its listing was applied.
    succeeded: bool,
    /// It linked a session or changed the registry.
    changed: bool,
    /// Its live interactive sessions, which may run in a multiplexer pane.
    hosted: Vec<volee_mux::Hosted>,
}

/// Lists `harness` at once, then again after each wait the cadence gives,
/// until `stop` changes; a new interactive subscription, or a refresh
/// request, cuts the wait short.
pub(crate) async fn run(
    shared: Arc<Shared>,
    harness: Arc<dyn Harness>,
    cadence: Cadence,
    mut stop: watch::Receiver<bool>,
) {
    let mut watching = shared.watchers.subscribe();
    let mut requests = shared.refreshes.requests.subscribe();
    requests.borrow_and_update();
    // The interactive subscriptions opened before the last listing began:
    // that listing gave them fresh data.
    let mut served = watching.borrow_and_update().opened;
    // When the last listing ended; none yet, so the first runs at once.
    let mut last: Option<Instant> = None;
    let mut backoff = Backoff::new(cadence.watched, cadence.watched_max);
    // Whether the next listing answers someone (the daemon starting, a new
    // watcher, a refresh request), which brings the short wait back.
    let mut asked = true;
    let mut failing = false;
    loop {
        let wait = cadence.wait(*watching.borrow(), backoff);
        let next = last.map_or_else(Instant::now, |last| last + wait);
        tokio::select! {
            () = tokio::time::sleep_until(next) => {}
            changed = watching.changed() => {
                if changed.is_err() {
                    return;
                }
                // Someone left: the next wait may be longer, still counted
                // from the last listing. Someone new watches: list now.
                if watching.borrow_and_update().opened == served {
                    continue;
                }
                asked = true;
            }
            changed = requests.changed() => {
                if changed.is_err() {
                    return;
                }
                asked = true;
            }
            _ = stop.changed() => return,
        }
        served = watching.borrow_and_update().opened;
        // This listing answers every request made until now.
        requests.borrow_and_update();
        shared
            .refreshes
            .progress
            .send_modify(|progress| progress.started += 1);
        let listed = list(&shared, &harness, &mut failing).await;
        if listed.succeeded {
            crate::panes::locate(&shared, &listed.hosted).await;
        }
        shared.refreshes.progress.send_modify(|progress| {
            progress.finished = progress.started;
            progress.succeeded = listed.succeeded;
        });
        let next_wait = backoff.after(asked || listed.changed);
        asked = false;
        tracing::debug!(
            changed = listed.changed,
            next_if_watched_ms = next_wait.as_millis(),
            "listed sessions"
        );
        last = Some(Instant::now());
    }
}

/// Lists the harness once and applies what changed. `failing` remembers
/// whether the previous listing failed, so that a failure is logged once.
async fn list(shared: &Arc<Shared>, harness: &Arc<dyn Harness>, failing: &mut bool) -> Listed {
    let lister = Arc::clone(harness);
    let mut listing = match tokio::task::spawn_blocking(move || lister.list()).await {
        Ok(Ok(listing)) => {
            if *failing {
                tracing::info!(harness = ?harness.kind(), "listing sessions works again");
                *failing = false;
            }
            listing
        }
        Ok(Err(error)) => {
            // Say it once, not at every listing.
            if !*failing {
                tracing::warn!(harness = ?harness.kind(), %error, "cannot list sessions");
                *failing = true;
            }
            return Listed::default();
        }
        Err(error) => {
            tracing::error!(%error, "the listing task panicked");
            return Listed::default();
        }
    };
    // volee's own one-off runs (thread summaries) are never agents.
    listing
        .sessions
        .retain(|session| !session.cwd.starts_with(&shared.paths.state_dir));
    let hosted = crate::panes::hosted(&listing.sessions);
    let mut state = shared.state.lock().await;
    let now = state.now();
    let claimed = claim_expected(&mut state, &listing, now);
    let mut linked = false;
    if !claimed.is_empty() {
        match state.commit(claimed) {
            Ok(_) => linked = true,
            Err(error) => tracing::error!(%error, "cannot link sessions started by volee"),
        }
    }
    state.sessions_mut().record(&listing);
    let events = reconcile(state.registry(), &listing, now, || state.new_agent_id());
    if events.is_empty() {
        return Listed {
            succeeded: true,
            changed: linked,
            hosted,
        };
    }
    let count = events.len();
    match state.commit(events) {
        Ok(_) => {
            tracing::debug!(events = count, "discovery applied");
            Listed {
                succeeded: true,
                changed: true,
                hosted,
            }
        }
        Err(error) => {
            tracing::error!(%error, "cannot apply discovered sessions");
            Listed {
                succeeded: false,
                changed: linked,
                hosted: Vec::new(),
            }
        }
    }
}

/// Links the sessions the daemon started or resumed, recognized by the
/// harness's handle, to their agent. A resumed session is often a fork with a
/// new id: without this, a listing that sees it before its first hook would
/// take it for a stranger.
fn claim_expected(state: &mut State, listing: &Listing, now: Timestamp) -> Vec<Event> {
    let mut events = Vec::new();
    for session in &listing.sessions {
        let Some(handle) = session.handle.as_deref() else {
            continue;
        };
        let owned = state.registry().agent_for_session(&session.key).is_some();
        let Some((agent, origin)) = state.take_expected(handle) else {
            continue;
        };
        if owned || state.registry().agent(agent).is_none() {
            continue;
        }
        events.push(Event::new(
            now,
            EventKind::SessionLinked {
                agent,
                session: session.key.clone(),
                origin,
            },
        ));
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn the_cadence_follows_the_backoff_only_while_someone_watches() {
        let cadence = Cadence {
            watched: secs(2),
            watched_max: secs(10),
            idle: secs(30),
        };
        let mut backoff = Backoff::new(cadence.watched, cadence.watched_max);
        let nobody = Watching { open: 0, opened: 3 };
        let someone = Watching { open: 1, opened: 4 };
        assert_eq!(cadence.wait(nobody, backoff), secs(30));
        assert_eq!(cadence.wait(someone, backoff), secs(2));
        backoff.after(false);
        assert_eq!(cadence.wait(someone, backoff), secs(4));
        assert_eq!(cadence.wait(nobody, backoff), secs(30), "idle stays idle");
    }

    #[test]
    fn the_wait_doubles_while_nothing_changes_up_to_the_longest() {
        let mut backoff = Backoff::new(secs(2), secs(10));
        let waits: Vec<Duration> = (0..5).map(|_| backoff.after(false)).collect();
        assert_eq!(waits, [secs(4), secs(8), secs(10), secs(10), secs(10)]);
    }

    #[test]
    fn a_change_or_a_request_brings_the_shortest_wait_back() {
        let mut backoff = Backoff::new(secs(2), secs(10));
        backoff.after(false);
        backoff.after(false);
        assert_eq!(backoff.after(true), secs(2));
        assert_eq!(backoff.after(false), secs(4));
    }

    #[test]
    fn a_longest_wait_below_the_shortest_means_no_backoff() {
        let mut backoff = Backoff::new(secs(2), secs(1));
        assert_eq!(backoff.after(false), secs(2));
    }
}
