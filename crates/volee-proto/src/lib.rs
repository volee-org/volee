//! The protocol between the volee daemon and its clients.
//!
//! The daemon listens on a Unix socket readable only by the current user.
//! Clients send JSON-RPC 2.0 requests, one JSON object per line, and receive
//! responses and notifications the same way (ADR 0003). Line-delimited
//! JSON-RPC is easy to speak from any language and from `nc -U`.
//!
//! - [`rpc`]: the JSON-RPC envelopes and error codes.
//! - [`methods`]: every method, with its parameter and result types.
//! - [`paths`]: where the socket, the database and the logs live.
//! - [`client`]: an async client for the interface, the CLI and the MCP
//!   server.
//! - [`blocking`]: a minimal synchronous client for hook commands, which must
//!   start and answer in milliseconds.

pub mod blocking;
pub mod client;
pub mod methods;
pub mod paths;
pub mod rpc;

/// The protocol version. A client and a daemon with different versions
/// refuse to talk to each other with a clear message.
pub const PROTOCOL_VERSION: u32 = 1;
