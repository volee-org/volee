//! Take an agent over in this terminal; Ctrl+Q comes back (ADR 0021).

use std::io::{self, Write};
use std::os::fd::AsFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::termios::{OptionalActions, Termios, tcgetattr, tcgetwinsize, tcsetattr};

use crate::client::{Connection, HostSender, Update};
use crate::keys;
use crate::proto::{Event, Exit, Role, Size};

/// Saves the person's screen and starts from a blank one.
const ENTER: &[u8] = b"\x1b[?1049h\x1b[H\x1b[2J\x1b[0m";
/// Clears the screen before a snapshot.
const CLEAR: &[u8] = b"\x1b[0m\x1b[H\x1b[2J";
/// Turns off what an agent may have left on (mouse reporting, focus events,
/// bracketed paste, kitty keyboard flags, a hidden cursor), then restores the
/// person's screen.
const LEAVE: &[u8] = b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1015l\x1b[?1004l\
\x1b[?2004l\x1b[=0;1u\x1b[<99u\x1b[?25h\x1b[0m\x1b[?1049l";

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

/// Connects this terminal to the host at `socket` as its input owner, until
/// the person presses Ctrl+Q or the program ends. stdin and stdout must be
/// the person's terminal.
pub fn attach(socket: &Path, client: &str) -> io::Result<AttachEnd> {
    let size = terminal_size().unwrap_or(Size { cols: 80, rows: 24 });
    let mut connection = Connection::open(socket, Role::Control, size, client)?;
    let sender = connection.sender()?;
    let raw = RawMode::enter()?;
    let mut out = io::stdout().lock();
    out.write_all(ENTER)?;
    out.flush()?;

    let stop = Arc::new(AtomicBool::new(false));
    let detached = Arc::new(AtomicBool::new(false));
    let keys_thread = {
        let (stop, detached) = (Arc::clone(&stop), Arc::clone(&detached));
        thread::spawn(move || forward_keys(&sender, &stop, &detached, size))
    };

    let end = relay(&mut connection, &mut out, &detached);
    // Whatever happened, give the terminal back as it was.
    stop.store(true, Ordering::SeqCst);
    if let Ok(sender) = connection.sender() {
        sender.close();
    }
    let _ = keys_thread.join();
    let _ = out.write_all(LEAVE).and_then(|()| out.flush());
    drop(raw);
    end
}

/// Copies the host's screen to the terminal until the take over ends.
fn relay(connection: &mut Connection, out: &mut impl Write, detached: &AtomicBool) -> io::Result<AttachEnd> {
    loop {
        match connection.next() {
            Ok(Some(Update::Snapshot(bytes))) => {
                out.write_all(CLEAR)?;
                out.write_all(&bytes)?;
                out.flush()?;
            }
            Ok(Some(Update::Output(bytes))) => {
                out.write_all(&bytes)?;
                out.flush()?;
            }
            Ok(Some(Update::Event(Event::Exited { exit }))) => return Ok(AttachEnd::Exited(exit)),
            Ok(Some(Update::Event(Event::InputTaken))) => return Ok(AttachEnd::InputTaken),
            Ok(Some(Update::Event(_))) => {}
            Ok(None) | Err(_) => {
                return Ok(if detached.load(Ordering::SeqCst) {
                    AttachEnd::Detached
                } else {
                    AttachEnd::HostGone
                });
            }
        }
    }
}

/// Reads the person's keys and sends them, without Ctrl+Q, and the
/// terminal's size when it changes. Polls so that it stops reading as soon
/// as the take over ends: a thread blocked on stdin would steal the next key
/// from whatever runs after.
fn forward_keys(sender: &HostSender, stop: &AtomicBool, detached: &AtomicBool, mut size: Size) {
    let stdin = io::stdin();
    let mut buf = [0u8; 4096];
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
                    let filtered = keys::filter(&buf[..n]);
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
            && now != size
        {
            size = now;
            if sender.resize(size).is_err() {
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
