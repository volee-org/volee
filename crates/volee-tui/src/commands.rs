//! The command palette (`:`): the actions of the current screen and the
//! themes, found by typing part of their name, run with Enter.

use crate::app::{Effect, Model, Overlay};
use crate::keymap::{Action, Context, KeyCombo};

/// What the palette holds: the text typed and the selected row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Palette {
    /// What was typed.
    pub(crate) text: String,
    /// The selected row among the matches.
    pub(crate) selected: usize,
}

/// One command of the palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Command {
    /// Its name, as typed: an action's name, or `theme <name>`.
    pub(crate) name: String,
    /// What it does, in a few words.
    pub(crate) label: &'static str,
    /// Its key, when it has one.
    pub(crate) keys: String,
    /// What running it does.
    pub(crate) run: Run,
}

/// What running a command does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Run {
    /// An action, as if its key were pressed in that context.
    Action(Context, Action),
    /// Show and keep a theme.
    Theme(usize),
}

/// Actions that move a selection or a cursor: they need a key, not a name.
const MOVES: [Action; 8] = [
    Action::Down,
    Action::Up,
    Action::Left,
    Action::Right,
    Action::First,
    Action::Last,
    Action::HalfPageDown,
    Action::HalfPageUp,
];

/// Every command the palette offers on the current screen.
pub(crate) fn all(model: &Model) -> Vec<Command> {
    let mut commands: Vec<Command> = Vec::new();
    for context in model.screen_contexts() {
        for binding in model.keymap.bindings() {
            if binding.context != context
                || MOVES.contains(&binding.action)
                || matches!(binding.action, Action::Commands | Action::Back)
                || commands
                    .iter()
                    .any(|command| command.run == Run::Action(context, binding.action))
                || commands
                    .iter()
                    .any(|command| command.name == binding.action.name())
            {
                continue;
            }
            commands.push(Command {
                name: binding.action.name().to_owned(),
                label: binding.description,
                keys: binding
                    .keys
                    .first()
                    .map(crate::keymap::KeySeq::short)
                    .unwrap_or_default(),
                run: Run::Action(context, binding.action),
            });
        }
    }
    for (index, name) in model.themes.names.iter().enumerate() {
        commands.push(Command {
            name: format!("theme {name}"),
            label: "colour theme",
            keys: String::new(),
            run: Run::Theme(index),
        });
    }
    commands
}

/// The commands that match `text`: each of its words is in the name or the
/// label, names that start with the text first.
pub(crate) fn matching(model: &Model, text: &str) -> Vec<Command> {
    let text = text.trim().to_lowercase();
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut found: Vec<Command> = all(model)
        .into_iter()
        .filter(|command| {
            let haystack = format!("{} {}", command.name, command.label).to_lowercase();
            words.iter().all(|word| haystack.contains(word))
        })
        .collect();
    found.sort_by_key(|command| !command.name.starts_with(&text));
    found
}

/// Opens the palette, empty.
pub(crate) fn open(model: &mut Model) {
    model.overlay = Some(Overlay::Commands(Palette::default()));
}

/// A key in the palette: its own keys, the text field's, then typing.
pub(crate) fn on_key(model: &mut Model, key: KeyCombo, effects: &mut Vec<Effect>) {
    let action = model.keymap.action(&[Context::Commands], key);
    let editing = model.keymap.action(&[Context::Editing], key);
    let Some(Overlay::Commands(palette)) = &model.overlay else {
        return;
    };
    let mut palette = palette.clone();
    let count = matching(model, &palette.text).len();
    match action {
        Some(Action::Apply) => {
            let chosen = matching(model, &palette.text)
                .get(palette.selected)
                .map(|command| command.run);
            model.overlay = None;
            match chosen {
                Some(command) => run(model, command, effects),
                None => model.notify("No command matches.", false),
            }
            return;
        }
        Some(Action::Back) => {
            model.overlay = None;
            return;
        }
        Some(Action::Quit) => {
            model.overlay = None;
            crate::app::request_quit(model, effects);
            return;
        }
        Some(Action::Down) if count > 0 => palette.selected = (palette.selected + 1) % count,
        Some(Action::Up) if count > 0 => palette.selected = (palette.selected + count - 1) % count,
        _ => {
            match editing {
                Some(Action::DeleteWord) => {
                    let trimmed = palette.text.trim_end().len();
                    let start = palette.text[..trimmed]
                        .rfind(' ')
                        .map_or(0, |space| space + 1);
                    palette.text.truncate(start);
                }
                Some(Action::DeleteToLineStart) => palette.text.clear(),
                _ => {
                    use ratatui::crossterm::event::KeyCode;
                    if key.code() == KeyCode::Backspace {
                        palette.text.pop();
                    } else if let Some(c) = key.typed_char() {
                        palette.text.push(c);
                    }
                }
            }
            palette.selected = 0;
        }
    }
    model.overlay = Some(Overlay::Commands(palette));
}

/// Runs a command.
fn run(model: &mut Model, command: Run, effects: &mut Vec<Effect>) {
    match command {
        Run::Action(context, action) => crate::app::perform(model, context, action, effects),
        Run::Theme(index) => crate::themes::choose(model, index, effects),
    }
}
