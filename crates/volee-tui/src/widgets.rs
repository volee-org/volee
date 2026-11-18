//! Drawing pieces shared by the views. Widgets only read the model.

pub(crate) mod braille;
pub(crate) mod canvas;
pub(crate) mod card;
pub(crate) mod composer;
pub(crate) mod detail;
pub(crate) mod diff;
pub(crate) mod for_you;
pub(crate) mod handover;
pub(crate) mod help;
pub(crate) mod hints;
pub(crate) mod inbox;
pub(crate) mod journal;
pub(crate) mod keys;
pub(crate) mod launch;
pub(crate) mod palette;
pub(crate) mod rail;
pub(crate) mod theme_picker;
pub(crate) mod tiles;
pub(crate) mod tools;
pub(crate) mod top_bar;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget};
use volee_core::AgentState;

use crate::app::{Model, inner};
use crate::text;
use crate::theme::{StyleRole, Theme};

/// The glyph of a state. With the state's word, states read without color.
pub(crate) const fn state_glyph(state: &AgentState) -> &'static str {
    match state {
        AgentState::NeedsInput { .. } => "!",
        AgentState::Working => "▸",
        AgentState::Idle => "·",
        AgentState::Done => "✓",
        AgentState::Failed => "✗",
        AgentState::Stopped => "■",
        AgentState::Lost => "∅",
        AgentState::Launching => "…",
    }
}

/// Draws a panel's border with its title and returns the area inside.
pub(crate) fn panel(
    buf: &mut Buffer,
    area: Rect,
    title: Vec<Span<'_>>,
    focused: bool,
    theme: &Theme,
) -> Rect {
    let border = theme.style(if focused {
        StyleRole::BorderFocused
    } else {
        StyleRole::Border
    });
    if focused {
        return panel_with(buf, area, title, border);
    }
    framed(
        buf,
        area,
        title,
        border,
        theme.style(StyleRole::Title),
        BorderType::Rounded,
    )
}

/// Draws one panel among several of a kind, such as a tile of the wall, and
/// returns the area inside. The selected one has a thick border: it shows
/// without colors too.
pub(crate) fn panel_among(
    buf: &mut Buffer,
    area: Rect,
    title: Vec<Span<'_>>,
    selected: bool,
    theme: &Theme,
) -> Rect {
    if selected {
        let border = theme.style(StyleRole::BorderFocused);
        return framed(
            buf,
            area,
            title,
            border,
            border.add_modifier(Modifier::BOLD),
            BorderType::Thick,
        );
    }
    panel(buf, area, title, false, theme)
}

/// Draws a panel whose border and bold title take `border`, such as the
/// screen of an agent in edit mode, and returns the area inside.
pub(crate) fn panel_with(
    buf: &mut Buffer,
    area: Rect,
    title: Vec<Span<'_>>,
    border: Style,
) -> Rect {
    framed(
        buf,
        area,
        title,
        border,
        border.add_modifier(Modifier::BOLD),
        BorderType::Rounded,
    )
}

fn framed(
    buf: &mut Buffer,
    area: Rect,
    title: Vec<Span<'_>>,
    border: Style,
    title_style: Style,
    border_type: BorderType,
) -> Rect {
    let mut spans = vec![Span::raw(" ")];
    for span in title {
        let style = title_style.patch(span.style);
        spans.push(Span::styled(span.content, style));
    }
    spans.push(Span::raw(" "));
    Block::bordered()
        .border_type(border_type)
        .border_style(border)
        .title(Line::from(spans))
        .render(area, buf);
    inner(area)
}

/// Draws a modal window in the manner of the cockpit's: everything drawn so
/// far fades behind it, then the window on the modal's own ground in a thick
/// lavender frame, `title` in the frame at the top left and `footer`, when
/// there is one, in the middle of its bottom. The frame stands a cell
/// further out than `window` on every side it can ([`modal_outer`]), for air
/// around what the window holds, as the cockpit leaves. Returns the area
/// inside `window`'s own edges, where its content goes as before.
pub(crate) fn modal(
    buf: &mut Buffer,
    window: Rect,
    title: Vec<Span<'_>>,
    footer: &str,
    theme: &Theme,
) -> Rect {
    let window = window.intersection(buf.area);
    draw_modal(buf, modal_outer(window, buf.area), title, footer, theme);
    inner(window)
}

/// A modal window whose layout already leaves air inside its frame, such as
/// the keys windows: the frame on `window`'s own edges. Returns the area
/// inside the frame.
pub(crate) fn modal_fitted(
    buf: &mut Buffer,
    window: Rect,
    title: Vec<Span<'_>>,
    footer: &str,
    theme: &Theme,
) -> Rect {
    draw_modal(buf, window.intersection(buf.area), title, footer, theme)
}

/// The frame of a modal `window`: a cell further out on every side, never
/// out of the `screen`, and never over its last row (the status line) when
/// the window was clear of it.
pub(crate) fn modal_outer(window: Rect, screen: Rect) -> Rect {
    let status = screen.bottom().saturating_sub(1);
    let left = if window.x > screen.x {
        window.x - 1
    } else {
        window.x
    };
    let top = if window.y > screen.y {
        window.y - 1
    } else {
        window.y
    };
    let right = if window.right() < screen.right() {
        window.right() + 1
    } else {
        window.right()
    };
    let bottom = if window.bottom() < status {
        window.bottom() + 1
    } else {
        window.bottom()
    };
    Rect::new(left, top, right - left, bottom - top)
}

