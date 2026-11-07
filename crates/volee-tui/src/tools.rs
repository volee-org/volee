//! Programs the person can open an agent's worktree with. None is required:
//! the command line passes the ones it finds or the configuration names
//! (`[tui.tools]`), and the interface only offers those.
//!
//! An editor, a diff viewer and a git interface run in this terminal: the
//! interface steps aside as for a take over and comes back when they exit.
//! An editor with a window of its own starts in the background.

use std::path::PathBuf;

use volee_core::{Agent, AgentId};

use crate::app::{Effect, Model, Overlay};
use crate::keymap::Action;
use crate::source::TakeOver;

/// What a tool is for, which also picks its key in the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum ToolKind {
    /// An editor that runs in the terminal.
    Editor,
    /// A diff viewer that runs in the terminal.
    Diff,
    /// A git interface that runs in the terminal.
    Git,
    /// An editor with a window of its own, started in the background.
    Ide,
}

impl ToolKind {
    /// Every kind, in menu order.
    pub const ALL: [Self; 4] = [Self::Editor, Self::Diff, Self::Git, Self::Ide];

    /// Whether the tool takes this terminal until it exits, rather than
    /// opening a window of its own.
    pub const fn runs_in_terminal(self) -> bool {
        !matches!(self, Self::Ide)
    }

    /// The word in the menu.
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Editor => "editor",
            Self::Diff => "diff",
            Self::Git => "git",
            Self::Ide => "window",
        }
    }

    /// The menu's action for this kind.
    pub(crate) const fn action(self) -> Action {
        match self {
            Self::Editor => Action::OpenEditor,
            Self::Diff => Action::OpenDiff,
            Self::Git => Action::OpenGit,
            Self::Ide => Action::OpenIde,
        }
    }
}

/// A program to open an agent's worktree with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tool {
    /// What it is for.
    pub kind: ToolKind,
    /// Its name in the menu, such as `nvim` or `VS Code`.
    pub label: String,
    /// The program and its arguments. `{dir}` stands for the worktree and
    /// `{base}` for the branch the agent started from (empty when unknown).
    /// It runs in the worktree.
    pub command: Vec<String>,
}

/// The menu of tools for one agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ToolMenu {
    /// Whose worktree.
    pub(crate) agent: AgentId,
}

/// Where an agent's work is: its worktree, else its directory.
pub(crate) fn dir_of(agent: &Agent) -> PathBuf {
    agent
        .workspace
        .worktree
        .clone()
        .unwrap_or_else(|| agent.workspace.cwd.clone())
}

/// A tool's command on an agent's worktree, its placeholders replaced.
pub(crate) fn take_over(tool: &Tool, agent: &Agent) -> TakeOver {
    let dir = dir_of(agent);
    let shown = dir.display().to_string();
    let base = agent.workspace.base.as_deref().unwrap_or("");
    let mut words = tool
        .command
        .iter()
        .map(|word| word.replace("{dir}", &shown).replace("{base}", base));
    TakeOver {
        program: words.next().unwrap_or_default(),
        args: words.collect(),
        cwd: Some(dir),
        env_remove: Vec::new(),
        hint: None,
    }
}

/// Opens the menu on an agent.
pub(crate) fn open_menu(model: &mut Model, agent: AgentId) {
    model.overlay = Some(Overlay::Tools(ToolMenu { agent }));
}

