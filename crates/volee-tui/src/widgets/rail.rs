//! The list of agents: each tree under its root, the roots grouped by kind
//! (leads, missions, sessions in a terminal, sessions in the background),
//! one row per agent, in the agent's color.
//!
//! ```text
//! ╭ Agents ─────────────────────╮
//! │ LEADS                       │
//! │▌t ▸ tower            24 min │
//! │ n ├─ · invoicing     14 min │
//! │ s │  ├─ ! invoice-s…  2 min │
//! │ TERMINALS                   │
//! │ i · infra            25 min │
//! ```

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::Span;
use volee_core::time::Elapsed;

use super::card::CardContext;
use super::{panel, put_spans, state_glyph, truncated};
use crate::app::{Model, Pane, RailEntry, RailOrder, RailRow};
use crate::text;
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
        let message = if model.registry.agents().next().is_some() {
            "No agent matches the filter"
        } else if model.layout.view.is_some() {
            // The view next to the list says more, with more room.
            "No agents yet"
        } else {
            model
                .settings
                .without_claude_code()
                .unwrap_or("No agents yet")
        };
        super::placeholder(model, message, inner, buf);
        return;
    }
    let rows = model.rail_rows();
    let context = CardContext::new(model);
    for (y, row) in (inner.y..inner.bottom()).zip(rows.iter().skip(model.rail_offset)) {
        let line = Rect::new(inner.x, y, inner.width, 1);
        match row {
            RailRow::Group(title) => {
                let style = theme.style(StyleRole::TextDim).add_modifier(Modifier::BOLD);
                put_spans(
                    buf,
                    line.x + 1,
                    y,
                    &[Span::styled(*title, style)],
                    line.width.saturating_sub(1),
                );
            }
            RailRow::Entry(index) => {
                if let Some(entry) = model.rail.get(*index) {
                    draw_entry(model, entry, line, &context, buf);
                }
            }
        }
    }
    // Arrows on the border tell that the list goes on.
    let arrow = theme.style(StyleRole::TextDim);
    if model.rail_offset > 0 {
        buf.set_string(area.right().saturating_sub(3), area.y, "▲", arrow);
    }
    if model.rail_offset + usize::from(inner.height) < rows.len() {
        buf.set_string(
            area.right().saturating_sub(3),
            area.bottom().saturating_sub(1),
            "▼",
            arrow,
        );
    }
}

/// One agent: its jump letter, its place in its tree, its state, its name in
/// its color, and how long it has been in its state.
fn draw_entry(
    model: &Model,
    entry: &RailEntry,
    area: Rect,
    context: &CardContext<'_>,
    buf: &mut Buffer,
) {
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
    let mut name_style = context.accent(agent);
    if selected {
        name_style = name_style.add_modifier(Modifier::BOLD);
    }
    let line = [
        Span::styled(letter, letter_style),
        Span::raw(" "),
        Span::styled(compact_guides(&entry.guides), theme.style(StyleRole::Edge)),
        Span::styled(state_glyph(&agent.state), theme.state(&agent.state)),
        Span::raw(" "),
        Span::styled(agent.name.as_str(), name_style),
    ];
    let width: usize = line.iter().map(|span| text::width(&span.content)).sum();
    let width = u16::try_from(width).unwrap_or(u16::MAX);
    // The time in its state when the whole name fits with it; the name
    // first when both do not.
    if width + 2 + elapsed_width < area.width {
        put_spans(buf, area.x + 1, area.y, &line, width);
        buf.set_string(area.right() - elapsed_width - 1, area.y, &elapsed, dim);
    } else {
        let room = area.width.saturating_sub(2);
        put_spans(buf, area.x + 1, area.y, &truncated(&line, room), room);
    }
}

/// Tree guides two cells a level instead of three: the list is narrow.
fn compact_guides(guides: &str) -> String {
    guides
        .replace("├─ ", "├ ")
        .replace("└─ ", "└ ")
        .replace("│  ", "│ ")
        .replace("   ", "  ")
}
