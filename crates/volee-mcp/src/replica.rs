//! A registry replica kept in sync with the daemon.

use std::sync::Arc;

use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use volee_core::{Event, Registry};
use volee_proto::client::{Client, appended_event};
use volee_proto::methods::{SinceParams, Subscribe};

use crate::server::McpError;

/// The daemon's registry, replayed locally and updated by notifications.
#[derive(Debug)]
pub struct Replica {
    registry: Arc<RwLock<Registry>>,
    task: JoinHandle<()>,
}

impl Replica {
    /// Subscribes `client` to the event log and builds the replica. The
    /// client must not have handed out its notification channel yet.
    pub async fn subscribe(client: &mut Client) -> Result<Self, McpError> {
        let mut notifications = client
            .take_notifications()
            .ok_or(McpError::AlreadySubscribed)?;
        let batch = client.call::<Subscribe>(SinceParams::default()).await?;
        let events: Vec<Event> = batch.events.into_iter().map(|e| e.event).collect();
        let registry = Arc::new(RwLock::new(Registry::from_events(&events)?));
        let target = Arc::clone(&registry);
        let task = tokio::spawn(async move {
            while let Some(notification) = notifications.recv().await {
                let Some(appended) = appended_event(&notification) else {
                    continue;
                };
                // The daemon only streams events it applied; a failure here
                // means this replica missed one, which the next restart fixes.
                let _ = target.write().await.apply(&appended.event);
            }
        });
        Ok(Self { registry, task })
    }

    /// Read access to the current registry.
    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, Registry> {
        self.registry.read().await
    }
}

impl Drop for Replica {
    fn drop(&mut self) {
        self.task.abort();
    }
}
