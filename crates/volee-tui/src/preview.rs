//! The preview: an agent's terminal screen, inside volee.
//!
//! Harnesses replay a session's recent output as raw terminal output (for
//! Claude Code, `claude logs`, with cursor addressing and colors), drawn for
//! the session's own terminal, which can be much larger than volee's panel.
//! That output goes through a terminal emulator of the size its cursor
//! addressing tells (at least 200x50), and the resulting cells are drawn,
//! cropped to the panel: the bottom of the screen stays in view, since that
//! is where the latest output and the prompt are.

use std::num::NonZeroU16;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::theme::{ColorDepth, StyleRole, Theme};

/// The smallest emulated terminal.
const ROWS: u16 = 50;
const COLUMNS: u16 = 200;

/// The largest emulated terminal.
const MAX_ROWS: u16 = 200;
const MAX_COLUMNS: u16 = 500;

/// Room after a forward move for the short text it places, such as the
/// counters Claude Code right-aligns at the edge of its screen.
const RIGHT_TEXT: u16 = 10;

/// The size the output was drawn for, as far as its cursor addressing
/// tells. Replayed in a smaller terminal, its lines wrap and its moves stop
/// at the edges, which turns the screen into a jumble.
fn screen_size(output: &str) -> (u16, u16) {
    let number = |text: &str| -> u16 {
        if text.is_empty() {
            1
        } else {
            text.parse().unwrap_or(u16::MAX)
        }
    };
    let (mut rows, mut columns) = (ROWS, COLUMNS);
    let mut rest = output;
    while let Some(start) = rest.find("\x1b[") {
        let after = &rest[start + 2..];
        let length = after
            .find(|c: char| !(c.is_ascii_digit() || c == ';'))
            .unwrap_or(after.len());
        let (parameters, tail) = after.split_at(length);
        let Some(command) = tail.chars().next() else {
            break;
        };
        let mut numbers = parameters.split(';').map(number);
        let first = numbers.next().unwrap_or(1);
        match command {
            'H' | 'f' => {
                rows = rows.max(first);
                columns = columns.max(numbers.next().unwrap_or(1));
            }
            'd' => rows = rows.max(first),
            'G' | '`' => columns = columns.max(first),
            'B' | 'E' => rows = rows.max(first.saturating_add(1)),
            'C' => columns = columns.max(first.saturating_add(1 + RIGHT_TEXT)),
            _ => {}
        }
        rest = &tail[command.len_utf8()..];
    }
    (rows.min(MAX_ROWS), columns.min(MAX_COLUMNS))
}

/// One cell of the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PreviewCell {
    symbol: String,
    style: Style,
    /// The right half of a wide character, drawn by its left half.
    continuation: bool,
}

/// A parsed screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PreviewScreen {
    /// Rows of cells, without trailing blanks.
    rows: Vec<Vec<PreviewCell>>,
    /// The last row worth showing: the cursor's or the last with text.
    anchor: u16,
}

impl PreviewScreen {
    /// Runs raw terminal output through the emulator.
    pub(crate) fn parse(output: &str, depth: ColorDepth) -> Self {
        let (height, width) = screen_size(output);
        let (Some(rows), Some(columns)) = (NonZeroU16::new(height), NonZeroU16::new(width)) else {
            return Self::default();
        };
        let mut parser = vt100::Parser::new(rows, columns, 0);
        parser.process(output.as_bytes());
        let screen = parser.screen();
        let mut parsed: Vec<Vec<PreviewCell>> = Vec::with_capacity(usize::from(height));
        let mut last_text = 0;
        for row in 0..height {
            let mut cells = Vec::new();
            for column in 0..width {
                let Some(cell) = screen.cell(row, column) else {
                    break;
                };
                cells.push(PreviewCell {
                    symbol: if cell.has_contents() {
                        cell.contents().to_owned()
                    } else {
                        " ".to_owned()
                    },
                    style: style_of(cell, depth),
                    continuation: cell.is_wide_continuation(),
                });
            }
            while cells
                .last()
                .is_some_and(|cell| cell.symbol == " " && cell.style.bg.is_none())
            {
                cells.pop();
            }
            if !cells.is_empty() {
                last_text = row;
            }
            parsed.push(cells);
        }
        let (cursor_row, _) = screen.cursor_position();
        Self {
            rows: parsed,
            anchor: last_text.max(cursor_row).min(height - 1),
        }
    }

    /// Draws the screen in `area`, bottom-anchored, `scroll` rows up.
    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer, scroll: u16, theme: &Theme) {
        let base = theme.style(StyleRole::Text);
        let bottom = self.anchor.saturating_sub(scroll);
        let top = (bottom + 1).saturating_sub(area.height);
        for (y, row) in (area.y..area.bottom()).zip(top..=bottom) {
            let Some(cells) = self.rows.get(usize::from(row)) else {
                continue;
            };
            for (x, cell) in (area.x..area.right()).zip(cells) {
                if cell.continuation {
                    continue;
                }
                let style = base.patch(cell.style);
                if x + u16::try_from(crate::text::width(&cell.symbol)).unwrap_or(1) > area.right() {
                    break;
                }
                buf.set_stringn(x, y, &cell.symbol, usize::from(area.right() - x), style);
            }
        }
    }

    /// The text of the screen, one string per row.
    pub(crate) fn text(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| {
                row.iter()
                    .filter(|cell| !cell.continuation)
                    .map(|cell| cell.symbol.as_str())
                    .collect::<String>()
            })
            .collect()
    }
}

