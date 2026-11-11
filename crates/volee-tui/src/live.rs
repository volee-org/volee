//! An agent's live screen in the full-screen view, and the edit mode that
//! types into it (ADR 0026).
//!
//! The interface connects to the agent's volee host: as a watcher while the
//! full-screen view shows the agent, as the input owner in edit mode. The
//! host sends a snapshot, then what the program writes; a terminal emulator
//! keeps the screen here. In edit mode, the agent's terminal takes the size
//! of the panel, so its screen is drawn cell for cell, and every key but the
//! leave key goes to it. Hosts that encode keys themselves get them by name;
//! older hosts get the bytes a terminal would send.

use std::cell::{Ref, RefCell};

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};
use volee_core::AgentId;
use volee_host::{Event as HostEvent, KeyPress, Size, Update, Welcome};
use volee_term::Screen;

use crate::preview::{PreviewScreen, adapted};
use crate::theme::{ColorDepth, StyleRole, Theme};

/// Lines of history kept for scrolling back.
const SCROLLBACK: usize = 2_000;

/// What the run loop sends a host for the live screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LiveInput {
    /// A key, for a host that encodes keys.
    Key(KeyPress),
    /// Pasted text, for a host that encodes keys.
    Paste(String),
    /// Bytes as a terminal sends them, for an older host.
    Bytes(Vec<u8>),
    /// The panel's new size, in edit mode.
    Resize(Size),
}

/// What the person typed in edit mode before the host answered: it goes to
/// the agent, in order, once the host does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TypedAhead {
    /// A key.
    Key(KeyEvent),
    /// Pasted text.
    Paste(String),
}

/// The most keys and pastes kept while a host has not answered: past them,
/// the person is typing into nothing for a long time, and what follows is
/// dropped.
pub(crate) const TYPED_AHEAD_MAX: usize = 1_000;

/// Where a live screen stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// The connection is being opened.
    Connecting,
    /// Connected: updates arrive.
    Live,
    /// The connection ended, and why.
    Closed(String),
}

/// What an update means for the interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The screen changed.
    Drawn,
    /// The program ended, with an error or not.
    Exited {
        /// It ended with a status other than success, or on a signal.
        failed: bool,
    },
    /// Another terminal took the input.
    InputTaken,
}

/// The live screen of an agent in a volee host.
pub(crate) struct Live {
    /// Whose screen.
    pub(crate) agent: AgentId,
    /// Which connection: updates of an older one are dropped.
    pub(crate) link: u64,
    /// The connection owns the input: edit mode.
    pub(crate) editing: bool,
    /// Where the connection stands.
    pub(crate) phase: Phase,
    /// The host encodes keys and pastes.
    pub(crate) keys: bool,
    /// The program ended.
    pub(crate) exited: bool,
    /// The program's terminal size.
    pub(crate) size: Size,
    /// The screen, once the first snapshot arrived: the same emulator as
    /// the host's (ADR 0020), fed the same bytes. Reading its cells updates
    /// the emulator's render state, hence the cell.
    screen: Option<RefCell<Screen>>,
    /// The screen's cells as the wrapping and cropping views draw them,
    /// kept until the screen changes: a wall of screens is drawn again each
    /// time one of them moves.
    cells: RefCell<Option<(ColorDepth, PreviewScreen)>>,
    /// Lines scrolled back into the history.
    pub(crate) scroll: usize,
    /// What the person typed in edit mode while the connection was being
    /// opened. Entering edit mode on a screen the interface follows opens a
    /// new connection, and the screen stays meanwhile: nothing tells the
    /// person to wait, so nothing they type is lost.
    pub(crate) typed_ahead: Vec<TypedAhead>,
}

impl std::fmt::Debug for Live {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Live")
            .field("agent", &self.agent)
            .field("link", &self.link)
            .field("editing", &self.editing)
            .field("phase", &self.phase)
            .field("keys", &self.keys)
            .field("exited", &self.exited)
            .field("size", &self.size)
            .field("screen", &self.screen.is_some())
            .field("typed_ahead", &self.typed_ahead.len())
            .finish_non_exhaustive()
    }
}

