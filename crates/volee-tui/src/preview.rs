//! The preview: an agent's terminal screen, inside volee.
//!
//! Harnesses replay a session's recent output as raw terminal output (for
//! Claude Code, `claude logs`, with cursor addressing and colors), drawn for
//! the session's own terminal, which can be much larger than volee's panel.
//! That output goes through a terminal emulator of the size its cursor
//! addressing and its text tell (at least 200x50), and the resulting cells
//! are drawn, cropped to the panel: the bottom of the screen stays in view,
//! since that is where the latest output and the prompt are.

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use unicode_width::UnicodeWidthChar;
use volee_term::Screen;

use crate::theme::{ColorDepth, StyleRole, Theme};

/// The smallest emulated terminal.
const ROWS: u16 = 50;
const COLUMNS: u16 = 200;

/// The largest emulated terminal.
const MAX_ROWS: u16 = 200;
const MAX_COLUMNS: u16 = 500;

/// Columns between two tab stops.
const TAB_STOP: u16 = 8;

/// The size the output was drawn for, as far as its cursor addressing and
/// the text it writes tell. Replayed in a narrower terminal, its lines wrap
/// and its moves stop at the edges, which turns the screen into a jumble;
/// replayed in a wider one, everything stays in place, since the output
/// places its rows with cursor moves. The text counts as much as the moves:
/// Claude Code pads some rows, such as the lines of a diff, to the full
/// width of its terminal, far past the last column any move names.
fn screen_size(output: &str) -> (u16, u16) {
    let mut extent = Extent {
        rows: ROWS,
        columns: COLUMNS,
        row: 1,
        column: 1,
        saved: (1, 1),
    };
    let mut rest = output;
    loop {
        let start = rest.find('\x1b').unwrap_or(rest.len());
        extent.write(&rest[..start]);
        let Some(sequence) = rest[start..].strip_prefix('\x1b') else {
            break;
        };
        let Some(after) = extent.escape(sequence) else {
            break;
        };
        rest = after;
    }
    (extent.rows.min(MAX_ROWS), extent.columns.min(MAX_COLUMNS))
}

/// What an output tells, so far, of the terminal it was drawn for.
struct Extent {
    rows: u16,
    columns: u16,
    /// The cursor's row and column, from 1, as far as the output lets them
    /// be followed. Line feeds do not move `row`: at the bottom of a
    /// terminal they scroll, and a taller emulator would not.
    row: u16,
    column: u16,
    /// Where the output last saved the cursor (`ESC 7` or `CSI s`).
    saved: (u16, u16),
}

impl Extent {
    /// Text written at the cursor: each character takes its width.
    fn write(&mut self, text: &str) {
        for character in text.chars() {
            match character {
                '\r' => self.column = 1,
                '\t' => {
                    let stop = self.column.saturating_sub(1) / TAB_STOP + 1;
                    self.column = stop.saturating_mul(TAB_STOP).saturating_add(1);
                }
                '\u{8}' => self.column = self.column.saturating_sub(1).max(1),
                character if character.is_control() => {}
                character => {
                    let width = character
                        .width()
                        .and_then(|width| u16::try_from(width).ok())
                        .unwrap_or(0);
                    if width > 0 {
                        self.columns = self.columns.max(self.column.saturating_add(width - 1));
                        self.column = self.column.saturating_add(width);
                    }
                }
            }
        }
    }

