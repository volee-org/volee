//! The help overlay: every key, generated from the keymap.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::widgets::{Clear, Widget};

use super::{panel, put_spans};
use crate::app::Model;
use crate::keymap::Context;
use crate::text;
use crate::theme::StyleRole;

/// The width of the keys column.
const KEYS: usize = 16;

/// The help's lines: a title per context, then its keys and what they do.
fn lines(model: &Model) -> Vec<Vec<Span<'static>>> {
    let theme = &model.theme;
    let mut lines = Vec::new();
    for context in Context::ALL {
        if !lines.is_empty() {
            lines.push(Vec::new());
        }
        lines.push(vec![Span::styled(
            context.title(),
            theme.style(StyleRole::Title),
        )]);
        for binding in model
            .keymap
            .bindings()
            .iter()
            .filter(|b| b.context == context)
        {
            let keys: Vec<String> = binding.keys.iter().map(ToString::to_string).collect();
            let keys = keys.join(", ");
            let padding = KEYS.saturating_sub(text::width(&keys)).max(1);
            lines.push(vec![
                Span::raw("  "),
                Span::styled(keys, theme.style(StyleRole::HintKey)),
                Span::raw(" ".repeat(padding)),
                Span::styled(binding.description, theme.style(StyleRole::Text)),
            ]);
        }
    }
    lines
}

/// The window for `lines` lines of help over `area`.
fn window(lines: usize, area: Rect) -> Rect {
    let height = u16::try_from(lines)
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(area.height.saturating_sub(2));
    let width = 78.min(area.width.saturating_sub(4));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

/// How far the help can scroll on the model's screen.
pub(crate) fn max_scroll(model: &Model) -> u16 {
    let count = lines(model).len();
    let area = Rect::new(0, 0, model.size.0, model.size.1);
    let visible = usize::from(window(count, area).height.saturating_sub(2));
    u16::try_from(count.saturating_sub(visible)).unwrap_or(u16::MAX)
}

/// Draws the help over `area`, scrolled by `scroll` lines.
pub(crate) fn render(model: &Model, scroll: u16, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let lines = lines(model);
    let window = window(lines.len(), area);
    Clear.render(window, buf);
    buf.set_style(window, theme.style(StyleRole::Background));
    let title = vec![
        Span::raw("Keys"),
        Span::styled(" · esc closes", theme.style(StyleRole::TextDim)),
    ];
    let inner = panel(buf, window, title, true, theme);
    let visible = usize::from(inner.height);
    let scroll = usize::from(scroll).min(lines.len().saturating_sub(visible));
    for (row, line) in (0u16..).zip(lines.iter().skip(scroll).take(visible)) {
        put_spans(
            buf,
            inner.x + 1,
            inner.y + row,
            line,
            inner.width.saturating_sub(2),
        );
    }
    if scroll + visible < lines.len() {
        buf.set_string(
            window.right().saturating_sub(3),
            window.bottom().saturating_sub(1),
            "▼",
            theme.style(StyleRole::TextDim),
        );
    }
}
