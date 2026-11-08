//! The detail panel (facts about the selected agent) and its preview (the
//! agent's terminal screen).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::Span;
use volee_core::Agent;
use volee_core::agent::Hold;
use volee_core::time::Elapsed;

use super::{panel, put_spans, state_glyph, truncated};
use crate::app::{Model, Pane};
use crate::preview::Fit;
use crate::text;
use crate::theme::StyleRole;

/// The width of the labels column.
const LABEL: u16 = 9;

/// Draws the facts about the selected agent.
pub(crate) fn render(model: &Model, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let focused = model.focus == Pane::Detail && model.overlay.is_none();
    let inner = panel(buf, area, vec![Span::raw("Detail")], focused, theme);
    if inner.is_empty() {
        return;
    }
    let Some(agent) = model.selected_agent() else {
        put_spans(
            buf,
            inner.x + 1,
            inner.y,
            &[Span::styled(
                "Select an agent",
                theme.style(StyleRole::Placeholder),
            )],
            inner.width.saturating_sub(1),
        );
        return;
    };
    let width = inner.width.saturating_sub(2);
    let dim = theme.style(StyleRole::TextDim);
    let mut lines: Vec<Vec<Span<'_>>> = vec![
        vec![
            Span::styled(
                agent.name.as_str(),
                theme.role(agent.role).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" · {}", agent.role), dim),
        ],
        vec![
            Span::styled(
                format!("{} {}", state_glyph(&agent.state), agent.state.label()),
                theme.state(&agent.state),
            ),
            Span::styled(
                format!(" · for {}", Elapsed::between(agent.state_since, model.now)),
                dim,
            ),
        ],
    ];
    if let Some(detail) = &agent.detail {
        lines.push(vec![Span::styled(
            detail.as_str(),
            theme.state(&agent.state).remove_modifier(Modifier::BOLD),
        )]);
    }
    for (label, value) in facts(model, agent) {
        let value = if matches!(label, "worktree" | "folder") {
            text::truncate_start(&value, usize::from(width.saturating_sub(LABEL))).into_owned()
        } else {
            value
        };
        lines.push(vec![
            Span::styled(format!("{label:<width$}", width = usize::from(LABEL)), dim),
            Span::styled(value, theme.style(StyleRole::Text)),
        ]);
    }
    for (row, line) in (0u16..).zip(&lines) {
        if row >= inner.height {
            break;
        }
        put_spans(
            buf,
            inner.x + 1,
            inner.y + row,
            &truncated(line, width),
            width,
        );
    }
}

/// The labelled facts, the most useful first.
fn facts(model: &Model, agent: &Agent) -> Vec<(&'static str, String)> {
    let mut facts = Vec::new();
    if let Some(mission) = agent.mission.and_then(|id| model.registry.mission(id)) {
        facts.push((
            "mission",
            format!("{} ({})", mission.title, mission.kind.label()),
        ));
        // The supervisor's last verdict on this worker, when there is one.
        if let Some(assessment) = mission.assessments.get(&agent.id) {
            facts.push(("verdict", assessment.verdict.label().to_owned()));
        }
    }
    let workspace = &agent.workspace;
    if workspace.repo.is_some() {
        let place = match (&workspace.base, &workspace.branch) {
            (Some(base), Some(_)) => format!("{} from {base}", workspace.label()),
            _ => workspace.label(),
        };
        facts.push(("repo", place));
        let path = workspace.worktree.as_ref().unwrap_or(&workspace.cwd);
        facts.push(("worktree", path.display().to_string()));
    } else {
        facts.push(("folder", workspace.cwd.display().to_string()));
    }
    let location = agent.location.label();
    let runs = match &agent.model {
        Some(model) => format!("{location} · {model}"),
        None => location,
    };
    facts.push(("runs in", runs));
    // volee will not resume a held agent on its own.
    if let Some(hold) = agent.hold {
        let (Hold::StoppedByHuman { at } | Hold::MissionOver { at }) = hold;
        facts.push((
            "held",
            format!(
                "{} · {} ago",
                hold.describe(),
                Elapsed::between(at, model.now)
            ),
        ));
    }
    let delivery = &agent.delivery;
    let nothing = delivery.commits == 0
        && delivery.pull_request.is_none()
        && !delivery.dirty
        && !delivery.report_delivered
        && !delivery.merged_into_base;
    facts.push((
        "delivery",
        if nothing {
            "nothing yet".to_owned()
        } else {
            delivery.summary()
        },
    ));
    if let Some(activity) = &agent.last_activity {
        facts.push((
            "last",
            format!(
                "{} · {} ago",
                activity.summary,
                Elapsed::between(activity.at, model.now)
            ),
        ));
    }
    if let Some(session) = agent.current_session() {
        facts.push((
            "session",
            format!("{} · {}", session.key.short(), session.key.harness),
        ));
    }
    facts
}

