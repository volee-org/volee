//! The daemon's state on the client side (ADR 0003).
//!
//! Clients do not ask the daemon for the list of agents: they apply its
//! events to their own [`Registry`]. [`subscribe`] gives the events so far,
//! then every new event, without a gap or a repeat: the daemon takes the
//! backlog and the subscription under one lock and answers with the backlog
//! before it sends any new event, and the feed drops anything at or below
//! the last sequence number it handed out.
//!
//! [`Replica`] applies a feed to a registry that callers read at any time;
//! the MCP server answers its tools from it. The interface follows a feed
//! directly, because it needs the events themselves as well as the registry,
//! and subscribes with [`subscribe_interactive`]: while a person watches,
//! the daemon looks for new sessions every few seconds instead of rarely.

use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::{RwLock, RwLockReadGuard, mpsc};
use tokio::task::JoinHandle;
use volee_core::registry::ApplyError;
use volee_core::{Event, Registry};

use crate::client::{Client, ClientError};
use crate::methods::{
    EVENTS_APPENDED, EVENTS_LAGGED, Lagged, SeqEvent, Subscribe, SubscribeParams,
};
use crate::rpc::Notification;

/// Why a replica could not start.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReplicaError {
    /// Talking to the daemon failed.
    #[error(transparent)]
    Client(#[from] ClientError),
    /// The daemon's events did not replay.
    #[error("the daemon sent events that do not fit together: {0}")]
    Registry(#[from] ApplyError),
    /// The client's notifications were already taken.
    #[error("this daemon connection already has a subscriber")]
    AlreadySubscribed,
}

/// Why a feed stopped.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FeedEnd {
    /// The connection to the daemon closed.
    #[error("the connection to the volee daemon closed")]
    Closed,
    /// The daemon dropped the subscription because it fell too far behind;
    /// [`Updates::resume`] picks it up again.
    #[error("the volee daemon dropped this subscription after it fell {skipped} events behind")]
    Lagged {
        /// How many events it missed.
        skipped: u64,
    },
    /// An event could not be read, so applying the next ones would be wrong.
    #[error("the volee daemon sent an event this version cannot read: {0}")]
    Unreadable(String),
}

/// Subscribes `client` to the daemon's events: the events so far, and the
/// events that follow. For programs, such as an agent's MCP server.
///
/// It takes the client's notifications, so a client has at most one feed;
/// calls on the client keep working.
pub async fn subscribe(client: &mut Client) -> Result<Feed, ReplicaError> {
    follow(client, false).await
}

/// Like [`subscribe`], for a client a person watches, such as the
/// interface: the daemon looks for new sessions at once, then every few
/// seconds until the feed's connection closes.
pub async fn subscribe_interactive(client: &mut Client) -> Result<Feed, ReplicaError> {
    follow(client, true).await
}

async fn follow(client: &mut Client, interactive: bool) -> Result<Feed, ReplicaError> {
    let notifications = client
        .take_notifications()
        .ok_or(ReplicaError::AlreadySubscribed)?;
    let batch = client
        .call::<Subscribe>(SubscribeParams {
            after: None,
            interactive,
        })
        .await?;
    let last_seq = batch.last_seq.unwrap_or(0);
    let backlog = batch.events.into_iter().map(|e| e.event).collect();
    Ok(Feed {
        backlog,
        updates: Updates {
            notifications,
            pending: VecDeque::new(),
            last_seq,
            end: None,
            interactive,
        },
    })
}

/// The daemon's events, as a subscription delivers them.
#[derive(Debug)]
pub struct Feed {
    /// The events so far, oldest first.
    pub backlog: Vec<Event>,
    /// Every later event.
    pub updates: Updates,
}

