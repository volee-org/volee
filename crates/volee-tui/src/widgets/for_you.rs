//! What waits for the person, always in sight on wide screens: the
//! questions agents asked them and that nobody answered, oldest first, then
//! the agents that wait for them (a permission, a dialog).
//!
//! ```text
//! ╭ For you · 2 ─────────────╮
//! │ ? invoice-tools          │
//! │   Should create_invoic…  │
//! │ ! invoice-schema         │
//! │   Bash: bun run db:mig…  │
//! ```

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use volee_core::message::Party;
use volee_core::state::StateCategory;
use volee_core::{AgentId, Registry};

use super::card::CardContext;
use super::{panel, put_spans, truncated};
use crate::app::{Model, inner};
use crate::text;
use crate::theme::StyleRole;

/// One thing that waits for the person.
struct Item {
    /// The agent it comes from.
    agent: AgentId,
    /// `?` for a question, `!` for an agent that waits.
    glyph: &'static str,
    /// What it asks.
    text: String,
}

/// The questions without an answer, oldest first, then the agents that wait
/// for the person and asked no question.
fn items(registry: &Registry) -> Vec<Item> {
    let mut questions = registry.unanswered_questions();
    questions.sort_by_key(|message| message.at);
    let mut items: Vec<Item> = questions
        .into_iter()
        .filter_map(|message| match message.from {
            Party::Agent(agent) => Some(Item {
                agent,
                glyph: "?",
                text: message.body.lines().next().unwrap_or_default().to_owned(),
            }),
            _ => None,
        })
        .collect();
    for agent in registry.agents_by_urgency() {
        if agent.state.category() != StateCategory::NeedsInput
            || items.iter().any(|item| item.agent == agent.id)
        {
            continue;
        }
        items.push(Item {
            agent: agent.id,
            glyph: "!",
            text: agent
                .detail
                .clone()
                .unwrap_or_else(|| agent.state.label().to_owned()),
        });
    }
    items
}

/// The rows each item takes.
const ITEM_ROWS: u16 = 2;

/// Where each item is drawn inside `area`, for the mouse: a click selects
/// its agent.
pub(crate) fn targets(model: &Model, area: Rect) -> Vec<(Rect, AgentId)> {
    let inside = inner(area);
    items(&model.registry)
        .into_iter()
        .zip((inside.y..inside.bottom()).step_by(usize::from(ITEM_ROWS)))
        .map(|(item, y)| {
            let rows = ITEM_ROWS.min(inside.bottom() - y);
            (Rect::new(inside.x, y, inside.width, rows), item.agent)
        })
        .collect()
}

/// Draws the panel in `area`.
pub(crate) fn render(model: &Model, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let items = items(&model.registry);
    let mut title = vec![Span::raw("For you")];
    if !items.is_empty() {
        title.push(Span::styled(
            format!(" · {}", items.len()),
            theme.style(StyleRole::CounterUrgent),
        ));
    }
    let inside = panel(buf, area, title, false, theme);
    if inside.is_empty() {
        return;
    }
    if items.is_empty() {
        super::placeholder(model, "Nothing waits for you.", inside, buf);
        return;
    }
    let context = CardContext::new(model);
    let room = inside.width.saturating_sub(1);
    let shown = usize::from(inside.height / ITEM_ROWS);
    for (item, y) in items
        .iter()
        .take(shown)
        .zip((inside.y..inside.bottom()).step_by(usize::from(ITEM_ROWS)))
    {
        let Some(agent) = model.registry.agent(item.agent) else {
            continue;
        };
        let selected = model.selected == Some(agent.id);
        let mark = theme.style(StyleRole::StateNeedsInput);
        let mut name = context.accent(agent).add_modifier(Modifier::BOLD);
        if selected {
            name = name.patch(Style {
                bg: theme.style(StyleRole::Selection).bg,
                ..Style::default()
            });
        }
        let head = [
            Span::styled(format!("{} ", item.glyph), mark),
            Span::styled(agent.name.as_str(), name),
        ];
        put_spans(buf, inside.x + 1, y, &truncated(&head, room), room);
        if y + 1 < inside.bottom() {
            let text = text::truncate(&item.text, usize::from(room.saturating_sub(2)));
            put_spans(
                buf,
                inside.x + 3,
                y + 1,
                &[Span::styled(text, theme.style(StyleRole::TextDim))],
                room.saturating_sub(2),
            );
        }
    }
    // More than the panel holds: say how many more.
    let hidden = items.len().saturating_sub(shown);
    if hidden > 0 {
        let more = format!("+{hidden} more: I");
        let width = u16::try_from(text::width(&more)).unwrap_or(0);
        buf.set_string(
            area.right().saturating_sub(width + 2),
            area.bottom().saturating_sub(1),
            &more,
            theme.style(StyleRole::TextDim),
        );
    }
}
