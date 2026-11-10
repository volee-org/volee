//! Take an agent over in this terminal; Ctrl+Q comes back (ADR 0021).
//!
//! With an [`AttachBar`], the last row of the terminal says that it is inside
//! volee, which agent it shows and how to come back; the agent gets the rows
//! above it.

use std::io::{self, Write};
use std::os::fd::AsFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::termios::{OptionalActions, Termios, tcgetattr, tcgetwinsize, tcsetattr};
use volee_term::{Boundary, Screen};

use crate::bar;
pub use crate::bar::AttachBar;
use crate::client::{Connection, HostSender, Update};
use crate::keys;
use crate::proto::{Event, Exit, Role, Size};

/// Saves the person's screen and starts from a blank one.
const ENTER: &[u8] = b"\x1b[?1049h\x1b[H\x1b[2J\x1b[0m";
/// Clears the screen before a snapshot.
const CLEAR: &[u8] = b"\x1b[0m\x1b[H\x1b[2J";
/// Turns off what an agent may have left on, then restores the person's
/// screen: mouse reporting, focus events, bracketed paste, synchronized
/// output, application cursor keys and keypad, the scrolling region, the
/// cursor style, modifyOtherKeys, kitty keyboard flags, an open hyperlink,
/// autowrap, color scheme reports and a hidden cursor.
const LEAVE: &[u8] = b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1015l\x1b[?1004l\
\x1b[?1007l\x1b[?2004l\x1b[?2026l\x1b[?2031l\x1b[?1l\x1b>\x1b[r\x1b[0 q\x1b[>4;0m\
\x1b[=0;1u\x1b[<99u\x1b]8;;\x1b\\\x1b[?7h\x1b[?25h\x1b[0m\x1b[?1049l";

/// How long after its last output an agent still counts as working, on the
/// bar.
const WORKING_FOR: Duration = Duration::from_millis(1500);

/// How a take over ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachEnd {
    /// The person pressed Ctrl+Q.
    Detached,
    /// The program ended.
    Exited(Exit),
    /// Someone took the agent over from another terminal.
    InputTaken,
    /// The host went away.
    HostGone,
}

/// How to take an agent over.
#[derive(Clone, Debug)]
pub struct AttachOptions {
    /// The leave key: Control and this letter (see [`keys::valid_leave`]).
    pub leave: char,
    /// The bar on the last row, if any.
    pub bar: Option<AttachBar>,
}

impl Default for AttachOptions {
    fn default() -> Self {
        Self {
            leave: keys::DEFAULT_LEAVE,
            bar: None,
        }
    }
}

/// Connects this terminal to the host at `socket` as its input owner, until
/// the person presses Ctrl+Q or the program ends. stdin and stdout must be
/// the person's terminal.
pub fn attach(socket: &Path, client: &str) -> io::Result<AttachEnd> {
    attach_with(socket, client, &AttachOptions::default())
}

/// Like [`attach`], leaving with Control and `leave` (a letter, see
/// [`keys::valid_leave`]) instead of Ctrl+Q.
pub fn attach_leaving(socket: &Path, client: &str, leave: char) -> io::Result<AttachEnd> {
    attach_with(
        socket,
        client,
        &AttachOptions {
            leave,
            ..AttachOptions::default()
        },
    )
}

/// Like [`attach`], with a leave key and a bar.
pub fn attach_with(socket: &Path, client: &str, options: &AttachOptions) -> io::Result<AttachEnd> {
    let full = terminal_size().unwrap_or(Size { cols: 80, rows: 24 });
    let bar_on = options.bar.is_some();
    // Raw mode first: connecting takes the input from whoever has it, which
    // must not happen when this is not a terminal.
    let raw = RawMode::enter()?;
    let mut connection = Connection::open(socket, Role::Control, agent_size(full, bar_on), client)?;
    let sender = connection.sender()?;
    let closer = connection.sender()?;
    let mut out = io::stdout().lock();
    out.write_all(ENTER)?;
    out.flush()?;

    let (tx, rx) = mpsc::channel::<Msg>();
    let stop = Arc::new(AtomicBool::new(false));
    let detached = Arc::new(AtomicBool::new(false));
    let keys_thread = {
        let (stop, detached, tx) = (Arc::clone(&stop), Arc::clone(&detached), tx.clone());
        let leave = options.leave;
        thread::spawn(move || forward_keys(&sender, &stop, &detached, full, bar_on, leave, &tx))
    };
    let reader = thread::spawn(move || {
        loop {
            if let Ok(Some(update)) = connection.next_update() {
                if tx.send(Msg::Update(update)).is_err() {
                    return;
                }
            } else {
                let _ = tx.send(Msg::Closed);
                return;
            }
        }
    });

    let mut painter = Painter::new(options, full);
    let end = relay(&rx, &mut out, &mut painter, &detached);
    // Whatever happened, give the terminal back as it was.
    stop.store(true, Ordering::SeqCst);
    closer.close();
    let _ = keys_thread.join();
    let _ = reader.join();
    let _ = out.write_all(LEAVE).and_then(|()| out.flush());
    drop(raw);
    end
}

/// What the relay hears about.
enum Msg {
    /// Something from the host.
    Update(Update),
    /// The connection ended.
    Closed,
    /// The person's terminal changed size.
    Resized(Size),
}

/// The size the agent gets: the terminal, less the bar's row.
fn agent_size(full: Size, bar: bool) -> Size {
    if bar && full.rows >= bar::MIN_ROWS {
        Size {
            cols: full.cols,
            rows: full.rows - 1,
        }
    } else {
        full
    }
}

/// Writes the host's screen to the terminal, and the bar when there is one.
struct Painter {
    leave: char,
    bar: Option<BarState>,
}

/// What drawing the bar needs: the agent's screen as the terminal shows it,
/// to put its cursor and pen back after the bar.
struct BarState {
    bar: AttachBar,
    full: Size,
    screen: Option<Screen>,
    boundary: Boundary,
    last_output: Option<Instant>,
    shown_working: Option<bool>,
}

impl Painter {
    fn new(options: &AttachOptions, full: Size) -> Self {
        Self {
            leave: options.leave,
            bar: options.bar.clone().map(|bar| BarState {
                bar,
                full,
                screen: None,
                boundary: Boundary::new(),
                last_output: None,
                shown_working: None,
            }),
        }
    }

    /// Whether the bar fits: the terminal has room for it.
    fn shows_bar(&self) -> bool {
        self.bar
            .as_ref()
            .is_some_and(|state| state.full.rows >= bar::MIN_ROWS)
    }

    /// A snapshot of the agent's screen, drawn from blank.
    fn snapshot(&mut self, bytes: &[u8], out: &mut impl Write) -> io::Result<()> {
        out.write_all(CLEAR)?;
        if !self.shows_bar() {
            out.write_all(bytes)?;
            return out.flush();
        }
        let leave = self.leave;
        let Some(state) = self.bar.as_mut() else {
            return Ok(());
        };
        let agent = agent_size(state.full, true);
        state.screen = Screen::new(agent.cols, agent.rows, 0).ok();
        state.boundary = Boundary::new();
        out.write_all(&bar::region(agent.rows))?;
        out.write_all(&bar::confine(bytes, agent.rows))?;
        if let Some(screen) = state.screen.as_mut() {
            screen.feed(bytes);
        }
        state.draw(leave, out)?;
        out.flush()
    }

    /// Output of the agent.
    fn output(&mut self, bytes: &[u8], out: &mut impl Write) -> io::Result<()> {
        if !self.shows_bar() {
            out.write_all(bytes)?;
            return out.flush();
        }
        let leave = self.leave;
        let Some(state) = self.bar.as_mut() else {
            return Ok(());
        };
        let agent = agent_size(state.full, true);
        // Only whole sequences, so that the bar never lands inside one.
        let chunk = state.boundary.push(bytes);
        out.write_all(&bar::confine(&chunk, agent.rows))?;
        if let Some(screen) = state.screen.as_mut() {
            screen.feed(&chunk);
        }
        state.last_output = Some(Instant::now());
        let synchronized = state
            .screen
            .as_ref()
            .is_some_and(Screen::in_synchronized_update);
        if !state.boundary.has_pending() && !synchronized {
            state.draw(leave, out)?;
        }
        out.flush()
    }

    /// The terminal changed size: the agent's region and the bar follow.
    fn resized(&mut self, full: Size, out: &mut impl Write) -> io::Result<()> {
        let leave = self.leave;
        let Some(state) = self.bar.as_mut() else {
            return Ok(());
        };
        state.full = full;
        if full.rows < bar::MIN_ROWS {
            out.write_all(b"\x1b[r")?;
            return out.flush();
        }
        let agent = agent_size(full, true);
        if let Some(screen) = state.screen.as_mut() {
            let _ = screen.resize(agent.cols, agent.rows);
        }
        out.write_all(&bar::region(agent.rows))?;
        state.draw(leave, out)?;
        out.flush()
    }

    /// Draws the bar again when the agent stopped or started working.
    fn tick(&mut self, out: &mut impl Write) -> io::Result<()> {
        if !self.shows_bar() {
            return Ok(());
        }
        let leave = self.leave;
        let Some(state) = self.bar.as_mut() else {
            return Ok(());
        };
        if state.shown_working != Some(state.working()) && !state.boundary.has_pending() {
            state.draw(leave, out)?;
            out.flush()?;
        }
        Ok(())
    }
}

impl BarState {
    fn working(&self) -> bool {
        self.last_output
            .is_some_and(|at| at.elapsed() < WORKING_FOR)
    }

    fn draw(&mut self, leave: char, out: &mut impl Write) -> io::Result<()> {
        let working = self.working();
        let line = bar::text(&self.bar, working, leave, self.full.cols);
        let restore = self
            .screen
            .as_mut()
            .and_then(|screen| screen.cursor_restore().ok())
            .unwrap_or_default();
        out.write_all(&bar::draw(&line, self.full.rows, &restore))?;
        self.shown_working = Some(working);
        Ok(())
    }
}

/// Copies the host's screen to the terminal until the take over ends.
fn relay(
    rx: &Receiver<Msg>,
    out: &mut impl Write,
    painter: &mut Painter,
    detached: &AtomicBool,
) -> io::Result<AttachEnd> {
    loop {
        let msg = match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(msg) => msg,
            Err(RecvTimeoutError::Timeout) => {
                painter.tick(out)?;
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => Msg::Closed,
        };
        match msg {
            Msg::Update(Update::Snapshot(bytes)) => painter.snapshot(&bytes, out)?,
            Msg::Update(Update::Output(bytes)) => painter.output(&bytes, out)?,
            Msg::Update(Update::Event(Event::Exited { exit })) => {
                return Ok(AttachEnd::Exited(exit));
            }
            Msg::Update(Update::Event(Event::InputTaken)) => return Ok(AttachEnd::InputTaken),
            Msg::Update(Update::Event(_)) => {}
            Msg::Resized(full) => painter.resized(full, out)?,
            Msg::Closed => {
                return Ok(if detached.load(Ordering::SeqCst) {
                    AttachEnd::Detached
                } else {
                    AttachEnd::HostGone
                });
            }
        }
    }
}

/// Reads the person's keys and sends them, without the leave key, and the
/// terminal's size when it changes. Polls so that it stops reading as soon
/// as the take over ends: a thread blocked on stdin would steal the next key
/// from whatever runs after.
fn forward_keys(
    sender: &HostSender,
    stop: &AtomicBool,
    detached: &AtomicBool,
    mut full: Size,
    bar: bool,
    leave: char,
    tx: &Sender<Msg>,
) {
    let stdin = io::stdin();
    let mut buf = [0u8; 4096];
    let mut filter = keys::KeyFilter::new(leave);
    let timeout = Timespec {
        tv_sec: 0,
        tv_nsec: 50_000_000,
    };
    while !stop.load(Ordering::SeqCst) {
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        if matches!(poll(&mut fds, Some(&timeout)), Ok(n) if n > 0) {
            match rustix::io::read(stdin.as_fd(), &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let filtered = filter.filter(&buf[..n]);
                    if !filtered.forward.is_empty() && sender.input(&filtered.forward).is_err() {
                        break;
                    }
                    if filtered.detach {
                        detached.store(true, Ordering::SeqCst);
                        sender.close();
                        break;
                    }
                }
            }
        }
        if let Some(now) = terminal_size()
            && now != full
        {
            full = now;
            let _ = tx.send(Msg::Resized(full));
            if sender.resize(agent_size(full, bar)).is_err() {
                break;
            }
        }
    }
}

/// The size of the terminal on stdout.
pub fn terminal_size() -> Option<Size> {
    let size = tcgetwinsize(io::stdout()).ok()?;
    (size.ws_col > 0 && size.ws_row > 0).then_some(Size {
        cols: size.ws_col,
        rows: size.ws_row,
    })
}

/// The terminal in raw mode until dropped.
struct RawMode {
    saved: Termios,
}

impl RawMode {
    fn enter() -> io::Result<Self> {
        let stdin = io::stdin();
        let saved = tcgetattr(&stdin)?;
        let mut raw = saved.clone();
        raw.make_raw();
        tcsetattr(&stdin, OptionalActions::Now, &raw)?;
        Ok(Self { saved })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = tcsetattr(io::stdin(), OptionalActions::Now, &self.saved);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn painter(bar: bool) -> Painter {
        let options = AttachOptions {
            leave: 'q',
            bar: bar.then(|| AttachBar {
                name: "billing".into(),
                place: None,
            }),
        };
        Painter::new(&options, Size { cols: 40, rows: 10 })
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn the_agent_gets_every_row_but_the_bar() {
        let full = Size { cols: 80, rows: 24 };
        assert_eq!(agent_size(full, true), Size { cols: 80, rows: 23 });
        assert_eq!(agent_size(full, false), full);
        let tiny = Size { cols: 80, rows: 2 };
        assert_eq!(agent_size(tiny, true), tiny, "no room for a bar");
    }

    #[test]
    fn without_a_bar_the_output_passes_as_it_is() {
        let mut painter = painter(false);
        let mut out = Vec::new();
        painter.output(b"\x1b[rhello", &mut out).unwrap();
        assert_eq!(out, b"\x1b[rhello");
    }

    #[test]
    fn the_bar_follows_the_output_and_puts_the_cursor_back() {
        let mut painter = painter(true);
        let mut out = Vec::new();
        painter.snapshot(b"", &mut out).unwrap();
        let drawn = text(&out);
        assert!(drawn.contains("\x1b[1;9r"), "the agent's region: {drawn:?}");
        assert!(drawn.contains("\x1b[10;1H"), "the bar on the last row");
        assert!(drawn.contains(" volee │ billing │"), "{drawn:?}");
        out.clear();
        // Resetting the region homes the cursor; "ab" then leaves it at the
        // third column of the first row.
        painter.output(b"\x1b[rab", &mut out).unwrap();
        let drawn = text(&out);
        assert!(drawn.starts_with("\x1b[1;9rab"), "{drawn:?}");
        assert!(drawn.contains("▸ working"));
        // The agent's cursor, after "ab" on the first row, comes back.
        assert!(drawn.contains("\x1b[1;3H"), "{drawn:?}");
        // A cut sequence waits; the bar is not drawn inside it.
        out.clear();
        painter.output(b"\x1b[3", &mut out).unwrap();
        assert!(!text(&out).contains("volee"), "{:?}", text(&out));
        painter.output(b"1m", &mut out).unwrap();
        assert!(text(&out).starts_with("\x1b[31m"));
    }

    #[test]
    fn a_resize_moves_the_region_and_the_bar() {
        let mut painter = painter(true);
        let mut out = Vec::new();
        painter.snapshot(b"", &mut out).unwrap();
        out.clear();
        painter
            .resized(Size { cols: 50, rows: 20 }, &mut out)
            .unwrap();
        let drawn = text(&out);
        assert!(
            drawn.contains("\x1b[1;19r") && drawn.contains("\x1b[20;1H"),
            "{drawn:?}"
        );
        out.clear();
        painter
            .resized(Size { cols: 50, rows: 2 }, &mut out)
            .unwrap();
        assert_eq!(text(&out), "\x1b[r", "too small: no bar");
    }
}
