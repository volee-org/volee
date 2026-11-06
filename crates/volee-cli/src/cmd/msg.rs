//! `volee msg send`.

use anyhow::{Context, Result};
use volee_core::message::{MessageKind, Party};
use volee_core::{Event, Registry};
use volee_proto::methods::{EventsSince, SendMessage, SendParams, SinceParams};

use crate::connect::connect_or_start;
use crate::resolve;

/// Sends a message from the human to an agent, named as a person types it.
pub(crate) async fn send(to: &str, body: String, kind: MessageKind) -> Result<()> {
    let client = connect_or_start().await?;
    let batch = client.call::<EventsSince>(SinceParams::default()).await?;
    let events: Vec<Event> = batch.events.into_iter().map(|e| e.event).collect();
    let registry = Registry::from_events(&events)
        .context("the daemon sent events that do not fit together")?;
    let to = resolve::agent(&registry, to)?;
    let name = registry
        .agent(to)
        .map_or_else(|| to.short(), |agent| agent.name.clone());
    let sent = client
        .call::<SendMessage>(SendParams {
            from: Party::Human,
            to: vec![Party::Agent(to)],
            kind,
            body,
            refs: vec![],
            in_reply_to: None,
            thread: None,
        })
        .await?;
    println!(
        "Sent {} to {name}. It reaches the agent at the end of its turn, or wakes it if it is idle.",
        sent.message.short()
    );
    Ok(())
}
