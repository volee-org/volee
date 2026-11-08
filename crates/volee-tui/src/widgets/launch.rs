//! The launch dialogs: each field under its name, the focused one with its
//! cursor, and the source's refusal at the bottom until it is fixed.
//!
//! ```text
//! ╭ New agent ───────────────────────────────────────────────────────╮
//! │ repository   billing   dashboard   gateway                       │
//! │ branch       feat/invoice-export█                                │
//! │ task         Export invoices as CSV, one file per month.         │
//! │                                                                  │
//! │ model        the configured default                              │
//! │                                                                  │
//! │ Claude Code does not trust ~/code/billing: run `claude` there    │
//! │ once and accept the trust prompt.                                │
//! ╰──────────────────────────────────────────────────────────────────╯
//! ```

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Clear, Widget};

use super::{panel, put_spans};
use crate::app::Model;
use crate::editor::TextField;
use crate::launch::{DialogKind, LaunchDialog, Value};
use crate::text;
use crate::theme::StyleRole;

/// The width of the names column.
const NAMES: u16 = 13;

/// Rows a several-line field shows.
const LINES: usize = 4;

/// Draws a dialog over `area`.
pub(crate) fn render(model: &Model, dialog: &LaunchDialog, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let width = 84.min(area.width.saturating_sub(4));
    let room = width.saturating_sub(4 + NAMES);
    let rows: usize = dialog
        .fields
        .iter()
        .map(|field| match field.value {
            Value::Lines(_) => LINES,
            _ => 1,
        })
        .sum::<usize>()
        + dialog.fields.len();
    let error_rows = dialog.error.as_deref().map_or(0, |error| {
        text::wrap(error, usize::from(width.saturating_sub(4))).len() + 1
    });
    let height = u16::try_from(rows + error_rows + 3)
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(2));
    if width < 30 || height < 6 || room < 10 {
        return;
    }
    let window = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    Clear.render(window, buf);
    buf.set_style(window, theme.style(StyleRole::Background));
    let mut title = vec![Span::raw(match dialog.kind {
        DialogKind::Agent => "New agent",
        DialogKind::Mission => "New mission",
    })];
    if dialog.sending {
        title.push(Span::styled(
            " · launching…",
            theme.style(StyleRole::TextDim),
        ));
    }
    let inner = panel(buf, window, title, true, theme);
    let x = inner.x + 1;
    let value_x = x + NAMES;
    let mut y = inner.y;
    for (index, field) in dialog.fields.iter().enumerate() {
        if y >= inner.bottom() {
            break;
        }
        let focused = index == dialog.focus && !dialog.sending;
        let name_style = if focused {
            theme
                .style(StyleRole::BorderFocused)
                .add_modifier(Modifier::BOLD)
        } else {
            theme.style(StyleRole::TextDim)
        };
        buf.set_string(x, y, field.name, name_style);
        let height = render_value(
            model,
            &field.value,
            field.placeholder,
            focused,
            Rect::new(value_x, y, room, inner.bottom() - y),
            buf,
        );
        y += height + 1;
    }
    let (note, style) = match &dialog.error {
        Some(error) => (error.as_str(), theme.style(StyleRole::NoticeError)),
        // The launch waits for the repository's setup commands, which can
        // take minutes: closing the dialog does not stop it.
        None if dialog.sending && dialog.kind == DialogKind::Agent => (
            "The setup may take a while: esc closes this window, the launch goes on.",
            theme.style(StyleRole::TextDim),
        ),
        None => return,
    };
    let width = usize::from(inner.width.saturating_sub(2));
    for (start, end) in text::wrap(note, width) {
        if y >= inner.bottom() {
            break;
        }
        buf.set_stringn(x, y, &note[start..end], width, style);
        y += 1;
    }
}

