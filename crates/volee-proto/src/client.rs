//! An async client for the daemon.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::methods::{EVENTS_APPENDED, Method, SeqEvent};
use crate::rpc::{Id, Incoming, Notification, Outcome, Request, RpcError, Version2};

/// Why a call failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// No daemon listens on the socket.
    #[error("cannot reach the volee daemon at {path}: {source}")]
    Connect {
        /// The socket path.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// Reading or writing the socket failed.
    #[error("the connection to the volee daemon failed: {0}")]
    Io(#[from] io::Error),
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

type Pending = Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Outcome>>>>;

/// A connection to the daemon. Calls may run concurrently; notifications
/// (such as new events after [`Subscribe`](crate::methods::Subscribe)) are
/// delivered on a separate channel.
#[derive(Debug)]
pub struct Client {
    writer: Mutex<OwnedWriteHalf>,
    pending: Pending,
    next_id: AtomicU64,
    notifications: Option<mpsc::UnboundedReceiver<Notification>>,
    reader: JoinHandle<()>,
}

impl Client {
    /// Connects to the daemon socket.
    pub async fn connect(path: &Path) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(path)
            .await
            .map_err(|source| ClientError::Connect {
                path: path.to_owned(),
                source,
            })?;
        let (read, write) = stream.into_split();
        let pending = Pending::default();
        let (sender, receiver) = mpsc::unbounded_channel();
        let reader = tokio::spawn(read_loop(read, Arc::clone(&pending), sender));
        Ok(Self {
            writer: Mutex::new(write),
            pending,
            next_id: AtomicU64::new(1),
            notifications: Some(receiver),
            reader,
        })
    }

    /// Calls a method and waits for its typed result.
    pub async fn call<M: Method>(&self, params: M::Params) -> Result<M::Output, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, sender);
        let request = Request {
            jsonrpc: Version2,
            id: Some(Id::Number(id)),
            method: M::NAME.to_owned(),
            params: serde_json::to_value(params)
                .map_err(|error| ClientError::Protocol(error.to_string()))?,
        };
        let mut line = serde_json::to_vec(&request)
            .map_err(|error| ClientError::Protocol(error.to_string()))?;
        line.push(b'\n');
        {
            let mut writer = self.writer.lock().await;
            writer.write_all(&line).await?;
            writer.flush().await?;
        }
        match receiver.await {
            Ok(Outcome::Result(value)) => serde_json::from_value(value)
                .map_err(|error| ClientError::Protocol(format!("{}: {error}", M::NAME))),
            Ok(Outcome::Error(error)) => Err(ClientError::Rpc(error)),
            Err(_) => Err(ClientError::Closed),
        }
    }

    /// Takes the notification channel. Only the first call returns it.
    pub fn take_notifications(&mut self) -> Option<mpsc::UnboundedReceiver<Notification>> {
        self.notifications.take()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// Decodes an [`EVENTS_APPENDED`] notification.
pub fn appended_event(notification: &Notification) -> Option<SeqEvent> {
    (notification.method == EVENTS_APPENDED)
        .then(|| serde_json::from_value(notification.params.clone()).ok())
        .flatten()
}

async fn read_loop(
    read: OwnedReadHalf,
    pending: Pending,
    notifications: mpsc::UnboundedSender<Notification>,
) {
    let mut reader = BufReader::new(read);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        match serde_json::from_str::<Incoming>(line.trim_end()) {
            Ok(Incoming::Response(response)) => {
                if let Some(Id::Number(id)) = response.id {
                    let sender = pending
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .remove(&id);
                    if let Some(sender) = sender {
                        let _ = sender.send(response.outcome);
                    }
                }
            }
            Ok(Incoming::Notification(notification)) => {
                let _ = notifications.send(notification);
            }
            Err(_) => {
                // A line this client cannot parse: skip it rather than drop
                // the connection, so a newer daemon can add messages.
            }
        }
    }
    // Dropping the senders wakes every pending call with `Closed`.
    pending
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}
