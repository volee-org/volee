//! Plain-text tables for the terminal.

use volee_core::mission::{Mission, Verdict};
use volee_core::role::Role;
use volee_core::time::{Elapsed, Timestamp};
use volee_core::{Agent, AgentState, Registry};

/// A symbol per state, so states read without color.
pub(crate) fn glyph(state: &AgentState) -> &'static str {
    match state {
        AgentState::Launching => "\u{2026}",
        AgentState::Working => "\u{25b8}",
        AgentState::NeedsInput { .. } => "!",
        AgentState::Idle => "\u{b7}",
        AgentState::Done => "\u{2713}",
        AgentState::Failed => "\u{2717}",
        AgentState::Stopped => "\u{25a0}",
        AgentState::Lost => "\u{2205}",
    }
}

/// Renders agents as an aligned table, one line per agent.
pub(crate) fn agents(agents: &[&Agent], now: Timestamp) -> String {
    let header = [
        "",
        "STATE",
        "AGENT",
        "ID",
        "ROLE",
        "WORKSPACE",
        "SINCE",
        "DETAIL",
    ];
    let mut rows: Vec<[String; 8]> = vec![header.map(str::to_owned)];
    for agent in agents {
        let detail = agent
            .detail
            .clone()
            .or_else(|| agent.last_activity.as_ref().map(|a| a.summary.clone()))
            .unwrap_or_default();
        rows.push([
            glyph(&agent.state).to_owned(),
            agent.state.label().to_owned(),
            agent.name.clone(),
            agent.id.short(),
            agent.role.label().to_owned(),
            agent.workspace.label(),
            Elapsed::between(agent.state_since, now).to_string(),
            truncate(&detail, 60),
        ]);
    }
    align(&rows)
}

/// Renders missions as an aligned table, one line per mission.
pub(crate) fn missions(registry: &Registry, missions: &[&Mission], now: Timestamp) -> String {
    let header = [
        "STATUS", "MISSION", "ID", "KIND", "WORKERS", "SINCE", "REPOS",
    ];
    let mut rows: Vec<[String; 7]> = vec![header.map(str::to_owned)];
    for mission in missions {
        let workers: Vec<_> = registry
            .mission_agents(mission.id)
            .into_iter()
            .filter(|agent| agent.role == Role::Worker)
            .collect();
        let delivered = workers
            .iter()
            .filter(|worker| {
                mission
                    .assessments
                    .get(&worker.id)
                    .is_some_and(|assessment| assessment.verdict == Verdict::Delivered)
            })
            .count();
        rows.push([
            mission.status.label().to_owned(),
            truncate(&mission.title, 40),
            mission.id.short(),
            mission.kind.label().to_owned(),
            format!("{delivered}/{} delivered", workers.len()),
            Elapsed::between(mission.created_at, now).to_string(),
            truncate(
                &mission
                    .repos
                    .iter()
                    .map(|repo| repo_label(repo))
                    .collect::<Vec<_>>()
                    .join(", "),
                40,
            ),
        ]);
    }
    align(&rows)
}

fn align<const N: usize>(rows: &[[String; N]]) -> String {
    let mut widths = [0_usize; N];
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in rows {
        let mut line = String::new();
        for (index, (cell, width)) in row.iter().zip(widths).enumerate() {
            if index > 0 {
                line.push_str("  ");
            }
            line.push_str(cell);
            if index + 1 < N {
                line.extend(std::iter::repeat_n(' ', width - cell.chars().count()));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// A repository as tables show it: its name, or the last part of its path.
pub(crate) fn repo_label(repo: &str) -> &str {
    std::path::Path::new(repo)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(repo)
}

pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let cut: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}\u{2026}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_are_aligned_and_trailing_spaces_trimmed() {
        let rows = vec![
            ["a".to_owned(), "long cell".to_owned()],
            ["bbbb".to_owned(), "x".to_owned()],
        ];
        assert_eq!(align(&rows), "a     long cell\nbbbb  x\n");
    }

    #[test]
    fn repositories_given_as_paths_show_their_last_part() {
        assert_eq!(repo_label("billing"), "billing");
        assert_eq!(repo_label("/work/code/billing"), "billing");
    }

    #[test]
    fn long_details_are_cut() {
        assert_eq!(truncate("abcdef", 4), "abc\u{2026}");
        assert_eq!(truncate("abc", 4), "abc");
    }

    #[test]
    fn every_state_has_a_distinct_glyph() {
        use volee_core::state::InputRequest;
        let states = [
            AgentState::Launching,
            AgentState::Working,
            AgentState::NeedsInput {
                request: InputRequest::Question,
            },
            AgentState::Idle,
            AgentState::Done,
            AgentState::Failed,
            AgentState::Stopped,
            AgentState::Lost,
        ];
        let mut glyphs: Vec<&str> = states.iter().map(glyph).collect();
        glyphs.sort_unstable();
        glyphs.dedup();
        assert_eq!(glyphs.len(), states.len());
    }
}