/// Draws a field's value; returns the rows it took.
fn render_value(
    model: &Model,
    value: &Value,
    placeholder: &str,
    focused: bool,
    area: Rect,
    buf: &mut Buffer,
) -> u16 {
    let theme = &model.theme;
    let dim = theme.style(StyleRole::TextDim);
    match value {
        Value::Line(field) => {
            render_text(
                model,
                field,
                placeholder,
                focused,
                Rect { height: 1, ..area },
                buf,
            );
            1
        }
        Value::Lines(field) => {
            let rows = u16::try_from(LINES).unwrap_or(1).min(area.height);
            render_text(
                model,
                field,
                placeholder,
                focused,
                Rect {
                    height: rows,
                    ..area
                },
                buf,
            );
            rows
        }
        Value::Choice { options, index } => {
            let items = options
                .iter()
                .enumerate()
                .map(|(position, option)| {
                    let style = if position == *index {
                        theme.style(StyleRole::Selection)
                    } else {
                        dim
                    };
                    Span::styled(format!(" {option} "), style)
                })
                .collect();
            let tip = focused.then(|| Span::styled("←/→", theme.style(StyleRole::HintKey)));
            put_spans(
                buf,
                area.x,
                area.y,
                &in_view(items, *index, tip, area.width),
                area.width,
            );
            1
        }
        Value::Many {
            options,
            chosen,
            cursor,
        } => {
            if options.is_empty() {
                put_spans(
                    buf,
                    area.x,
                    area.y,
                    &[Span::styled(
                        placeholder.to_owned(),
                        theme.style(StyleRole::Placeholder),
                    )],
                    area.width,
                );
                return 1;
            }
            let items = options
                .iter()
                .zip(chosen)
                .enumerate()
                .map(|(position, (option, chosen))| {
                    let mut style = if *chosen {
                        theme.style(StyleRole::Input)
                    } else {
                        dim
                    };
                    if focused && position == *cursor {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    let mark = if *chosen { "■" } else { "□" };
                    Span::styled(format!("{mark} {option}"), style)
                })
                .collect();
            let tip = focused.then(|| Span::styled("space picks", theme.style(StyleRole::HintKey)));
            put_spans(
                buf,
                area.x,
                area.y,
                &in_view(items, *cursor, tip, area.width),
                area.width,
            );
            1
        }
    }
}

/// Options side by side, starting late enough that the one at `focus`
/// shows, with `‹` and `›` where more are hidden, and a tip after them when
/// it fits.
fn in_view<'a>(
    items: Vec<Span<'a>>,
    focus: usize,
    tip: Option<Span<'a>>,
    width: u16,
) -> Vec<Span<'a>> {
    const GAP: usize = 2;
    let width = usize::from(width);
    let widths: Vec<usize> = items
        .iter()
        .map(|item| text::width(&item.content))
        .collect();
    let span = |from: usize, to: usize| -> usize {
        widths[from..=to].iter().sum::<usize>() + GAP * (to - from)
    };
    let mut first = 0;
    // Room for the marks on both sides.
    while first < focus && span(first, focus) + 4 > width {
        first += 1;
    }
    let mut spans = Vec::new();
    let mut used = 0;
    if first > 0 {
        spans.push(Span::raw("‹ "));
        used += 2;
    }
    for (position, item) in items.into_iter().enumerate().skip(first) {
        let gap = if position > first { GAP } else { 0 };
        if used + gap + widths[position] + 2 > width {
            spans.push(Span::raw(" ›"));
            return spans;
        }
        spans.push(Span::raw(" ".repeat(gap)));
        used += gap + widths[position];
        spans.push(item);
    }
    if let Some(tip) = tip
        && used + GAP + text::width(&tip.content) <= width
    {
        spans.push(Span::raw(" ".repeat(GAP)));
        spans.push(tip);
    }
    spans
}

/// Draws a text field, wrapped, with its cursor when focused, or its
/// placeholder when empty.
fn render_text(
    model: &Model,
    field: &TextField,
    placeholder: &str,
    focused: bool,
    area: Rect,
    buf: &mut Buffer,
) {
    let theme = &model.theme;
    let width = usize::from(area.width);
    if field.text().is_empty() {
        buf.set_stringn(
            area.x,
            area.y,
            placeholder,
            width,
            theme.style(StyleRole::Placeholder),
        );
    }
    let rows = text::wrap(field.text(), width.saturating_sub(1));
    let (cursor_row, column) = text::cursor_position(&rows, field.text(), field.cursor());
    let visible = usize::from(area.height);
    let offset = (cursor_row + 1).saturating_sub(visible);
    for (row, (start, end)) in (0u16..).zip(rows.iter().skip(offset).take(visible)) {
        buf.set_stringn(
            area.x,
            area.y + row,
            &field.text()[*start..*end],
            width,
            theme.style(StyleRole::Text),
        );
    }
    if focused {
        let x = area.x + u16::try_from(column).unwrap_or(0);
        let y = area.y + u16::try_from(cursor_row - offset).unwrap_or(0);
        if x < area.right()
            && let Some(cell) = buf.cell_mut((x, y))
        {
            cell.set_style(
                theme
                    .style(StyleRole::Input)
                    .patch(Style::default().add_modifier(Modifier::REVERSED)),
            );
        }
    }
}
