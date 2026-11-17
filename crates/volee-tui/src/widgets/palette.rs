//! The command palette, in the middle of the screen as LazyVim's command
//! line: what was typed after `:` on the window's first row, and the
//! matching commands under it, the selected one highlighted. The first row
//! stays where it is while the matches change.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::widgets::{Clear, Widget};

use super::{panel, put_spans};
use crate::app::Model;
use crate::commands::{Command, Palette, matching};
use crate::text;
use crate::theme::{StyleRole, Theme};

/// The most commands listed at once.
const ROWS: usize = 12;

/// The widest the window gets: a long description is cut, not spread.
const MOST_WIDTH: u16 = 100;

/// The widths of a row's columns: name, what it does, keys.
#[derive(Clone, Copy)]
struct Columns {
    name: usize,
    label: usize,
    keys: usize,
}

/// Draws the palette over `area`.
pub(crate) fn render(model: &Model, palette: &Palette, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let area = area.intersection(buf.area);
    if area.width < 16 || area.height < 4 {
        return;
    }
    let found = matching(model, &palette.text);
    let selected = palette.selected.min(found.len().saturating_sub(1));
    let most = usize::from(area.height.saturating_sub(5)).clamp(1, ROWS);
    let first = selected.saturating_sub(most - 1);
    let shown: Vec<&Command> = found.iter().skip(first).take(most).collect();
    let widest = |width: fn(&Command) -> usize| shown.iter().map(|c| width(c)).max().unwrap_or(0);
    let mut columns = Columns {
        name: widest(|c| text::width(&c.name)).max(10),
        label: widest(|c| text::width(c.label)),
        keys: widest(|c| text::width(&c.keys)),
    };
    let inner_width = columns.name + 2 + columns.label + 2 + columns.keys;
    let width = u16::try_from(inner_width + 4)
        .unwrap_or(u16::MAX)
        .max(60)
        .min(MOST_WIDTH)
        .min(area.width.saturating_sub(2));
    // The descriptions take what the names and the keys leave.
    columns.label = usize::from(width)
        .saturating_sub(4 + columns.name + 2 + 2 + columns.keys)
        .min(columns.label);
    let height = u16::try_from(shown.len().max(1) + 3)
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(1));
    // Centred across; down, where the tallest window would be centred, so
    // that the typed row does not move as the matches change.
    let tallest = u16::try_from(most + 3).unwrap_or(u16::MAX);
    let window = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(tallest) / 2,
        width,
        height,
    );
    Clear.render(window, buf);
    buf.set_style(window, theme.style(StyleRole::Background));
    let title = vec![
        Span::raw("Command line"),
        Span::styled(
            format!(" · {} · enter runs · esc", found.len()),
            theme.style(StyleRole::TextDim),
        ),
    ];
    let inner = panel(buf, window, title, true, theme);
    render_prompt(theme, &palette.text, inner, buf);
    let inner = Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(1),
    );
    if shown.is_empty() {
        let none = [Span::styled(
            "no command matches",
            theme.style(StyleRole::TextDim),
        )];
        put_spans(
            buf,
            inner.x + 1,
            inner.y,
            &none,
            inner.width.saturating_sub(2),
        );
        return;
    }
    for (row, command) in (0u16..).zip(shown.iter()) {
        if row >= inner.height {
            break;
        }
        let row_area = Rect::new(inner.x, inner.y + row, inner.width, 1);
        let is_selected = first + usize::from(row) == selected;
        render_row(theme, command, columns, is_selected, row_area, buf);
    }
}

/// The window's first row: `:` and what was typed.
fn render_prompt(theme: &Theme, typed: &str, area: Rect, buf: &mut Buffer) {
    let line = Rect::new(area.x, area.y, area.width, 1);
    let prompt = [
        Span::styled(":", theme.style(StyleRole::HintKey)),
        Span::styled(typed.to_owned(), theme.style(StyleRole::Input)),
        Span::styled("▏", theme.style(StyleRole::HintKey)),
    ];
    put_spans(
        buf,
        line.x + 1,
        line.y,
        &prompt,
        line.width.saturating_sub(2),
    );
}

/// One command: its name, what it does and its key.
fn render_row(
    theme: &Theme,
    command: &Command,
    columns: Columns,
    selected: bool,
    area: Rect,
    buf: &mut Buffer,
) {
    if selected {
        buf.set_style(area, theme.style(StyleRole::Selection));
    }
    let style = |role| theme.style(if selected { StyleRole::Selection } else { role });
    let padded = |text: &str, width: usize| {
        format!(
            "{text}{}",
            " ".repeat(width.saturating_sub(text::width(text)) + 2)
        )
    };
    let spans = [
        Span::styled(padded(&command.name, columns.name), style(StyleRole::Text)),
        Span::styled(
            padded(&text::truncate(command.label, columns.label), columns.label),
            style(StyleRole::TextDim),
        ),
        Span::styled(command.keys.clone(), style(StyleRole::HintKey)),
    ];
    put_spans(
        buf,
        area.x + 1,
        area.y,
        &spans,
        area.width.saturating_sub(2),
    );
}