/// The events that follow a feed's backlog, in log order.
#[derive(Debug)]
pub struct Updates {
    notifications: mpsc::UnboundedReceiver<Notification>,
    /// Events fetched by [`Updates::resume`], served before notifications.
    pending: VecDeque<SeqEvent>,
    last_seq: u64,
    end: Option<FeedEnd>,
    /// Whether the subscription is interactive, which a resumed one keeps.
    interactive: bool,
}

impl Updates {
    /// The next event. Once the feed has stopped, every call fails with the
    /// same reason, until [`Updates::resume`] picks it up again.
    pub async fn next(&mut self) -> Result<Event, FeedEnd> {
        if let Some(end) = &self.end {
            return Err(end.clone());
        }
        while let Some(seq_event) = self.pending.pop_front() {
            if let Some(event) = self.accept(seq_event) {
                return Ok(event);
            }
        }
        loop {
            let Some(notification) = self.notifications.recv().await else {
                return Err(self.stop(FeedEnd::Closed));
            };
            match notification.method.as_str() {
                EVENTS_APPENDED => match serde_json::from_value::<SeqEvent>(notification.params) {
                    Ok(seq_event) => {
                        if let Some(event) = self.accept(seq_event) {
                            return Ok(event);
                        }
                    }
                    Err(error) => return Err(self.stop(FeedEnd::Unreadable(error.to_string()))),
                },
                EVENTS_LAGGED => {
                    let skipped = serde_json::from_value::<Lagged>(notification.params)
                        .map_or(0, |lagged| lagged.skipped);
                    return Err(self.stop(FeedEnd::Lagged { skipped }));
                }
                // Other notifications are not about events.
                _ => {}
            }
        }
    }

    /// Subscribes again after the daemon dropped the subscription for
    /// falling behind: the events missed since the last one handed out come
    /// first, then the feed goes on. `client` must be the connection the
    /// feed came from. An interactive feed stays interactive.
    pub async fn resume(&mut self, client: &Client) -> Result<(), ClientError> {
        let after = self.last_seq();
        let batch = client
            .call::<Subscribe>(SubscribeParams {
                after,
                interactive: self.interactive,
            })
            .await?;
        self.pending.extend(batch.events);
        self.end = None;
        Ok(())
    }

    /// The sequence number of the last event handed out, backlog included.
    pub fn last_seq(&self) -> Option<u64> {
        (self.last_seq > 0).then_some(self.last_seq)
    }

    /// The event, unless it was already handed out.
    fn accept(&mut self, seq_event: SeqEvent) -> Option<Event> {
        if seq_event.seq <= self.last_seq {
            return None;
        }
        self.last_seq = seq_event.seq;
        Some(seq_event.event)
    }

    fn stop(&mut self, end: FeedEnd) -> FeedEnd {
        self.end = Some(end.clone());
        end
    }
}

/// A copy of the daemon's registry, kept up to date by a [`Feed`].
///
/// The copy stops changing when the feed stops (the daemon went away or
/// dropped the subscription); a new connection gives a new replica.
#[derive(Debug)]
pub struct Replica {
    registry: Arc<RwLock<Registry>>,
    task: JoinHandle<()>,
}

impl Replica {
    /// Subscribes `client` to the daemon's events and builds the registry
    /// from the events so far.
    pub async fn subscribe(client: &mut Client) -> Result<Self, ReplicaError> {
        let feed = subscribe(client).await?;
        Self::follow(feed)
    }

    /// Builds the registry from a feed's backlog, then applies the feed's
    /// updates in a task. Must be called inside a Tokio runtime.
    pub fn follow(feed: Feed) -> Result<Self, ReplicaError> {
        let Feed {
            backlog,
            mut updates,
        } = feed;
        let registry = Arc::new(RwLock::new(Registry::from_events(&backlog)?));
        let target = Arc::clone(&registry);
        let task = tokio::spawn(async move {
            while let Ok(event) = updates.next().await {
                // The daemon validated the event against the same rules; one
                // it applied cannot fail here.
                let _ = target.write().await.apply(&event);
            }
        });
        Ok(Self { registry, task })
    }