    /// Follows the escape sequence `sequence` starts with (after its ESC) and
    /// returns what comes after it; `None` when the output cuts it short.
    fn escape<'a>(&mut self, sequence: &'a str) -> Option<&'a str> {
        let mut characters = sequence.chars();
        match characters.next()? {
            '[' => {
                // Parameter bytes, intermediate bytes, then the command.
                let body = characters.as_str();
                let end = body
                    .find(|c: char| !('\x30'..='\x3f').contains(&c))
                    .unwrap_or(body.len());
                let (parameters, after) = body.split_at(end);
                let after = after.trim_start_matches(|c: char| ('\x20'..='\x2f').contains(&c));
                let command = after.chars().next()?;
                self.control(parameters, command);
                Some(&after[command.len_utf8()..])
            }
            // Strings, such as a window title or a link, end at BEL or ST.
            ']' | 'P' | '_' | '^' | 'X' => {
                let body = characters.as_str();
                let end = body.find(['\x07', '\x1b'])?;
                let terminator = if body[end..].starts_with('\x07') {
                    1
                } else if body[end..].starts_with("\x1b\\") {
                    2
                } else {
                    0
                };
                Some(&body[end + terminator..])
            }
            // A character set designation names its set in one more character.
            '(' | ')' | '*' | '+' | '#' | '%' => {
                characters.next()?;
                Some(characters.as_str())
            }
            '7' => {
                self.saved = (self.row, self.column);
                Some(characters.as_str())
            }
            '8' => {
                (self.row, self.column) = self.saved;
                Some(characters.as_str())
            }
            _ => Some(characters.as_str()),
        }
    }

    /// A control sequence, `ESC [ parameters command`.
    fn control(&mut self, parameters: &str, command: char) {
        if parameters.starts_with(['<', '=', '>', '?']) {
            // Private modes, such as hiding the cursor, move nothing.
            return;
        }
        // A missing parameter, or 0, means 1 for every move.
        let number = |text: &str| -> u16 {
            if text.is_empty() {
                1
            } else {
                text.parse().map_or(u16::MAX, |value: u16| value.max(1))
            }
        };
        let mut numbers = parameters.split(';').map(number);
        let first = numbers.next().unwrap_or(1);
        match command {
            'H' | 'f' => {
                self.row = first;
                self.column = numbers.next().unwrap_or(1);
            }
            'd' => self.row = first,
            'G' | '`' => self.column = first,
            'B' | 'e' => self.row = self.row.saturating_add(first),
            'E' => (self.row, self.column) = (self.row.saturating_add(first), 1),
            'A' => self.row = self.row.saturating_sub(first).max(1),
            'F' => (self.row, self.column) = (self.row.saturating_sub(first).max(1), 1),
            'C' | 'a' => self.column = self.column.saturating_add(first),
            'D' => self.column = self.column.saturating_sub(first).max(1),
            's' if parameters.is_empty() => self.saved = (self.row, self.column),
            'u' if parameters.is_empty() => (self.row, self.column) = self.saved,
            _ => {}
        }
        // Every row the cursor goes to exists. A column counts once a move
        // names it outright, or once text lands there (see `write`).
        self.rows = self.rows.max(self.row);
        if matches!(command, 'H' | 'f' | 'G' | '`') {
            self.columns = self.columns.max(self.column);
        }
    }
}

/// One cell of the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PreviewCell {
    symbol: String,
    style: Style,
    /// The right half of a wide character, drawn by its left half.
    continuation: bool,
}

/// How a screen fits rows wider than the area it is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fit {
    /// Each row on one line, cut at the edge: the preview, a glance at as
    /// many rows as fit.
    Crop,
    /// A row whose text goes past the edge goes on over several lines: the
    /// full-screen view, for reading.
    Wrap,
}

/// Whether a cell holds only blanks or box lines (U+2500 to U+257F), which
/// a row may lose past the edge without losing any of its text.
fn decorative(symbol: &str) -> bool {
    symbol
        .chars()
        .all(|c| c == ' ' || ('\u{2500}'..='\u{257f}').contains(&c))
}

/// A parsed screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PreviewScreen {
    /// Rows of cells, without trailing blanks; a run of blank rows is one.
    rows: Vec<Vec<PreviewCell>>,
    /// The last row worth showing: the cursor's or the last with text.
    anchor: u16,
}

impl PreviewScreen {
    /// Runs the replay of a session's output through the emulator
    /// (libghostty-vt, as everywhere in volee: ADR 0020). A replay is not
    /// the session's screen: its size is told by what it writes, and it
    /// leaves blank what it never redrew.
    pub(crate) fn parse(output: &str, depth: ColorDepth) -> Self {
        let (height, width) = screen_size(output);
        let Ok(mut screen) = Screen::new(width, height, 0) else {
            return Self::default();
        };
        screen.feed(output.as_bytes());
        Self::read(&mut screen, depth, true).unwrap_or_default()
    }

