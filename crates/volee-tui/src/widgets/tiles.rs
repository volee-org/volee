//! The wall of screens (ADR 0028): a tile per agent that runs, over the
//! whole interface but its hint line.
//!
//! ```text
//!  Wall · 5 agents run
//! ╭ ▸ invoice-schema · working · live ─╮╭ ! tax-rules · needs you ─────────╮
//! │ ⏺ The tests pass: 356 of 356.      ││ Do you want to run this command? │
//! │ > commit and push                  ││ ❯ 1. Yes                         │
//! ╰────────────────────────────────────╯╰──────────────────────────────────╯
//! ```

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::Span;
use ratatui::widgets::{Clear, Widget};

use super::{panel_among, placeholder, put_spans, state_glyph, truncated};
use crate::app::Model;
use crate::live::Phase;
use crate::preview::Fit;
use crate::text;
use crate::theme::StyleRole;
use crate::tiles::{Tile, TileScreen, Wall};

/// Draws the wall over `area`, the whole screen, but its last row.
pub(crate) fn render(model: &Model, area: Rect, buf: &mut Buffer) {
    let Some(wall) = &model.wall else {
        return;
    };
    let window = Rect {
        height: area.height.saturating_sub(1),
        ..area
    };
    if window.width < 10 || window.height < 4 {
        return;
    }
    Clear.render(window, buf);
    buf.set_style(window, model.theme.style(StyleRole::Background));
    let (header, areas) = crate::tiles::areas((area.width, area.height), wall.tiles.len());
    draw_header(model, wall, header, buf);
    for (tile, area) in wall.tiles.iter().zip(areas) {
        draw_tile(model, tile, area, buf);
    }
}

/// What the wall holds, on its first row.
fn draw_header(model: &Model, wall: &Wall, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let dim = theme.style(StyleRole::TextDim);
    let count = match wall.count {
        1 => "1 agent runs".to_owned(),
        count => format!("{count} agents run"),
    };
    let mut spans = vec![
        Span::styled(
            "Wall",
            theme.style(StyleRole::Title).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" · {count}"), dim),
    ];
    let (page, pages) = wall.pages();
    if pages > 1 {
        spans.push(Span::styled(
            format!(" · page {} of {pages}", page + 1),
            dim,
        ));
    }
    put_spans(
        buf,
        area.x + 1,
        area.y,
        &spans,
        area.width.saturating_sub(2),
    );
}

/// One tile: the agent's name and state on the border, the bottom of its
/// screen inside.
fn draw_tile(model: &Model, tile: &Tile, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let dim = theme.style(StyleRole::TextDim);
    let selected = model.selected == Some(tile.agent);
    let mut label = Vec::new();
    if let Some(agent) = model.registry.agent(tile.agent) {
        label.push(Span::styled(
            state_glyph(&agent.state),
            theme.state(&agent.state),
        ));
        label.push(Span::raw(" "));
        label.push(Span::styled(agent.name.clone(), theme.role(agent.role)));
        label.push(Span::styled(" · ", dim));
        label.push(Span::styled(agent.state.label(), theme.state(&agent.state)));
    }
    if let TileScreen::Live(live) = &tile.screen {
        let link = match &live.phase {
            _ if live.exited => "ended",
            Phase::Connecting => "connecting",
            Phase::Closed(_) => "disconnected",
            Phase::Live => "live",
        };
        label.push(Span::styled(format!(" · {link}"), dim));
    }
    // The border's corners and a blank on each side of the title.
    let room = area.width.saturating_sub(4);
    let inner = panel_among(buf, area, truncated(&label, room), selected, theme);
    if inner.is_empty() {
        return;
    }
    match &tile.screen {
        TileScreen::Live(live) => match (live.preview(theme), &live.phase) {
            (Some(screen), _) => screen.render(inner, buf, 0, Fit::Crop, theme),
            (None, Phase::Closed(reason)) => {
                let reason = text::clause(reason);
                let message = format!("No screen: its volee host does not answer ({reason}).");
                placeholder(model, &message, inner, buf);
            }
            (None, _) => placeholder(model, "Connecting to the agent's volee host…", inner, buf),
        },
        TileScreen::Read(reading) => match (&reading.screen, &reading.error) {
            (Some(screen), _) => screen.render(inner, buf, 0, Fit::Crop, theme),
            (None, Some(error)) => {
                placeholder(model, &format!("No screen: {error}"), inner, buf);
            }
            // The source is gone: nothing more will be read.
            (None, None) if model.offline.is_some() => {
                placeholder(model, "No screen: nothing is read any more.", inner, buf);
            }
            (None, None) => placeholder(model, "Reading the agent's screen…", inner, buf),
        },
    }
}
