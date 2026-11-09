//! The theme picker: every theme volee can load, the one shown while the
//! person moves through them, and how the choice is kept.

use std::fmt;
use std::sync::Arc;

use crate::app::{Effect, Model, Overlay};
use crate::keymap::Action;
use crate::theme::Theme;

/// Keeps a theme chosen in the picker, for example in the configuration
/// file. Returns a sentence saying where, or why it could not.
#[derive(Clone)]
pub struct ThemeSaver(Arc<SaveFn>);

/// What a [`ThemeSaver`] calls with the theme's name.
type SaveFn = dyn Fn(&str) -> Result<String, String> + Send + Sync;

impl ThemeSaver {
    /// A saver that calls `save` with the theme's name.
    pub fn new(save: impl Fn(&str) -> Result<String, String> + Send + Sync + 'static) -> Self {
        Self(Arc::new(save))
    }

    /// Keeps `name`.
    pub(crate) fn save(&self, name: &str) -> Result<String, String> {
        (self.0)(name)
    }
}

impl fmt::Debug for ThemeSaver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ThemeSaver")
    }
}

/// The themes the picker offers.
#[derive(Clone, Debug, Default)]
pub(crate) struct ThemeMenu {
    /// Their names, in the picker's order.
    pub(crate) names: Vec<String>,
    /// The themes, in the same order.
    pub(crate) themes: Vec<Theme>,
    /// The theme on screen.
    pub(crate) current: usize,
    /// Where a choice is kept; none in tests and the demo.
    pub(crate) saver: Option<ThemeSaver>,
}

impl ThemeMenu {
    /// A menu of `themes`, by name, `current` being on screen.
    pub(crate) fn new(themes: Vec<(String, Theme)>, current: &str) -> Self {
        let current = themes
            .iter()
            .position(|(name, _)| name == current)
            .unwrap_or(0);
        let (names, themes) = themes.into_iter().unzip();
        Self {
            names,
            themes,
            current,
            saver: None,
        }
    }
}

/// Opens the picker on the theme shown now.
pub(crate) fn open(model: &mut Model) {
    if model.themes.themes.is_empty() {
        model.notify(
            "No theme to pick from here: set `theme` under [tui] in the configuration.",
            false,
        );
        return;
    }
    let current = model.themes.current;
    model.overlay = Some(Overlay::Themes {
        selected: current,
        original: current,
    });
}

/// Shows the theme at `index` at once.
fn show(model: &mut Model, index: usize) {
    if let Some(theme) = model.themes.themes.get(index) {
        model.theme = theme.clone();
        model.themes.current = index;
    }
}

/// Applies the theme at `index` and asks the loop to keep it.
pub(crate) fn choose(model: &mut Model, index: usize, effects: &mut Vec<Effect>) {
    show(model, index);
    if let Some(name) = model.themes.names.get(index) {
        effects.push(Effect::SaveTheme(name.clone()));
    }
}

/// A key of the picker: moving shows the theme, Enter keeps it, Escape goes
/// back to the one there was.
pub(crate) fn perform(model: &mut Model, action: Action, effects: &mut Vec<Effect>) {
    let Some(Overlay::Themes { selected, original }) = model.overlay else {
        return;
    };
    let count = model.themes.themes.len().max(1);
    match action {
        Action::Down | Action::Up => {
            let next = if action == Action::Down {
                (selected + 1) % count
            } else {
                (selected + count - 1) % count
            };
            show(model, next);
            model.overlay = Some(Overlay::Themes {
                selected: next,
                original,
            });
        }
        Action::Apply => {
            model.overlay = None;
            if selected != original {
                choose(model, selected, effects);
            }
        }
        Action::Back => {
            show(model, original);
            model.overlay = None;
        }
        _ => {}
    }
}

/// What the loop learnt from keeping a theme.
fn saved(model: &mut Model, name: &str, result: Result<String, String>) {
    match result {
        Ok(place) => model.notify(format!("Theme {name}: {place}."), false),
        Err(error) => model.notify(
            format!("Theme {name} is shown, but volee could not keep it: {error}"),
            true,
        ),
    }
}

/// Keeps a theme with the menu's saver, when there is one, and says how it
/// went.
pub(crate) fn keep(model: &mut Model, name: &str) {
    let result = match &model.themes.saver {
        Some(saver) => saver.save(name),
        None => Ok("for this session".to_owned()),
    };
    saved(model, name, result);
}
