//! `volee ls`: every agent, the ones that need you first.

use anyhow::{Context, Result};
use volee_core::{Event, Registry};
use volee_daemon::{Clock, SystemClock};
use volee_proto::methods::{EventsSince, SinceParams};

use crate::connect::connect_or_start;
use crate::table;

/// Prints the agents as a table, or as JSON with `--json`.
pub(crate) async fn run(json: bool) -> Result<()> {
    let client = connect_or_start().await?;
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
