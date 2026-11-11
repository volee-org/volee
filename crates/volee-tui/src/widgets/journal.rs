//! The journal panel: what happened lately, newest at the bottom, each line
//! with how long ago.
//!
//! ```text
//! ╭ Journal ─────────────────╮
//! │  4 min tower → invoicing │
//! │  2 min invoicing ✉ sch…  │
//! │  1 min schema ! needs y… │
//! ```

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;
use volee_core::message::Party;
use volee_core::time::Elapsed;

use super::card::CardContext;
use super::{panel, put_spans, state_glyph, truncated};
use crate::app::Model;
use crate::journal::What;
use crate::theme::StyleRole;

/// Draws the panel in `area`.
pub(crate) fn render(model: &Model, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let inside = panel(buf, area, vec![Span::raw("Journal")], false, theme);
    if inside.is_empty() {
        return;
    }
    if model.journal.is_empty() {
        super::placeholder(model, "Nothing happened yet.", inside, buf);
        return;
    }
    let context = CardContext::new(model);
    let dim = theme.style(StyleRole::TextDim);
    let room = inside.width.saturating_sub(1);
    for (y, line) in (inside.y..).zip(model.journal.last(usize::from(inside.height))) {
        let age = Elapsed::between(line.at, model.now).to_string();
        let mut spans = vec![Span::styled(format!("{age:>6} "), dim)];
        let party = |party: Party| -> Span<'static> {
            match party {
                Party::Agent(agent) => Span::styled(
                    model.name_of(agent),
                    model
                        .registry
                        .agent(agent)
                        .map_or(dim, |agent| context.accent(agent)),
                ),
                Party::Human => Span::styled("you", theme.style(StyleRole::Text)),
                Party::System => Span::styled("volee", dim),
            }
        };
        match &line.what {
            What::Launched { parent, agent } => {
                spans.push(party(Party::Agent(*parent)));
                spans.push(Span::styled(" → ", dim));
                spans.push(party(Party::Agent(*agent)));
            }
            What::Appeared(agent) => {
                spans.push(Span::styled("+ ", theme.style(StyleRole::StateDone)));
                spans.push(party(Party::Agent(*agent)));
            }
            What::Message { from, to, question } => {
                spans.push(party(*from));
                let glyph = if *question { " ? " } else { " ✉ " };
                spans.push(Span::styled(glyph, theme.style(StyleRole::EdgeMessage)));
                spans.push(party(*to));
            }
            What::State { agent, state } => {
                spans.push(party(Party::Agent(*agent)));
                spans.push(Span::styled(
                    format!(" {} {}", state_glyph(state), state.label()),
                    theme.state(state),
                ));
            }
            What::Left(agent) => {
                spans.push(party(Party::Agent(*agent)));
                spans.push(Span::styled(" left", dim));
            }
        }
        put_spans(buf, inside.x, y, &truncated(&spans, room), room);
    }
}
