//! `volee mission new | ls | show | abort | close`.

use std::fmt::Write as _;

use anyhow::{Context, Result, bail};
use volee_core::message::{MessageKind, Party};
use volee_core::mission::{Mission, MissionKind};
use volee_core::role::Role;
use volee_core::time::{Elapsed, Timestamp};
use volee_core::{MissionId, Registry};
use volee_daemon::{Clock, SystemClock};
use volee_proto::PROTOCOL_VERSION;
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{
    AbortMission, AbortParams, CloseMission, CloseParams, CreateMission, CreateMissionParams,
    Status, StatusParams,
};
use volee_proto::rpc;

use crate::connect::{connect_or_start, paths};
use crate::{resolve, table, text};

/// What `volee mission new` asks for.
#[derive(Debug)]
pub(crate) struct NewMission {
    pub(crate) title: String,
    pub(crate) goal: String,
    pub(crate) kind: MissionKind,
    pub(crate) repos: Vec<String>,
}

/// Creates a mission.
pub(crate) async fn new(request: NewMission) -> Result<()> {
    resolve::as_the_person(
        "starts a mission from the command line (a lead calls start_mission, so that the \
         mission reports to it)",
    )?;
    let config = resolve::load_config()?;
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    let repos = request
        .repos
        .iter()
        .map(|repo| resolve::repository(&config, &cwd, repo))
        .collect::<Result<Vec<_>>>()?;
    let client = connect_or_start().await?;
    let created = client
        .call::<CreateMission>(CreateMissionParams {
            title: request.title.clone(),
            kind: request.kind,
            goal: request.goal,
            repos,
            requested_by: None,
        })
        .await?;
    let short = created.mission.short();
    println!(
        "Created mission {short} \"{}\" ({}).",
        request.title.trim(),
        request.kind.label()
    );
    match created.coordinator {
        Some(coordinator) => println!(
            "Its coordinator ({}) is planning; it launches the workers. Follow it with `volee \
             mission show {short}`.",
            coordinator.short()
        ),
        None => bail!(
            "{}\nTo abort it: `volee mission abort {short}`.",
            no_coordinator(&client, created.mission).await
        ),
    }
    Ok(())
}

/// Why no coordinator started for `mission`: the alert the daemon posted in
/// the mission's thread when its launch failed, else what the daemon says
/// about Claude Code, without which it launches nothing.
async fn no_coordinator(client: &Client, mission: MissionId) -> String {
    const FAILED: &str = "volee could not launch the coordinator of this mission: ";
    let alert = resolve::registry(client).await.ok().and_then(|registry| {
        let thread = registry.mission(mission)?.thread;
        registry
            .thread_messages(thread)
            .into_iter()
            .rev()
            .find(|message| message.from == Party::System && message.kind == MessageKind::Alert)
            .map(|message| text::printable(&message.body).into_owned())
    });
    if let Some(alert) = alert {
        if let Some(reason) = alert.strip_prefix(FAILED) {
            return format!("no coordinator could start: {reason}");
        }
        return alert;
    }
    let status = client
        .call::<Status>(StatusParams {
            protocol: PROTOCOL_VERSION,
        })
        .await;
    if let Some(harness) = status.ok().and_then(|status| {
        status
            .harnesses
            .into_iter()
            .find(|harness| !harness.available)
    }) {
        let detail = harness
            .detail
            .map(|detail| format!(" ({})", text::one_line(&detail)))
            .unwrap_or_default();
        return format!(
            "no coordinator could start: Claude Code is not available to the daemon{detail}; see \
             `volee daemon status`. The mission stays in planning."
        );
    }
    match paths() {
        Ok(paths) => format!(
            "no coordinator could start, and the daemon did not say why: see {}. The mission \
             stays in planning.",
            paths.log_dir().join("daemon.log").display()
        ),
        Err(_) => "no coordinator could start, and the daemon did not say why. The mission \
                   stays in planning."
            .to_owned(),
    }
}

/// Lists missions, open ones only unless `all`.
pub(crate) async fn list(all: bool, json: bool) -> Result<()> {
    let registry = resolve::registry(&connect_or_start().await?).await?;
    let mut missions: Vec<&Mission> = registry
        .missions()
        .filter(|mission| all || !mission.status.is_over())
        .collect();
    missions.sort_by_key(|mission| std::cmp::Reverse(mission.created_at));
    if json {
        println!("{}", serde_json::to_string_pretty(&missions)?);
    } else if missions.is_empty() {
        println!(
            "No {}missions. Start one with `volee mission new <title> <goal> --repo <repo>`.",
            if all { "" } else { "open " }
        );
    } else {
        print!(
            "{}",
            table::missions(&registry, &missions, SystemClock.now())
        );
    }
    Ok(())
}

/// Shows one mission: its agents, the supervisor's verdicts and its thread.
pub(crate) async fn show(input: &str) -> Result<()> {
    let registry = resolve::registry(&connect_or_start().await?).await?;
    let id = resolve::mission(&registry, input)?;
    let mission = registry
        .mission(id)
        .context("the mission disappeared while reading it")?;
    print!("{}", describe(&registry, mission, SystemClock.now()));
    Ok(())
}

