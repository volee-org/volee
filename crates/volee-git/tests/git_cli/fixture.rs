//! Temporary repositories for the integration tests.
//!
//! Every git command of the tests, the fixture's own and the ones `GitCli`
//! runs, gets the same environment: a temporary `HOME`, no global or system
//! configuration, and a ceiling at the sandbox so git never discovers a
//! repository above it. The tests therefore never read the developer's
//! settings and never touch their home directory.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;
use volee_git::GitCli;

/// Variables that would tie git to a repository, cleared in case the tests
/// run from a git hook.
const REPOSITORY_ENV: [&str; 5] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
];

/// A temporary directory that holds repositories, removed on drop.
pub(crate) struct Sandbox {
    _dir: TempDir,
    root: PathBuf,
}

impl Sandbox {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().expect("create a temporary directory");
        // git prints real paths: on macOS the temporary directory is under
        // `/var`, a link to `/private/var`.
        let root = dir
            .path()
            .canonicalize()
            .expect("resolve the temporary directory");
        fs::create_dir_all(root.join("home/.config")).expect("create the temporary home");
        Self { _dir: dir, root }
    }

    /// A path inside the sandbox.
    pub(crate) fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// A new, empty directory inside the sandbox.
    pub(crate) fn dir(&self, relative: &str) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(&path).expect("create a directory");
        path
    }

    /// The environment of every git command in the tests.
    fn env(&self) -> Vec<(&'static str, OsString)> {
        let home = self.root.join("home");
        vec![
            ("GIT_CONFIG_GLOBAL", "/dev/null".into()),
            ("GIT_CONFIG_NOSYSTEM", "1".into()),
            ("HOME", home.clone().into()),
            ("XDG_CONFIG_HOME", home.join(".config").into()),
            ("GIT_CEILING_DIRECTORIES", self.root.clone().into()),
            ("GIT_TERMINAL_PROMPT", "0".into()),
            ("LC_ALL", "C".into()),
        ]
    }

    /// A `GitCli` that runs git with the sandbox's environment.
    pub(crate) fn cli(&self) -> GitCli {
        self.cli_with(GitCli::new())
    }

    /// `cli` with the sandbox's environment added.
    pub(crate) fn cli_with(&self, cli: GitCli) -> GitCli {
        self.env()
            .into_iter()
            .fold(cli, |cli, (key, value)| cli.env(key, value))
    }

    /// Runs git in `dir` and returns how it ended.
    pub(crate) fn try_git(&self, dir: &Path, args: &[&str]) -> Output {
        let mut command = Command::new("git");
        command.arg("-C").arg(dir).args(args).envs(self.env());
        for key in REPOSITORY_ENV {
            command.env_remove(key);
        }
        command.output().expect("run git")
    }

    /// Runs git in `dir` and returns its trimmed standard output. Panics
    /// with git's error when it fails.
    pub(crate) fn git(&self, dir: &Path, args: &[&str]) -> String {
        let output = self.try_git(dir, args);
        assert!(
            output.status.success(),
            "git {} failed in {}: {}",
            args.join(" "),
            dir.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("git printed UTF-8")
            .trim()
            .to_owned()
    }

    /// A repository on `main` without any commit.
    pub(crate) fn empty_repo(&self, name: &str) -> PathBuf {
        let path = self.path(name);
        self.git(
            &self.root,
            &["init", "--quiet", "--initial-branch=main", name],
        );
        self.configure(&path);
        path
    }

    /// A repository on `main` with one commit.
    pub(crate) fn repo(&self, name: &str) -> PathBuf {
        let path = self.empty_repo(name);
        self.commit(&path, "README.md", "# Sample\n", "Initial commit");
        path
    }

    /// A bare repository whose `HEAD` is `main`, to use as `origin`.
    pub(crate) fn bare(&self, name: &str) -> PathBuf {
        self.git(
            &self.root,
            &["init", "--quiet", "--bare", "--initial-branch=main", name],
        );
        self.path(name)
    }

    /// A clone of a new bare `origin` that has one commit on `main`, made
    /// the way a person would, so `origin/HEAD` is set. Returns the clone
    /// and the origin.
    pub(crate) fn cloned(&self, name: &str) -> (PathBuf, PathBuf) {
        let origin = self.bare(&format!("{name}-origin.git"));
        let seed = self.repo(&format!("{name}-seed"));
        self.git(&seed, &["remote", "add", "origin", path_str(&origin)]);
        self.git(&seed, &["push", "--quiet", "origin", "main"]);
        (self.clone_of(&origin, name), origin)
    }

    /// Another clone of `origin`, such as a teammate's.
    pub(crate) fn clone_of(&self, origin: &Path, name: &str) -> PathBuf {
        self.git(&self.root, &["clone", "--quiet", path_str(origin), name]);
        let path = self.path(name);
        self.configure(&path);
        path
    }

    fn configure(&self, repo: &Path) {
        self.git(repo, &["config", "user.name", "Volee Tests"]);
        self.git(repo, &["config", "user.email", "tests@volee.invalid"]);
    }

    /// Writes `contents` to `path`, creating its directories. Refuses paths
    /// outside the sandbox, so a test can never write elsewhere.
    pub(crate) fn write(&self, path: &Path, contents: &str) {
        assert!(
            path.starts_with(&self.root),
            "{} is outside the sandbox",
            path.display()
        );
        fs::create_dir_all(path.parent().expect("a file has a parent"))
            .expect("create directories");
        fs::write(path, contents).expect("write a file");
    }

    /// Writes `file` in `repo`, commits it and returns the new commit.
    pub(crate) fn commit(&self, repo: &Path, file: &str, contents: &str, message: &str) -> String {
        self.write(&repo.join(file), contents);
        self.git(repo, &["add", "--", file]);
        self.git(repo, &["commit", "--quiet", "--message", message]);
        self.head(repo)
    }

    /// The commit checked out in `repo`.
    pub(crate) fn head(&self, repo: &Path) -> String {
        self.git(repo, &["rev-parse", "HEAD"])
    }

    /// The commit a revision names in `repo`.
    pub(crate) fn rev(&self, repo: &Path, revision: &str) -> String {
        self.git(repo, &["rev-parse", "--verify", revision])
    }

    /// Whether `revision` names anything in `repo`.
    pub(crate) fn has_rev(&self, repo: &Path, revision: &str) -> bool {
        self.try_git(repo, &["rev-parse", "--verify", "--quiet", revision])
            .status
            .success()
    }

    /// Creates the worktree of a new branch `branch` from `start`, without
    /// volee, as a person would.
    pub(crate) fn worktree(&self, repo: &Path, name: &str, branch: &str, start: &str) -> PathBuf {
        let path = self.path(name);
        self.git(
            repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "--no-track",
                "-b",
                branch,
                path_str(&path),
                start,
            ],
        );
        path
    }
}

/// A path as a command-line argument. Sandbox paths are UTF-8.
pub(crate) fn path_str(path: &Path) -> &str {
    path.to_str().expect("sandbox paths are UTF-8")
}
