//! Drawing: a pure function of the model (ADR 0009).

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;

use crate::app::{Model, Overlay, Pane, Tab};
use crate::theme::StyleRole;
use crate::views::{hierarchy, network, repos, timeline};
use crate::widgets::{
    composer, detail, diff, handover, help, hints, inbox, keys, launch, palette, panel, rail,
    theme_picker, tiles, tools, top_bar,
};

/// Draws the whole screen from the model.
pub(crate) fn view(model: &Model, frame: &mut Frame<'_>) {
    let area = frame.area();
    let buf = frame.buffer_mut();
    let theme = &model.theme;
    buf.set_style(area, theme.style(StyleRole::Background));
    let layout = &model.layout;

    top_bar::render(model, layout.top, buf);
    rail::render(model, layout.rail, buf);
    if let Some(view) = layout.view {
        let focused = model.focus == Pane::View && model.overlay.is_none();
        let mut title = vec![Span::raw(model.tab.title())];
        if model.tab == Tab::Hierarchy {
            for root in &model.subtree {
                let name = model
                    .registry
                    .agent(*root)
                    .map_or_else(|| root.short(), |agent| agent.name.clone());
                title.push(Span::styled(
                    format!(" › {name}"),
                    theme.style(StyleRole::Text),
                ));
            }
            let zoom = if model.scene.automatic {
                model.scene.zoom.label().to_owned()
            } else {
                format!("{} (fixed)", model.scene.zoom.label())
            };
            title.push(Span::styled(
                format!(" · {zoom}"),
                theme.style(StyleRole::TextDim),
            ));
        }
        match model.tab {
            Tab::Hierarchy => {}
            Tab::Network => title.extend(network::title(model)),
            Tab::Repos => title.extend(repos::title(model)),
            Tab::Timeline => title.extend(timeline::title(model)),
        }
        let inner = panel(buf, view, title, focused, theme);
        match model.tab {
            Tab::Hierarchy => {
                hierarchy::render(model, inner, buf);
                scroll_marks(model, view, inner, buf);
            }
            Tab::Network => network::render(model, inner, buf),
            Tab::Repos => repos::render(model, inner, buf),
            Tab::Timeline => timeline::render(model, inner, buf),
        }
    }
    detail::render(model, layout.detail, buf);
    if let Some(preview) = layout.preview {
        detail::render_preview(model, preview, buf);
    }
    hints::render(model, layout.hints, buf);
    let mut cursor = None;
    match &model.overlay {
        Some(Overlay::Keys) => keys::render_screen(model, area, buf),
        Some(Overlay::Help { scroll }) => help::render(model, *scroll, area, buf),
        Some(Overlay::Commands(palette)) => palette::render(model, palette, area, buf),
        Some(Overlay::Themes { selected, .. }) => {
            // Above the hint bar, one cell from the right edge, like the
            // other popups.
            let above = Rect::new(
                area.x,
                area.y,
                area.width.saturating_sub(1),
                area.height.saturating_sub(1),
            );
            theme_picker::render(&model.themes.names, *selected, &model.theme, above, buf);
        }
        Some(Overlay::Composer) => {
            if let Some(draft) = &model.draft {
                composer::render(model, draft, area, buf);
            }
        }
        Some(Overlay::Diff(diff)) => diff::render(model, diff, area, buf),
        Some(Overlay::Inbox(inbox)) => inbox::render(model, inbox, area, buf),
        Some(Overlay::Tools(menu)) => tools::render(model, *menu, area, buf),
        Some(Overlay::Launch(dialog)) => launch::render(model, dialog, area, buf),
        Some(Overlay::Screen) => cursor = detail::render_full_screen(model, area, buf),
        Some(Overlay::Wall) => tiles::render(model, area, buf),
        // While volee waits for a session to leave its terminal, what the
        // person must do there is spelled out (ADR 0034).
        None => handover::render(model, area, buf),
    }
    if !model.pending_keys.is_empty() {
        keys::render_pending(model, area, buf);
    }
    // In edit mode, the agent's cursor is the terminal's.
    if let Some(cursor) = cursor {
        frame.set_cursor_position(cursor);
    }
}

/// Arrows on the view's border where the hierarchy goes on out of sight.
fn scroll_marks(model: &Model, panel: Rect, inner: Rect, buf: &mut Buffer) {
    let (x, y) = model.scene_offset;
    let style = model.theme.style(StyleRole::TextDim);
    let middle_row = panel.y + panel.height / 2;
    let middle_column = panel.x + panel.width / 2;
    let marks = [
        (x > 0, panel.x, middle_row, "◀"),
        (
            x + i32::from(inner.width) < i32::from(model.scene.width),
            panel.right().saturating_sub(1),
            middle_row,
            "▶",
        ),
        (y > 0, middle_column, panel.y, "▲"),
        (
            y + i32::from(inner.height) < i32::from(model.scene.height),
            middle_column,
            panel.bottom().saturating_sub(1),
            "▼",
        ),
    ];
    for (shown, column, row, mark) in marks {
        if shown {
            buf.set_string(column, row, mark, style);
        }
    }
}
