//! Launching from the interface: a new agent in a new worktree
//! (`agents.spawn`), or a new mission, whose coordinator volee launches
//! (`missions.create`). A dialog asks for what is needed, and keeps the
//! source's refusal on screen until the person fixes what it says.

use volee_core::mission::MissionKind;

use crate::app::{Effect, Model, Overlay};
use crate::editor::{self, TextField};
use crate::keymap::{Action, Context, KeyCombo};
use crate::source::{MissionRequest, NewMission, SourceError, SpawnRequest};

/// What the dialog launches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialogKind {
    /// An agent in a new worktree.
    Agent,
    /// A mission and its coordinator.
    Mission,
}

/// One field of a dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Field {
    /// Its name, shown above it.
    pub(crate) name: &'static str,
    /// What it holds.
    pub(crate) value: Value,
    /// What to write when it is empty.
    pub(crate) placeholder: &'static str,
}

/// What a field holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Value {
    /// One line of text.
    Line(TextField),
    /// Several lines of text.
    Lines(TextField),
    /// One option among several.
    Choice {
        /// The options.
        options: Vec<String>,
        /// The chosen one.
        index: usize,
    },
    /// Any number of options.
    Many {
        /// The options.
        options: Vec<String>,
        /// Which are chosen.
        chosen: Vec<bool>,
        /// The option under the cursor.
        cursor: usize,
    },
}

impl Value {
    fn text(&self) -> String {
        match self {
            Self::Line(field) | Self::Lines(field) => field.text().trim().to_owned(),
            Self::Choice { options, index } => options.get(*index).cloned().unwrap_or_default(),
            Self::Many {
                options, chosen, ..
            } => options
                .iter()
                .zip(chosen)
                .filter(|(_, chosen)| **chosen)
                .map(|(option, _)| option.as_str())
                .collect::<Vec<_>>()
                .join(","),
        }
    }
}

/// A launch dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LaunchDialog {
    /// What it launches.
    pub(crate) kind: DialogKind,
    /// Its fields, in order.
    pub(crate) fields: Vec<Field>,
    /// The field with the keyboard.
    pub(crate) focus: usize,
    /// What is missing, or why the source refused.
    pub(crate) error: Option<String>,
    /// The source is launching it.
    pub(crate) sending: bool,
}

const KINDS: [MissionKind; 2] = [MissionKind::Change, MissionKind::Analysis];

/// Opens a dialog. The repositories come from the configuration; without
/// any, an agent's repository is written as a path.
pub(crate) fn open(model: &mut Model, kind: DialogKind) {
    let repos = model.settings.repos.clone();
    let line = Value::Line(TextField::default());
    let lines = Value::Lines(TextField::default());
    let fields = match kind {
        DialogKind::Agent => vec![
            Field {
                name: "repository",
                value: if repos.is_empty() {
                    line.clone()
                } else {
                    Value::Choice {
                        options: repos,
                        index: 0,
                    }
                },
                placeholder: "the path of a git checkout",
            },
            Field {
                name: "branch",
                value: line.clone(),
                placeholder: "the new branch, such as feat/invoice-export",
            },
            Field {
                name: "task",
                value: lines.clone(),
                placeholder: "what the agent should do: its first prompt",
            },
            Field {
                name: "model",
                value: line,
                placeholder: "the configured default",
            },
        ],
        DialogKind::Mission => vec![
            Field {
                name: "title",
                value: line,
                placeholder: "a short name, such as Invoice export",
            },
            Field {
                name: "kind",
                value: Value::Choice {
                    options: KINDS.iter().map(|kind| kind.label().to_owned()).collect(),
                    index: 0,
                },
                placeholder: "",
            },
            Field {
                name: "repositories",
                value: Value::Many {
                    chosen: vec![false; repos.len()],
                    options: repos,
                    cursor: 0,
                },
                placeholder: "add repositories under [repos] in the configuration first",
            },
            Field {
                name: "goal",
                value: lines,
                placeholder: "what the mission delivers, as its coordinator will read it",
            },
        ],
    };
    model.overlay = Some(Overlay::Launch(Box::new(LaunchDialog {
        kind,
        fields,
        focus: 0,
        error: None,
        sending: false,
    })));
}

