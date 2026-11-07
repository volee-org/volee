//! `volee summary`: a model's summary of a mission's thread, or of your
//! thread with an agent.

use anyhow::{Context, Result, bail};
use volee_core::message::Party;
use volee_core::{Event, Registry};
use volee_proto::methods::{EventsSince, SinceParams, Summarize, SummarizeParams};

use crate::connect::connect_or_start;
use crate::{resolve, text};

/// Prints the summary of the thread `target` names: a mission, else an agent.
pub(crate) async fn run(target: &str) -> Result<()> {
    let client = connect_or_start().await?;
    let batch = client.call::<EventsSince>(SinceParams::default()).await?;
    let events: Vec<Event> = batch.events.into_iter().map(|e| e.event).collect();
    let registry = Registry::from_events(&events)
        .context("the daemon sent events that do not fit together")?;
    let thread = if let Ok(mission) = resolve::mission(&registry, target) {
        registry
            .mission(mission)
            .map(|mission| mission.thread)
            .context("the mission disappeared while reading it")?
    } else {
        let agent = resolve::agent(&registry, target).with_context(|| {
            format!(
                "no mission or agent is named `{target}`; see `volee mission ls` and `volee ls`"
            )
        })?;
        match registry.thread_between(Party::Human, Party::Agent(agent)) {
            Some(thread) => thread.id,
            None => bail!("you have no thread with that agent yet"),
        }
    };
    let summary = client.call::<Summarize>(SummarizeParams { thread }).await?;
    println!("{}", text::printable(&summary.summary));
    if !summary.model.is_empty() {
        println!(
            "\n({} messages, summarized by {} with your Claude Code account)",
            summary.messages, summary.model
        );
    }
    Ok(())
}
