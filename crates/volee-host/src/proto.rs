//! The host's socket protocol (ADR 0019).
//!
//! A frame is a 4-byte big-endian length, a 1-byte kind and a payload of
//! that length. Control payloads are JSON; screen bytes and keys are raw.
//! The first frame of a client is [`Hello`], the first answer [`Welcome`].

use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The protocol version this build speaks. A newer volee keeps speaking the
/// versions of hosts still running, since a host outlives upgrades.
pub const PROTOCOL: u32 = 1;

/// The largest payload accepted, to refuse garbage early.
const MAX_FRAME: u32 = 64 << 20;

/// What a frame carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// Client to host, JSON [`Hello`].
    Hello = 1,
    /// Host to client, JSON [`Welcome`].
    Welcome = 2,
    /// Host to client, raw VT bytes: reset your screen to this.
    Snapshot = 3,
    /// Host to client, raw bytes the program wrote.
    Output = 4,
    /// Input owner to host, raw keys.
    Input = 5,
    /// Input owner to host, JSON [`Size`].
    Resize = 6,
    /// Host to client, JSON [`Event`].
    Event = 7,
    /// Client to host, empty: end the program.
    Stop = 8,
}

impl Kind {
    fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            1 => Self::Hello,
            2 => Self::Welcome,
            3 => Self::Snapshot,
            4 => Self::Output,
            5 => Self::Input,
            6 => Self::Resize,
            7 => Self::Event,
            8 => Self::Stop,
            _ => return None,
        })
    }
}

/// A frame read from a socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// What it carries.
    pub kind: Kind,
    /// Its payload.
    pub payload: Vec<u8>,
}

/// Writes one frame.
pub fn write_frame(mut writer: impl Write, kind: Kind, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len())
        .ok()
        .filter(|len| *len <= MAX_FRAME)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    let mut header = [0u8; 5];
    header[..4].copy_from_slice(&len.to_be_bytes());
    header[4] = kind as u8;
    writer.write_all(&header)?;
    writer.write_all(payload)?;
    writer.flush()
}

/// Writes one frame with a JSON payload.
pub fn write_json<T: Serialize>(writer: impl Write, kind: Kind, value: &T) -> io::Result<()> {
    let payload = serde_json::to_vec(value).map_err(io::Error::other)?;
    write_frame(writer, kind, &payload)
}

/// Reads one frame; `Ok(None)` at a clean end of stream.
pub fn read_frame(mut reader: impl Read) -> io::Result<Option<Frame>> {
    let mut header = [0u8; 5];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let kind = Kind::from_byte(header[4])
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unknown frame kind"))?;
    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload)?;
    Ok(Some(Frame { kind, payload }))
}

/// Parses a frame's JSON payload.
pub fn parse<T: for<'de> Deserialize<'de>>(frame: &Frame) -> io::Result<T> {
    serde_json::from_slice(&frame.payload)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// A terminal size in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
}

/// What a client wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Read the screen; never write, never resize.
    Watch,
    /// Type into the program; its size is the program's size.
    Control,
}

/// A client's first frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// The protocol version the client speaks.
    pub protocol: u32,
    /// What it wants.
    pub role: Role,
    /// Its terminal size; used only for [`Role::Control`].
    pub size: Size,
    /// Who it is, for logs (`volee 0.8.0 attach`).
    pub client: String,
}

/// How a program ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exit {
    /// Its exit code, when it exited.
    pub code: Option<u32>,
    /// The signal that ended it, when one did.
    pub signal: Option<String>,
    /// When, in seconds since the Unix epoch.
    pub at: u64,
}

/// The host's first frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    /// The protocol version the host speaks.
    pub protocol: u32,
    /// The host's id: the agent's id.
    pub id: String,
    /// The program's command line.
    pub command: Vec<String>,
    /// Its working directory.
    pub cwd: PathBuf,
    /// The program's process id.
    pub child_pid: Option<u32>,
    /// Its current size.
    pub size: Size,
    /// The lines of history the host keeps, so that a watcher keeps as many.
    pub scrollback: usize,
    /// The window title the program set.
    pub title: Option<String>,
    /// How it ended, if it did.
    pub exit: Option<Exit>,
    /// Whether this client got the input.
    pub input: bool,
}

/// Something that happened, sent to every client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The input owner changed the size; a snapshot follows.
    Resized {
        /// The new size.
        size: Size,
    },
    /// The program set its window title.
    Title {
        /// The title.
        title: String,
    },
    /// The program rang the bell.
    Bell,
    /// The program ended.
    Exited {
        /// How.
        exit: Exit,
    },
    /// Another client took the input; this client should return to its
    /// caller.
    InputTaken,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut wire = Vec::new();
        write_frame(&mut wire, Kind::Output, b"\x1b[1mhi").expect("write");
        write_json(&mut wire, Kind::Event, &Event::Bell).expect("write");
        let mut reader = wire.as_slice();
        let first = read_frame(&mut reader).expect("read").expect("frame");
        assert_eq!(first.kind, Kind::Output);
        assert_eq!(first.payload, b"\x1b[1mhi");
        let second = read_frame(&mut reader).expect("read").expect("frame");
        assert_eq!(parse::<Event>(&second).expect("json"), Event::Bell);
        assert!(read_frame(&mut reader).expect("read").is_none());
    }

    #[test]
    fn garbage_is_refused() {
        let wire = [0, 0, 0, 1, 99, 0];
        assert!(read_frame(wire.as_slice()).is_err());
        let huge = [0xff, 0xff, 0xff, 0xff, 4];
        assert!(read_frame(huge.as_slice()).is_err());
    }

    #[test]
    fn events_are_tagged() {
        let json = serde_json::to_string(&Event::Resized {
            size: Size { cols: 80, rows: 24 },
        })
        .expect("json");
        assert_eq!(json, r#"{"event":"resized","size":{"cols":80,"rows":24}}"#);
    }
}
