//! Plain-text tables for the terminal.

use volee_core::time::{Elapsed, Timestamp};
use volee_core::{Agent, AgentState};

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
    let header = ["", "STATE", "AGENT", "ROLE", "WORKSPACE", "SINCE", "DETAIL"];
    let mut rows: Vec<[String; 7]> = vec![header.map(str::to_owned)];
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
            agent.role.label().to_owned(),
            agent.workspace.label(),
            Elapsed::between(agent.state_since, now).to_string(),
            truncate(&detail, 60),
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

fn truncate(text: &str, max: usize) -> String {
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