impl Live {
    /// A connection under way, to watch or to edit.
    pub(crate) fn new(agent: AgentId, link: u64, editing: bool) -> Self {
        Self {
            agent,
            link,
            editing,
            phase: Phase::Connecting,
            keys: false,
            exited: false,
            size: Size { cols: 0, rows: 0 },
            screen: None,
            cells: RefCell::new(None),
            scroll: 0,
            typed_ahead: Vec::new(),
        }
    }

    /// A new connection to the same agent: the screen stays until the new
    /// snapshot replaces it.
    pub(crate) fn reconnect(&mut self, link: u64, editing: bool) {
        self.link = link;
        self.editing = editing;
        self.phase = Phase::Connecting;
        self.scroll = 0;
        // What was typed for edit mode never reaches an agent the person
        // stopped typing into.
        if !editing {
            self.typed_ahead.clear();
        }
    }

    /// The host welcomed the connection.
    pub(crate) fn opened(&mut self, welcome: &Welcome) {
        self.phase = Phase::Live;
        self.keys = welcome.keys;
        self.size = welcome.size;
        self.exited = welcome.exit.is_some();
        // A program that ended takes no keys.
        self.editing &= welcome.input;
    }

    /// Applies an update from the host.
    pub(crate) fn apply(&mut self, update: Update) -> Outcome {
        if matches!(update, Update::Snapshot(_) | Update::Output(_)) {
            *self.cells.get_mut() = None;
        }
        match update {
            Update::Snapshot(bytes) => {
                self.screen = emulator(self.size).map(|mut screen| {
                    feed(&mut screen, &bytes);
                    RefCell::new(screen)
                });
                self.scroll = 0;
                Outcome::Drawn
            }
            Update::Output(bytes) => {
                if let Some(screen) = &mut self.screen {
                    let screen = screen.get_mut();
                    feed(screen, &bytes);
                    // A view scrolled back stays on its lines; how far back
                    // they are grows as the program writes.
                    if self.scroll > 0 {
                        self.scroll = screen.scrolled_back();
                    }
                }
                Outcome::Drawn
            }
            // A snapshot of the new size follows.
            Update::Event(HostEvent::Resized { size }) => {
                self.size = size;
                Outcome::Drawn
            }
            Update::Event(HostEvent::Exited { exit }) => {
                self.exited = true;
                self.editing = false;
                Outcome::Exited {
                    failed: exit.signal.is_some() || exit.code.is_some_and(|code| code != 0),
                }
            }
            Update::Event(HostEvent::InputTaken) => {
                self.editing = false;
                Outcome::InputTaken
            }
            Update::Event(HostEvent::Title { .. } | HostEvent::Bell) => Outcome::Drawn,
        }
    }

    /// The connection ended.
    pub(crate) fn closed(&mut self, reason: String) {
        self.phase = Phase::Closed(reason);
        self.editing = false;
        self.typed_ahead.clear();
    }

    /// Whether there is a screen to draw.
    pub(crate) fn has_screen(&self) -> bool {
        self.screen.is_some()
    }

    /// Scrolls `lines` up (positive) or down (negative) in the history, and
    /// returns how far back the view now is.
    pub(crate) fn scroll_by(&mut self, lines: isize) -> usize {
        let Some(screen) = &mut self.screen else {
            return 0;
        };
        self.scroll = screen.get_mut().scroll(lines);
        *self.cells.get_mut() = None;
        self.scroll
    }

    /// Back to the newest lines.
    pub(crate) fn scroll_to_bottom(&mut self) {
        if let Some(screen) = &mut self.screen {
            screen.get_mut().scroll_to_bottom();
        }
        *self.cells.get_mut() = None;
        self.scroll = 0;
    }

    /// Whether the program asked for mouse events.
    pub(crate) fn wants_mouse(&self) -> bool {
        self.screen
            .as_ref()
            .is_some_and(|screen| screen.borrow().wants_mouse())
    }

    /// The bytes for a mouse event, encoded the way the program asked;
    /// empty when it did not ask for it.
    pub(crate) fn mouse_bytes(&self, input: &volee_term::MouseInput) -> Vec<u8> {
        self.screen
            .as_ref()
            .and_then(|screen| screen.borrow().encode_mouse(input).ok())
            .unwrap_or_default()
    }

