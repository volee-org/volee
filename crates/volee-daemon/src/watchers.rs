//! Who watches: the interactive subscriptions open now (an interface a
//! person looks at, not an agent's MCP server). Discovery lists sessions
//! every few seconds while one is open, and rarely otherwise.

use tokio::sync::watch;

/// The interactive subscriptions, as discovery sees them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Watching {
    /// Interactive subscriptions open now.
    pub(crate) open: usize,
    /// Interactive subscriptions opened since the daemon started. When it
    /// grows, someone new watches and should see fresh data at once.
    pub(crate) opened: u64,
}

/// Counts the interactive subscriptions and tells discovery when the count
/// changes.
#[derive(Debug)]
pub(crate) struct Watchers(watch::Sender<Watching>);

impl Watchers {
    pub(crate) fn new() -> Self {
        Self(watch::Sender::new(Watching::default()))
    }

    /// Records a new interactive subscription, open until the returned
    /// guard is dropped (the subscription ends, or its connection closes).
    pub(crate) fn watch(&self) -> Watcher {
        self.0.send_modify(|watching| {
            watching.open += 1;
            watching.opened += 1;
        });
        Watcher(self.0.clone())
    }

    /// Follows the count.
    pub(crate) fn subscribe(&self) -> watch::Receiver<Watching> {
        self.0.subscribe()
    }
}

/// An open interactive subscription.
#[derive(Debug)]
pub(crate) struct Watcher(watch::Sender<Watching>);

impl Drop for Watcher {
    fn drop(&mut self) {
        self.0.send_modify(|watching| {
            watching.open = watching.open.saturating_sub(1);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_watcher_counts_until_it_is_dropped() {
        let watchers = Watchers::new();
        let mut seen = watchers.subscribe();
        let first = watchers.watch();
        let second = watchers.watch();
        assert!(seen.has_changed().unwrap());
        assert_eq!(*seen.borrow_and_update(), Watching { open: 2, opened: 2 });
        drop(first);
        assert_eq!(*seen.borrow_and_update(), Watching { open: 1, opened: 2 });
        drop(second);
        assert_eq!(*seen.borrow_and_update(), Watching { open: 0, opened: 2 });
        let _third = watchers.watch();
        assert_eq!(*seen.borrow(), Watching { open: 1, opened: 3 });
    }
}
