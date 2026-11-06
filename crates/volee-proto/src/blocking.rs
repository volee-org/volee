//! A minimal synchronous client.
//!
//! Hook commands run dozens of times per agent turn. They must start, send
//! one request and exit in milliseconds, without an async runtime. When the
//! daemon is unreachable they fall back to the spool, so connection errors
//! are reported distinctly.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::methods::Method;
use crate::rpc::{Id, Incoming, Outcome, Request, RpcError, Version2};

/// Why a blocking call failed.
#[derive(Debug, thiserror::Error)]
pub enum BlockingError {
    /// No daemon listens on the socket; callers may spool instead.
    #[error("the volee daemon is not reachable: {0}")]
    Unreachable(#[source] io::Error),
    /// The daemon did not answer in time.
    #[error("the volee daemon did not answer in time")]
    Timeout,
    /// Reading or writing the socket failed.
    #[error("the connection to the volee daemon failed: {0}")]
    Io(#[source] io::Error),
    /// The daemon closed the connection before answering.
    #[error("the volee daemon closed the connection")]
    Closed,
    /// The daemon sent something this client does not understand.
    #[error("unexpected data from the volee daemon: {0}")]
    Protocol(String),
    /// The daemon answered with an error.
    #[error(transparent)]
    Rpc(#[from] RpcError),
}

/// Sends one request and waits at most `timeout` for its response.
pub fn call<M: Method>(
    socket: &Path,
    params: M::Params,
    timeout: Duration,
) -> Result<M::Output, BlockingError> {
    let stream = UnixStream::connect(socket).map_err(BlockingError::Unreachable)?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(BlockingError::Io)?;
    let request = Request {
        jsonrpc: Version2,
        id: Some(Id::Number(1)),
        method: M::NAME.to_owned(),
        params: serde_json::to_value(params)
            .map_err(|error| BlockingError::Protocol(error.to_string()))?,
    };
    let mut line =
        serde_json::to_vec(&request).map_err(|error| BlockingError::Protocol(error.to_string()))?;
    line.push(b'\n');
    (&stream).write_all(&line).map_err(io_error)?;

    let mut reader = BufReader::new(&stream);
    let mut buffer = String::new();
    loop {
        buffer.clear();
        if reader.read_line(&mut buffer).map_err(io_error)? == 0 {
            return Err(BlockingError::Closed);
        }
        let Ok(Incoming::Response(response)) = serde_json::from_str::<Incoming>(buffer.trim_end())
        else {
            // Notifications or unknown lines are not for this call.
            continue;
        };
        if response.id != Some(Id::Number(1)) {
            continue;
        }
        return match response.outcome {
            Outcome::Result(value) => serde_json::from_value(value)
                .map_err(|error| BlockingError::Protocol(format!("{}: {error}", M::NAME))),
            Outcome::Error(error) => Err(BlockingError::Rpc(error)),
        };
    }
}

fn io_error(error: io::Error) -> BlockingError {
    match error.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => BlockingError::Timeout,
        _ => BlockingError::Io(error),
    }
}
