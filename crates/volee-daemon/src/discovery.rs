//! Discovery (ADR 0004): every session a harness lists becomes an agent, and
//! the state of every known agent follows its sessions.
//!
//! The daemon lists the harness on a timer, outside the state lock (a listing
//! runs a program and can take a second), then reconciles the listing with
//! the registry and commits the difference. Hooks give agents launched by
//! volee a richer, faster picture; discovery is what makes every other
//! session visible, and what notices sessions that ended without a word.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::MissedTickBehavior;
use volee_core::reconcile::reconcile;
use volee_harness::Harness;

use crate::server::Shared;

/// How often the harness is listed when the configuration says nothing.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(2);

/// Lists `harness` every `every` until `stop` changes, and applies what
/// changed.
pub(crate) async fn run(
    shared: Arc<Shared>,
    harness: Arc<dyn Harness>,
    every: Duration,
    mut stop: watch::Receiver<bool>,
) {
    let mut ticker = tokio::time::interval(every);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut failing = false;
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = stop.changed() => return,
        }
        let lister = Arc::clone(&harness);
        let listing = match tokio::task::spawn_blocking(move || lister.list()).await {
            Ok(Ok(listing)) => {
                if failing {
                    tracing::info!(harness = ?harness.kind(), "listing sessions works again");
                    failing = false;
                }
                listing
            }
            Ok(Err(error)) => {
                // Say it once, not every two seconds.
                if !failing {
                    tracing::warn!(harness = ?harness.kind(), %error, "cannot list sessions");
                    failing = true;
                }
                continue;
            }
            Err(error) => {
                tracing::error!(%error, "the listing task panicked");
                continue;
            }
        };
        let mut state = shared.state.lock().await;
        let now = state.now();
        crate::control::apply_listing(&mut state, &listing);
        let events = reconcile(state.registry(), &listing, now, || state.new_agent_id());
        if events.is_empty() {
            continue;
        }
        let count = events.len();
        match state.commit(events) {
            Ok(_) => tracing::debug!(events = count, "discovery applied"),
            Err(error) => tracing::error!(%error, "cannot apply discovered sessions"),
        }
    }
}
