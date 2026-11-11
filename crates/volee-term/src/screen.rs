//! One emulated terminal: what a program drew, and how to say it again.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use libghostty_vt::Terminal;
use libghostty_vt::fmt::{Format, Formatter, FormatterOptions};
use libghostty_vt::render::{CellIterator, RenderState, RowIterator};
use libghostty_vt::screen::CellWide;
use libghostty_vt::terminal::{
    ConformanceLevel, DeviceAttributeFeature, DeviceAttributes, DeviceType, Mode, Options,
    PrimaryDeviceAttributes, ScrollViewport, SecondaryDeviceAttributes, TertiaryDeviceAttributes,
};

use crate::Error;
use crate::keys::{self, KeyInput};
use crate::mouse::{self, MouseInput};

/// How volee answers a version query (XTVERSION) when it answers for the
/// person's terminal.
const VERSION_REPLY: &str = concat!("volee ", env!("CARGO_PKG_VERSION"));

/// What happened on a screen since the last [`Screen::take_signals`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Signals {
    /// The program rang the bell.
    pub bell: bool,
    /// The program changed the window title.
    pub title_changed: bool,
}

/// A styled cell read back from a screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StyledCell {
    /// The grapheme drawn in the cell; empty for a blank cell or the second
    /// half of a wide character.
    pub symbol: String,
    /// Its style, in ratatui's terms. Palette colors stay indexed so that the
    /// person's own palette applies.
    pub style: ratatui::style::Style,
    /// The cell is the second half of a wide character, drawn by the cell
    /// before it: it shows nothing of its own.
    pub continuation: bool,
}

impl StyledCell {
    /// Whether the cell shows nothing: a blank, with no background and no
    /// attribute that draws on a blank. Reverse video, an underline and a
    /// strike do: an agent's interface draws its cursor as a blank in
    /// reverse video.
    pub fn shows_nothing(&self) -> bool {
        use ratatui::style::Modifier;
        self.symbol.trim().is_empty()
            && self.style.bg.is_none()
            && !self
                .style
                .add_modifier
                .intersects(Modifier::REVERSED | Modifier::UNDERLINED | Modifier::CROSSED_OUT)
    }
}

/// The visible screen at one moment, row by row, with the cursor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    /// Rows of cells, top to bottom.
    pub rows: Vec<Vec<StyledCell>>,
    /// The cursor's column and row, when it is visible.
    pub cursor: Option<(u16, u16)>,
}

impl Frame {
    /// The index of the last row that shows something, if any.
    pub fn last_drawn_row(&self) -> Option<usize> {
        self.rows
            .iter()
            .rposition(|row| row.iter().any(|cell| !cell.shows_nothing()))
    }
}

/// One terminal, emulated by libghostty-vt (ADR 0020).
///
/// Feed it what a program writes; read back its screen as VT sequences
/// ([`Screen::snapshot`]), as plain text, or as styled cells. A `Screen` is
/// neither `Send` nor `Sync`: keep it on one thread.
#[derive(Debug)]
pub struct Screen {
    terminal: Terminal<'static, 'static>,
    render: RenderState<'static>,
    rows: RowIterator<'static>,
    cells: CellIterator<'static>,
    replies: Rc<RefCell<Vec<u8>>>,
    signals: Rc<Cell<Signals>>,
}

impl Screen {
    /// A blank screen of `cols` by `rows` cells that keeps up to
    /// `scrollback` lines of history.
    pub fn new(cols: u16, rows: u16, scrollback: usize) -> Result<Self, Error> {
        let mut terminal = Terminal::new(Options {
            cols: cols.max(1),
            rows: rows.max(1),
            max_scrollback: scrollback,
        })?;
        let replies = Rc::new(RefCell::new(Vec::new()));
        let signals = Rc::new(Cell::new(Signals::default()));
        {
            let replies = Rc::clone(&replies);
            terminal.on_pty_write(move |_, data| replies.borrow_mut().extend_from_slice(data))?;
        }
        {
            let signals = Rc::clone(&signals);
            terminal.on_bell(move |_| {
                let mut now = signals.get();
                now.bell = true;
                signals.set(now);
            })?;
        }
        {
            let signals = Rc::clone(&signals);
            terminal.on_title_changed(move |_| {
                let mut now = signals.get();
                now.title_changed = true;
                signals.set(now);
            })?;
        }
        terminal.on_xtversion(|_| Some(VERSION_REPLY))?;
        terminal.on_device_attributes(|_| Some(device_attributes()))?;
        Ok(Self {
            terminal,
            render: RenderState::new()?,
            rows: RowIterator::new()?,
            cells: CellIterator::new()?,
            replies,
            signals,
        })
    }