/// The modal window itself, its frame on `window`'s edges.
fn draw_modal(
    buf: &mut Buffer,
    window: Rect,
    title: Vec<Span<'_>>,
    footer: &str,
    theme: &Theme,
) -> Rect {
    fade(buf, theme);
    Clear.render(window, buf);
    buf.set_style(window, theme.style(StyleRole::Modal));
    let inside = framed(
        buf,
        window,
        title,
        theme.style(StyleRole::ModalBorder),
        theme.style(StyleRole::ModalTitle),
        BorderType::Thick,
    );
    let words = format!(" {footer} ");
    let width = u16::try_from(text::width(&words)).unwrap_or(u16::MAX);
    if !footer.is_empty() && window.height > 1 && width + 4 <= window.width {
        buf.set_stringn(
            window.x + (window.width - width) / 2,
            window.bottom() - 1,
            &words,
            usize::from(width),
            theme.style(StyleRole::ModalTitle),
        );
    }
    inside
}

/// Fades everything drawn so far toward the modal's ground, as the cockpit
/// does under its windows: colours half way, grounds a third of the way, no
/// bold. A colour of the terminal's own palette, with no value to blend, is
/// dimmed by the terminal instead.
fn fade(buf: &mut Buffer, theme: &Theme) {
    let toward = theme.style(StyleRole::Modal).bg;
    let area = buf.area;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let Some(cell) = buf.cell_mut((x, y)) else {
                continue;
            };
            match (cell.fg, toward) {
                (Color::Rgb(..), Some(to @ Color::Rgb(..))) => cell.fg = blend(cell.fg, to, 50),
                _ => cell.modifier.insert(Modifier::DIM),
            }
            if let (Color::Rgb(..), Some(to @ Color::Rgb(..))) = (cell.bg, toward) {
                cell.bg = blend(cell.bg, to, 30);
            }
            cell.modifier.remove(Modifier::BOLD);
        }
    }
}

/// `from` moved `percent` of the way to `to`, for two RGB colours; `from`
/// for any other.
fn blend(from: Color, to: Color, percent: u16) -> Color {
    match (from, to) {
        (Color::Rgb(r, g, b), Color::Rgb(to_r, to_g, to_b)) => {
            let mix = |a: u8, b: u8| {
                u8::try_from((u16::from(a) * (100 - percent) + u16::from(b) * percent) / 100)
                    .unwrap_or(u8::MAX)
            };
            Color::Rgb(mix(r, to_r), mix(g, to_g), mix(b, to_b))
        }
        _ => from,
    }
}

/// Spans cut to `max` cells; the last one kept ends with `…` when cut.
pub(crate) fn truncated<'a>(spans: &[Span<'a>], max: u16) -> Vec<Span<'a>> {
    let mut out = Vec::new();
    let mut left = usize::from(max);
    for span in spans {
        let width = text::width(&span.content);
        if width <= left {
            out.push(span.clone());
            left -= width;
        } else {
            out.push(Span::styled(
                text::truncate(&span.content, left).into_owned(),
                span.style,
            ));
            break;
        }
    }
    out
}

/// Writes a message that stands in for missing content, wrapped to the
/// width of `area`, from its first row, one cell in.
pub(crate) fn placeholder(model: &Model, message: &str, area: Rect, buf: &mut Buffer) {
    put_placeholder(model, message, area, false, buf);
}

/// The same, in the middle of `area`: for a view with nothing else to show.
pub(crate) fn centered_placeholder(model: &Model, message: &str, area: Rect, buf: &mut Buffer) {
    put_placeholder(model, message, area, true, buf);
}

fn put_placeholder(model: &Model, message: &str, area: Rect, centered: bool, buf: &mut Buffer) {
    let area = area.intersection(buf.area);
    if area.width < 3 || area.height == 0 {
        return;
    }
    let rows = text::wrap(message, usize::from(area.width - 2));
    let height = u16::try_from(rows.len()).unwrap_or(u16::MAX);
    let top = if centered {
        area.y + area.height.saturating_sub(height) / 2
    } else {
        area.y
    };
    let style = model.theme.style(StyleRole::Placeholder);
    for ((start, end), y) in rows.into_iter().zip(top..area.bottom()) {
        let row = &message[start..end];
        let x = if centered {
            let used = u16::try_from(text::width(row)).unwrap_or(area.width);
            area.x + area.width.saturating_sub(used) / 2
        } else {
            area.x + 1
        };
        buf.set_stringn(x, y, row, usize::from(area.right() - x), style);
    }
}

/// Writes spans from (`x`, `y`) without going past `max` cells.
pub(crate) fn put_spans(buf: &mut Buffer, x: u16, y: u16, spans: &[Span<'_>], max: u16) -> u16 {
    let mut used = 0;
    for span in spans {
        if used >= max {
            break;
        }
        let (end, _) = buf.set_stringn(
            x + used,
            y,
            &span.content,
            usize::from(max - used),
            span.style,
        );
        used = end - x;
    }
    used
}
