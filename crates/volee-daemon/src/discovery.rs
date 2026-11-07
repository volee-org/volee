//! Discovery (ADR 0004): every session a harness lists becomes an agent, and
//! the state of every known agent follows its sessions.
//!
//! The daemon lists the harness outside the state lock (a listing runs a
//! program and can take a second), then reconciles the listing with the
//! registry and commits the difference. Hooks give agents launched by volee
//! a richer, faster picture; discovery is what makes every other session
//! visible, and what notices sessions that ended without a word.
//!
//! A listing costs CPU, so its cadence follows who watches: every few
//! seconds while an interactive client (the interface) is subscribed, rarely
//! otherwise, and at once when a new interactive client subscribes, so that
//! it opens on fresh data. Each wait starts when the previous listing ends,
//! so a slow listing on a loaded machine never runs back to back.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;
use volee_core::reconcile::{Listing, reconcile};
use volee_core::time::Timestamp;
use volee_core::{Event, EventKind};
use volee_harness::Harness;

use crate::server::Shared;
use crate::state::State;
use crate::watchers::Watching;

/// How often the harness is listed while an interactive client watches,
/// when the configuration says nothing.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(2);

/// How often the harness is listed while no interactive client watches,
/// when the configuration says nothing.
pub const DEFAULT_IDLE_INTERVAL: Duration = Duration::from_secs(30);

/// How long discovery waits between two listings.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cadence {
    /// While at least one interactive client is subscribed.
    pub(crate) watched: Duration,
    /// While none is.
    pub(crate) idle: Duration,
}

impl Cadence {
    fn wait(self, watching: Watching) -> Duration {
        if watching.open > 0 {
            self.watched
        } else {
            self.idle
        }
    }
}

/// Lists `harness` at once, then again after each wait the cadence gives,
/// until `stop` changes; a new interactive subscription cuts the wait short.
pub(crate) async fn run(
    shared: Arc<Shared>,
    harness: Arc<dyn Harness>,
    cadence: Cadence,
    mut stop: watch::Receiver<bool>,
) {
    let mut watching = shared.watchers.subscribe();
    // The interactive subscriptions opened before the last listing began:
    // that listing gave them fresh data.
    let mut served = watching.borrow_and_update().opened;
    // When the last listing ended; none yet, so the first runs at once.
    let mut last: Option<Instant> = None;
    let mut failing = false;
    loop {
        let wait = cadence.wait(*watching.borrow());
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
            }
            _ = stop.changed() => return,
        }
        served = watching.borrow_and_update().opened;
        list(&shared, &harness, &mut failing).await;
        last = Some(Instant::now());
    }
}

/// Lists the harness once and applies what changed. `failing` remembers
/// whether the previous listing failed, so that a failure is logged once.
async fn list(shared: &Arc<Shared>, harness: &Arc<dyn Harness>, failing: &mut bool) {
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
            return;
        }
        Err(error) => {
            tracing::error!(%error, "the listing task panicked");
            return;
        }
    };
    // volee's own one-off runs (thread summaries) are never agents.
    listing
        .sessions
        .retain(|session| !session.cwd.starts_with(&shared.paths.state_dir));
    let mut state = shared.state.lock().await;
    let now = state.now();
    let claimed = claim_expected(&mut state, &listing, now);
    if !claimed.is_empty()
        && let Err(error) = state.commit(claimed)
    {
        tracing::error!(%error, "cannot link sessions started by volee");
    }
    state.sessions_mut().record(&listing);
    let events = reconcile(state.registry(), &listing, now, || state.new_agent_id());
    if events.is_empty() {
        return;
    }
    let count = events.len();
    match state.commit(events) {
        Ok(_) => tracing::debug!(events = count, "discovery applied"),
        Err(error) => tracing::error!(%error, "cannot apply discovered sessions"),
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

    #[test]
    fn the_cadence_is_fast_only_while_someone_watches() {
        let cadence = Cadence {
            watched: Duration::from_secs(2),
            idle: Duration::from_secs(30),
        };
        let nobody = Watching { open: 0, opened: 3 };
        let someone = Watching { open: 1, opened: 4 };
        assert_eq!(cadence.wait(nobody), Duration::from_secs(30));
        assert_eq!(cadence.wait(someone), Duration::from_secs(2));
    }
}