    /// The registry as of now.
    pub async fn read(&self) -> RwLockReadGuard<'_, Registry> {
        self.registry.read().await
    }
}

impl Drop for Replica {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
    use ulid::Ulid;
    use volee_core::EventKind;
    use volee_core::harness::HarnessKind;
    use volee_core::ids::AgentId;
    use volee_core::role::{LaunchedBy, Role};
    use volee_core::time::Timestamp;
    use volee_core::workspace::{Location, Workspace};

    use super::*;

    fn agent(n: u128) -> AgentId {
        AgentId::from_ulid(Ulid::from_parts(1_000, n))
    }

    fn registered(n: u128) -> Event {
        Event::new(
            Timestamp::from_millis(1),
            EventKind::AgentRegistered {
                agent: agent(n),
                name: format!("agent-{n}"),
                role: Role::Lead,
                harness: HarnessKind::ClaudeCode,
                launched_by: LaunchedBy::Human,
                parent: None,
                mission: None,
                workspace: Workspace::at("/work"),
                location: Location::Background,
            },
        )
    }

    fn seq_event(seq: u64, n: u128) -> Value {
        json!({ "seq": seq, "event": registered(n) })
    }

    /// One side of a fake daemon: reads requests and writes lines.
    struct FakeDaemon {
        lines: tokio::io::Lines<BufReader<OwnedReadHalf>>,
        write: OwnedWriteHalf,
    }

