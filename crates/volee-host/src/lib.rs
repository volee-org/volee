//! volee-host: one small process per agent that holds its terminal
//! ([ADR 0019]).
//!
//! [`server::run`] starts a program in a pseudo-terminal, keeps its screen in
//! a [`volee_term::Screen`] and serves a Unix socket. Clients connect with
//! [`client::Connection`]: a [`client::Watch`] keeps a live copy of the
//! screen without ever writing or resizing, and [`attach::attach`] connects
//! the person's terminal as the input owner until Ctrl+Q ([ADR 0021]).
//! Hosts live in a [`record::HostDir`], each with a socket and a record.
//!
//! [ADR 0019]: https://volee-org.github.io/volee/adr/0019-a-host-process-holds-each-agent-terminal.html
//! [ADR 0021]: https://volee-org.github.io/volee/adr/0021-take-over-watch-and-the-wall-of-screens.html
#![warn(missing_docs)]

pub mod attach;
pub mod client;
pub mod keys;
pub mod proto;
pub mod record;
pub mod server;

pub use attach::{AttachEnd, attach, attach_leaving, terminal_size};
pub use client::{Connection, HostSender, Update, Watch, snapshot, stop};
pub use proto::{Event, Exit, KeyPress, Role, Size, Welcome};
pub use record::{HostDir, HostRecord};
pub use server::{HostConfig, HostError, run};
