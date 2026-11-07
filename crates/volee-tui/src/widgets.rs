//! Drawing pieces shared by the views. Widgets only read the model.

pub(crate) mod braille;
pub(crate) mod canvas;
pub(crate) mod card;
pub(crate) mod composer;
pub(crate) mod detail;
pub(crate) mod diff;
pub(crate) mod help;
pub(crate) mod hints;
pub(crate) mod inbox;
pub(crate) mod launch;
pub(crate) mod rail;
pub(crate) mod tools;
pub(crate) mod top_bar;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Widget};
use volee_core::{Agent, AgentState};

use crate::app::{Model, inner};
use crate::text;
use crate::theme::{StyleRole, Theme};

/// The glyph of a state. With the state's word, states read without color.
pub(crate) const fn state_glyph(state: &AgentState) -> &'static str {
    match state {
        AgentState::NeedsInput { .. } => "!",
        AgentState::Working => "▸",
        AgentState::Idle => "·",
        AgentState::Done => "✓",
        AgentState::Failed => "✗",
        AgentState::Stopped => "■",
        AgentState::Lost => "∅",
        AgentState::Launching => "…",
    }
}

/// Draws a panel's border with its title and returns the area inside.
pub(crate) fn panel(
    buf: &mut Buffer,
    area: Rect,
    title: Vec<Span<'_>>,
    focused: bool,
    theme: &Theme,
) -> Rect {
    let border = theme.style(if focused {
        StyleRole::BorderFocused
    } else {
        StyleRole::Border
    });
    let title_style = if focused {
        theme
            .style(StyleRole::BorderFocused)
            .add_modifier(Modifier::BOLD)
    } else {
        theme.style(StyleRole::Title)
    };
    let mut spans = vec![Span::raw(" ")];
    for span in title {
        let style = title_style.patch(span.style);
        spans.push(Span::styled(span.content, style));
    }
    spans.push(Span::raw(" "));
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border)
        .title(Line::from(spans))
        .render(area, buf);
    inner(area)
}

/// Where an agent works, in a few words: `repo › branch`, or its mission
/// when it works outside any repository, like a coordinator.
pub(crate) fn place_label(model: &Model, agent: &Agent) -> String {
    if agent.workspace.repo.is_none()
        && let Some(mission) = agent.mission.and_then(|id| model.registry.mission(id))
    {
        return format!("◆ {}", mission.title);
    }
    agent.workspace.label()
}

/// Spans cut to `max` cells; the last one kept ends with `…` when cut.
pub(crate) fn truncated<'a>(spans: &[Span<'a>], max: u16) -> Vec<Span<'a>> {
    let mut out = Vec::new();
    let mut left = usize::from(max);
    for span in spans {
        let width = text::width(&span.content);
        if width <= left {
            out.push(span.clone());
            left -= width;
        } else {
            out.push(Span::styled(
                text::truncate(&span.content, left).into_owned(),
                span.style,
            ));
            break;
        }
    }
    out
}

/// Writes a message that stands in for missing content, wrapped to the
/// width of `area`, from its first row, one cell in.
pub(crate) fn placeholder(model: &Model, message: &str, area: Rect, buf: &mut Buffer) {
    put_placeholder(model, message, area, false, buf);
}

/// The same, in the middle of `area`: for a view with nothing else to show.
pub(crate) fn centered_placeholder(model: &Model, message: &str, area: Rect, buf: &mut Buffer) {
    put_placeholder(model, message, area, true, buf);
}

fn put_placeholder(model: &Model, message: &str, area: Rect, centered: bool, buf: &mut Buffer) {
    let area = area.intersection(buf.area);
    if area.width < 3 || area.height == 0 {
        return;
    }
    let rows = text::wrap(message, usize::from(area.width - 2));
    let height = u16::try_from(rows.len()).unwrap_or(u16::MAX);
    let top = if centered {
        area.y + area.height.saturating_sub(height) / 2
    } else {
        area.y
    };
    let style = model.theme.style(StyleRole::Placeholder);
    for ((start, end), y) in rows.into_iter().zip(top..area.bottom()) {
        let row = &message[start..end];
        let x = if centered {
            let used = u16::try_from(text::width(row)).unwrap_or(area.width);
            area.x + area.width.saturating_sub(used) / 2
        } else {
            area.x + 1
        };
        buf.set_stringn(x, y, row, usize::from(area.right() - x), style);
    }
}

/// Writes spans from (`x`, `y`) without going past `max` cells.
pub(crate) fn put_spans(buf: &mut Buffer, x: u16, y: u16, spans: &[Span<'_>], max: u16) -> u16 {
    let mut used = 0;
    for span in spans {
        if used >= max {
            break;
        }
        let (end, _) = buf.set_stringn(
            x + used,
            y,
            &span.content,
            usize::from(max - used),
            span.style,
        );
        used = end - x;
    }
    used
}