    /// The cells of a screen volee holds exactly (an agent in a volee host,
    /// a view on a session): row for row, as the full-screen view draws
    /// them, so that a view that crops them shows the same cells (ADR
    /// 0028). `None` when the emulator cannot be read.
    pub(crate) fn from_emulator(screen: &mut Screen, depth: ColorDepth) -> Option<Self> {
        Self::read(screen, depth, false)
    }

    /// Reads the emulator's cells. Each row keeps its cells down to the
    /// last one that shows something; of a `replay`, a run of blank rows
    /// is kept as one.
    fn read(screen: &mut Screen, depth: ColorDepth, replay: bool) -> Option<Self> {
        let frame = screen.frame().ok()?;
        let height = frame.rows.len();
        if height == 0 {
            return Some(Self::default());
        }
        let mut parsed: Vec<Vec<PreviewCell>> = Vec::with_capacity(height);
        let mut last_text = 0;
        for (row, drawn) in frame.rows.iter().enumerate() {
            // What a row ends with that shows nothing is left out. A blank in
            // reverse video, underlined or struck shows: it stays.
            let kept = drawn
                .iter()
                .rposition(|cell| cell.continuation || !cell.shows_nothing())
                .map_or(0, |last| last + 1);
            let cells: Vec<PreviewCell> = drawn[..kept]
                .iter()
                .map(|cell| PreviewCell {
                    symbol: if cell.symbol.is_empty() {
                        " ".to_owned()
                    } else {
                        cell.symbol.clone()
                    },
                    style: adapted(cell.style, depth),
                    continuation: cell.continuation,
                })
                .collect();
            if !cells.is_empty() {
                last_text = row;
            }
            parsed.push(cells);
        }
        let (_, cursor_row) = screen.cursor_position();
        let anchor = last_text.max(usize::from(cursor_row)).min(height - 1);
        if !replay {
            return Some(Self {
                rows: parsed,
                anchor: u16::try_from(anchor).unwrap_or(u16::MAX),
            });
        }
        // A replay leaves blank what it never redrew: on a tall Claude Code
        // screen, rows of nothing between the conversation and the status
        // line, where the frame of the prompt was drawn before the replay
        // starts. A run of blank rows is kept as one, so that the end of the
        // conversation stays in view.
        let mut rows: Vec<Vec<PreviewCell>> = Vec::with_capacity(parsed.len());
        let mut kept_anchor = 0;
        for (index, cells) in parsed.into_iter().enumerate() {
            let repeated_blank = cells.is_empty() && rows.last().is_some_and(Vec::is_empty);
            if !repeated_blank {
                rows.push(cells);
            }
            if index == anchor {
                kept_anchor = rows.len().saturating_sub(1);
            }
        }
        Some(Self {
            rows,
            anchor: u16::try_from(kept_anchor).unwrap_or(u16::MAX),
        })
    }

    /// How many lines the screen takes in `width` columns (see
    /// [`Self::lines`]).
    pub(crate) fn line_count(&self, width: u16, fit: Fit) -> u16 {
        u16::try_from(self.lines(width, fit).len()).unwrap_or(u16::MAX)
    }

    /// The lines the screen takes in `width` columns, down to its last row
    /// worth showing, each as a row and a range of its cells. With
    /// [`Fit::Wrap`], a row with text past `width` goes on over as many
    /// lines as it needs, never inside a wide character. A line ends after
    /// its last blank, between two words, when that keeps at least half of
    /// it; otherwise, as for a long path, inside the word, as a terminal
    /// wraps. What a row ends with that is only blanks and box lines, such
    /// as the padding of a diff line or the right side of a frame, does not
    /// count: such a row stays one line, cut at the edge.
    fn lines(&self, width: u16, fit: Fit) -> Vec<(usize, Range<usize>)> {
        let width = usize::from(width.max(1));
        let mut lines = Vec::new();
        let shown = usize::from(self.anchor) + 1;
        for (row, cells) in self.rows.iter().enumerate().take(shown) {
            let text_end = match fit {
                Fit::Crop => 0,
                Fit::Wrap => cells
                    .iter()
                    .rposition(|cell| !decorative(&cell.symbol))
                    .map_or(0, |last| last + 1),
            };
            let (mut start, mut used) = (0, 0);
            // The cell after the line's last blank, and the line's width up
            // to that cell.
            let mut after_blank: Option<(usize, usize)> = None;
            for (position, cell) in cells.iter().enumerate().take(text_end) {
                if cell.continuation {
                    continue;
                }
                let cell_width = crate::text::width(&cell.symbol).max(1);
                if used > 0 && used + cell_width > width {
                    match after_blank {
                        Some((next, before)) if before * 2 >= width => {
                            lines.push((row, start..next));
                            (start, used) = (next, used - before);
                        }
                        _ => {
                            lines.push((row, start..position));
                            (start, used) = (position, 0);
                        }
                    }
                    after_blank = None;
                }
                used += cell_width;
                if cell.symbol == " " {
                    after_blank = Some((position + 1, used));
                }
            }
            lines.push((row, start..cells.len()));
        }
        lines
    }