/// Draws the selected agent's screen.
pub(crate) fn render_preview(model: &Model, area: Rect, buf: &mut Buffer) {
    let focused = model.focus == Pane::Detail && model.overlay.is_none();
    draw_screen(model, area, buf, "Preview", focused, Fit::Crop);
}

/// Draws the selected agent's screen over the whole interface but its hint
/// line (`z`).
pub(crate) fn render_full_screen(model: &Model, area: Rect, buf: &mut Buffer) {
    let window = Rect {
        height: area.height.saturating_sub(1),
        ..area
    };
    if window.width < 10 || window.height < 3 {
        return;
    }
    ratatui::widgets::Widget::render(ratatui::widgets::Clear, window, buf);
    buf.set_style(window, model.theme.style(StyleRole::Background));
    draw_screen(model, window, buf, "Screen", true, Fit::Wrap);
}

/// The panel of an agent's screen, titled `name`, its rows fitted as `fit`
/// says.
fn draw_screen(
    model: &Model,
    area: Rect,
    buf: &mut Buffer,
    name: &'static str,
    focused: bool,
    fit: Fit,
) {
    let theme = &model.theme;
    let mut title = vec![Span::raw(name)];
    if let Some(agent) = model.preview.agent.and_then(|id| model.registry.agent(id)) {
        title.push(Span::styled(
            format!(" · {}", agent.name),
            theme.style(StyleRole::TextDim),
        ));
    }
    if model.preview.loading && model.preview.screen.is_none() {
        title.push(Span::styled(" · loading", theme.style(StyleRole::TextDim)));
    }
    let inner = panel(buf, area, title, focused, theme);
    if inner.is_empty() {
        return;
    }
    let message = match (&model.preview.screen, &model.preview.error) {
        (Some(screen), _) => {
            screen.render(
                Rect::new(
                    inner.x + 1,
                    inner.y,
                    inner.width.saturating_sub(1),
                    inner.height,
                ),
                buf,
                model.preview.scroll,
                fit,
                theme,
            );
            return;
        }
        (None, Some(error)) => format!("No preview: {error}"),
        (None, None) if model.preview.loading => "Loading the agent's screen…".to_owned(),
        (None, None) => "No preview".to_owned(),
    };
    put_spans(
        buf,
        inner.x + 1,
        inner.y,
        &[Span::styled(message, theme.style(StyleRole::Placeholder))],
        inner.width.saturating_sub(1),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::INVOICE_PAGE;
    use crate::test_support::{id, model, start};

    #[test]
    fn a_held_agent_says_why_volee_leaves_it_alone() {
        let model = model(120, 40);
        let mut agent = model.registry.agent(id(INVOICE_PAGE)).unwrap().clone();
        assert!(
            !facts(&model, &agent)
                .iter()
                .any(|(name, _)| *name == "held")
        );
        agent.hold = Some(Hold::StoppedByHuman {
            at: start().plus_millis(-120_000),
        });
        let held = facts(&model, &agent)
            .into_iter()
            .find(|(name, _)| *name == "held")
            .map(|(_, value)| value);
        assert_eq!(held.as_deref(), Some("stopped by the human · 2 min ago"));
    }
}
