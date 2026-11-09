//! The list of every key, generated from the keymap: one block per context,
//! the blocks set in as many columns as the screen holds, so that little or
//! nothing scrolls.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::widgets::{Clear, Widget};

use super::{panel, put_spans};
use crate::app::Model;
use crate::keymap::{Context, KeySeq};
use crate::text;
use crate::theme::StyleRole;

/// The widest keys shown before they are cut.
const KEYS: usize = 14;

/// Cells between two columns.
const GAP: usize = 3;

/// One line of the list.
#[derive(Clone, Debug)]
enum Line {
    /// A context's title.
    Title(&'static str),
    /// Keys and what they do.
    Keys(String, &'static str),
    /// Space between two blocks.
    Blank,
}

/// The blocks: a title per context, then its keys.
fn blocks(model: &Model) -> Vec<Vec<Line>> {
    Context::ALL
        .iter()
        .filter_map(|context| {
            let rows: Vec<Line> = model
                .keymap
                .bindings()
                .iter()
                .filter(|binding| binding.context == *context && !binding.keys.is_empty())
                .map(|binding| {
                    let keys: Vec<String> = binding.keys.iter().map(KeySeq::short).collect();
                    Line::Keys(keys.join(", "), binding.label)
                })
                .collect();
            (!rows.is_empty()).then(|| {
                let mut block = vec![Line::Title(context.title())];
                block.extend(rows);
                block
            })
        })
        .collect()
}

/// The width of one column.
fn column_width(blocks: &[Vec<Line>]) -> usize {
    let mut keys = 0;
    let mut label = 0;
    let mut title = 0;
    for line in blocks.iter().flatten() {
        match line {
            Line::Title(text) => title = title.max(text::width(text)),
            Line::Keys(k, l) => {
                keys = keys.max(text::width(k).min(KEYS));
                label = label.max(text::width(l));
            }
            Line::Blank => {}
        }
    }
    (keys + 2 + label).max(title)
}

/// The window over `area`: nearly all of it.
fn window(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4);
    let height = area.height.saturating_sub(2);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

/// The blocks set in columns for an inner width: each block goes to the
/// shortest column so far.
fn columns(model: &Model, inner_width: u16) -> (Vec<Vec<Line>>, usize) {
    let blocks = blocks(model);
    let column = column_width(&blocks);
    let count = ((usize::from(inner_width.saturating_sub(2)) + GAP) / (column + GAP)).max(1);
    let mut columns: Vec<Vec<Line>> = vec![Vec::new(); count];
    for block in blocks {
        let shortest = (0..count)
            .min_by_key(|index| columns[*index].len())
            .unwrap_or(0);
        if !columns[shortest].is_empty() {
            columns[shortest].push(Line::Blank);
        }
        columns[shortest].extend(block);
    }
    (columns, column)
}

/// How far the list can scroll on the model's screen.
pub(crate) fn max_scroll(model: &Model) -> u16 {
    let area = Rect::new(0, 0, model.size.0, model.size.1);
    let window = window(area);
    let (columns, _) = columns(model, window.width.saturating_sub(2));
    let tallest = columns.iter().map(Vec::len).max().unwrap_or(0);
    let visible = usize::from(window.height.saturating_sub(2));
    u16::try_from(tallest.saturating_sub(visible)).unwrap_or(u16::MAX)
}

/// Draws the list over `area`, scrolled by `scroll` lines.
pub(crate) fn render(model: &Model, scroll: u16, area: Rect, buf: &mut Buffer) {
    let theme = &model.theme;
    let window = window(area);
    if window.width < 10 || window.height < 3 {
        return;
    }
    Clear.render(window, buf);
    buf.set_style(window, theme.style(StyleRole::Background));
    let title = vec![
        Span::raw("All keys"),
        Span::styled(
            " · [tui.keys] in config.toml changes them · esc closes",
            theme.style(StyleRole::TextDim),
        ),
    ];
    let inner = panel(buf, window, title, true, theme);
    let (columns, column) = columns(model, inner.width);
    let visible = usize::from(inner.height);
    let tallest = columns.iter().map(Vec::len).max().unwrap_or(0);
    let scroll = usize::from(scroll).min(tallest.saturating_sub(visible));
    let keys_width = columns
        .iter()
        .flatten()
        .filter_map(|line| match line {
            Line::Keys(keys, _) => Some(text::width(keys).min(KEYS)),
            _ => None,
        })
        .max()
        .unwrap_or(1);
    for (index, lines) in columns.iter().enumerate() {
        let x = inner.x + 1 + u16::try_from(index * (column + GAP)).unwrap_or(u16::MAX);
        if x >= inner.right() {
            break;
        }
        let max = inner.right().saturating_sub(x);
        for (row, line) in (0u16..).zip(lines.iter().skip(scroll).take(visible)) {
            let spans = match line {
                Line::Title(title) => vec![Span::styled(*title, theme.style(StyleRole::Title))],
                Line::Keys(keys, label) => {
                    let keys: String = keys.chars().take(KEYS).collect();
                    let padding = keys_width.saturating_sub(text::width(&keys)) + 2;
                    vec![
                        Span::styled(keys, theme.style(StyleRole::HintKey)),
                        Span::raw(" ".repeat(padding)),
                        Span::styled(*label, theme.style(StyleRole::Text)),
                    ]
                }
                Line::Blank => continue,
            };
            put_spans(buf, x, inner.y + row, &spans, max);
        }
    }
    if scroll + visible < tallest {
        buf.set_string(
            window.right().saturating_sub(3),
            window.bottom().saturating_sub(1),
            "▼",
            theme.style(StyleRole::TextDim),
        );
    }
}