    impl FakeDaemon {
        /// Reads the next request and returns its id and params.
        async fn request(&mut self, method: &str) -> (Value, Value) {
            let line = self.lines.next_line().await.unwrap().unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], method);
            (request["id"].clone(), request["params"].clone())
        }

        async fn send(&mut self, value: Value) {
            let mut line = value.to_string();
            line.push('\n');
            self.write.write_all(line.as_bytes()).await.unwrap();
        }

        async fn respond(&mut self, id: Value, result: Value) {
            self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
                .await;
        }

        async fn notify(&mut self, method: &str, params: Value) {
            self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }))
                .await;
        }
    }

    /// A client connected to a fake daemon.
    async fn connect() -> (Client, FakeDaemon, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("volee.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let client = Client::connect(&socket).await.unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let (read, write) = stream.into_split();
        let daemon = FakeDaemon {
            lines: BufReader::new(read).lines(),
            write,
        };
        (client, daemon, dir)
    }

    #[tokio::test]
    async fn the_feed_has_no_gap_and_no_repeat_and_resumes_after_a_lag() {
        let (mut client, mut daemon, _dir) = connect().await;
        let subscribed = tokio::spawn(async move {
            let feed = subscribe(&mut client).await.unwrap();
            (client, feed)
        });
        let (id, params) = daemon.request("events.subscribe").await;
        assert_eq!(params["after"], Value::Null);
        assert_eq!(params["interactive"], false, "a program does not watch");
        daemon
            .respond(
                id,
                json!({ "events": [seq_event(1, 1), seq_event(2, 2)], "last_seq": 2 }),
            )
            .await;
        // An event already in the backlog, a new one, a notification about
        // something else, then the daemon gives up on this subscriber.
        daemon.notify(EVENTS_APPENDED, seq_event(2, 2)).await;
        daemon.notify(EVENTS_APPENDED, seq_event(3, 3)).await;
        daemon.notify("hooks.something", json!({})).await;
        daemon
            .notify(EVENTS_LAGGED, json!({ "skipped": 4100 }))
            .await;

        let (
            client,
            Feed {
                backlog,
                mut updates,
            },
        ) = subscribed.await.unwrap();
        assert_eq!(backlog, [registered(1), registered(2)]);
        assert_eq!(updates.next().await, Ok(registered(3)));
        let lagged = FeedEnd::Lagged { skipped: 4100 };
        assert_eq!(updates.next().await, Err(lagged.clone()));
        assert_eq!(updates.next().await, Err(lagged), "it stays stopped");
        assert_eq!(updates.last_seq(), Some(3));

        let resumed = tokio::spawn(async move {
            updates.resume(&client).await.unwrap();
            (client, updates)
        });
        let (id, params) = daemon.request("events.subscribe").await;
        assert_eq!(params["after"], 3);
        assert_eq!(params["interactive"], false);
        daemon
            .respond(
                id,
                json!({ "events": [seq_event(4, 4), seq_event(5, 5)], "last_seq": 5 }),
            )
            .await;
        daemon.notify(EVENTS_APPENDED, seq_event(5, 5)).await;
        daemon.notify(EVENTS_APPENDED, seq_event(6, 6)).await;
        let (client, mut updates) = resumed.await.unwrap();
        for n in 4..=6 {
            assert_eq!(updates.next().await, Ok(registered(n)));
        }
        drop(daemon);
        assert_eq!(updates.next().await, Err(FeedEnd::Closed));
        let mut client = client;
        assert!(matches!(
            subscribe(&mut client).await,
            Err(ReplicaError::AlreadySubscribed)
        ));
    }

    #[tokio::test]
    async fn an_interactive_feed_says_so_and_stays_interactive_when_resumed() {
        let (mut client, mut daemon, _dir) = connect().await;
        let subscribed = tokio::spawn(async move {
            let feed = subscribe_interactive(&mut client).await.unwrap();
            (client, feed)
        });
        let (id, params) = daemon.request("events.subscribe").await;
        assert_eq!(params["interactive"], true);
        daemon
            .respond(id, json!({ "events": [seq_event(1, 1)], "last_seq": 1 }))
            .await;
        daemon
            .notify(EVENTS_LAGGED, json!({ "skipped": 5000 }))
            .await;
        let (client, mut feed) = subscribed.await.unwrap();
        assert!(matches!(
            feed.updates.next().await,
            Err(FeedEnd::Lagged { .. })
        ));
        let resumed = tokio::spawn(async move {
            feed.updates.resume(&client).await.unwrap();
            client
        });
        let (id, params) = daemon.request("events.subscribe").await;
        assert_eq!(params["after"], 1);
        assert_eq!(params["interactive"], true);
        daemon
            .respond(id, json!({ "events": [], "last_seq": 1 }))
            .await;
        resumed.await.unwrap();
    }

    #[tokio::test]
    async fn an_unreadable_event_stops_the_feed() {
        let (mut client, mut daemon, _dir) = connect().await;
        let subscribed = tokio::spawn(async move { subscribe(&mut client).await.unwrap() });
        let (id, _) = daemon.request("events.subscribe").await;
        daemon
            .respond(id, json!({ "events": [], "last_seq": null }))
            .await;
        daemon
            .notify(
                EVENTS_APPENDED,
                json!({ "seq": 1, "event": { "type": "teleported" } }),
            )
            .await;
        let mut feed = subscribed.await.unwrap();
        assert_eq!(feed.backlog, Vec::<Event>::new());
        assert!(matches!(
            feed.updates.next().await,
            Err(FeedEnd::Unreadable(_))
        ));
    }

    #[tokio::test]
    async fn a_replica_applies_new_events() {
        let (mut client, mut daemon, _dir) = connect().await;
        let subscribed = tokio::spawn(async move {
            let replica = Replica::subscribe(&mut client).await.unwrap();
            (client, replica)
        });
        let (id, _) = daemon.request("events.subscribe").await;
        daemon
            .respond(id, json!({ "events": [seq_event(1, 1)], "last_seq": 1 }))
            .await;
        daemon.notify(EVENTS_APPENDED, seq_event(2, 2)).await;
        let (_client, replica) = subscribed.await.unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while replica.read().await.agents().count() < 2 {
            assert!(tokio::time::Instant::now() < deadline, "event not applied");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(replica.read().await.agent(agent(2)).is_some());
    }
}
