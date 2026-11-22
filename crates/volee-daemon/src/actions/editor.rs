//! Opening a file in the person's own editor (v2, step 9).
//!
//! The editors come from the rules (`editors`, each `{ name, kind, cmd }`).
//! volee never builds a shell line: the template is split into words by
//! `volee-rules`, the placeholders are filled after, and the program is run
//! with exactly those arguments. A `terminal` editor goes through the
//! terminal adapter (Rex: a floating pane in the agent's session), an
//! `external` one is just run.

use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use volee_rules::{Editor, EditorKind, EditorTarget};

/// What to run, resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub(crate) kind: EditorKind,
    pub(crate) name: String,
    pub(crate) argv: Vec<String>,
    /// Where it runs: the agent's worktree.
    pub(crate) cwd: PathBuf,
}

/// The absolute path of `file` (relative to the worktree), when it stays
/// inside the worktree and exists.
pub(crate) fn inside(worktree: &Path, file: &str) -> Result<PathBuf, String> {
    let relative = Path::new(file);
    if file.is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!("{file:?} is not a path inside the worktree"));
    }
    let path = worktree.join(relative);
    if !path.is_file() {
        return Err(format!("{file} does not exist in the worktree"));
    }
    Ok(path)
}

/// The command for editor `name`, filled for `file` at `line`.
pub(crate) fn resolve(
    editors: &[Editor],
    name: &str,
    worktree: &Path,
    repo: &str,
    file: &str,
    line: u32,
) -> Result<Resolved, String> {
    let editor = editors
        .iter()
        .find(|editor| editor.name == name)
        .ok_or_else(|| format!("no editor named {name:?} in your rules"))?;
    let path = inside(worktree, file)?;
    let argv = editor
        .command(&EditorTarget {
            file: path.to_string_lossy().into_owned(),
            line: line.max(1),
            dir: worktree.to_string_lossy().into_owned(),
            repo: repo.to_owned(),
        })
        .map_err(|error| error.to_string())?;
    Ok(Resolved {
        kind: editor.kind,
        name: editor.name.clone(),
        argv,
        cwd: worktree.to_path_buf(),
    })
}

/// Runs an external editor's command, detached: the person's editor lives
/// on after the daemon's request ends.
pub(crate) fn run_external(resolved: &Resolved) -> Result<String, String> {
    let (program, args) = resolved
        .argv
        .split_first()
        .ok_or_else(|| "the editor command is empty".to_owned())?;
    let mut child = Command::new(program)
        .args(args)
        .current_dir(&resolved.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    // Reap it when it ends, so it never stays a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(format!("Opened with {}", resolved.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editors() -> Vec<Editor> {
        vec![
            Editor {
                name: "nvim".into(),
                kind: EditorKind::Terminal,
                cmd: "nvim +{line} {file}".into(),
            },
            Editor {
                name: "code".into(),
                kind: EditorKind::External,
                cmd: "code -g {file}:{line} --folder-uri 'a b {repo}'".into(),
            },
        ]
    }

    #[test]
    fn the_argv_has_the_file_and_line_filled_after_the_split() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("my tree");
        std::fs::create_dir_all(tree.join("src")).unwrap();
        std::fs::write(tree.join("src/a b.rs"), "x").unwrap();
        let nvim = resolve(&editors(), "nvim", &tree, "alpha", "src/a b.rs", 12).unwrap();
        assert_eq!(nvim.kind, EditorKind::Terminal);
        assert_eq!(nvim.argv.len(), 3, "a path with spaces stays one argument");
        assert_eq!(nvim.argv[1], "+12");
        assert!(nvim.argv[2].ends_with("my tree/src/a b.rs"));
        let code = resolve(&editors(), "code", &tree, "alpha", "src/a b.rs", 3).unwrap();
        assert_eq!(code.kind, EditorKind::External);
        assert!(code.argv[2].ends_with("a b.rs:3"));
        assert_eq!(code.argv[4], "a b alpha", "quotes group words");
    }

    #[test]
    fn a_file_outside_the_worktree_or_missing_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("here.txt"), "x").unwrap();
        for bad in ["../x", "/etc/passwd", "a/../../b", ""] {
            assert!(inside(dir.path(), bad).is_err(), "{bad}");
        }
        assert!(inside(dir.path(), "nope.txt").is_err());
        assert!(inside(dir.path(), "here.txt").is_ok());
        assert!(resolve(&editors(), "emacs", dir.path(), "r", "here.txt", 1).is_err());
    }

    #[test]
    fn an_external_editor_runs_with_exactly_its_arguments() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "x").unwrap();
        let log = dir.path().join("argv.log");
        let script = dir.path().join("fake-editor.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do echo \"$a\" >> '{}'; done\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let editors = vec![Editor {
            name: "fake".into(),
            kind: EditorKind::External,
            cmd: format!("{} --at {{line}} {{file}}", script.display()),
        }];
        let resolved = resolve(&editors, "fake", dir.path(), "r", "f.txt", 7).unwrap();
        run_external(&resolved).unwrap();
        for _ in 0..50 {
            if log.is_file() && std::fs::read_to_string(&log).unwrap().lines().count() >= 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let lines: Vec<String> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(lines[0], "--at");
        assert_eq!(lines[1], "7");
        assert!(lines[2].ends_with("f.txt"));
    }
}
