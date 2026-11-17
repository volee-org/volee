//! The command palette, in the middle of the screen as LazyVim's command
//! line: what was typed after `:` on the window's first row, and the
//! matching commands under it, the selected one highlighted. The first row
//! stays where it is while the matches change.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::widgets::{Clear, Widget};

use super::put_spans;
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
    if area.width < 16 || area.height < 6 {
        return;
    }
    let found = matching(model, &palette.text);
    let selected = palette.selected.min(found.len().saturating_sub(1));
    // The band, a blank row, the prompt, a blank row, the commands and a
    // blank row below them, above the status line.
    let most = usize::from(area.height.saturating_sub(7)).clamp(1, ROWS);
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
        .clamp(60, MOST_WIDTH)
        .min(area.width.saturating_sub(2));
    // The descriptions take what the names and the keys leave.
    columns.label = usize::from(width)
        .saturating_sub(4 + columns.name + 2 + 2 + columns.keys)
        .min(columns.label);
    let height = u16::try_from(shown.len().max(1) + 5)
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(1));
    // Centred across; down, where the tallest window would be centred, so
    // that the typed row does not move as the matches change.
    let tallest = u16::try_from(most + 5).unwrap_or(u16::MAX);
    let window = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(tallest) / 2,
        width,
        height,
    );
    // The leader's design: a violet ground, a band at the top with the
    // mode in a block, no border.
    Clear.render(window, buf);
    buf.set_style(window, theme.style(StyleRole::KeysPanel));
    buf.set_style(
        Rect::new(window.x, window.y, window.width, 1),
        theme.style(StyleRole::KeysHeader),
    );
    put_spans(
        buf,
        window.x + 1,
        window.y,
        &[Span::styled(" COMMAND ", theme.style(StyleRole::KeysTitle))],
        window.width.saturating_sub(2),
    );
    let hint = format!(" {} · enter runs · esc ", found.len());
    let hint_width = u16::try_from(text::width(&hint)).unwrap_or(u16::MAX);
    put_spans(
        buf,
        window.right().saturating_sub(hint_width + 1),
        window.y,
        &[Span::styled(hint, theme.style(StyleRole::KeysHeader))],
        hint_width,
    );
    let inner = Rect::new(
        window.x + 1,
        window.y + 2,
        window.width.saturating_sub(2),
        window.height.saturating_sub(3),
    );
    render_prompt(theme, &palette.text, inner, buf);
    let inner = Rect::new(
        inner.x,
        inner.y + 2,
        inner.width,
        inner.height.saturating_sub(2),
    );
    if shown.is_empty() {
        let none = [Span::styled(
            "no command matches",
            theme.style(StyleRole::KeysHeading),
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
        Span::styled(":", theme.style(StyleRole::KeysKey)),
        Span::styled(typed.to_owned(), theme.style(StyleRole::Input)),
        Span::styled("▏", theme.style(StyleRole::KeysKey)),
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
    // The chosen command in a block, as the leader's panel draws its title.
    if selected {
        buf.set_style(area, theme.style(StyleRole::KeysTitle));
    }
    let style = |role| theme.style(if selected { StyleRole::KeysTitle } else { role });
    let padded = |text: &str, width: usize| {
        format!(
            "{text}{}",
            " ".repeat(width.saturating_sub(text::width(text)) + 2)
        )
    };
    let spans = [
        Span::styled(
            padded(&command.name, columns.name),
            style(StyleRole::KeysKey),
        ),
        Span::styled(
            padded(&text::truncate(command.label, columns.label), columns.label),
            style(StyleRole::KeysPanel),
        ),
        Span::styled(command.keys.clone(), style(StyleRole::KeysHeading)),
    ];
    put_spans(
        buf,
        area.x + 1,
        area.y,
        &spans,
        area.width.saturating_sub(2),
    );
}
