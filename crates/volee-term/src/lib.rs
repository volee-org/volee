//! Terminal emulation for volee ([ADR 0020]).
//!
//! A [`Screen`] is one terminal emulated by libghostty-vt: feed it what a
//! program writes, then read its screen back as VT sequences (a snapshot a
//! late client can replay), as plain text, or as styled cells that
//! [`FrameView`] draws into a ratatui buffer. [`Boundary`] cuts a stream
//! between complete sequences, so that a snapshot never falls inside one.
//!
//! No other volee crate uses libghostty-vt directly.
//!
//! [ADR 0020]: https://volee-org.github.io/volee/adr/0020-libghostty-vt-emulates-terminals.html
#![warn(missing_docs)]

mod boundary;
mod keys;
mod screen;
mod style;
mod widget;

pub use boundary::Boundary;
pub use keys::KeyInput;
pub use screen::{Frame, Screen, Signals, StyledCell};
pub use widget::FrameView;

/// A terminal emulator call failed.
#[derive(Debug, thiserror::Error)]
#[error("terminal emulator: {0}")]
pub struct Error(#[from] libghostty_vt::error::Error);