/// The menu's keys: a tool's key runs it; the menu closes either way.
pub(crate) fn perform(model: &mut Model, action: Action, effects: &mut Vec<Effect>) {
    let Some(Overlay::Tools(menu)) = model.overlay else {
        return;
    };
    if action == Action::Back {
        model.overlay = None;
        return;
    }
    let Some(kind) = ToolKind::ALL
        .into_iter()
        .find(|kind| kind.action() == action)
    else {
        return;
    };
    let tool = model
        .settings
        .tools
        .iter()
        .find(|tool| tool.kind == kind)
        .cloned();
    let Some(agent) = model.registry.agent(menu.agent) else {
        model.overlay = None;
        return;
    };
    let Some(tool) = tool else {
        model.notify(
            format!(
                "No {} tool: set one under [tui.tools] in the configuration.",
                kind.title()
            ),
            false,
        );
        return;
    };
    let take_over = take_over(&tool, agent);
    model.overlay = None;
    effects.push(Effect::RunTool {
        label: tool.label,
        detached: !kind.runs_in_terminal(),
        take_over,
    });
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use volee_core::Registry;
    use volee_core::time::Timestamp;

    use super::*;
    use crate::clock::ManualClock;
    use crate::demo::{DemoSource, INFRA, INVOICE_PAGE, agent_id};

    fn agents() -> Registry {
        let start = Timestamp::from_millis(1_793_986_617_000);
        let demo = DemoSource::new(Arc::new(ManualClock::new(start)));
        Registry::from_events(&demo.events_until(start)).unwrap()
    }

    #[test]
    fn placeholders_name_the_worktree_and_its_base() {
        let start = Timestamp::from_millis(1_793_986_617_000);
        let registry = agents();
        let page = registry.agent(agent_id(start, INVOICE_PAGE)).unwrap();
        let diff = Tool {
            kind: ToolKind::Diff,
            label: "nvim".into(),
            command: vec!["nvim".into(), "-c".into(), "DiffviewOpen {base}".into()],
        };
        let run = take_over(&diff, page);
        assert_eq!(run.program, "nvim");
        assert_eq!(run.args, ["-c", "DiffviewOpen main"]);
        let dir = run.cwd.unwrap();
        assert!(
            dir.ends_with("worktrees/dashboard/feat-invoices-page"),
            "{dir:?}"
        );
        let code = Tool {
            kind: ToolKind::Ide,
            label: "VS Code".into(),
            command: vec!["code".into(), "{dir}".into()],
        };
        assert_eq!(take_over(&code, page).args, [dir.display().to_string()]);
        // Any agent's work is in its worktree, else its folder.
        let infra = registry.agent(agent_id(start, INFRA)).unwrap();
        assert!(dir_of(infra).ends_with(Path::new("infra/main")));
        assert!(ToolKind::Git.runs_in_terminal());
        assert!(!ToolKind::Ide.runs_in_terminal());
    }

    mod keys {
        use super::*;
        use crate::keymap::Context;
        use crate::test_support::{id, model, press};

        fn tools() -> Vec<Tool> {
            vec![
                Tool {
                    kind: ToolKind::Editor,
                    label: "nvim".into(),
                    command: vec!["nvim".into(), ".".into()],
                },
                Tool {
                    kind: ToolKind::Ide,
                    label: "VS Code".into(),
                    command: vec!["code".into(), "{dir}".into()],
                },
            ]
        }

        #[test]
        fn o_offers_the_tools_and_runs_the_one_chosen() {
            let mut model = model(120, 40);
            model.settings.tools = tools();
            press(&mut model, "'po");
            assert_eq!(model.contexts(), [Context::Tools]);
            let effects = press(&mut model, "e");
            assert_eq!(model.overlay, None);
            let [
                Effect::RunTool {
                    label,
                    detached,
                    take_over,
                },
            ] = effects.as_slice()
            else {
                panic!("nothing to run: {effects:?}");
            };
            assert_eq!((label.as_str(), *detached), ("nvim", false));
            assert_eq!(take_over.program, "nvim");
            assert!(
                take_over
                    .cwd
                    .as_ref()
                    .unwrap()
                    .ends_with("feat-invoices-page")
            );
            let effects = press(&mut model, "ow");
            assert!(matches!(
                effects.as_slice(),
                [Effect::RunTool { detached: true, .. }]
            ));
            assert!(press(&mut model, "og").is_empty(), "no git tool");
            assert_eq!(
                model.notice.as_ref().unwrap().text,
                "No git tool: set one under [tui.tools] in the configuration."
            );
            press(&mut model, "q");
            assert_eq!(model.overlay, None);
        }

        #[test]
        fn the_changes_window_opens_the_tools_on_its_agent() {
            let mut model = model(120, 40);
            model.settings.tools = tools();
            press(&mut model, "'pdo");
            assert_eq!(
                model.overlay,
                Some(Overlay::Tools(ToolMenu {
                    agent: id(INVOICE_PAGE)
                }))
            );
        }
    }
}