/// The screen that raw terminal output leaves, as plain text: what was
/// visible last, one line per row, without colors, escape sequences or
/// trailing blanks. `volee logs` prints it: the raw output replays every
/// frame of the agent's interface, and carries sequences that should not
/// reach a terminal unread.
pub fn screen_text(output: &str) -> String {
    let screen = PreviewScreen::parse(output, ColorDepth::TrueColor);
    let mut lines: Vec<String> = screen
        .text()
        .into_iter()
        .take(usize::from(screen.anchor) + 1)
        .map(|line| line.trim_end().to_owned())
        .collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

fn color(color: vt100::Color, depth: ColorDepth) -> Option<Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(index) => Some(Color::Indexed(index)),
        vt100::Color::Rgb(r, g, b) => Some(depth.adapt(Color::Rgb(r, g, b))),
    }
}

fn style_of(cell: &vt100::Cell, depth: ColorDepth) -> Style {
    let mut style = Style {
        fg: color(cell.fgcolor(), depth),
        bg: color(cell.bgcolor(), depth),
        ..Style::default()
    };
    for (on, modifier) in [
        (cell.bold(), Modifier::BOLD),
        (cell.dim(), Modifier::DIM),
        (cell.italic(), Modifier::ITALIC),
        (cell.underline(), Modifier::UNDERLINED),
        (cell.inverse(), Modifier::REVERSED),
    ] {
        if on {
            style = style.add_modifier(modifier);
        }
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_screen_text_is_the_last_frame_without_escapes() {
        // A spinner redrawn in place, a window title, then the prompt.
        let output = "\x1b[2J\x1b[H\x1b[38;5;208m\u{273b} Ideating\x1b[0m\x1b[H\x1b[38;5;208m\
                      \u{2736} Ideating\x1b[0m\x1b[H\x1b[K\u{273b} Done for 3s\r\n\x1b]0;title\x07> ";
        assert_eq!(screen_text(output), "\u{273b} Done for 3s\n>");
        assert_eq!(screen_text(""), "");
    }

    #[test]
    fn escape_sequences_become_styled_cells() {
        let screen = PreviewScreen::parse(
            "\x1b[2J\x1b[H\x1b[1mbold\x1b[0m \x1b[38;5;111mblue\x1b[0m\r\nnext",
            ColorDepth::TrueColor,
        );
        let text = screen.text();
        assert_eq!(text[0], "bold blue");
        assert_eq!(text[1], "next");
        assert_eq!(screen.rows[0][0].style.add_modifier, Modifier::BOLD);
        assert_eq!(screen.rows[0][5].style.fg, Some(Color::Indexed(111)));
        assert_eq!(screen.anchor, 1);
    }

    #[test]
    fn cursor_addressing_and_scrolling_are_honoured() {
        use std::fmt::Write as _;
        let mut output = String::from("\x1b[10;5Hhere");
        let screen = PreviewScreen::parse(&output, ColorDepth::TrueColor);
        assert_eq!(screen.text()[9], "    here");
        for line in 0..60 {
            write!(output, "\r\nline {line}").unwrap();
        }
        let screen = PreviewScreen::parse(&output, ColorDepth::TrueColor);
        assert_eq!(screen.text()[49], "line 59");
        assert_eq!(screen.anchor, 49);
    }

    #[test]
    fn a_larger_terminal_is_replayed_at_its_own_size() {
        // A counter right-aligned on a wide screen, below row 50, then the
        // cursor parked on the prompt.
        let output = "\x1b[H\r\x1b[293C\x1b[70B42\x1b[73;1Hprompt";
        assert_eq!(screen_size(output), (73, 304));
        let screen = PreviewScreen::parse(output, ColorDepth::TrueColor);
        let text = screen.text();
        assert_eq!(text[70], format!("{}42", " ".repeat(293)));
        assert_eq!(text[72], "prompt");
        assert_eq!(screen.anchor, 72);
        assert_eq!(screen_size("\x1b[10;5Hhere\x1b[?25l"), (ROWS, COLUMNS));
        assert_eq!(
            screen_size("\x1b[9999;99999H\x1b["),
            (MAX_ROWS, MAX_COLUMNS)
        );
    }

    #[test]
    fn rgb_colors_follow_the_color_depth() {
        let screen = PreviewScreen::parse("\x1b[38;2;255;0;0mred", ColorDepth::Ansi256);
        assert_eq!(screen.rows[0][0].style.fg, Some(Color::Indexed(196)));
    }

    #[test]
    fn the_bottom_of_the_screen_stays_in_view() {
        use std::fmt::Write as _;
        let mut output = String::new();
        for n in 0..20 {
            write!(output, "row {n}\r\n").unwrap();
        }
        let screen = PreviewScreen::parse(&output, ColorDepth::TrueColor);
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        screen.render(area, &mut buf, 0, &Theme::default());
        let row = |buf: &Buffer, y: u16| -> String {
            (0..10).map(|x| buf[(x, y)].symbol().to_owned()).collect()
        };
        // The cursor sits on the empty row after "row 19".
        assert_eq!(row(&buf, 0), "row 18    ");
        assert_eq!(row(&buf, 1), "row 19    ");
        let mut buf = Buffer::empty(area);
        screen.render(area, &mut buf, 5, &Theme::default());
        assert_eq!(row(&buf, 2), "row 15    ");
    }
}
