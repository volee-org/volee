//! The terminal interface of volee.
//!
//! One screen shows every agent on the machine: a top bar with the views and
//! the counters, the list of agents sorted by urgency, the current view (the
//! hierarchy of who launched whom, as a tree of cards), the selected agent's
//! facts and its terminal screen, and the keys that apply.
//!
//! The interface follows the Elm architecture (ADR 0009): a model, messages,
//! a pure update that returns effects, and a pure view. It keeps its own
//! replica of the [`Registry`](volee_core::Registry), built from the events
//! of a [`Source`]: [`DaemonSource`] in `volee`, or [`DemoSource`] for
//! `volee demo`.
//!
//! ```no_run
//! use std::sync::Arc;
//! use volee_tui::{DemoSource, Options, SystemClock};
//!
//! # async fn demo() -> Result<(), volee_tui::TuiError> {
//! let source = DemoSource::new(Arc::new(SystemClock));
//! volee_tui::run(source, Options::default()).await
//! # }
//! ```

mod activity;
mod app;
mod clock;
mod daemon;
mod demo;
mod editor;
mod inbox;
pub mod keymap;
mod launch;
mod layout;
mod letters;
mod preview;
mod run;
pub mod source;
mod text;
pub mod theme;
mod tools;
mod view;
mod views;
mod widgets;

#[cfg(test)]
mod screen_tests;
#[cfg(test)]
mod test_support;

use std::sync::Arc;

pub use clock::{Clock, ManualClock, SystemClock};
pub use daemon::DaemonSource;
pub use demo::DemoSource;
pub use keymap::{Keymap, KeymapError};
pub use source::{
    Changes, MissionRequest, NewMission, Outgoing, Source, SourceError, SpawnRequest, Subscription,
    Summary, TakeOver,
};
pub use theme::{ColorDepth, Theme, ThemeError};
pub use tools::{Tool, ToolKind};

use crate::app::{Model, Msg};
use crate::run::{TerminalScreen, event_loop};

/// How the interface looks and behaves.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Options {
    /// The colors.
    pub theme: Theme,
    /// The key bindings.
    pub keymap: Keymap,
    /// Where the time comes from; tests and recordings use a fixed clock.
    pub clock: Arc<dyn Clock>,
    /// A few words shown at the right of the top bar, such as
    /// `demo · simulated data`.
    pub banner: Option<String>,
    /// Programs to open an agent's worktree with; none is required.
    pub tools: Vec<Tool>,
    /// Repositories agents and missions can be launched in, by name.
    pub repos: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            keymap: Keymap::default(),
            clock: Arc::new(SystemClock),
            banner: None,
            tools: Vec::new(),
            repos: Vec::new(),
        }
    }
}

/// Why the interface stopped with an error.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TuiError {
    /// The terminal could not be set up, read or drawn.
    #[error("terminal error: {0}")]
    Terminal(#[from] std::io::Error),
    /// The source refused the subscription.
    #[error("cannot get the agents: {0}")]
    Source(#[from] SourceError),
}

/// Runs the interface in the current terminal until the person quits.
///
/// The terminal is restored on return, on error and on panic. Taking over an
/// agent suspends the interface, runs the command of [`Source::attach`] in
/// this terminal, and comes back to the same agent.
pub async fn run(source: impl Source, options: Options) -> Result<(), TuiError> {
    let Subscription { snapshot, updates } = source.subscribe().await?;
    let mut screen = TerminalScreen::enter()?;
    let size = run::Screen::size(&screen)?;
    let mut model = Model::new(
        options.theme,
        options.keymap,
        options.banner,
        size,
        options.clock.now(),
    );
    model.settings.tools = options.tools;
    model.settings.repos = options.repos;
    let result = event_loop(
        &mut screen,
        Arc::new(source),
        updates,
        &mut model,
        options.clock.as_ref(),
        Msg::Events(snapshot),
    )
    .await;
    screen.leave()?;
    Ok(result?)
}
