//! `volee lead`: start a lead in this directory and talk to it (ADR 0033).
//!
//! A lead is the agent the person talks to. It launches agents and missions
//! for them, hears their reports, and never codes. The daemon starts it in a
//! volee host; from a terminal, this command opens it at once, and `Ctrl+Q`
//! leaves it running.

use std::io::IsTerminal;

use anyhow::{Context, Result};
use volee_proto::methods::{LeadParams, StartLead};

use crate::connect::connect_or_start;
use crate::{resolve, text};

/// Starts a lead in the current directory, then takes it over in this
/// terminal when there is one.
pub(crate) async fn run(
    task: Option<String>,
    name: Option<String>,
    model: Option<String>,
) -> Result<()> {
    resolve::as_the_person(
        "starts a lead: an agent talks to its own lead, or to the person, with send_message",
    )?;
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    let cwd = cwd
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", cwd.display()))?;
    let client = connect_or_start().await?;
    let started = client
        .call::<StartLead>(LeadParams {
            cwd: cwd.display().to_string(),
            task,
            name,
            model,
        })
        .await?;
    let name = text::one_line(&started.name).into_owned();
    drop(client);
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        // By its id: a name may be shared.
        return super::agent::attach(&started.agent.to_string()).await;
    }
    println!(
        "{name} ({}) is your lead here: `volee attach {name}` opens it, or enter in the \
         interface.",
        started.agent.short()
    );
    Ok(())
}
