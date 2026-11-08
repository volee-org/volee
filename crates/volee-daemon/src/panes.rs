//! Location adapters ([ADR 0015]): the terminal multiplexer panes that
//! interactive sessions run in.
//!
//! After each discovery round, every multiplexer is asked, outside the state
//! lock, about the live interactive sessions of the listing; what it finds
//! becomes `LocationChanged` events ([`relocate`]). A multiplexer that does
//! not answer (no server runs) changes nothing, and says so in
//! `volee daemon status`. The preview of an agent in a pane captures the
//! pane, and its take over brings the pane forward ([`crate::control`]).
//!
//! A background session runs in no pane, but a pane may show it: `claude
//! agents` or `claude attach` titles its terminal with the session's name.
//! After each round the daemon also asks which panes show the live
//! background sessions, and keeps the answer, a mirror, for previews only
//! ([ADR 0018]): it records no event, since the session runs elsewhere.
//!
//! [ADR 0015]: https://volee-org.github.io/volee/adr/0015-tuios-location-adapter.html
//! [ADR 0018]: https://volee-org.github.io/volee/adr/0018-panes-that-show-background-sessions.html

use std::sync::{Arc, PoisonError};

use volee_core::harness::SessionKey;
use volee_core::reconcile::{ListedSession, relocate};
use volee_core::workspace::Location;
use volee_mux::{Hosted, Multiplexer};
use volee_proto::methods::MultiplexerStatus;

use crate::server::Shared;

/// The multiplexers the daemon uses, and what it last learned about each.
pub(crate) struct Panes {
    adapters: Vec<Arc<dyn Multiplexer>>,
    statuses: std::sync::Mutex<Vec<MultiplexerStatus>>,
    /// The panes that showed background sessions at the last round.
    mirrors: std::sync::Mutex<Vec<Mirror>>,
}

/// A pane that shows a background session (ADR 0018).
#[derive(Clone, Debug)]
struct Mirror {
    key: SessionKey,
    multiplexer: &'static str,
    target: String,
}

impl std::fmt::Debug for Panes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Panes")
            .field("adapters", &self.adapters)
            .finish_non_exhaustive()
    }
}