/// A key in a dialog: its own keys first, then the focused field's.
pub(crate) fn on_key(model: &mut Model, key: KeyCombo, effects: &mut Vec<Effect>) {
    let action = model.keymap.action(&[Context::Dialog], key);
    let Some(Overlay::Launch(dialog)) = &mut model.overlay else {
        return;
    };
    match action {
        Some(Action::Back) => model.overlay = None,
        Some(Action::Quit) => crate::app::request_quit(model, effects),
        Some(Action::NextField) => move_focus(dialog, true),
        Some(Action::PreviousField) => move_focus(dialog, false),
        Some(Action::Launch) => submit(dialog, effects),
        // Enter: the task and the goal take several lines, and a launch
        // needs its own key, so Enter only moves on from the other fields.
        Some(Action::NewLine) => match dialog.fields.get_mut(dialog.focus) {
            Some(Field {
                value: Value::Lines(field),
                ..
            }) => {
                if !dialog.sending {
                    field.insert("\n");
                }
            }
            _ => move_focus(dialog, true),
        },
        _ if dialog.sending => {}
        _ => {
            if let Some(field) = dialog.fields.get_mut(dialog.focus) {
                edit(&mut field.value, key);
            }
        }
    }
}

/// Gives the keyboard to the next field, or the previous one, round the
/// dialog.
fn move_focus(dialog: &mut LaunchDialog, forward: bool) {
    let count = dialog.fields.len().max(1);
    dialog.focus = if forward {
        (dialog.focus + 1) % count
    } else {
        (dialog.focus + count - 1) % count
    };
}

/// Pasted text goes to the focused text field.
pub(crate) fn paste(dialog: &mut LaunchDialog, text: &str) {
    if dialog.sending {
        return;
    }
    match dialog
        .fields
        .get_mut(dialog.focus)
        .map(|field| &mut field.value)
    {
        Some(Value::Lines(field)) => field.insert(&editor::clean_paste(text)),
        Some(Value::Line(field)) => {
            let line: String = text.chars().filter(|c| !c.is_control()).collect();
            field.insert(&line);
        }
        _ => {}
    }
}

/// Keys the dialog does not bind: text for text fields, ←/→ for choices,
/// space to pick a repository.
fn edit(value: &mut Value, key: KeyCombo) {
    use ratatui::crossterm::event::KeyCode;
    match value {
        Value::Line(field) => {
            if !matches!(key.code(), KeyCode::Up | KeyCode::Down) {
                editor::apply_key(field, key);
            }
        }
        Value::Lines(field) => {
            editor::apply_key(field, key);
        }
        Value::Choice { options, index } => {
            let count = options.len().max(1);
            match key.code() {
                KeyCode::Left => *index = (*index + count - 1) % count,
                KeyCode::Right | KeyCode::Char(' ') => *index = (*index + 1) % count,
                _ => {}
            }
        }
        Value::Many {
            options,
            chosen,
            cursor,
        } => {
            let count = options.len().max(1);
            match key.code() {
                KeyCode::Left => *cursor = (*cursor + count - 1) % count,
                KeyCode::Right => *cursor = (*cursor + 1) % count,
                KeyCode::Char(' ') => {
                    if let Some(chosen) = chosen.get_mut(*cursor) {
                        *chosen = !*chosen;
                    }
                }
                _ => {}
            }
        }
    }
}

/// Checks the dialog and asks the source to launch it.
fn submit(dialog: &mut LaunchDialog, effects: &mut Vec<Effect>) {
    if dialog.sending {
        return;
    }
    let value = |name: &str| {
        dialog
            .fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.text())
            .unwrap_or_default()
    };
    let missing =
        |names: &[&'static str]| names.iter().copied().find(|name| value(name).is_empty());
    let effect = match dialog.kind {
        DialogKind::Agent => {
            if let Some(name) = missing(&["repository", "branch", "task"]) {
                dialog.error = Some(format!("The agent needs a {name}."));
                return;
            }
            let branch = value("branch");
            if branch.contains(char::is_whitespace) {
                dialog.error = Some(format!("A branch name has no spaces: `{branch}`."));
                return;
            }
            let model = value("model");
            Effect::Spawn(SpawnRequest {
                repo: value("repository"),
                branch,
                task: value("task"),
                model: (!model.is_empty()).then_some(model),
            })
        }
        DialogKind::Mission => {
            if let Some(name) = missing(&["title", "repositories", "goal"]) {
                dialog.error = Some(if name == "repositories" {
                    "Choose at least one repository: ←/→ move, space picks.".to_owned()
                } else {
                    format!("The mission needs a {name}.")
                });
                return;
            }
            let kind = KINDS
                .into_iter()
                .find(|kind| kind.label() == value("kind"))
                .unwrap_or(MissionKind::Change);
            Effect::CreateMission(MissionRequest {
                title: value("title"),
                kind,
                repos: value("repositories")
                    .split(',')
                    .map(str::to_owned)
                    .collect(),
                goal: value("goal"),
            })
        }
    };
    dialog.error = None;
    dialog.sending = true;
    effects.push(effect);
}

