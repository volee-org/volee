//! The connections of live screens to volee hosts (ADR 0026): the run loop
//! opens, writes to and closes them; one thread per connection reads the
//! host and sends what it says to the loop as messages.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use tokio::sync::mpsc::UnboundedSender;
use volee_host::{Connection, HostSender, Role, Size};

use crate::app::{Effect, Msg};
use crate::live::LiveInput;

/// Where a connection stands.
#[derive(Debug)]
enum Slot {
    /// Its thread is connecting.
    Opening,
    /// Connected: the loop writes through this.
    Open(HostSender),
    /// Closed before it connected: its thread drops it.
    Closed,
}

/// The connections, by number.
#[derive(Debug, Default)]
pub(crate) struct LiveLinks {
    slots: Arc<Mutex<HashMap<u64, Slot>>>,
}

fn lock(slots: &Mutex<HashMap<u64, Slot>>) -> MutexGuard<'_, HashMap<u64, Slot>> {
    // A thread that panicked holding the lock left the map consistent: every
    // change is one insert or remove.
    slots.lock().unwrap_or_else(PoisonError::into_inner)
}

impl LiveLinks {
    /// Connects to the host at `socket` in a thread: as a watcher, or as the
    /// input owner of edit mode with the panel's `size`.
    pub(crate) fn open(
        &self,
        link: u64,
        socket: PathBuf,
        edit: bool,
        size: Size,
        results: &UnboundedSender<Msg>,
    ) {
        lock(&self.slots).insert(link, Slot::Opening);
        let slots = Arc::clone(&self.slots);
        let results = results.clone();
        thread::spawn(move || serve(link, &socket, edit, size, &slots, &results));
    }

    /// Sends keys, a paste or a size; a connection that is gone says so to
    /// its reader, which tells the loop.
    pub(crate) fn send(&self, link: u64, input: LiveInput) {
        let slots = lock(&self.slots);
        let Some(Slot::Open(sender)) = slots.get(&link) else {
            return;
        };
        let sent = match input {
            LiveInput::Key(press) => sender.key(&press),
            LiveInput::Paste(text) => sender.paste(&text),
            LiveInput::Bytes(bytes) => sender.input(&bytes),
            LiveInput::Resize(size) => sender.resize(size),
        };
        if let Err(error) = sent {
            tracing::debug!(link, %error, "a live screen's host did not take the input");
        }
    }

    /// Closes a connection; its reader ends.
    pub(crate) fn close(&self, link: u64) {
        let mut slots = lock(&self.slots);
        match slots.remove(&link) {
            Some(Slot::Open(sender)) => sender.close(),
            Some(Slot::Opening) => {
                slots.insert(link, Slot::Closed);
            }
            Some(Slot::Closed) | None => {}
        }
    }

    /// Carries out a live screen's effect; returns any other effect.
    pub(crate) fn carry_out(
        &self,
        effect: Effect,
        results: &UnboundedSender<Msg>,
    ) -> Option<Effect> {
        match effect {
            Effect::LiveOpen {
                link,
                socket,
                edit,
                size,
            } => self.open(link, socket, edit, size, results),
            Effect::LiveSend { link, input } => self.send(link, input),
            Effect::LiveClose { link } => self.close(link),
            other => return Some(other),
        }
        None
    }

    /// Closes every connection, when the interface ends.
    pub(crate) fn close_all(&self) {
        let links: Vec<u64> = lock(&self.slots).keys().copied().collect();
        for link in links {
            self.close(link);
        }
    }
}

/// One connection's thread: connects, then reads until the host or the loop
/// closes it.
fn serve(
    link: u64,
    socket: &std::path::Path,
    edit: bool,
    size: Size,
    slots: &Mutex<HashMap<u64, Slot>>,
    results: &UnboundedSender<Msg>,
) {
    let client = format!("volee {} interface", volee_core::VERSION);
    let opened = if edit {
        Connection::open_embedded(socket, size, &client)
    } else {
        Connection::open(socket, Role::Watch, Size { cols: 0, rows: 0 }, &client)
    };
    let mut connection = match opened.and_then(|connection| {
        let sender = connection.sender()?;
        Ok((connection, sender))
    }) {
        Ok((connection, sender)) => {
            let mut slots = lock(slots);
            if matches!(slots.get(&link), Some(Slot::Closed) | None) {
                // Closed while connecting: dropping the connection closes it.
                slots.remove(&link);
                return;
            }
            slots.insert(link, Slot::Open(sender));
            connection
        }
        Err(error) => {
            lock(slots).remove(&link);
            let _ = results.send(Msg::LiveClosed {
                link,
                reason: error.to_string(),
            });
            return;
        }
    };
    let welcome = Box::new(connection.welcome.clone());
    if results.send(Msg::LiveOpened { link, welcome }).is_err() {
        return;
    }
    let reason = loop {
        match connection.next_update() {
            Ok(Some(update)) => {
                if results.send(Msg::LiveUpdate { link, update }).is_err() {
                    return;
                }
            }
            Ok(None) => break "the host closed the connection".to_owned(),
            Err(error) => break error.to_string(),
        }
    };
    lock(slots).remove(&link);
    let _ = results.send(Msg::LiveClosed { link, reason });
}