impl Panes {
    /// The daemon's multiplexers; their status is checked by [`check`].
    pub(crate) fn new(adapters: Vec<Arc<dyn Multiplexer>>) -> Self {
        let statuses = adapters
            .iter()
            .map(|adapter| MultiplexerStatus {
                name: adapter.name().to_owned(),
                available: false,
                version: None,
                detail: Some("not checked yet".to_owned()),
            })
            .collect();
        Self {
            adapters,
            statuses: std::sync::Mutex::new(statuses),
            mirrors: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// The pane that showed the background session `key` at the last round,
    /// when one did: its multiplexer and its address.
    pub(crate) fn mirror_of(&self, key: &SessionKey) -> Option<(Arc<dyn Multiplexer>, String)> {
        let mirrors = self.mirrors.lock().unwrap_or_else(PoisonError::into_inner);
        let mirror = mirrors.iter().find(|mirror| &mirror.key == key)?;
        self.adapter(mirror.multiplexer)
            .map(|adapter| (adapter, mirror.target.clone()))
    }

    /// What is known about each multiplexer, for `volee daemon status`.
    pub(crate) fn statuses(&self) -> Vec<MultiplexerStatus> {
        // Plain values, never left half-written: a poisoned lock still
        // holds valid ones.
        self.statuses
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The multiplexer named `name`, when the daemon uses it.
    pub(crate) fn adapter(&self, name: &str) -> Option<Arc<dyn Multiplexer>> {
        self.adapters
            .iter()
            .find(|adapter| adapter.name() == name)
            .cloned()
    }

    /// Records whether `name` answered. Returns whether it answered the
    /// time before, so that a change is logged once.
    fn record(&self, name: &str, answer: Result<(), String>) -> bool {
        let mut statuses = self.statuses.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(status) = statuses.iter_mut().find(|status| status.name == name) else {
            return false;
        };
        let before = status.available;
        status.available = answer.is_ok();
        status.detail = answer.err();
        before
    }
}

/// Checks each multiplexer once, in the background, for its version and
/// whether a server runs.
pub(crate) async fn check(shared: &Arc<Shared>) {
    for adapter in shared.panes.adapters.clone() {
        let Ok(status) = tokio::task::spawn_blocking(move || adapter.status()).await else {
            continue;
        };
        let mut statuses = shared
            .panes
            .statuses
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(known) = statuses.iter_mut().find(|known| known.name == status.name) {
            *known = MultiplexerStatus {
                name: status.name,
                available: status.available,
                version: status.version,
                detail: status.detail,
            };
        }
    }
}

/// The live interactive sessions of a listing: the ones that may run in a
/// pane.
pub(crate) fn hosted(sessions: &[ListedSession]) -> Vec<Hosted> {
    sessions
        .iter()
        .filter(|session| session.alive && !session.background)
        .map(|session| Hosted {
            key: session.key.clone(),
            pid: session.pid,
        })
        .collect()
}

/// The live background sessions a pane may show, with their names. A pane
/// is found by the name in its title, so only names that no other live
/// session shares are kept (ADR 0018).
pub(crate) fn shown(sessions: &[ListedSession]) -> Vec<(SessionKey, String)> {
    let live = || sessions.iter().filter(|session| session.alive);
    let named = |session: &ListedSession| {
        session
            .name
            .as_deref()
            .map(volee_mux::shown_name)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
    };
    live()
        .filter(|session| session.background)
        .filter_map(|session| {
            let name = named(session)?;
            let alike = live()
                .filter(|other| named(other).as_deref() == Some(name.as_str()))
                .count();
            (alike == 1).then(|| (session.key.clone(), name))
        })
        .collect()
}

/// Asks every multiplexer which panes show `shown` background sessions, and
/// keeps the answer until the next round (ADR 0018). A session one
/// multiplexer found is not offered to the next; one that does not answer
/// finds none this round.
pub(crate) async fn mirror(shared: &Arc<Shared>, shown: &[(SessionKey, String)]) {
    let mut found: Vec<Mirror> = Vec::new();
    for adapter in shared.panes.adapters.clone() {
        let asked: Vec<(SessionKey, String)> = shown
            .iter()
            .filter(|(key, _)| !found.iter().any(|mirror| &mirror.key == key))
            .cloned()
            .collect();
        if asked.is_empty() {
            break;
        }
        let multiplexer = adapter.name();
        match tokio::task::spawn_blocking(move || adapter.mirrors(&asked)).await {
            Ok(Ok(located)) => found.extend(located.into_iter().map(|located| Mirror {
                key: located.key,
                multiplexer,
                target: located.target,
            })),
            Ok(Err(error)) => {
                tracing::debug!(multiplexer, %error, "cannot find the panes that show sessions");
            }
            Err(error) => tracing::error!(%error, "the mirror task panicked"),
        }
    }
    *shared
        .panes
        .mirrors
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = found;
}

/// Asks every multiplexer which panes `live` sessions run in, and records
/// where their agents run. Agents left in a pane whose session no longer
/// runs go back to a plain terminal, and so do agents in a pane of a
/// multiplexer this daemon does not use (it was used by an earlier run).
///
/// The multiplexers are asked in order, and a session one of them found is
/// not offered to the next: two that both claimed it (a session in tmux,
/// inside a tuios pane whose variables it inherited) would otherwise move
/// its agent back and forth at every round.
pub(crate) async fn locate(shared: &Arc<Shared>, live: &[Hosted]) {
    forget_unused(shared).await;
    let mut claimed: Vec<SessionKey> = Vec::new();
    for adapter in shared.panes.adapters.clone() {
        let name = adapter.name();
        let live: Vec<Hosted> = live
            .iter()
            .filter(|session| !claimed.contains(&session.key))
            .cloned()
            .collect();
        let live = live.as_slice();
        let ended = in_panes_of(shared, name, live).await;
        if live.is_empty() && ended.is_empty() {
            continue;
        }
        let asked = live.to_vec();
        let found = if asked.is_empty() {
            Ok(Vec::new())
        } else {
            let finder = Arc::clone(&adapter);
            match tokio::task::spawn_blocking(move || finder.locate(&asked)).await {
                Ok(found) => found.map_err(|error| error.to_string()),
                Err(error) => Err(format!("the search for panes failed: {error}")),
            }
        };
        let found = match found {
            Ok(found) => {
                if !shared.panes.record(name, Ok(())) {
                    tracing::info!(multiplexer = name, "looking for agents in panes");
                }
                found
            }
            Err(error) => {
                if shared.panes.record(name, Err(error.clone())) {
                    tracing::warn!(multiplexer = name, %error, "cannot look for agents in panes");
                }
                continue;
            }
        };
        let found: Vec<(SessionKey, String)> = found
            .into_iter()
            .map(|located| (located.key, located.target))
            .collect();
        claimed.extend(found.iter().map(|(key, _)| key.clone()));
        let asked: Vec<SessionKey> = live
            .iter()
            .map(|session| session.key.clone())
            .chain(ended)
            .collect();
        let mut state = shared.state.lock().await;
        let now = state.now();
        let events = relocate(state.registry(), name, &asked, &found, now);
        if events.is_empty() {
            continue;
        }
        let count = events.len();
        match state.commit(events) {
            Ok(_) => tracing::debug!(multiplexer = name, events = count, "agents located"),
            Err(error) => tracing::error!(%error, "cannot record where agents run"),
        }
    }
}

/// Moves the agents recorded in a pane of a multiplexer this daemon does not
/// use back to a plain terminal: volee can neither show nor reach that pane.
async fn forget_unused(shared: &Arc<Shared>) {
    let mut state = shared.state.lock().await;
    let now = state.now();
    let mut unused: Vec<(String, Vec<SessionKey>)> = Vec::new();
    for agent in state.registry().agents() {
        let Location::Pane { multiplexer, .. } = &agent.location else {
            continue;
        };
        let Some(link) = agent.current_session() else {
            continue;
        };
        if shared.panes.adapter(multiplexer).is_some() {
            continue;
        }
        match unused.iter_mut().find(|(name, _)| name == multiplexer) {
            Some((_, keys)) => keys.push(link.key.clone()),
            None => unused.push((multiplexer.clone(), vec![link.key.clone()])),
        }
    }
    let events: Vec<_> = unused
        .iter()
        .flat_map(|(name, keys)| relocate(state.registry(), name, keys, &[], now))
        .collect();
    if events.is_empty() {
        return;
    }
    if let Err(error) = state.commit(events) {
        tracing::error!(%error, "cannot record where agents run");
    }
}

/// The sessions of agents recorded in a pane of `multiplexer` that are not
/// among `live`: they ended, or run elsewhere now.
async fn in_panes_of(shared: &Arc<Shared>, multiplexer: &str, live: &[Hosted]) -> Vec<SessionKey> {
    let state = shared.state.lock().await;
    state
        .registry()
        .agents()
        .filter(|agent| {
            matches!(&agent.location, Location::Pane { multiplexer: own, .. } if own == multiplexer)
        })
        .filter_map(|agent| agent.current_session().map(|link| link.key.clone()))
        .filter(|key| !live.iter().any(|session| &session.key == key))
        .collect()
}

#[cfg(test)]
mod tests {
    use volee_core::state::AgentState;

    use super::*;

    fn session(id: &str, name: &str, background: bool, alive: bool) -> ListedSession {
        let mut session = ListedSession::new(SessionKey::claude(id), "/work", AgentState::Working);
        session.name = Some(name.to_owned());
        session.background = background;
        session.alive = alive;
        session
    }

    #[test]
    fn panes_may_show_live_background_sessions_with_a_name_of_their_own() {
        let sessions = [
            session("b60aa3b9", "cockpit open launch failure", true, true),
            // Ended.
            session("4402670e", "neon billing", true, false),
            // Runs in a terminal, where its process tells its pane.
            session("1f0c2d3e", "fix the parser", false, true),
            // Two live sessions with one name: a title cannot tell them apart.
            session("aaaa0001", "same name", true, true),
            session("aaaa0002", "same name", true, true),
            // Its name, without its status mark, is the terminal one's.
            session("bbbb0001", "◑ fix the parser", true, true),
        ];
        assert_eq!(
            shown(&sessions),
            [(
                SessionKey::claude("b60aa3b9"),
                "cockpit open launch failure".to_owned()
            )]
        );
    }
}