/// The source launched the agent, or refused: the dialog closes, or shows
/// why and stays.
pub(crate) fn launched(
    model: &mut Model,
    result: Result<volee_core::AgentId, SourceError>,
) -> bool {
    let Some(Overlay::Launch(dialog)) = &mut model.overlay else {
        return false;
    };
    if dialog.kind != DialogKind::Agent {
        return false;
    }
    match result {
        Ok(agent) => {
            model.overlay = None;
            model.notify(
                "Agent launched: it appears in the list as it starts.",
                false,
            );
            model.follow(agent);
        }
        Err(error) => {
            dialog.sending = false;
            dialog.error = Some(crate::text::sentence(&error.to_string()));
        }
    }
    true
}

/// The source created the mission, or refused.
pub(crate) fn mission_created(model: &mut Model, result: Result<NewMission, SourceError>) -> bool {
    let Some(Overlay::Launch(dialog)) = &mut model.overlay else {
        return false;
    };
    if dialog.kind != DialogKind::Mission {
        return false;
    }
    match result {
        Ok(created) => {
            model.overlay = None;
            match created.coordinator {
                Some(coordinator) => {
                    model.notify("Mission created: its coordinator is starting.", false);
                    model.follow(coordinator);
                }
                None => model.notify(
                    "Mission created, but volee could not launch its coordinator: see its thread.",
                    true,
                ),
            }
        }
        Err(error) => {
            dialog.sending = false;
            dialog.error = Some(crate::text::sentence(&error.to_string()));
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;

    fn dialog(kind: DialogKind, repos: &[&str]) -> LaunchDialog {
        let mut model = Model::new(
            crate::theme::Theme::default(),
            crate::keymap::Keymap::default(),
            None,
            (120, 40),
            volee_core::time::Timestamp::from_millis(0),
        );
        model.settings.repos = repos.iter().map(|repo| (*repo).to_owned()).collect();
        open(&mut model, kind);
        let Some(Overlay::Launch(dialog)) = model.overlay else {
            panic!("no dialog");
        };
        *dialog
    }

    fn key(code: KeyCode) -> KeyCombo {
        KeyCombo::from_event(&KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn typed(dialog: &mut LaunchDialog, text: &str) {
        let field = &mut dialog.fields[dialog.focus];
        for c in text.chars() {
            edit(&mut field.value, key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn an_agent_needs_a_repository_a_branch_and_a_task() {
        let mut dialog = dialog(DialogKind::Agent, &["billing", "dashboard"]);
        let mut effects = Vec::new();
        edit(&mut dialog.fields[0].value, key(KeyCode::Right));
        submit(&mut dialog, &mut effects);
        assert_eq!(dialog.error.as_deref(), Some("The agent needs a branch."));
        dialog.focus = 1;
        typed(&mut dialog, "feat/export csv");
        submit(&mut dialog, &mut effects);
        assert_eq!(dialog.error.as_deref(), Some("The agent needs a task."));
        dialog.focus = 2;
        typed(&mut dialog, "Export invoices as CSV.");
        submit(&mut dialog, &mut effects);
        assert!(dialog.error.as_deref().unwrap().contains("no spaces"));
        dialog.focus = 1;
        for _ in 0..4 {
            edit(&mut dialog.fields[1].value, key(KeyCode::Backspace));
        }
        typed(&mut dialog, "-csv");
        submit(&mut dialog, &mut effects);
        assert_eq!(dialog.error, None);
        assert!(dialog.sending);
        assert_eq!(
            effects,
            [Effect::Spawn(SpawnRequest {
                repo: "dashboard".into(),
                branch: "feat/export-csv".into(),
                task: "Export invoices as CSV.".into(),
                model: None,
            })]
        );
        submit(&mut dialog, &mut effects);
        assert_eq!(effects.len(), 1, "launched once");
    }

    #[test]
    fn without_repositories_an_agent_takes_a_path() {
        let dialog = dialog(DialogKind::Agent, &[]);
        assert!(matches!(dialog.fields[0].value, Value::Line(_)));
    }

    #[test]
    fn a_mission_picks_its_kind_and_repositories() {
        let mut dialog = dialog(DialogKind::Mission, &["billing", "dashboard", "gateway"]);
        let mut effects = Vec::new();
        typed(&mut dialog, "Invoice export");
        dialog.focus = 1;
        edit(&mut dialog.fields[1].value, key(KeyCode::Right));
        dialog.focus = 2;
        submit(&mut dialog, &mut effects);
        assert!(
            dialog
                .error
                .as_deref()
                .unwrap()
                .starts_with("Choose at least one")
        );
        edit(&mut dialog.fields[2].value, key(KeyCode::Char(' ')));
        edit(&mut dialog.fields[2].value, key(KeyCode::Right));
        edit(&mut dialog.fields[2].value, key(KeyCode::Right));
        edit(&mut dialog.fields[2].value, key(KeyCode::Char(' ')));
        dialog.focus = 3;
        typed(&mut dialog, "Find why exports are slow.");
        submit(&mut dialog, &mut effects);
        assert_eq!(
            effects,
            [Effect::CreateMission(MissionRequest {
                title: "Invoice export".into(),
                kind: MissionKind::Analysis,
                repos: vec!["billing".into(), "gateway".into()],
                goal: "Find why exports are slow.".into(),
            })]
        );
    }

    mod keys {
        use volee_core::event::{Event, EventKind};
        use volee_core::harness::HarnessKind;
        use volee_core::role::{LaunchedBy, Role};
        use volee_core::workspace::{Location, Workspace};
        use volee_core::{AgentId, MissionId};

        use super::*;
        use crate::app::{Msg, update};
        use crate::test_support::{ctrl, key, model, press, start};

        fn new_agent() -> AgentId {
            AgentId::from_ulid(ulid::Ulid::from_parts(9, 9))
        }

        fn launches(effects: &[Effect]) -> bool {
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::Spawn(_) | Effect::CreateMission(_)))
        }

        fn focused(model: &Model) -> (usize, String) {
            let Some(Overlay::Launch(dialog)) = &model.overlay else {
                panic!("the dialog closed");
            };
            (dialog.focus, dialog.fields[dialog.focus].value.text())
        }

        #[test]
        fn n_launches_an_agent_and_selects_it_once_it_appears() {
            let mut model = model(120, 40);
            model.settings.repos = vec!["billing".into()];
            press(&mut model, "n");
            assert_eq!(model.contexts(), [Context::Dialog]);
            key(&mut model, KeyCode::Tab);
            press(&mut model, "feat/export");
            key(&mut model, KeyCode::Tab);
            // `q` and `n` type: the dialog owns the keys.
            press(&mut model, "quick note");
            let effects = ctrl(&mut model, 's');
            assert!(
                matches!(effects.as_slice(), [Effect::Spawn(_)]),
                "{effects:?}"
            );
            update(&mut model, Msg::Launched(Ok(new_agent())));
            assert_eq!(model.overlay, None);
            assert_eq!(model.awaited, Some(new_agent()));
            let effects = update(
                &mut model,
                Msg::Events(vec![Event::new(
                    start(),
                    EventKind::AgentRegistered {
                        agent: new_agent(),
                        name: "billing-feat-export".into(),
                        role: Role::Lead,
                        harness: HarnessKind::ClaudeCode,
                        launched_by: LaunchedBy::Human,
                        parent: None,
                        mission: None,
                        workspace: Workspace::at("/home/dev/code/billing"),
                        location: Location::Background,
                    },
                )]),
            );
            assert_eq!(model.selected, Some(new_agent()));
            assert_eq!(model.awaited, None);
            assert!(
                effects.contains(&Effect::LoadPreview(new_agent())),
                "its screen is asked for: {effects:?}"
            );
        }

        #[test]
        fn a_refusal_stays_in_the_dialog_until_fixed() {
            let mut model = model(120, 40);
            model.settings.repos = vec!["billing".into()];
            press(&mut model, "N");
            press(&mut model, "Export");
            key(&mut model, KeyCode::Tab);
            key(&mut model, KeyCode::Tab);
            key(&mut model, KeyCode::Char(' '));
            key(&mut model, KeyCode::Tab);
            press(&mut model, "Export invoices.");
            assert!(matches!(
                ctrl(&mut model, 's').as_slice(),
                [Effect::CreateMission(_)]
            ));
            update(
                &mut model,
                Msg::MissionCreated(Err(SourceError::Failed(
                    "a mission needs at least one repository".into(),
                ))),
            );
            let Some(Overlay::Launch(dialog)) = &model.overlay else {
                panic!("the dialog closed");
            };
            assert_eq!(
                dialog.error.as_deref(),
                Some("A mission needs at least one repository")
            );
            assert!(!dialog.sending);
            update(
                &mut model,
                Msg::MissionCreated(Ok(NewMission {
                    mission: MissionId::from_ulid(ulid::Ulid::from_parts(9, 1)),
                    coordinator: None,
                })),
            );
            assert_eq!(model.overlay, None);
            let notice = model.notice.as_ref().unwrap();
            assert!(notice.error);
            assert!(notice.text.starts_with("Mission created, but"));
        }

        #[test]
        fn enter_moves_on_or_starts_a_line_and_never_launches() {
            let mut model = model(120, 40);
            model.settings.repos = vec!["billing".into()];
            press(&mut model, "n");
            // From the repository and the branch, Enter moves on.
            assert!(!launches(&key(&mut model, KeyCode::Enter)));
            press(&mut model, "feat/export");
            assert!(!launches(&key(&mut model, KeyCode::Enter)));
            // In the task, with everything a launch needs filled in, it
            // starts a new line.
            press(&mut model, "Export invoices.");
            assert!(!launches(&key(&mut model, KeyCode::Enter)));
            press(&mut model, "One file per month.");
            assert_eq!(
                focused(&model),
                (2, "Export invoices.\nOne file per month.".to_owned())
            );
            // From the last field, Enter goes round to the first.
            key(&mut model, KeyCode::Tab);
            assert!(!launches(&key(&mut model, KeyCode::Enter)));
            assert_eq!(focused(&model).0, 0);
            // Only ctrl-s launches, with the whole task.
            assert_eq!(
                ctrl(&mut model, 's'),
                [Effect::Spawn(SpawnRequest {
                    repo: "billing".into(),
                    branch: "feat/export".into(),
                    task: "Export invoices.\nOne file per month.".into(),
                    model: None,
                })]
            );
        }

        #[test]
        fn a_mission_s_goal_takes_several_lines() {
            let mut model = model(120, 40);
            model.settings.repos = vec!["billing".into()];
            press(&mut model, "N");
            press(&mut model, "Export");
            // Title, kind, then repositories: Enter moves on.
            key(&mut model, KeyCode::Enter);
            key(&mut model, KeyCode::Enter);
            key(&mut model, KeyCode::Char(' '));
            key(&mut model, KeyCode::Enter);
            press(&mut model, "Export invoices.");
            assert!(!launches(&key(&mut model, KeyCode::Enter)));
            press(&mut model, "Then report.");
            assert_eq!(
                focused(&model),
                (3, "Export invoices.\nThen report.".to_owned())
            );
            assert_eq!(
                ctrl(&mut model, 's'),
                [Effect::CreateMission(MissionRequest {
                    title: "Export".into(),
                    kind: MissionKind::Change,
                    repos: vec!["billing".into()],
                    goal: "Export invoices.\nThen report.".into(),
                })]
            );
        }

        #[test]
        fn escape_cancels_and_ctrl_c_quits() {
            let mut model = model(120, 40);
            press(&mut model, "n");
            key(&mut model, KeyCode::Esc);
            assert_eq!(model.overlay, None);
            press(&mut model, "n");
            let effects = update(
                &mut model,
                Msg::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            );
            assert_eq!(effects, [Effect::Quit]);
        }
    }
}