    /// The bytes a terminal sends for a key, for an older host.
    pub(crate) fn legacy_bytes(&self, key: &KeyEvent) -> Vec<u8> {
        let application = self
            .screen
            .as_ref()
            .is_some_and(|screen| screen.borrow().application_cursor());
        legacy_bytes(key, application)
    }

    /// Pasted text as a terminal sends it, for an older host.
    pub(crate) fn legacy_paste(&self, text: &str) -> Vec<u8> {
        let bracketed = self
            .screen
            .as_ref()
            .is_some_and(|screen| screen.borrow().bracketed_paste());
        legacy_paste(text, bracketed)
    }

    /// Whether the screen fits in `area` cell for cell.
    pub(crate) fn fits(&self, area: Rect) -> bool {
        self.size.cols <= area.width && self.size.rows <= area.height
    }

    /// Draws the screen cell for cell at the top left of `area`, cut at its
    /// edges, and returns where the program's cursor is when it shows one
    /// and the view is not scrolled back.
    pub(crate) fn render_exact(
        &self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
    ) -> Option<Position> {
        let frame = self.screen.as_ref()?.borrow_mut().frame().ok()?;
        let base = theme.style(StyleRole::Text);
        let depth = theme.color_depth();
        let right = area.right();
        for (y, row) in (area.y..area.bottom()).zip(&frame.rows) {
            for (x, cell) in (area.x..right).zip(row) {
                // The cell before it draws both halves of a wide character.
                if cell.continuation {
                    continue;
                }
                let symbol = if cell.symbol.is_empty() {
                    " "
                } else {
                    cell.symbol.as_str()
                };
                // A wide character cut by the edge is left out.
                let width = u16::try_from(crate::text::width(symbol)).unwrap_or(1);
                if x + width.max(1) > right {
                    continue;
                }
                let style = base.patch(adapted(cell.style, depth));
                buf.set_stringn(x, y, symbol, usize::from(right - x), style);
            }
        }
        let (column, row) = frame.cursor?;
        let visible = self.scroll == 0 && row < area.height && column < area.width;
        visible.then(|| Position::new(area.x + column, area.y + row))
    }

    /// The screen for the views that crop or wrap it: the preview, a tile
    /// of the wall, and the full-screen view when it does not fit cell for
    /// cell. Read from the emulator once per change of the screen.
    pub(crate) fn preview(&self, theme: &Theme) -> Option<Ref<'_, PreviewScreen>> {
        let depth = theme.color_depth();
        let fresh = self
            .cells
            .borrow()
            .as_ref()
            .is_some_and(|(kept, _)| *kept == depth);
        if !fresh {
            let mut screen = self.screen.as_ref()?.borrow_mut();
            let cells = PreviewScreen::from_emulator(&mut screen, depth)?;
            *self.cells.borrow_mut() = Some((depth, cells));
        }
        Ref::filter_map(self.cells.borrow(), |kept| {
            kept.as_ref().map(|(_, cells)| cells)
        })
        .ok()
    }

    /// The text of the screen, one string per row; for tests.
    #[cfg(test)]
    pub(crate) fn text(&self) -> String {
        self.screen
            .as_ref()
            .and_then(|screen| screen.borrow().text().ok())
            .unwrap_or_default()
    }
}

/// An emulator of `size`, at least one cell, that keeps a history.
fn emulator(size: Size) -> Option<Screen> {
    Screen::new(size.cols.max(1), size.rows.max(1), SCROLLBACK).ok()
}

/// Feeds what the host sent. The host answers the program's queries: what
/// this copy would answer is dropped.
fn feed(screen: &mut Screen, bytes: &[u8]) {
    screen.feed(bytes);
    let _ = screen.take_replies();
    let _ = screen.take_signals();
}