    /// Draws the screen in `area`, bottom-anchored, `scroll` lines up, rows
    /// wider than the area fitted as `fit` says.
    pub(crate) fn render(
        &self,
        area: Rect,
        buf: &mut Buffer,
        scroll: u16,
        fit: Fit,
        theme: &Theme,
    ) {
        let base = theme.style(StyleRole::Text);
        let lines = self.lines(area.width, fit);
        // The first line stays in sight however far the screen is scrolled.
        let bottom = lines
            .len()
            .saturating_sub(usize::from(scroll))
            .max(1)
            .min(lines.len());
        let top = bottom.saturating_sub(usize::from(area.height));
        for (y, (row, range)) in (area.y..area.bottom()).zip(&lines[top..bottom]) {
            let Some(cells) = self
                .rows
                .get(*row)
                .and_then(|cells| cells.get(range.clone()))
            else {
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

/// A cell's style for a terminal of this color depth: palette colors and
/// default colors stay as they are, so that the person's theme applies, and
/// an RGB color becomes what the terminal can show.
pub(crate) fn adapted(mut style: Style, depth: ColorDepth) -> Style {
    let adapt = |color: Option<Color>| color.map(|color| depth.adapt(color));
    style.fg = adapt(style.fg);
    style.bg = adapt(style.bg);
    style
}

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;

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
        // The nine blank rows above it are kept as one.
        assert_eq!(screen.text()[..2], ["", "    here"]);
        for line in 0..60 {
            write!(output, "\r\nline {line}").unwrap();
        }
        let screen = PreviewScreen::parse(&output, ColorDepth::TrueColor);
        assert_eq!(screen.text()[49], "line 59");
        assert_eq!(screen.anchor, 49);
    }

    #[test]
    fn a_captured_pane_is_drawn_where_its_rows_were() {
        // A multiplexer capture as volee-mux redraws it (ADR 0015): each
        // row placed with a cursor move, then the cursor after the last row
        // with text, as far right as the widest row.
        let wide = "x".repeat(300);
        let output = format!(
            "\x1b[0m\x1b[1;1H{wide}\x1b[0m\x1b[2;1H\x1b[1mClaude Code\x1b[m\
             \x1b[0m\x1b[3;1H> \x1b[0m\x1b[4;1H   \x1b[0m\x1b[3;300H"
        );
        assert_eq!(screen_size(&output), (ROWS, 300));
        let text = screen_text(&output);
        assert_eq!(text, format!("{wide}\nClaude Code\n>"), "no row wraps");
    }

    #[test]
    fn a_larger_terminal_is_replayed_at_its_own_size() {
        // A counter right-aligned on a wide screen, below row 50, then the
        // cursor parked on the prompt.
        let output = "\x1b[H\r\x1b[293C\x1b[70B42\x1b[73;1Hprompt";
        assert_eq!(screen_size(output), (73, 295));
        let screen = PreviewScreen::parse(output, ColorDepth::TrueColor);
        let text = screen.text();
        // The 70 blank rows above the counter are kept as one.
        assert_eq!(text[1], format!("{}42", " ".repeat(293)));
        assert_eq!(text[3], "prompt");
        assert_eq!(screen.anchor, 3);
        assert_eq!(screen_size("\x1b[10;5Hhere\x1b[?25l"), (ROWS, COLUMNS));
        assert_eq!(
            screen_size("\x1b[9999;99999H\x1b["),
            (MAX_ROWS, MAX_COLUMNS)
        );
    }

    #[test]
    fn rows_padded_past_every_move_set_the_width() {
        // How Claude Code replays a diff in a 308-column terminal: each
        // line placed by moves from the top left, then padded with colored
        // blanks to the edge, far past any column a move names. A narrower
        // emulator wraps each line onto the next one.
        let padded = |text: &str| format!("{text}{}", " ".repeat(303 - text.len()));
        let output = format!(
            "\x1b[H\r\x1b[5C\x1b[1B\x1b[48;5;52m{}\x1b[0m\
             \r\x1b[5C\x1b[1B\x1b[48;5;22m{}\x1b[0m\
             \r\x1b[1B  next\x1b[73;1H\x1b[69;3H",
            padded(" 5 -old"),
            padded(" 5 +new"),
        );
        assert_eq!(screen_size(&output), (73, 308));
        let text = PreviewScreen::parse(&output, ColorDepth::TrueColor).text();
        assert_eq!(text[1].trim_end(), "      5 -old");
        assert_eq!(text[2].trim_end(), "      5 +new");
        assert_eq!(text[3], "  next");
    }

    #[test]
    fn runs_of_blank_rows_shrink_to_one() {
        // A tall Claude Code screen as its replay leaves it: the
        // conversation at the top, the status line at the bottom, and blank
        // between them where the frame of the prompt was drawn earlier.
        let output = "\x1b[1;1Hfirst\x1b[3;1Hsecond\x1b[40;1Hlast\x1b[73;1H18%\x1b[69;3H";
        let screen = PreviewScreen::parse(output, ColorDepth::TrueColor);
        assert_eq!(screen.anchor, 6);
        assert_eq!(screen_text(output), "first\n\nsecond\n\nlast\n\n18%");
        let area = Rect::new(0, 0, 6, 3);
        let mut buf = Buffer::empty(area);
        screen.render(area, &mut buf, 0, Fit::Crop, &Theme::default());
        let row = |y: u16| -> String { (0..6).map(|x| buf[(x, y)].symbol().to_owned()).collect() };
        assert_eq!([row(0), row(1), row(2)], ["last  ", "      ", "18%   "]);
    }

    #[test]
    fn rows_wider_than_the_area_go_on_over_several_lines() {
        // A long row, a padded diff row, a wide character at the edge, and
        // the top of a frame wider than the area.
        let output = format!(
            "{}\r\n\x1b[48;5;22mbbb{}\x1b[0m\r\nabcdefghi\u{754c}\r\n\u{256d}{}\u{256e}\r\nend",
            "a".repeat(15),
            " ".repeat(17),
            "\u{2500}".repeat(15),
        );
        let screen = PreviewScreen::parse(&output, ColorDepth::TrueColor);
        assert_eq!(screen.line_count(10, Fit::Wrap), 7);
        assert_eq!(screen.line_count(20, Fit::Wrap), 5);
        assert_eq!(screen.line_count(10, Fit::Crop), 5);
        let area = Rect::new(0, 0, 10, 7);
        let mut buf = Buffer::empty(area);
        screen.render(area, &mut buf, 0, Fit::Wrap, &Theme::default());
        let row = |buf: &Buffer, y: u16| -> String {
            (0..10).map(|x| buf[(x, y)].symbol().to_owned()).collect()
        };
        assert_eq!(row(&buf, 0), "aaaaaaaaaa");
        assert_eq!(row(&buf, 1), "aaaaa     ");
        assert_eq!(row(&buf, 2), "bbb       ");
        assert_eq!(buf[(9, 2)].bg, Color::Indexed(22), "padded up to the edge");
        assert_eq!(row(&buf, 3), "abcdefghi ");
        assert_eq!(buf[(0, 4)].symbol(), "\u{754c}");
        let frame = format!("\u{256d}{}", "\u{2500}".repeat(9));
        assert_eq!(row(&buf, 5), frame, "a frame is cut, not wrapped");
        assert_eq!(row(&buf, 6), "end       ");
        // Cropped, each row takes one line.
        let mut buf = Buffer::empty(area);
        screen.render(area, &mut buf, 0, Fit::Crop, &Theme::default());
        assert_eq!(row(&buf, 0), "aaaaaaaaaa");
        assert_eq!(row(&buf, 1), "bbb       ");
        // Scrolled past the top, the first line stays in sight.
        let mut buf = Buffer::empty(area);
        screen.render(area, &mut buf, 40, Fit::Wrap, &Theme::default());
        assert_eq!(row(&buf, 0), "aaaaaaaaaa");
        assert_eq!(row(&buf, 1), "          ");
    }

    #[test]
    fn wrapped_lines_end_between_words() {
        let screen = PreviewScreen::parse(
            "hello world again\r\na bcdefghijklmnop",
            ColorDepth::TrueColor,
        );
        assert_eq!(screen.line_count(10, Fit::Wrap), 5);
        let area = Rect::new(0, 0, 10, 5);
        let mut buf = Buffer::empty(area);
        screen.render(area, &mut buf, 0, Fit::Wrap, &Theme::default());
        let row = |y: u16| -> String { (0..10).map(|x| buf[(x, y)].symbol().to_owned()).collect() };
        // Between two words when that keeps half a line or more...
        assert_eq!(row(0), "hello     ");
        assert_eq!(row(1), "world     ");
        assert_eq!(row(2), "again     ");
        // ...inside the word otherwise, as a terminal wraps.
        assert_eq!(row(3), "a bcdefghi");
        assert_eq!(row(4), "jklmnop   ");
    }

    #[test]
    fn titles_links_and_private_modes_are_not_text() {
        let title = "t".repeat(400);
        let output = format!(
            "\x1b]0;{title}\x07\x1b[?25l\x1b[?2004h\x1b(B\
             \x1b]8;;https://example.com/{title}\x1b\\link\x1b]8;;\x1b\\> "
        );
        assert_eq!(screen_size(&output), (ROWS, COLUMNS));
        assert_eq!(screen_text(&output), "link>");
    }

    #[test]
    fn relative_moves_and_saved_cursors_are_followed() {
        use std::fmt::Write as _;
        // Seventy rows reached one move down at a time, with no move that
        // names a row: a 50-row emulator would stop the cursor at its
        // bottom and lose the rows past it.
        let mut output = String::from("\x1b[H");
        for row in 0..70 {
            write!(output, "\x1b[1B\rrow{row}").unwrap();
        }
        assert_eq!(screen_size(&output), (71, COLUMNS));
        let text = screen_text(&output);
        assert!(text.contains("row47\nrow48"), "{text}");
        assert!(text.ends_with("row68\nrow69"), "{text}");
        // The cursor saved at column 250, then restored for 100 more columns.
        let saved = format!("\x1b[250G\x1b7\rab\x1b8{}", "x".repeat(100));
        assert_eq!(screen_size(&saved), (ROWS, 349));
        let saved = format!("\x1b[1;250H\x1b[s\r\x1b[u{}", "y".repeat(10));
        assert_eq!(screen_size(&saved), (ROWS, 259));
        // Up and back to the first column.
        assert_eq!(screen_size("\x1b[60;1H\x1b[5Fz\x1b[2A"), (60, COLUMNS));
        // A parameter of 0 moves as 1 does.
        assert_eq!(screen_size("\x1b[1;300H\x1b[0Cx"), (ROWS, 301));
    }

    #[test]
    fn wide_characters_and_tabs_take_their_columns() {
        assert_eq!(screen_size("\x1b[1;250H\u{754c}"), (ROWS, 251));
        let tabbed = format!("{}\tx", "a".repeat(250));
        assert_eq!(screen_size(&tabbed), (ROWS, 257));
        assert_eq!(screen_size("\x1b[1;300H\x1b[10Dx"), (ROWS, 300));
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
        screen.render(area, &mut buf, 0, Fit::Crop, &Theme::default());
        let row = |buf: &Buffer, y: u16| -> String {
            (0..10).map(|x| buf[(x, y)].symbol().to_owned()).collect()
        };
        // The cursor sits on the empty row after "row 19".
        assert_eq!(row(&buf, 0), "row 18    ");
        assert_eq!(row(&buf, 1), "row 19    ");
        let mut buf = Buffer::empty(area);
        screen.render(area, &mut buf, 5, Fit::Crop, &Theme::default());
        assert_eq!(row(&buf, 2), "row 15    ");
    }
}
