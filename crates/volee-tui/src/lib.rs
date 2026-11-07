//! The terminal interface of volee.
//!
//! One screen shows every agent on the machine: a top bar with the views and
//! the counters, the list of agents sorted by urgency, the current view (the
//! hierarchy of who launched whom, the network of who talks to whom, the
//! repositories with their worktrees and delivery evidence, or the timeline
//! of what happened when), the selected agent's facts and its terminal
//! screen, and the keys that apply.
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
mod history;
pub mod keymap;
mod layout;
mod letters;
mod preview;
mod pulses;
mod run;
pub mod source;
mod text;
pub mod theme;
mod view;
mod views;
mod widgets;

#[cfg(test)]
mod screen_tests;

use std::sync::Arc;

pub use clock::{Clock, ManualClock, SystemClock};
pub use daemon::DaemonSource;
pub use demo::DemoSource;
pub use keymap::{Keymap, KeymapError};
pub use source::{Changes, Outgoing, Source, SourceError, Subscription, TakeOver};
pub use theme::{ColorDepth, Theme, ThemeError};

use crate::app::{Model, Motion, Msg};
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
    /// No travelling animations: message pulses on the network become
    /// short highlights and spinners stop.
    pub reduced_motion: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            keymap: Keymap::default(),
            clock: Arc::new(SystemClock),
            banner: None,
            reduced_motion: false,
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
    if options.reduced_motion {
        model.motion = Motion::Reduced;
    }
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