/// A key as the host names it, or `None` for a key it has no name for
/// (a lone modifier, a media key).
pub(crate) fn key_press(key: &KeyEvent) -> Option<KeyPress> {
    let mods = key.modifiers;
    let mut shift = mods.contains(KeyModifiers::SHIFT);
    let name = match key.code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "enter".to_owned(),
        KeyCode::Tab => "tab".to_owned(),
        KeyCode::BackTab => {
            shift = true;
            "tab".to_owned()
        }
        KeyCode::Backspace => "backspace".to_owned(),
        KeyCode::Esc => "escape".to_owned(),
        KeyCode::Up => "up".to_owned(),
        KeyCode::Down => "down".to_owned(),
        KeyCode::Left => "left".to_owned(),
        KeyCode::Right => "right".to_owned(),
        KeyCode::Home => "home".to_owned(),
        KeyCode::End => "end".to_owned(),
        KeyCode::PageUp => "pageup".to_owned(),
        KeyCode::PageDown => "pagedown".to_owned(),
        KeyCode::Insert => "insert".to_owned(),
        KeyCode::Delete => "delete".to_owned(),
        KeyCode::F(number @ 1..=12) => format!("f{number}"),
        _ => return None,
    };
    Some(KeyPress {
        key: name,
        shift,
        alt: mods.contains(KeyModifiers::ALT),
        ctrl: mods.contains(KeyModifiers::CONTROL),
    })
}

/// The bytes a terminal in its usual (legacy) mode sends for a key;
/// `application` is the program's cursor key mode.
pub(crate) fn legacy_bytes(key: &KeyEvent, application: bool) -> Vec<u8> {
    let mods = key.modifiers;
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    let mut bytes = match key.code {
        KeyCode::Char(c) if ctrl => control_byte(c).map_or_else(|| encode_char(c), |b| vec![b]),
        KeyCode::Char(c) => encode_char(c),
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => b"\x7f".to_vec(),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Up => cursor(b'A', application),
        KeyCode::Down => cursor(b'B', application),
        KeyCode::Right => cursor(b'C', application),
        KeyCode::Left => cursor(b'D', application),
        KeyCode::Home => cursor(b'H', application),
        KeyCode::End => cursor(b'F', application),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::F(number) => function_key(number),
        _ => Vec::new(),
    };
    if alt && !bytes.is_empty() {
        bytes.insert(0, 0x1b);
    }
    bytes
}