    /// Processes bytes the program wrote.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.terminal.vt_write(bytes);
    }

    /// The answers to terminal queries produced since the last call. The
    /// caller decides whether they go to the program (ADR 0019: only while
    /// no person's terminal is connected to answer them).
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut *self.replies.borrow_mut())
    }

    /// The bell and title changes since the last call.
    pub fn take_signals(&mut self) -> Signals {
        self.signals.replace(Signals::default())
    }

    /// Changes the size, reflowing the primary screen.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), Error> {
        Ok(self.terminal.resize(cols.max(1), rows.max(1), 0, 0)?)
    }

    /// Columns and rows.
    pub fn size(&self) -> (u16, u16) {
        (
            self.terminal.cols().unwrap_or(0),
            self.terminal.rows().unwrap_or(0),
        )
    }

    /// The bytes for a key, encoded the way the program asked: keyboard
    /// protocol (legacy or kitty), cursor and keypad modes.
    pub fn encode_key(&self, input: &KeyInput) -> Result<Vec<u8>, Error> {
        keys::encode_key(&self.terminal, input)
    }

    /// The kitty keyboard protocol flags the program turned on (0 when it
    /// uses the usual encoding), for diagnostics.
    pub fn keyboard_flags(&self) -> u8 {
        self.terminal
            .kitty_keyboard_flags()
            .map_or(0, |flags| flags.bits())
    }

    /// The bytes for pasted text: in bracketed paste when the program turned
    /// it on, with control characters made harmless.
    pub fn encode_paste(&self, text: &str) -> Result<Vec<u8>, Error> {
        keys::encode_paste(&self.terminal, text)
    }

    /// The bytes for a mouse event, encoded the way the program asked
    /// (tracking mode and format); empty when it did not ask for that kind
    /// of event, or for no mouse event at all.
    pub fn encode_mouse(&self, input: &MouseInput) -> Result<Vec<u8>, Error> {
        mouse::encode_mouse(&self.terminal, input, self.size())
    }

    /// Whether the program asked for mouse events.
    pub fn wants_mouse(&self) -> bool {
        self.terminal.is_mouse_tracking().unwrap_or(false)
    }

    /// Whether the program asked for application cursor keys (DECCKM).
    pub fn application_cursor(&self) -> bool {
        self.terminal.mode(Mode::DECCKM).unwrap_or(false)
    }

    /// Whether the program turned bracketed paste on.
    pub fn bracketed_paste(&self) -> bool {
        self.terminal.mode(Mode::BRACKETED_PASTE).unwrap_or(false)
    }

    /// Moves the view `lines` lines back into the history (towards the
    /// newest lines when negative) and returns how many lines back it now
    /// is. A view that is scrolled back stays on the same lines while the
    /// program writes. [`Screen::frame`] reads the view; [`Screen::text`]
    /// and [`Screen::snapshot`] do not depend on it.
    pub fn scroll(&mut self, lines: isize) -> usize {
        if lines != 0 {
            // For the emulator, up is negative.
            self.terminal
                .scroll_viewport(ScrollViewport::Delta(lines.saturating_neg()));
        }
        self.scrolled_back()
    }

    /// Brings the view back to the newest lines.
    pub fn scroll_to_bottom(&mut self) {
        self.terminal.scroll_viewport(ScrollViewport::Bottom);
    }

    /// How many lines back in the history the view is; 0 at the newest
    /// lines.
    pub fn scrolled_back(&self) -> usize {
        self.terminal.scrollbar().map_or(0, |bar| {
            let below = bar.total.saturating_sub(bar.offset.saturating_add(bar.len));
            usize::try_from(below).unwrap_or(usize::MAX)
        })
    }

    /// The cursor's column and row on the screen, whether the program
    /// shows it or not.
    pub fn cursor_position(&self) -> (u16, u16) {
        (
            self.terminal.cursor_x().unwrap_or(0),
            self.terminal.cursor_y().unwrap_or(0),
        )
    }

    /// The window title the program set, if any.
    pub fn title(&self) -> Option<String> {
        self.terminal
            .title()
            .ok()
            .filter(|title| !title.is_empty())
            .map(str::to_owned)
    }

    /// Whether the program is in the middle of a synchronized update (DEC
    /// mode 2026): its screen is half drawn and should not be snapshotted.
    pub fn in_synchronized_update(&self) -> bool {
        self.terminal.mode(Mode::SYNC_OUTPUT).unwrap_or(false)
    }

    /// The screen as VT sequences that redraw it on a blank terminal of the
    /// same size: text, colors, cursor, modes and keyboard protocol. It holds
    /// state only, never a query, so replaying it asks nothing.
    pub fn snapshot(&mut self) -> Result<Vec<u8>, Error> {
        let options = FormatterOptions::new()
            .with_format(Format::Vt)
            .with_trim(false)
            .with_unwrap(false)
            .with_style(true)
            .with_hyperlink(true)
            .with_cursor(true)
            .with_modes(true)
            .with_keyboard(true)
            .with_kitty_keyboard(true)
            .with_scrolling_region(true)
            // libghostty-vt 0.2.2 writes tab stops after the cursor, with
            // moves that leave the cursor on the last stop. Agents keep the
            // default stops, so they are left out and the cursor is placed
            // again at the end.
            .with_tabstops(false)
            .with_charsets(true)
            .with_protection(true)
            // The person's terminal keeps its own palette and directory.
            .with_palette(false)
            .with_pwd(false);
        let mut formatter = Formatter::new(&self.terminal, options)?;
        let mut bytes = formatter.format_alloc(None)?.to_vec();
        drop(formatter);
        let (x, y) = (self.terminal.cursor_x()?, self.terminal.cursor_y()?);
        bytes.extend_from_slice(format!("\x1b[{};{}H", y + 1, x + 1).as_bytes());
        if self.terminal.is_cursor_pending_wrap()? {
            // A program that just filled the last column waits to wrap on
            // the next character. Placing the cursor cancels that; printing
            // the last cell again restores it.
            let frame = self.frame()?;
            let cell = frame
                .rows
                .get(usize::from(y))
                .and_then(|row| row.get(usize::from(x)));
            if let Some(cell) = cell {
                bytes.extend_from_slice(&crate::style::to_sgr(cell.style));
                let symbol = if cell.symbol.is_empty() {
                    " "
                } else {
                    &cell.symbol
                };
                // The style stays on: it is most likely the program's pen,
                // since it just drew that cell.
                bytes.extend_from_slice(symbol.as_bytes());
            }
        }
        if !self.terminal.is_cursor_visible()? {
            bytes.extend_from_slice(b"\x1b[?25l");
        }
        Ok(bytes)
    }

    /// The screen as plain text, trailing blanks trimmed.
    pub fn text(&self) -> Result<String, Error> {
        let options = FormatterOptions::new()
            .with_format(Format::Plain)
            .with_trim(true)
            .with_unwrap(false);
        let mut formatter = Formatter::new(&self.terminal, options)?;
        Ok(String::from_utf8_lossy(&formatter.format_alloc(None)?).into_owned())
    }

    /// The visible screen as styled cells.
    pub fn frame(&mut self) -> Result<Frame, Error> {
        let snapshot = self.render.update(&self.terminal)?;
        let cursor = if snapshot.cursor_visible()? {
            snapshot
                .cursor_viewport()?
                .filter(|cursor| !cursor.at_wide_tail)
                .map(|cursor| (cursor.x, cursor.y))
        } else {
            None
        };
        let mut rows = Vec::new();
        let mut row_iteration = self.rows.update(&snapshot)?;
        while let Some(row) = row_iteration.next() {
            let mut cells = Vec::new();
            let mut cell_iteration = self.cells.update(row)?;
            while let Some(cell) = cell_iteration.next() {
                let mut symbol = String::new();
                if cell.graphemes_len()? > 0 {
                    cell.graphemes_utf8(&mut symbol)?;
                }
                // A space and a cell never written look the same.
                if symbol == " " {
                    symbol.clear();
                }
                let style = crate::style::to_ratatui(&cell.style()?);
                let continuation = matches!(cell.raw_cell()?.wide()?, CellWide::SpacerTail);
                cells.push(StyledCell {
                    symbol,
                    style,
                    continuation,
                });
            }
            rows.push(cells);
        }
        Ok(Frame { rows, cursor })
    }
}

