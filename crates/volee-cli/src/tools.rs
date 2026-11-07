//! The programs the interface offers to open a worktree with: the ones the
//! configuration names under `[tui.tools]`, else the ones found on this
//! machine. None is required, and none is forced: an empty setting turns a
//! tool off.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use volee_config::{Tools, split_command};
use volee_tui::{Tool, ToolKind};

/// What the resolution reads from the machine, so tests can fake it.
pub(crate) struct Machine<'a> {
    /// An environment variable.
    pub(crate) var: &'a dyn Fn(&str) -> Option<String>,
    /// Whether a program can be run: found on `PATH`, or a path to a file.
    pub(crate) has: &'a dyn Fn(&str) -> bool,
    /// Whether Neovim has the diffview plugin.
    pub(crate) diffview: bool,
}

/// The tools to offer, in menu order.
pub(crate) fn resolve(config: &Tools, machine: &Machine<'_>) -> Result<Vec<Tool>> {
    let mut tools = Vec::new();
    for (kind, setting) in [
        (ToolKind::Editor, config.editor.as_deref()),
        (ToolKind::Diff, config.diff.as_deref()),
        (ToolKind::Git, config.git.as_deref()),
        (ToolKind::Ide, config.ide.as_deref()),
    ] {
        let command = match setting {
            // Turned off.
            Some(line) if line.trim().is_empty() => continue,
            // Named by the person: offered as written.
            Some(line) => split_command(line).map_err(|error| anyhow!("[tui.tools]: {error}"))?,
            None => match found(kind, machine) {
                Some(command) => command,
                None => continue,
            },
        };
        let Some(program) = command.first() else {
            continue;
        };
        tools.push(Tool {
            kind,
            label: label(program),
            command,
        });
    }
    Ok(tools)
}

/// The default command of a kind, when its program is on this machine.
fn found(kind: ToolKind, machine: &Machine<'_>) -> Option<Vec<String>> {
    let words = |line: &str| split_command(line).ok().filter(|words| !words.is_empty());
    let command = match kind {
        ToolKind::Editor => {
            let editor = ["VISUAL", "EDITOR"]
                .into_iter()
                .find_map(|name| (machine.var)(name).and_then(|line| words(&line)))?;
            let mut command = editor;
            command.push(".".to_owned());
            command
        }
        ToolKind::Diff if machine.diffview => {
            vec!["nvim".into(), "-c".into(), "DiffviewOpen {base}".into()]
        }
        ToolKind::Git => vec!["lazygit".into()],
        ToolKind::Ide => vec!["code".into(), "{dir}".into()],
        _ => return None,
    };
    (machine.has)(&command[0]).then_some(command)
}

/// A program's name in the menu.
fn label(program: &str) -> String {
    let name = Path::new(program).file_name().map_or_else(
        || program.to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    match name.as_str() {
        "code" => "VS Code".to_owned(),
        _ => name,
    }
}

/// Whether `program` can be run from `path` (the `PATH` variable).
pub(crate) fn on_path(program: &str, path: Option<&str>) -> bool {
    if program.contains('/') {
        return is_executable(Path::new(program));
    }
    path.into_iter()
        .flat_map(std::env::split_paths)
        .any(|dir| is_executable(&dir.join(program)))
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Whether Neovim has diffview.nvim, looking where plugin managers put it
/// under Neovim's data directory: lazy.nvim, packer and native packages,
/// and vim-plug.
pub(crate) fn diffview_installed(data: &Path) -> bool {
    let nvim = data.join("nvim");
    let packs = nvim.join("site").join("pack");
    let in_packs = std::fs::read_dir(&packs).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            ["start", "opt"]
                .iter()
                .any(|kind| entry.path().join(kind).join("diffview.nvim").is_dir())
        })
    });
    in_packs
        || [nvim.join("lazy"), nvim.join("plugged")]
            .iter()
            .any(|dir| dir.join("diffview.nvim").is_dir())
}

/// Neovim's data directory's parent: `$XDG_DATA_HOME`, else
/// `~/.local/share`.
pub(crate) fn data_home(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    var("XDG_DATA_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|home| PathBuf::from(home).join(".local/share")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(
        config: &Tools,
        vars: &[(&str, &str)],
        installed: &[&str],
        diffview: bool,
    ) -> Vec<(ToolKind, String, Vec<String>)> {
        let var = |name: &str| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        };
        let has = |program: &str| installed.contains(&program);
        let machine = Machine {
            var: &var,
            has: &has,
            diffview,
        };
        resolve(config, &machine)
            .unwrap()
            .into_iter()
            .map(|tool| (tool.kind, tool.label, tool.command))
            .collect()
    }

    #[test]
    fn found_tools_are_offered_and_missing_ones_are_not() {
        let tools = resolved(
            &Tools::default(),
            &[("EDITOR", "nvim"), ("VISUAL", "")],
            &["nvim", "code"],
            true,
        );
        assert_eq!(
            tools,
            [
                (
                    ToolKind::Editor,
                    "nvim".into(),
                    vec!["nvim".into(), ".".into()]
                ),
                (
                    ToolKind::Diff,
                    "nvim".into(),
                    vec!["nvim".into(), "-c".into(), "DiffviewOpen {base}".into()]
                ),
                (
                    ToolKind::Ide,
                    "VS Code".into(),
                    vec!["code".into(), "{dir}".into()]
                ),
            ]
        );
        // Neither lazygit nor diffview: no git tool, no diff tool.
        let tools = resolved(&Tools::default(), &[("EDITOR", "hx")], &["hx"], false);
        assert_eq!(tools.len(), 1);
    }

    #[test]
    fn the_configuration_wins_and_empty_turns_a_tool_off() {
        let config = Tools {
            editor: Some("emacsclient -t".into()),
            diff: Some(String::new()),
            git: Some("gitui".into()),
            ide: None,
        };
        let tools = resolved(&config, &[("EDITOR", "nvim")], &["nvim", "lazygit"], true);
        assert_eq!(
            tools,
            [
                (
                    ToolKind::Editor,
                    "emacsclient".into(),
                    vec!["emacsclient".into(), "-t".into()]
                ),
                (ToolKind::Git, "gitui".into(), vec!["gitui".into()]),
            ]
        );
    }

    #[test]
    fn programs_are_found_on_the_path_and_diffview_where_managers_put_it() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let program = bin.join("lazygit");
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = bin.display().to_string();
        assert!(on_path("lazygit", Some(&path)));
        assert!(!on_path("code", Some(&path)));
        assert!(on_path(&program.display().to_string(), None));
        assert!(!on_path("lazygit", None));

        let data = dir.path().join("share");
        assert!(!diffview_installed(&data));
        std::fs::create_dir_all(data.join("nvim/lazy/diffview.nvim")).unwrap();
        assert!(diffview_installed(&data));
        let other = dir.path().join("other");
        std::fs::create_dir_all(other.join("nvim/site/pack/plugins/start/diffview.nvim")).unwrap();
        assert!(diffview_installed(&other));
        assert_eq!(
            data_home(|name| (name == "HOME").then(|| "/home/ada".to_owned())),
            Some(PathBuf::from("/home/ada/.local/share"))
        );
    }
}
