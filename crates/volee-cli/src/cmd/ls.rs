//! `volee ls`: every agent, the ones that need you first.

use anyhow::{Context, Result};
use volee_core::{Event, Registry};
use volee_daemon::{Clock, SystemClock};
use volee_proto::methods::{EventsSince, RefreshDiscovery, RefreshParams, SinceParams};

use crate::connect::connect_or_start;
use crate::table;

/// How long `volee ls` waits for a fresh listing of the sessions.
const REFRESH_TIMEOUT_MS: u64 = 3_000;

/// Prints the agents as a table, or as JSON with `--json`.
pub(crate) async fn run(json: bool) -> Result<()> {
    let client = connect_or_start().await?;
    // While no interface is open, the daemon lists sessions only every
    // thirty seconds: ask for a listing now, so a session started a moment
    // ago is in the table. A slow listing or an older daemon only means
    // the list is as fresh as the daemon's last listing.
    let _ = client
        .call::<RefreshDiscovery>(RefreshParams {
            timeout_ms: Some(REFRESH_TIMEOUT_MS),
        })
        .await;
    let batch = client.call::<EventsSince>(SinceParams::default()).await?;
    let events: Vec<Event> = batch.events.into_iter().map(|e| e.event).collect();
    let registry = Registry::from_events(&events)
        .context("the daemon sent events that do not fit together")?;
    let agents = registry.agents_by_urgency();
    if json {
        println!("{}", serde_json::to_string_pretty(&agents)?);
    } else if agents.is_empty() {
        println!("No agents yet. Agents you start with Claude Code appear here within seconds.");
    } else {
        print!("{}", table::agents(&agents, SystemClock.now()));
    }
    Ok(())
}