/// What volee says it is when it answers a device attributes query: a
/// VT220-class terminal with colors, like most modern terminals.
fn device_attributes() -> DeviceAttributes {
    DeviceAttributes {
        primary: PrimaryDeviceAttributes::new(
            ConformanceLevel::LEVEL_2,
            &[DeviceAttributeFeature::ANSI_COLOR],
        ),
        secondary: SecondaryDeviceAttributes {
            device_type: DeviceType::VT220,
            firmware_version: 1,
            rom_cartridge: 0,
        },
        tertiary: TertiaryDeviceAttributes { unit_id: 0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen_after(cols: u16, rows: u16, bytes: &[u8]) -> Screen {
        let mut screen = Screen::new(cols, rows, 1000).expect("screen");
        screen.feed(bytes);
        screen
    }

    #[test]
    fn a_blank_that_draws_is_not_nothing() {
        // A letter, a plain blank, then blanks in reverse video, underlined,
        // struck and on a colored background: only the plain one shows
        // nothing.
        let mut screen = screen_after(
            10,
            4,
            b"a \x1b[7m \x1b[0m\x1b[4m \x1b[0m\x1b[9m \x1b[0m\x1b[44m \x1b[0m",
        );
        let frame = screen.frame().expect("frame");
        let shows: Vec<bool> = frame.rows[0][..7]
            .iter()
            .map(|cell| !cell.shows_nothing())
            .collect();
        assert_eq!(shows, [true, false, true, true, true, true, false]);
    }

    #[test]
    fn a_row_that_holds_only_a_cursor_block_is_drawn() {
        // An agent's interface draws its cursor as a blank in reverse video:
        // alone on the last row, it is still the last row drawn.
        let mut screen = screen_after(10, 6, b"text\r\n\r\n\x1b[7m \x1b[27m");
        let frame = screen.frame().expect("frame");
        assert_eq!(frame.last_drawn_row(), Some(2));
        let mut screen = screen_after(10, 6, b"text\r\n\r\n ");
        assert_eq!(screen.frame().expect("frame").last_drawn_row(), Some(0));
    }

    /// Replays a snapshot on a blank screen of the same size.
    fn replay(screen: &mut Screen) -> Screen {
        let (cols, rows) = screen.size();
        screen_after(cols, rows, &screen.snapshot().expect("snapshot"))
    }

    #[test]
    fn a_snapshot_redraws_the_same_screen() {
        let mut original = screen_after(
            40,
            6,
            b"\x1b[1;31mred bold\x1b[0m plain\r\n\x1b[44mblue bg\x1b[0m\r\n\x1b[3;10Hhere",
        );
        let mut copy = replay(&mut original);
        assert_eq!(copy.text().expect("text"), original.text().expect("text"));
        assert_same_frames(
            &copy.frame().expect("frame"),
            &original.frame().expect("frame"),
        );
    }

    fn assert_same_frames(copy: &Frame, original: &Frame) {
        assert_eq!(copy.cursor, original.cursor, "cursor");
        assert_eq!(copy.rows.len(), original.rows.len(), "rows");
        for (y, (a, b)) in copy.rows.iter().zip(&original.rows).enumerate() {
            for (x, (a, b)) in a.iter().zip(b).enumerate() {
                assert_eq!(a, b, "cell at column {x}, row {y}");
            }
        }
    }

    #[test]
    fn a_snapshot_keeps_the_cursor_and_modes() {
        // Bracketed paste and the kitty keyboard protocol switched on.
        let mut original = screen_after(30, 5, b"\x1b[?2004h\x1b[>1u\x1b[2;7Hx");
        let copy = replay(&mut original);
        assert!(copy.terminal.mode(Mode::BRACKETED_PASTE).expect("mode"));
        assert_eq!(
            copy.terminal.cursor_x().ok(),
            original.terminal.cursor_x().ok()
        );
        assert_eq!(
            copy.terminal.cursor_y().ok(),
            original.terminal.cursor_y().ok()
        );
        assert_eq!(
            copy.terminal.kitty_keyboard_flags().ok(),
            original.terminal.kitty_keyboard_flags().ok()
        );
    }

    #[test]
    fn a_snapshot_after_scrolling_shows_the_visible_rows() {
        let mut bytes = Vec::new();
        for line in 0..30 {
            bytes.extend_from_slice(format!("line {line}\r\n").as_bytes());
        }
        bytes.extend_from_slice(b"prompt> ");
        let mut original = screen_after(20, 5, &bytes);
        let mut copy = replay(&mut original);
        let visible = |screen: &mut Screen| {
            screen
                .frame()
                .expect("frame")
                .rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| cell.symbol.as_str())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(visible(&mut copy), visible(&mut original));
        assert_eq!(
            copy.frame().expect("frame").cursor,
            original.frame().expect("frame").cursor
        );
    }

    #[test]
    fn a_snapshot_keeps_a_pending_wrap() {
        // The last column is filled: the next character goes to a new line.
        let mut original = screen_after(10, 3, b"\x1b[33m0123456789");
        let mut copy = replay(&mut original);
        original.feed(b"X");
        copy.feed(b"X");
        assert_eq!(copy.text().expect("text"), original.text().expect("text"));
        assert_eq!(
            copy.frame().expect("frame"),
            original.frame().expect("frame")
        );
    }

    #[test]
    fn a_snapshot_holds_no_query() {
        let mut original = screen_after(20, 5, b"\x1b[c\x1b[6n\x1b[>q");
        assert!(!original.take_replies().is_empty(), "queries were answered");
        let mut copy = replay(&mut original);
        assert!(copy.take_replies().is_empty(), "the replay asked something");
    }

    #[test]
    fn queries_are_answered() {
        let mut screen = screen_after(20, 5, b"\x1b[3;4H\x1b[6n");
        assert_eq!(screen.take_replies(), b"\x1b[3;4R");
        screen.feed(b"\x1b[>q");
        let reply = String::from_utf8(screen.take_replies()).expect("utf8");
        assert!(reply.contains("volee"), "{reply:?}");
        screen.feed(b"\x1b[c");
        assert!(screen.take_replies().starts_with(b"\x1b[?62"));
    }

    #[test]
    fn bell_title_and_synchronized_updates_are_seen() {
        let mut screen = screen_after(20, 5, b"\x07\x1b]0;busy\x07");
        assert_eq!(
            screen.take_signals(),
            Signals {
                bell: true,
                title_changed: true
            }
        );
        assert_eq!(screen.title().as_deref(), Some("busy"));
        assert!(!screen.in_synchronized_update());
        screen.feed(b"\x1b[?2026h");
        assert!(screen.in_synchronized_update());
        screen.feed(b"\x1b[?2026l");
        assert!(!screen.in_synchronized_update());
    }

    #[test]
    fn palette_colors_stay_indexed() {
        let mut screen = screen_after(10, 2, b"\x1b[38;5;208mo\x1b[38;2;1;2;3mr");
        let frame = screen.frame().expect("frame");
        assert_eq!(
            frame.rows[0][0].style.fg,
            Some(ratatui::style::Color::Indexed(208))
        );
        assert_eq!(
            frame.rows[0][1].style.fg,
            Some(ratatui::style::Color::Rgb(1, 2, 3))
        );
    }

    #[test]
    fn a_wide_character_takes_two_cells_and_draws_once() {
        let mut screen = screen_after(10, 2, "a\u{4e2d}b".as_bytes());
        let frame = screen.frame().expect("frame");
        let row = &frame.rows[0];
        let cells: Vec<(&str, bool)> = row[..4]
            .iter()
            .map(|cell| (cell.symbol.as_str(), cell.continuation))
            .collect();
        assert_eq!(
            cells,
            [("a", false), ("\u{4e2d}", false), ("", true), ("b", false)]
        );
        // A blank cell is not the half of anything.
        assert!(!row[5].continuation);
    }

    /// The first line of the view, trimmed.
    fn top_line(screen: &mut Screen) -> String {
        let frame = screen.frame().expect("frame");
        let line: String = frame.rows[0]
            .iter()
            .map(|cell| {
                if cell.symbol.is_empty() {
                    " "
                } else {
                    cell.symbol.as_str()
                }
            })
            .collect();
        line.trim().to_owned()
    }

    #[test]
    fn the_view_scrolls_back_and_stays_on_its_lines() {
        let mut screen = Screen::new(20, 4, 1000).expect("screen");
        for line in 1..=20 {
            screen.feed(format!("line {line}\r\n").as_bytes());
        }
        assert_eq!(screen.scrolled_back(), 0);
        assert_eq!(top_line(&mut screen), "line 18");
        assert_eq!(screen.scroll(5), 5);
        assert_eq!(top_line(&mut screen), "line 13");
        // The frame reads the view too.
        let frame = screen.frame().expect("frame");
        assert_eq!(frame.rows[0][5].symbol, "1");
        assert_eq!(frame.rows[0][6].symbol, "3");
        // What the program writes meanwhile does not move the view.
        screen.feed(b"line 21\r\nline 22\r\n");
        assert_eq!(top_line(&mut screen), "line 13");
        assert_eq!(screen.scrolled_back(), 7);
        // Forward again, and never past either end.
        assert_eq!(screen.scroll(-3), 4);
        assert_eq!(screen.scroll(-100), 0);
        assert_eq!(top_line(&mut screen), "line 20");
        let all = screen.scroll(1_000);
        assert_eq!(top_line(&mut screen), "line 1");
        assert_eq!(screen.scroll(1), all, "already at the first line");
        screen.scroll_to_bottom();
        assert_eq!(screen.scrolled_back(), 0);
    }

    fn mouse(kind: crate::MouseKind, column: u16, row: u16) -> MouseInput {
        MouseInput {
            kind,
            column,
            row,
            shift: false,
            alt: false,
            ctrl: false,
        }
    }

    #[test]
    fn the_mouse_is_encoded_only_as_the_program_asked() {
        use crate::{MouseButton, MouseKind};
        let press = mouse(MouseKind::Press(MouseButton::Left), 4, 2);
        // A program that did not ask gets nothing.
        let quiet = screen_after(40, 10, b"hello");
        assert!(!quiet.wants_mouse());
        assert_eq!(quiet.encode_mouse(&press).expect("encode"), b"");

        // Clicks and the wheel, in the SGR format.
        let clicks = screen_after(40, 10, b"\x1b[?1000h\x1b[?1006h");
        assert!(clicks.wants_mouse());
        let encode = |screen: &Screen, input: MouseInput| {
            String::from_utf8(screen.encode_mouse(&input).expect("encode")).expect("text")
        };
        assert_eq!(encode(&clicks, press), "\x1b[<0;5;3M");
        let release = mouse(MouseKind::Release(MouseButton::Left), 4, 2);
        assert_eq!(encode(&clicks, release), "\x1b[<0;5;3m");
        let right = mouse(MouseKind::Press(MouseButton::Right), 0, 0);
        assert_eq!(encode(&clicks, right), "\x1b[<2;1;1M");
        assert_eq!(
            encode(&clicks, mouse(MouseKind::WheelUp, 4, 2)),
            "\x1b[<64;5;3M"
        );
        assert_eq!(
            encode(&clicks, mouse(MouseKind::WheelDown, 4, 2)),
            "\x1b[<65;5;3M"
        );
        // It asked for clicks, not for moves.
        let drag = mouse(MouseKind::Drag(MouseButton::Left), 5, 2);
        assert_eq!(encode(&clicks, drag), "");

        // Moves with a button held, once it asks for them.
        let drags = screen_after(40, 10, b"\x1b[?1002h\x1b[?1006h");
        assert_eq!(encode(&drags, drag), "\x1b[<32;6;3M");
        assert_eq!(encode(&drags, mouse(MouseKind::Move, 5, 2)), "");
    }

    #[test]
    fn modifiers_are_part_of_a_mouse_event() {
        use crate::{MouseButton, MouseKind};
        let screen = screen_after(40, 10, b"\x1b[?1000h\x1b[?1006h");
        let input = MouseInput {
            ctrl: true,
            ..mouse(MouseKind::Press(MouseButton::Left), 0, 0)
        };
        assert_eq!(
            screen.encode_mouse(&input).expect("encode"),
            b"\x1b[<16;1;1M"
        );
    }

    #[test]
    fn a_snapshot_keeps_the_mouse_and_cursor_key_modes() {
        let mut original = screen_after(30, 5, b"\x1b[?1000h\x1b[?1006h\x1b[?1h\x1b[?2004h");
        assert!(original.application_cursor() && original.bracketed_paste());
        let copy = replay(&mut original);
        assert!(
            copy.wants_mouse(),
            "the copy encodes the mouse as the original"
        );
        assert!(copy.application_cursor());
        assert!(copy.bracketed_paste());
        let plain = screen_after(30, 5, b"hello");
        assert!(!plain.application_cursor() && !plain.bracketed_paste());
    }
}
