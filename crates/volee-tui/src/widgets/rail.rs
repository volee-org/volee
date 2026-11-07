//! The list of agents, the most urgent first (or as a tree on small screens).
//!
//! ```text
//! ╭ Agents ─────────────────────╮
//! │ s ! invoice-schema    2 min │
//! │     needs you · billing › … │
//! ```

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::Span;
use volee_core::time::Elapsed;

use super::{panel, place_label, put_spans, state_glyph, truncated};
use crate::app::{Model, Pane, RailEntry, RailOrder};
use crate::theme::StyleRole;

/// Draws the list in `area`.
pub(crate) fn render(model: &Model, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let mut title = vec![Span::raw("Agents")];
    if model.rail_order == RailOrder::Tree {
        title.push(Span::raw(" · tree"));
    }
    if let Some(filter) = (!model.filter.text.trim().is_empty()).then_some(&model.filter.text) {
        title.push(Span::styled(
            format!(" · /{filter}"),
            theme.style(StyleRole::Input),
        ));
    }
    let focused = model.focus == Pane::Rail && model.overlay.is_none();
    let inner = panel(buf, area, title, focused, theme);
    if inner.is_empty() {
        return;
    }
    if model.rail.is_empty() {
        let message = if model.registry.agents().next().is_none() {
            "No agents yet"
        } else {
            "No agent matches the filter"
        };
        put_spans(
            buf,
            inner.x + 1,
            inner.y,
            &[Span::styled(message, theme.style(StyleRole::Placeholder))],
            inner.width.saturating_sub(1),
        );
        return;
    }
    let height = model.rail_row_height();
    for (row, entry) in (0u16..).zip(model.rail.iter().skip(model.rail_offset)) {
        let y = inner.y + row * height;
        if y >= inner.bottom() {
            break;
        }
        let rows = height.min(inner.bottom() - y);
        draw_entry(model, entry, Rect::new(inner.x, y, inner.width, rows), buf);
    }
    // Arrows on the border tell that the list goes on.
    let arrow = theme.style(StyleRole::TextDim);
    if model.rail_offset > 0 {
        buf.set_string(area.right().saturating_sub(3), area.y, "▲", arrow);
    }
    let shown = usize::from(inner.height / height.max(1));
    if model.rail_offset + shown < model.rail.len() {
        buf.set_string(
            area.right().saturating_sub(3),
            area.bottom().saturating_sub(1),
            "▼",
            arrow,
        );
    }
}

/// One entry: letter, state, name and time in state, then (when there are
/// two rows) the state's word and where the agent works.
fn draw_entry(model: &Model, entry: &RailEntry, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let dim = theme.style(StyleRole::TextDim);
    let Some(agent) = model.registry.agent(entry.agent) else {
        return;
    };
    let selected = model.selected == Some(agent.id);
    if selected {
        buf.set_style(area, theme.style(StyleRole::Selection));
        buf.set_string(area.x, area.y, "▌", theme.style(StyleRole::BorderFocused));
    }
    let letter_style = if model.prompt == Some(crate::app::Prompt::Jump) {
        theme
            .style(StyleRole::Letter)
            .add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        theme.style(StyleRole::Letter)
    };
    let letter = model
        .letters
        .of(agent.id)
        .map_or_else(|| " ".to_owned(), |letter| letter.to_string());
    let elapsed = Elapsed::between(agent.state_since, model.now).to_string();
    let elapsed_width = u16::try_from(elapsed.len()).unwrap_or(0);
    let mut name_style = theme.role(agent.role);
    if selected {
        name_style = name_style.add_modifier(Modifier::BOLD);
    }
    let line = [
        Span::styled(letter, letter_style),
        Span::raw(" "),
        Span::styled(entry.guides.as_str(), theme.style(StyleRole::Edge)),
        Span::styled(state_glyph(&agent.state), theme.state(&agent.state)),
        Span::raw(" "),
        Span::styled(agent.name.as_str(), name_style),
    ];
    let room = area.width.saturating_sub(2 + elapsed_width + 1);
    let used = put_spans(buf, area.x + 1, area.y, &truncated(&line, room), room);
    if area.width > used + elapsed_width + 2 {
        buf.set_string(area.right() - elapsed_width - 1, area.y, &elapsed, dim);
    }
    if area.height > 1 {
        let second = [
            Span::styled(agent.state.label(), theme.state(&agent.state)),
            Span::styled(" · ", dim),
            Span::styled(place_label(model, agent), dim),
        ];
        let room = area.width.saturating_sub(5);
        put_spans(buf, area.x + 4, area.y + 1, &truncated(&second, room), room);
    }
}