/// Aborts a mission. Its agents are told to stop; nothing is deleted.
pub(crate) async fn abort(input: &str, reason: Option<String>) -> Result<()> {
    let client = connect_or_start().await?;
    let id = resolve::mission(&resolve::registry(&client).await?, input)?;
    client
        .call::<AbortMission>(AbortParams {
            mission: id,
            reason,
        })
        .await?;
    println!(
        "Aborted mission {}. Its agents were told to stop; their branches and worktrees are \
         kept.",
        id.short()
    );
    Ok(())
}

/// Closes a mission, with the report its coordinator presented. volee then
/// tidies its workers and stops its coordinator.
pub(crate) async fn close(input: &str, force: bool) -> Result<()> {
    resolve::as_the_person("closes a mission from the command line")?;
    let client = connect_or_start().await?;
    let registry = resolve::registry(&client).await?;
    let id = resolve::mission(&registry, input)?;
    let closed = client
        .call::<CloseMission>(CloseParams {
            mission: id,
            report: None,
            force,
            by: None,
        })
        .await;
    match closed {
        Ok(_) => {
            println!(
                "Closed mission {}. volee tidies its workers whose work landed and stops its \
                 coordinator; it tells you in your messages what it kept, and why.",
                id.short()
            );
            Ok(())
        }
        // Not every worker delivered: say how to close it anyway.
        Err(ClientError::Rpc(error))
            if error.code == rpc::REFUSED
                && registry
                    .mission(id)
                    .is_some_and(|m| !m.status.is_over() && !m.status.is_ready()) =>
        {
            bail!(
                "{} To close it anyway: `volee mission close {} --force`.",
                error.message,
                id.short()
            )
        }
        Err(error) => Err(error.into()),
    }
}

/// The text of `volee mission show`.
fn describe(registry: &Registry, mission: &Mission, now: Timestamp) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} ({})",
        text::one_line(&mission.title),
        mission.id.short()
    );
    let since = Elapsed::between(mission.created_at, now);
    let _ = writeln!(
        out,
        "  status       {} ({} mission, started {since} ago)",
        mission.status.label(),
        mission.kind.label()
    );
    let _ = writeln!(
        out,
        "  repos        {}",
        text::one_line(&mission.repos.join(", "))
    );
    let _ = writeln!(out, "  goal         {}", first_line(&mission.goal, 100));
    let agents = registry.mission_agents(mission.id);
    match agents.iter().find(|agent| agent.role == Role::Coordinator) {
        Some(coordinator) => {
            let _ = writeln!(
                out,
                "  coordinator  {} ({}), {}",
                text::one_line(&coordinator.name),
                coordinator.id.short(),
                coordinator.state.label()
            );
        }
        None => {
            let _ = writeln!(out, "  coordinator  none yet");
        }
    }
    let workers: Vec<_> = agents
        .iter()
        .filter(|agent| agent.role == Role::Worker)
        .collect();
    if workers.is_empty() {
        let _ = writeln!(out, "  workers      none yet");
    } else {
        let _ = writeln!(out, "  workers");
        for worker in workers {
            let verdict = mission.assessments.get(&worker.id).map_or_else(
                || worker.state.label().to_owned(),
                |assessment| format!("{}: {}", assessment.verdict.label(), assessment.evidence),
            );
            let _ = writeln!(
                out,
                "    {} {} ({})  {}",
                table::glyph(&worker.state),
                text::one_line(&worker.name),
                text::one_line(&worker.workspace.label()),
                first_line(&verdict, 90)
            );
        }
    }
    if let Some(report) = mission.report.and_then(|id| registry.message(id)) {
        let _ = writeln!(out, "  report       {}", first_line(&report.body, 100));
    }
    let messages = registry.thread_messages(mission.thread);
    if !messages.is_empty() {
        let _ = writeln!(out, "\nThread, latest last:");
        for message in messages.iter().rev().take(12).rev() {
            let to: Vec<String> = message
                .receipts
                .iter()
                .map(|receipt| party(registry, receipt.to))
                .collect();
            let _ = writeln!(
                out,
                "  {:>4}  {} \u{2192} {}: {}",
                Elapsed::between(message.at, now).to_string(),
                party(registry, message.from),
                to.join(", "),
                first_line(&message.body, 90)
            );
        }
    }
    out
}

fn party(registry: &Registry, party: Party) -> String {
    match party {
        Party::Human => "you".to_owned(),
        Party::System => "volee".to_owned(),
        Party::Agent(id) => registry
            .name_of(id)
            .map_or_else(|| id.short(), |name| text::one_line(name).into_owned()),
    }
}

/// The first line of `text`, printable, cut to `max` characters.
fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default();
    table::truncate(text::one_line(line).trim(), max)
}

#[cfg(test)]
mod tests {
    use super::first_line;

    #[test]
    fn a_first_line_is_printable_and_cut() {
        assert_eq!(
            first_line("\x1b]0;owned\x07Done.\nsecond line", 40),
            "]0;ownedDone."
        );
        assert_eq!(first_line("  report\r\nmore", 40), "report");
        assert_eq!(first_line("abcdef", 4), "abc\u{2026}");
    }
}
