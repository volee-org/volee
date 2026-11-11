//! The detail panel (facts about the selected agent) and its preview (the
//! agent's terminal screen).

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Modifier;
use ratatui::text::Span;
use volee_core::Agent;
use volee_core::agent::Hold;
use volee_core::time::Elapsed;

use super::{panel, panel_with, put_spans, state_glyph, truncated};
use crate::app::{Model, Pane};
use crate::keymap::{Action, Context};
use crate::live::{Live, Phase};
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
    lines.extend(fact_rows(model, agent, width));
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

/// The facts about an agent as rows of a label and a value, for a panel
/// `width` cells wide: a long path keeps its end.
pub(crate) fn fact_rows(model: &Model, agent: &Agent, width: u16) -> Vec<Vec<Span<'static>>> {
    let theme = &model.theme;
    let dim = theme.style(StyleRole::TextDim);
    facts(model, agent)
        .into_iter()
        .map(|(label, value)| {
            let value = if matches!(label, "worktree" | "folder") {
                text::truncate_start(&value, usize::from(width.saturating_sub(LABEL))).into_owned()
            } else {
                value
            };
            vec![
                Span::styled(format!("{label:<width$}", width = usize::from(LABEL)), dim),
                Span::styled(value, theme.style(StyleRole::Text)),
            ]
        })
        .collect()
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
    if model.layout.mode == crate::layout::Mode::Wide {
        action_bar(model, area, buf);
    }
}

/// On the preview's top border, after its title: the selected agent's state
/// as a badge with how long it has lasted and what it does, and at the
/// right the keys of what can be done with it, as keys.
fn action_bar(model: &Model, area: Rect, buf: &mut Buffer) {
    let Some(agent) = model.selected_agent() else {
        return;
    };
    let theme = &model.theme;
    let context = super::card::CardContext::new(model);
    let dim = theme.style(StyleRole::TextDim);
    let cap = theme.style(StyleRole::HintKey);
    let mut actions: Vec<Span<'static>> = Vec::new();
    for (action, word) in [
        (Action::TakeOver, "open"),
        (Action::Message, "message"),
        (Action::Diff, "diff"),
        (Action::FullScreen, "full screen"),
    ] {
        let Some(key) = model.keymap.first_key(Context::Global, action) else {
            continue;
        };
        actions.push(Span::styled(format!(" {} ", key.short()), cap));
        actions.push(Span::styled(format!(" {word}  "), dim));
    }
    let width = |spans: &[Span<'_>]| -> u16 {
        spans
            .iter()
            .map(|span| u16::try_from(text::width(&span.content)).unwrap_or(0))
            .sum()
    };
    let actions_width = width(&actions);
    // The title the panel drew ends at its first border cell after it.
    let title_end = (area.x + 1..area.right())
        .rev()
        .find(|x| buf[(*x, area.y)].symbol() != "─" && buf[(*x, area.y)].symbol() != "╮")
        .map_or(area.x + 2, |x| x + 2);
    let right = area.right().saturating_sub(2);
    let actions_x = right.saturating_sub(actions_width);
    let mut state = super::card::badge_spans(agent, &context);
    if let Some(doing) = agent
        .last_activity
        .as_ref()
        .map(|activity| activity.summary.clone())
        .or_else(|| agent.detail.clone())
    {
        state.push(Span::styled(format!(" · {doing} "), dim));
    }
    let room = actions_x.saturating_sub(title_end + 2);
    if room > 8 {
        put_spans(buf, title_end, area.y, &truncated(&state, room), room);
    }
    if actions_x > title_end + 2 {
        put_spans(buf, actions_x, area.y, &actions, actions_width);
    }
}

/// Draws the selected agent's screen over the whole interface but its hint
/// line (`z`): live for an agent in a volee host, else as the source last
/// read it. Returns where the cursor goes in edit mode.
pub(crate) fn render_full_screen(model: &Model, area: Rect, buf: &mut Buffer) -> Option<Position> {
    let window = Rect {
        height: area.height.saturating_sub(1),
        ..area
    };
    if window.width < 10 || window.height < 3 {
        return None;
    }
    ratatui::widgets::Widget::render(ratatui::widgets::Clear, window, buf);
    buf.set_style(window, model.theme.style(StyleRole::Background));
    if let Some(live) = model
        .live
        .as_ref()
        .filter(|live| live.has_screen() && model.preview.agent == Some(live.agent))
    {
        return draw_live(model, live, window, buf);
    }
    draw_screen(model, window, buf, "Screen", true, Fit::Wrap);
    None
}

/// The live screen of an agent in a volee host: cell for cell when it fits,
/// as in edit mode, where the agent has the panel's size; otherwise wrapped
/// like the screen the source reads.
fn draw_live(model: &Model, live: &Live, area: Rect, buf: &mut Buffer) -> Option<Position> {
    let theme = &model.theme;
    let dim = theme.style(StyleRole::TextDim);
    let mut title = Vec::new();
    if live.editing {
        title.push(Span::styled(
            " EDIT ",
            theme.style(StyleRole::Notice).add_modifier(Modifier::BOLD),
        ));
        title.push(Span::raw(" "));
    }
    title.push(Span::raw("Screen"));
    title.push(Span::styled(
        format!(" · {}", model.name_of(live.agent)),
        dim,
    ));
    let state = match &live.phase {
        _ if live.exited => Some("ended"),
        Phase::Connecting => Some("connecting"),
        Phase::Closed(_) => Some("disconnected"),
        Phase::Live if live.editing => None,
        Phase::Live => Some("live"),
    };
    if let Some(state) = state {
        title.push(Span::styled(format!(" · {state}"), dim));
    }
    if live.scroll > 0 {
        title.push(Span::styled(format!(" · {} lines back", live.scroll), dim));
    }
    let inner = if live.editing {
        panel_with(buf, area, title, theme.style(StyleRole::Input))
    } else {
        panel(buf, area, title, true, theme)
    };
    let screen = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(1),
        inner.height,
    );
    if live.fits(screen) {
        let cursor = live.render_exact(screen, buf, theme);
        return cursor.filter(|_| live.editing);
    }
    if let Some(preview) = live.preview(theme) {
        preview.render(screen, buf, model.preview.scroll, Fit::Wrap, theme);
    }
    None
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
    // The screen of an agent in a volee host is followed live (ADR 0028);
    // any other is what the source read last.
    let live = model
        .live
        .as_ref()
        .filter(|live| {
            live.has_screen()
                && live.phase == Phase::Live
                && model.preview.agent == Some(live.agent)
        })
        .and_then(|live| live.preview(theme));
    if live.is_some() {
        title.push(Span::styled(" · live", theme.style(StyleRole::TextDim)));
    } else if model.preview.loading && model.preview.screen.is_none() {
        title.push(Span::styled(" · loading", theme.style(StyleRole::TextDim)));
    }
    let inner = panel(buf, area, title, focused, theme);
    if inner.is_empty() {
        return;
    }
    let message = match (
        live.as_deref().or(model.preview.screen.as_ref()),
        &model.preview.error,
    ) {
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
