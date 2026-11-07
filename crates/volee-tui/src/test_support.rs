//! Helpers for tests that drive the model with keys, on the demo at its
//! start.

use std::sync::Arc;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use volee_core::AgentId;
use volee_core::time::Timestamp;

use crate::app::{Effect, Model, Msg, update};
use crate::clock::ManualClock;
use crate::demo::DemoSource;
use crate::keymap::Keymap;
use crate::theme::Theme;

/// When the demo starts.
pub(crate) fn start() -> Timestamp {
    Timestamp::from_millis(1_793_986_617_000)
}

/// A member of the demo.
pub(crate) fn id(member: usize) -> AgentId {
    crate::demo::agent_id(start(), member)
}

/// The demo, on a clock that stays at its start.
pub(crate) fn demo() -> DemoSource {
    DemoSource::new(Arc::new(ManualClock::new(start())))
}

/// The demo at its start, in a terminal of the given size.
pub(crate) fn model(width: u16, height: u16) -> Model {
    let mut model = Model::new(
        Theme::default(),
        Keymap::default(),
        None,
        (width, height),
        start(),
    );
    update(&mut model, Msg::Events(demo().events_until(start())));
    model
}

/// One key, without modifiers.
pub(crate) fn key(model: &mut Model, code: KeyCode) -> Vec<Effect> {
    update(
        model,
        Msg::Key(KeyEvent::new_with_kind(
            code,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
    )
}

/// A letter with Ctrl held, such as `ctrl-s`.
pub(crate) fn ctrl(model: &mut Model, letter: char) -> Vec<Effect> {
    update(
        model,
        Msg::Key(KeyEvent::new_with_kind(
            KeyCode::Char(letter),
            KeyModifiers::CONTROL,
            KeyEventKind::Press,
        )),
    )
}

/// Characters typed one after the other.
pub(crate) fn press(model: &mut Model, keys: &str) -> Vec<Effect> {
    keys.chars()
        .flat_map(|c| key(model, KeyCode::Char(c)))
        .collect()
}
