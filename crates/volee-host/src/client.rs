//! Connect to a host: as the input owner, or as a watcher with its own copy
//! of the screen (ADR 0019, ADR 0021).

use std::io;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use volee_term::{Frame, Screen};

use crate::proto::{self, Event, Exit, Hello, Kind, PROTOCOL, Role, Size, Welcome};

/// What a host sent after its welcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Update {
    /// Reset your screen to these VT bytes.
    Snapshot(Vec<u8>),
    /// Bytes the program wrote.
    Output(Vec<u8>),
    /// Something happened.
    Event(Event),
}

/// One connection to a host.
#[derive(Debug)]
pub struct Connection {
    stream: UnixStream,
    /// The host's welcome.
    pub welcome: Welcome,
}

impl Connection {
    /// Connects to the host at `socket` and says hello.
    pub fn open(socket: &Path, role: Role, size: Size, client: &str) -> io::Result<Self> {
        let stream = UnixStream::connect(socket)?;
        let hello = Hello {
            protocol: PROTOCOL,
            role,
            size,
            client: client.to_owned(),
        };
        proto::write_json(&stream, Kind::Hello, &hello)?;
        let frame = proto::read_frame(&stream)?
            .filter(|frame| frame.kind == Kind::Welcome)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "the host did not welcome us"))?;
        let welcome: Welcome = proto::parse(&frame)?;
        if welcome.protocol != PROTOCOL {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "the host speaks protocol {}, this volee speaks {PROTOCOL}",
                    welcome.protocol
                ),
            ));
        }
        Ok(Self { stream, welcome })
    }

    /// The next update; `Ok(None)` when the host closed the connection.
    pub fn next(&mut self) -> io::Result<Option<Update>> {
        loop {
            let Some(frame) = proto::read_frame(&self.stream)? else {
                return Ok(None);
            };
            return Ok(Some(match frame.kind {
                Kind::Snapshot => Update::Snapshot(frame.payload),
                Kind::Output => Update::Output(frame.payload),
                Kind::Event => Update::Event(proto::parse(&frame)?),
                _ => continue,
            }));
        }
    }

    /// A handle that writes to the host from another thread.
    pub fn sender(&self) -> io::Result<HostSender> {
        Ok(HostSender {
            stream: self.stream.try_clone()?,
        })
    }
}

/// Writes to a host: keys, size, stop. Only the input owner's keys and size
/// are heeded.
#[derive(Debug)]
pub struct HostSender {
    stream: UnixStream,
}

impl HostSender {
    /// Sends keys.
    pub fn input(&self, keys: &[u8]) -> io::Result<()> {
        proto::write_frame(&self.stream, Kind::Input, keys)
    }

    /// Sends the terminal's new size.
    pub fn resize(&self, size: Size) -> io::Result<()> {
        proto::write_json(&self.stream, Kind::Resize, &size)
    }

    /// Asks the host to end its program.
    pub fn stop(&self) -> io::Result<()> {
        proto::write_frame(&self.stream, Kind::Stop, &[])
    }

    /// Closes the connection, which ends a blocked [`Connection::next`].
    pub fn close(&self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

/// A watcher: a live copy of a host's screen, kept on the caller's thread
/// (a [`Screen`] is not `Send`), fed by a thread that reads the socket.
#[derive(Debug)]
pub struct Watch {
    welcome: Welcome,
    updates: Receiver<io::Result<Update>>,
    sender: HostSender,
    screen: Option<Screen>,
    size: Size,
    title: Option<String>,
    exit: Option<Exit>,
    closed: bool,
    bells: u64,
}

impl Watch {
    /// Starts watching the host at `socket`.
    pub fn start(socket: &Path, client: &str) -> io::Result<Self> {
        let mut connection = Connection::open(socket, Role::Watch, Size { cols: 0, rows: 0 }, client)?;
        let welcome = connection.welcome.clone();
        let sender = connection.sender()?;
        let (tx, updates) = mpsc::channel();
        thread::spawn(move || {
            loop {
                let update = connection.next();
                let end = !matches!(update, Ok(Some(_)));
                let sent = match update {
                    Ok(Some(update)) => tx.send(Ok(update)),
                    Ok(None) => break,
                    Err(error) => tx.send(Err(error)),
                };
                if end || sent.is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            size: welcome.size,
            title: welcome.title.clone(),
            exit: welcome.exit.clone(),
            welcome,
            updates,
            sender,
            screen: None,
            closed: false,
            bells: 0,
        })
    }

    /// Applies what arrived since the last call, without waiting. Returns
    /// whether the screen or the state changed.
    pub fn pump(&mut self) -> bool {
        let mut changed = false;
        loop {
            match self.updates.try_recv() {
                Ok(Ok(update)) => {
                    changed = true;
                    self.apply(update);
                }
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                    changed |= !self.closed;
                    self.closed = true;
                    return changed;
                }
                Err(TryRecvError::Empty) => return changed,
            }
        }
    }

    fn apply(&mut self, update: Update) {
        match update {
            Update::Snapshot(bytes) => {
                match Screen::new(self.size.cols, self.size.rows, self.welcome.scrollback) {
                    Ok(mut screen) => {
                        screen.feed(&bytes);
                        self.screen = Some(screen);
                    }
                    Err(error) => tracing::warn!(%error, "cannot create a screen"),
                }
            }
            Update::Output(bytes) => {
                if let Some(screen) = &mut self.screen {
                    screen.feed(&bytes);
                    // Queries are the host's to answer.
                    let _ = screen.take_replies();
                }
            }
            Update::Event(Event::Resized { size }) => self.size = size,
            Update::Event(Event::Title { title }) => self.title = Some(title),
            Update::Event(Event::Bell) => self.bells += 1,
            Update::Event(Event::Exited { exit }) => self.exit = Some(exit),
            Update::Event(Event::InputTaken) => {}
        }
    }

    /// The screen as it is now, once the first snapshot arrived.
    pub fn frame(&mut self) -> Option<Frame> {
        self.screen.as_mut().and_then(|screen| screen.frame().ok())
    }

    /// The screen as plain text, once the first snapshot arrived.
    pub fn text(&self) -> Option<String> {
        self.screen.as_ref().and_then(|screen| screen.text().ok())
    }

    /// The host's welcome.
    pub fn welcome(&self) -> &Welcome {
        &self.welcome
    }

    /// The program's size.
    pub fn size(&self) -> Size {
        self.size
    }

    /// The last window title.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// How the program ended, if it did.
    pub fn exit(&self) -> Option<&Exit> {
        self.exit.as_ref()
    }

    /// Whether the host closed the connection.
    pub fn closed(&self) -> bool {
        self.closed
    }

    /// How many times the bell rang since the watch started.
    pub fn bells(&self) -> u64 {
        self.bells
    }

    /// Stops watching.
    pub fn close(&self) {
        self.sender.close();
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.close();
    }
}