/// Pasted text as a terminal sends it: in bracketed paste when the program
/// turned it on, with the end marker taken out of the text; otherwise with
/// line feeds as carriage returns, like typed lines.
pub(crate) fn legacy_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let clean = text.replace("\x1b[201~", "");
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend_from_slice(clean.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

fn encode_char(c: char) -> Vec<u8> {
    let mut buf = [0; 4];
    c.encode_utf8(&mut buf).as_bytes().to_vec()
}

/// Control and a character: the C0 byte terminals send.
fn control_byte(c: char) -> Option<u8> {
    let byte = u8::try_from(c.to_ascii_lowercase()).ok()?;
    match byte {
        b'a'..=b'z' => Some(byte - b'a' + 1),
        b' ' | b'@' | b'2' => Some(0),
        b'[' | b'3' => Some(0x1b),
        b'\\' | b'4' => Some(0x1c),
        b']' | b'5' => Some(0x1d),
        b'^' | b'6' => Some(0x1e),
        b'_' | b'-' | b'7' => Some(0x1f),
        b'?' | b'8' => Some(0x7f),
        _ => None,
    }
}

fn cursor(letter: u8, application: bool) -> Vec<u8> {
    vec![0x1b, if application { b'O' } else { b'[' }, letter]
}

fn function_key(number: u8) -> Vec<u8> {
    match number {
        1..=4 => vec![0x1b, b'O', b'P' + (number - 1)],
        5 => b"\x1b[15~".to_vec(),
        6..=10 => format!("\x1b[{}~", number + 11).into_bytes(),
        11 | 12 => format!("\x1b[{}~", number + 12).into_bytes(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn plain(code: KeyCode) -> KeyEvent {
        key(code, KeyModifiers::NONE)
    }

    fn welcome(size: Size, keys: bool) -> Welcome {
        Welcome {
            protocol: volee_host::proto::PROTOCOL,
            id: "h".to_owned(),
            command: vec!["claude".to_owned()],
            cwd: "/tmp".into(),
            child_pid: Some(1),
            size,
            scrollback: 100,
            title: None,
            exit: None,
            input: true,
            keys,
        }
    }

    #[test]
    fn keys_are_named_for_the_host() {
        let press = key_press(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)).expect("key");
        assert_eq!((press.key.as_str(), press.ctrl), ("c", true));
        let press = key_press(&plain(KeyCode::BackTab)).expect("key");
        assert_eq!((press.key.as_str(), press.shift), ("tab", true));
        let press = key_press(&key(KeyCode::Char('A'), KeyModifiers::SHIFT)).expect("key");
        assert_eq!((press.key.as_str(), press.shift), ("A", true));
        assert_eq!(key_press(&plain(KeyCode::F(5))).expect("key").key, "f5");
        assert_eq!(key_press(&plain(KeyCode::Esc)).expect("key").key, "escape");
        assert!(key_press(&plain(KeyCode::CapsLock)).is_none());
    }

    #[test]
    fn older_hosts_get_the_bytes_of_a_terminal() {
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(
            legacy_bytes(&plain(KeyCode::Char('é')), false),
            "é".as_bytes()
        );
        assert_eq!(legacy_bytes(&key(KeyCode::Char('c'), ctrl), false), b"\x03");
        assert_eq!(legacy_bytes(&key(KeyCode::Char('z'), ctrl), false), b"\x1a");
        assert_eq!(legacy_bytes(&key(KeyCode::Char(' '), ctrl), false), b"\x00");
        assert_eq!(
            legacy_bytes(&key(KeyCode::Char('b'), KeyModifiers::ALT), false),
            b"\x1bb"
        );
        assert_eq!(legacy_bytes(&plain(KeyCode::Enter), false), b"\r");
        assert_eq!(legacy_bytes(&plain(KeyCode::Up), false), b"\x1b[A");
        assert_eq!(legacy_bytes(&plain(KeyCode::Up), true), b"\x1bOA");
        assert_eq!(legacy_bytes(&plain(KeyCode::F(1)), false), b"\x1bOP");
        assert_eq!(legacy_bytes(&plain(KeyCode::F(5)), false), b"\x1b[15~");
        assert_eq!(legacy_bytes(&plain(KeyCode::F(10)), false), b"\x1b[21~");
        assert_eq!(legacy_bytes(&plain(KeyCode::F(12)), false), b"\x1b[24~");
        assert_eq!(legacy_bytes(&plain(KeyCode::BackTab), false), b"\x1b[Z");
    }

    #[test]
    fn pastes_are_bracketed_when_the_program_asks() {
        assert_eq!(legacy_paste("a\nb", false), b"a\rb");
        assert_eq!(
            legacy_paste("a\x1b[201~b", true),
            b"\x1b[200~ab\x1b[201~".to_vec()
        );
    }

    #[test]
    fn a_live_screen_follows_snapshots_and_output() {
        let mut live = Live::new(crate::test_support::id(1), 1, true);
        live.opened(&welcome(Size { cols: 20, rows: 4 }, true));
        assert!(live.editing && live.keys);
        assert_eq!(
            live.apply(Update::Snapshot(b"hello".to_vec())),
            Outcome::Drawn
        );
        live.apply(Update::Output(b" world".to_vec()));
        assert!(live.text().contains("hello world"), "{:?}", live.text());

        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 6));
        let area = Rect::new(2, 1, 25, 5);
        assert!(live.fits(area));
        let cursor = live.render_exact(area, &mut buf, &Theme::default());
        assert_eq!(cursor, Some(Position::new(2 + 11, 1)));
        assert_eq!(buf[(2, 1)].symbol(), "h");

        assert_eq!(
            live.apply(Update::Event(HostEvent::InputTaken)),
            Outcome::InputTaken
        );
        assert!(!live.editing);
    }

    #[test]
    fn an_ended_program_takes_no_keys() {
        let mut live = Live::new(crate::test_support::id(1), 1, true);
        let mut ended = welcome(Size { cols: 20, rows: 4 }, true);
        ended.exit = Some(volee_host::Exit {
            code: Some(0),
            signal: None,
            at: 0,
        });
        ended.input = false;
        live.opened(&ended);
        assert!(!live.editing && live.exited);
    }
}
