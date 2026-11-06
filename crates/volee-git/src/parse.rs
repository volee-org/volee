//! Pure parsers for what git prints.
//!
//! Every function here takes git's output and returns plain data, so the
//! formats are tested on fixture strings without running git. Outputs that
//! carry paths are read as bytes: a path is not always valid UTF-8.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use volee_core::delivery::{DiffStat, PushState};

use crate::WorktreeInfo;

/// Output that does not have the expected shape.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub(crate) struct ParseError(String);

impl ParseError {
    pub(crate) fn new(detail: impl Into<String>) -> Self {
        Self(detail.into())
    }
}

/// What `git status --porcelain=v2 --branch -z` says about a working tree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StatusSummary {
    /// The checked-out commit; `None` before the first commit.
    pub(crate) oid: Option<String>,
    /// The checked-out branch; `None` when `HEAD` is detached.
    pub(crate) branch: Option<String>,
    /// Changed tracked files, conflicts included.
    pub(crate) changed: usize,
    /// Untracked files and directories that are not ignored.
    pub(crate) untracked: usize,
}

impl StatusSummary {
    /// Whether the working tree has anything not committed.
    pub(crate) fn is_dirty(&self) -> bool {
        self.changed > 0 || self.untracked > 0
    }
}

/// Parses `git worktree list --porcelain -z`.
///
/// Each attribute ends with a NUL byte and each worktree ends with an empty
/// attribute. Attributes this parser does not know are skipped, as git's
/// documentation asks of readers of the porcelain format.
pub(crate) fn parse_worktree_porcelain(output: &[u8]) -> Result<Vec<WorktreeInfo>, ParseError> {
    let mut worktrees = Vec::new();
    let mut current: Option<WorktreeInfo> = None;
    for field in output.split(|&byte| byte == 0) {
        if field.is_empty() {
            worktrees.extend(current.take());
            continue;
        }
        let (key, value) = split_attribute(field);
        if key == b"worktree" {
            worktrees.extend(current.take());
            current = Some(WorktreeInfo {
                path: path_from_bytes(value),
                ..WorktreeInfo::default()
            });
            continue;
        }
        let Some(worktree) = current.as_mut() else {
            return Err(ParseError::new(format!(
                "attribute `{}` comes before any `worktree` line",
                lossy(key)
            )));
        };
        match key {
            b"HEAD" => worktree.head = parse_head(value)?,
            b"branch" => worktree.branch = Some(branch_name(&lossy(value))),
            b"detached" => worktree.detached = true,
            b"bare" => worktree.bare = true,
            b"locked" => worktree.locked = Some(lossy(value)),
            b"prunable" => worktree.prunable = Some(lossy(value)),
            _ => {}
        }
    }
    worktrees.extend(current);
    Ok(worktrees)
}

/// Parses `git status --porcelain=v2 --branch -z`.
///
/// Only the `HEAD` headers and the number of entries are kept; other headers
/// (upstream, ahead and behind counts, stash) are skipped. An entry of a kind
/// this parser does not know counts as a change: when in doubt, the tree is
/// dirty, which keeps the cleanup rule on the safe side.
pub(crate) fn parse_status_porcelain_v2(output: &[u8]) -> StatusSummary {
    let mut status = StatusSummary::default();
    let mut fields = output.split(|&byte| byte == 0);
    while let Some(field) = fields.next() {
        match field.first() {
            None | Some(b'!') => {}
            Some(b'#') => read_status_header(&lossy(field), &mut status),
            Some(b'?') => status.untracked += 1,
            Some(b'2') => {
                status.changed += 1;
                // A rename or a copy: the original path is the next field.
                fields.next();
            }
            Some(_) => status.changed += 1,
        }
    }
    status
}

fn read_status_header(field: &str, status: &mut StatusSummary) {
    let Some(header) = field.strip_prefix("# ") else {
        return;
    };
    let (key, value) = header.split_once(' ').unwrap_or((header, ""));
    match key {
        "branch.oid" => status.oid = (value != "(initial)").then(|| value.to_owned()),
        "branch.head" => status.branch = (value != "(detached)").then(|| value.to_owned()),
        _ => {}
    }
}

/// The upstream of a branch, as `git for-each-ref --format=%(upstream)`
/// prints it, when it is a remote-tracking branch. An upstream that is a
/// local branch says nothing about what reached the remote.
pub(crate) fn remote_upstream(output: &str) -> Option<String> {
    let reference = output.trim();
    reference
        .starts_with("refs/remotes/")
        .then(|| reference.to_owned())
}

/// The short name of a remote-tracking branch: `origin/feat` for
/// `refs/remotes/origin/feat`.
pub(crate) fn remote_branch_name(reference: &str) -> String {
    reference
        .strip_prefix("refs/remotes/")
        .unwrap_or(reference)
        .to_owned()
}

/// How many bytes of `output` to keep so it fits in `limit`: everything when
/// it fits, otherwise up to the last line break within the limit, so no line
/// is cut. A single line longer than the limit is cut at a character
/// boundary.
pub(crate) fn truncation_point(output: &[u8], limit: usize) -> usize {
    if output.len() <= limit {
        return output.len();
    }
    let window = &output[..limit];
    if let Some(newline) = window.iter().rposition(|&byte| byte == b'\n') {
        return newline + 1;
    }
    // UTF-8 continuation bytes look like 0b10xx_xxxx: step back to the
    // start of a character.
    let mut cut = limit;
    while cut > 0 && output[cut] & 0b1100_0000 == 0b1000_0000 {
        cut -= 1;
    }
    cut
}

/// Parses the summary line of `git diff --shortstat`, such as
/// ` 3 files changed, 10 insertions(+), 2 deletions(-)`. No output means no
/// change. Counts too large for a `u32` saturate.
///
/// The wording is git's English, which `LC_ALL=C` guarantees.
pub(crate) fn parse_shortstat(output: &str) -> Result<DiffStat, ParseError> {
    let mut stat = DiffStat::default();
    let line = output.trim();
    if line.is_empty() {
        return Ok(stat);
    }
    for part in line.split(',') {
        let part = part.trim();
        let invalid = || ParseError::new(format!("`{part}` is not part of a diff summary"));
        let (count, label) = part.split_once(' ').ok_or_else(invalid)?;
        let count: u64 = count.parse().map_err(|_| invalid())?;
        let count = u32::try_from(count).unwrap_or(u32::MAX);
        if label.starts_with("file") {
            stat.files = count;
        } else if label.starts_with("insertion") {
            stat.insertions = count;
        } else if label.starts_with("deletion") {
            stat.deletions = count;
        } else {
            return Err(invalid());
        }
    }
    Ok(stat)
}

/// Parses a count printed by `git rev-list --count`. Counts too large for a
/// `u32` saturate.
pub(crate) fn parse_count(output: &str) -> Result<u32, ParseError> {
    let text = output.trim();
    let count: u64 = text
        .parse()
        .map_err(|_| ParseError::new(format!("`{text}` is not a count")))?;
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// Parses an object name printed by git, such as a commit: 40 hexadecimal
/// digits (SHA-1) or 64 (SHA-256).
pub(crate) fn parse_object_id(output: &str) -> Result<String, ParseError> {
    let text = output.trim();
    if is_object_id(text) {
        Ok(text.to_owned())
    } else {
        Err(ParseError::new(format!("`{text}` is not an object name")))
    }
}

fn is_object_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Parses `git rev-parse --path-format=absolute --is-inside-work-tree
/// --git-dir --git-common-dir`: whether the directory is in a working tree,
/// its git directory and the repository's common directory.
pub(crate) fn parse_repository_probe(
    output: &[u8],
) -> Result<(bool, PathBuf, PathBuf), ParseError> {
    let mut lines = output
        .strip_suffix(b"\n")
        .unwrap_or(output)
        .split(|&byte| byte == b'\n');
    let mut next = |what: &str| {
        lines
            .next()
            .ok_or_else(|| ParseError::new(format!("the {what} is missing")))
    };
    let inside = match next("work tree flag")? {
        b"true" => true,
        b"false" => false,
        other => {
            return Err(ParseError::new(format!(
                "`{}` is not a boolean",
                lossy(other)
            )));
        }
    };
    let git_dir = path_from_bytes(next("git directory")?);
    let common_dir = path_from_bytes(next("common directory")?);
    Ok((inside, git_dir, common_dir))
}

/// Parses a single path printed on its own line, such as the output of
/// `git rev-parse --show-toplevel`.
pub(crate) fn parse_path_line(output: &[u8]) -> Result<PathBuf, ParseError> {
    let line = output.strip_suffix(b"\n").unwrap_or(output);
    if line.is_empty() {
        Err(ParseError::new("the path is missing"))
    } else {
        Ok(path_from_bytes(line))
    }
}

/// Parses a NUL-separated list of paths, such as `git ls-files -z`.
pub(crate) fn parse_nul_paths(output: &[u8]) -> Vec<PathBuf> {
    output
        .split(|&byte| byte == 0)
        .filter(|path| !path.is_empty())
        .map(path_from_bytes)
        .collect()
}

/// A branch name without its `refs/heads/` prefix. Other references are
/// returned whole.
pub(crate) fn branch_name(reference: &str) -> String {
    let reference = reference.trim();
    reference
        .strip_prefix("refs/heads/")
        .unwrap_or(reference)
        .to_owned()
}

/// The first of `main`, `master` and `develop` among existing references
/// (as printed by `git for-each-ref --format=%(refname)`).
pub(crate) fn pick_default_branch(references: &str) -> Option<String> {
    let existing: Vec<&str> = references.lines().map(str::trim).collect();
    ["main", "master", "develop"]
        .into_iter()
        .find(|name| existing.contains(&format!("refs/heads/{name}").as_str()))
        .map(str::to_owned)
}

/// Whether git's standard error says the directory is in no repository.
///
/// Matching a message is the only way git offers to tell this apart from
/// other failures (such as an unsafe directory); `LC_ALL=C` keeps it in
/// English.
pub(crate) fn reports_no_repository(stderr: &[u8]) -> bool {
    lossy(stderr).contains("not a git repository")
}

/// The push state for a branch `ahead` commits ahead of its remote branch.
pub(crate) fn push_state(ahead: u32) -> PushState {
    if ahead == 0 {
        PushState::UpToDate
    } else {
        PushState::Ahead { commits: ahead }
    }
}

/// The main checkout of a repository whose common directory is
/// `common_dir`, when its layout says so: a common directory named `.git`
/// lives in the main checkout. Bare repositories, submodules and separate git
/// directories return `None`.
pub(crate) fn checkout_of_git_dir(common_dir: &Path) -> Option<PathBuf> {
    if common_dir.file_name() == Some(OsStr::new(".git")) {
        common_dir.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

/// A display name for the repository whose main checkout is
/// `main_checkout`: its directory name without a `.git` suffix, or for a
/// hidden bare repository (`project/.bare`), the name of its parent.
pub(crate) fn repository_name(main_checkout: &Path) -> String {
    let file_name = |path: &Path| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    };
    let Some(name) = file_name(main_checkout) else {
        return main_checkout.display().to_string();
    };
    let stem = name.strip_suffix(".git").unwrap_or(&name);
    if !stem.is_empty() && !stem.starts_with('.') {
        return stem.to_owned();
    }
    main_checkout.parent().and_then(file_name).unwrap_or(name)
}

/// Whether a path printed by git names a directory (it ends with `/`), as
/// `git ls-files --others` prints untracked directories and nested
/// repositories.
pub(crate) fn names_directory(path: &Path) -> bool {
    path.as_os_str().as_encoded_bytes().ends_with(b"/")
}

fn parse_head(value: &[u8]) -> Result<Option<String>, ParseError> {
    let text = lossy(value);
    if !text.is_empty() && text.bytes().all(|byte| byte == b'0') {
        // The null object name: a branch without commits yet.
        return Ok(None);
    }
    parse_object_id(&text).map(Some)
}

fn split_attribute(field: &[u8]) -> (&[u8], &[u8]) {
    match field.iter().position(|&byte| byte == b' ') {
        Some(space) => (&field[..space], &field[space + 1..]),
        None => (field, &[]),
    }
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A path from the bytes git printed, without loss on Unix.
#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(OsStr::from_bytes(bytes))
}

/// A path from the bytes git printed. volee supports Unix only (ADR 0012);
/// elsewhere, paths that are not UTF-8 are not preserved.
#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(lossy(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA_A: &str = "6adc11fa9619748617663ded520ce631e9292269";
    const SHA_B: &str = "265bd8922301c04cf126524f4b1935ce7bbb0c78";

    /// Turns a readable fixture with one attribute per line into git's
    /// NUL-separated form.
    fn nul(text: &str) -> Vec<u8> {
        text.replace('\n', "\0").into_bytes()
    }

    #[test]
    fn worktree_list_reads_every_kind_of_entry() {
        let output = nul(&format!(
            "worktree /work/volee\nHEAD {SHA_A}\nbranch refs/heads/main\n\n\
             worktree /work/wt/feat x\nHEAD {SHA_B}\nbranch refs/heads/feat/x\n\n\
             worktree /work/wt/detached\nHEAD {SHA_A}\ndetached\nlocked\n\n\
             worktree /work/wt/gone\nHEAD {SHA_A}\nbranch refs/heads/gone\n\
             prunable gitdir file points to non-existent location\n\n"
        ));
        let worktrees = parse_worktree_porcelain(&output).unwrap();
        assert_eq!(
            worktrees,
            [
                WorktreeInfo {
                    path: "/work/volee".into(),
                    head: Some(SHA_A.into()),
                    branch: Some("main".into()),
                    ..WorktreeInfo::default()
                },
                WorktreeInfo {
                    path: "/work/wt/feat x".into(),
                    head: Some(SHA_B.into()),
                    branch: Some("feat/x".into()),
                    ..WorktreeInfo::default()
                },
                WorktreeInfo {
                    path: "/work/wt/detached".into(),
                    head: Some(SHA_A.into()),
                    detached: true,
                    locked: Some(String::new()),
                    ..WorktreeInfo::default()
                },
                WorktreeInfo {
                    path: "/work/wt/gone".into(),
                    head: Some(SHA_A.into()),
                    branch: Some("gone".into()),
                    prunable: Some("gitdir file points to non-existent location".into()),
                    ..WorktreeInfo::default()
                },
            ]
        );
    }

    #[test]
    fn worktree_list_keeps_lock_reasons_with_newlines() {
        let mut output = format!("worktree /work/wt\0HEAD {SHA_A}\0detached\0").into_bytes();
        output.extend_from_slice(b"locked on a\nusb disk\0\0");
        let worktrees = parse_worktree_porcelain(&output).unwrap();
        assert_eq!(worktrees[0].locked.as_deref(), Some("on a\nusb disk"));
    }

    #[test]
    fn worktree_list_reads_bare_repositories_and_unborn_branches() {
        let output = nul("worktree /work/volee.git\nbare\n\n\
             worktree /work/new\nHEAD 0000000000000000000000000000000000000000\n\
             branch refs/heads/main\n\n");
        let worktrees = parse_worktree_porcelain(&output).unwrap();
        assert!(worktrees[0].bare);
        assert_eq!(worktrees[0].head, None);
        assert_eq!(worktrees[1].head, None);
        assert_eq!(worktrees[1].branch.as_deref(), Some("main"));
    }

    #[test]
    fn worktree_list_skips_unknown_attributes() {
        let output = nul(&format!(
            "worktree /work/volee\nHEAD {SHA_A}\nbranch refs/heads/main\nshiny new\n\n"
        ));
        let worktrees = parse_worktree_porcelain(&output).unwrap();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn worktree_list_rejects_attributes_outside_a_worktree() {
        let error = parse_worktree_porcelain(&nul(&format!("HEAD {SHA_A}\n\n"))).unwrap_err();
        assert!(error.to_string().contains("HEAD"));
        assert!(parse_worktree_porcelain(&nul("worktree /w\nHEAD xyz\n\n")).is_err());
    }

    #[test]
    fn worktree_list_of_nothing_is_empty() {
        assert_eq!(parse_worktree_porcelain(b"").unwrap(), []);
    }

    #[cfg(unix)]
    #[test]
    fn worktree_paths_that_are_not_utf8_survive() {
        use std::os::unix::ffi::OsStrExt;
        let mut output = b"worktree /work/caf\xe9\0".to_vec();
        output.extend_from_slice(format!("HEAD {SHA_A}\0detached\0\0").as_bytes());
        let worktrees = parse_worktree_porcelain(&output).unwrap();
        assert_eq!(worktrees[0].path.as_os_str().as_bytes(), b"/work/caf\xe9");
    }

    #[test]
    fn status_reads_head_headers_and_skips_the_others() {
        let output = nul(&format!(
            "# branch.oid {SHA_A}\n# branch.head feat/x\n\
             # branch.upstream origin/feat/x\n# branch.ab +? -?\n# stash 2\n"
        ));
        let status = parse_status_porcelain_v2(&output);
        assert_eq!(
            status,
            StatusSummary {
                oid: Some(SHA_A.into()),
                branch: Some("feat/x".into()),
                changed: 0,
                untracked: 0,
            }
        );
        assert!(!status.is_dirty());
    }

    #[test]
    fn status_reads_detached_and_unborn_heads() {
        let detached = nul(&format!("# branch.oid {SHA_A}\n# branch.head (detached)\n"));
        let status = parse_status_porcelain_v2(&detached);
        assert_eq!(status.oid.as_deref(), Some(SHA_A));
        assert_eq!(status.branch, None);

        let unborn = nul("# branch.oid (initial)\n# branch.head main\n");
        let status = parse_status_porcelain_v2(&unborn);
        assert_eq!(status.oid, None);
        assert_eq!(status.branch.as_deref(), Some("main"));
    }

    #[test]
    fn status_counts_entries_and_skips_rename_sources() {
        let output = nul(&format!(
            "# branch.oid {SHA_A}\n# branch.head feat\n\
             1 .M N... 100644 100644 100644 {SHA_A} {SHA_A} src/lib.rs\n\
             2 R. N... 100644 100644 100644 {SHA_A} {SHA_A} R100 b.txt\n\
             ? looks like an entry.txt\n\
             u UU N... 100644 100644 100644 100644 {SHA_A} {SHA_A} {SHA_A} conflict.rs\n\
             ? newdir/\n? u.txt\n! target/\n"
        ));
        let status = parse_status_porcelain_v2(&output);
        // `? looks like an entry.txt` is the original path of the rename, not
        // an untracked file.
        assert_eq!(status.changed, 3);
        assert_eq!(status.untracked, 2);
        assert!(status.is_dirty());
    }

    #[test]
    fn status_counts_unknown_entry_kinds_as_changes() {
        let status = parse_status_porcelain_v2(b"z something new\0");
        assert_eq!(status.changed, 1);
        assert!(status.is_dirty());
    }

    #[test]
    fn only_remote_tracking_branches_count_as_upstreams() {
        assert_eq!(
            remote_upstream("refs/remotes/origin/feat\n").as_deref(),
            Some("refs/remotes/origin/feat")
        );
        assert_eq!(remote_upstream("refs/heads/main\n"), None);
        assert_eq!(remote_upstream("\n"), None);
        assert_eq!(
            remote_branch_name("refs/remotes/origin/feat/x"),
            "origin/feat/x"
        );
    }

    #[test]
    fn truncation_keeps_whole_lines() {
        let output = b"one\ntwo\nthree\n";
        assert_eq!(truncation_point(output, 100), output.len());
        assert_eq!(truncation_point(output, output.len()), output.len());
        assert_eq!(truncation_point(output, 10), b"one\ntwo\n".len());
        // One long line is cut at a character boundary: `\u{e9}` takes two
        // bytes and is not split.
        let long = "abc\u{e9}xyz\u{e9}".as_bytes();
        assert_eq!(truncation_point(long, 4), 3);
        assert_eq!(truncation_point(long, 5), 5);
    }

    #[test]
    fn shortstat_reads_every_combination() {
        let cases = [
            ("", (0, 0, 0)),
            (" 1 file changed, 1 insertion(+)\n", (1, 1, 0)),
            (" 1 file changed, 2 deletions(-)\n", (1, 0, 2)),
            (
                " 3 files changed, 10 insertions(+), 2 deletions(-)\n",
                (3, 10, 2),
            ),
            (
                " 1 file changed, 0 insertions(+), 0 deletions(-)\n",
                (1, 0, 0),
            ),
        ];
        for (output, (files, insertions, deletions)) in cases {
            let expected = DiffStat {
                files,
                insertions,
                deletions,
            };
            assert_eq!(parse_shortstat(output).unwrap(), expected, "{output:?}");
        }
    }

    #[test]
    fn shortstat_saturates_and_rejects_garbage() {
        let huge = parse_shortstat(" 1 file changed, 99999999999 insertions(+)").unwrap();
        assert_eq!(huge.insertions, u32::MAX);
        assert!(parse_shortstat("fatal: bad revision").is_err());
        assert!(parse_shortstat(" 1 file changed, 3 grumbles").is_err());
    }

    #[test]
    fn counts_and_object_names_are_validated() {
        assert_eq!(parse_count("12\n").unwrap(), 12);
        assert!(parse_count("twelve").is_err());
        assert_eq!(parse_object_id(&format!("{SHA_A}\n")).unwrap(), SHA_A);
        let sha256 = "a".repeat(64);
        assert_eq!(parse_object_id(&sha256).unwrap(), sha256);
        assert!(parse_object_id("main").is_err());
        assert!(parse_object_id(&SHA_A[..12]).is_err());
    }

    #[test]
    fn repository_probe_reads_three_lines() {
        let output = b"true\n/work/volee/.git/worktrees/wt\n/work/volee/.git\n";
        let (inside, git_dir, common_dir) = parse_repository_probe(output).unwrap();
        assert!(inside);
        assert_eq!(git_dir, Path::new("/work/volee/.git/worktrees/wt"));
        assert_eq!(common_dir, Path::new("/work/volee/.git"));
        assert!(!parse_repository_probe(b"false\n/a\n/a\n").unwrap().0);
        assert!(parse_repository_probe(b"maybe\n/a\n/a\n").is_err());
        assert!(parse_repository_probe(b"true\n/a\n").is_err());
    }

    #[test]
    fn paths_are_read_from_lines_and_nul_lists() {
        assert_eq!(
            parse_path_line(b"/work/volee\n").unwrap(),
            Path::new("/work/volee")
        );
        assert!(parse_path_line(b"\n").is_err());
        assert_eq!(
            parse_nul_paths(b"newdir/n.txt\0u.txt\0"),
            [PathBuf::from("newdir/n.txt"), PathBuf::from("u.txt")]
        );
        assert_eq!(parse_nul_paths(b""), Vec::<PathBuf>::new());
    }

    #[test]
    fn branch_names_lose_their_prefix_only() {
        assert_eq!(branch_name("refs/heads/feat/x\n"), "feat/x");
        assert_eq!(
            branch_name("refs/remotes/origin/main"),
            "refs/remotes/origin/main"
        );
    }

    #[test]
    fn default_branch_prefers_main_then_master_then_develop() {
        let all = "refs/heads/develop\nrefs/heads/main\nrefs/heads/master\n";
        assert_eq!(pick_default_branch(all).as_deref(), Some("main"));
        let legacy = "refs/heads/develop\nrefs/heads/master\n";
        assert_eq!(pick_default_branch(legacy).as_deref(), Some("master"));
        assert_eq!(
            pick_default_branch("refs/heads/develop\n").as_deref(),
            Some("develop")
        );
        assert_eq!(pick_default_branch(""), None);
        // A prefix match is not a match.
        assert_eq!(pick_default_branch("refs/heads/main-old\n"), None);
    }

    #[test]
    fn missing_repositories_are_recognized_by_message() {
        assert!(reports_no_repository(
            b"fatal: not a git repository (or any of the parent directories): .git\n"
        ));
        assert!(reports_no_repository(
            b"fatal: not a git repository (or any parent up to mount point /)\n"
        ));
        assert!(!reports_no_repository(
            b"fatal: detected dubious ownership in repository at '/work'\n"
        ));
    }

    #[test]
    fn push_state_follows_the_ahead_count() {
        assert_eq!(push_state(0), PushState::UpToDate);
        assert_eq!(push_state(2), PushState::Ahead { commits: 2 });
    }

    #[test]
    fn the_main_checkout_holds_a_git_directory_named_dot_git() {
        assert_eq!(
            checkout_of_git_dir(Path::new("/work/volee/.git")),
            Some(PathBuf::from("/work/volee"))
        );
        assert_eq!(checkout_of_git_dir(Path::new("/work/volee.git")), None);
        assert_eq!(
            checkout_of_git_dir(Path::new("/work/super/.git/modules/lib")),
            None
        );
    }

    #[test]
    fn repository_names_read_well() {
        assert_eq!(repository_name(Path::new("/work/volee")), "volee");
        assert_eq!(repository_name(Path::new("/work/volee.git")), "volee");
        assert_eq!(repository_name(Path::new("/work/volee/.bare")), "volee");
        assert_eq!(repository_name(Path::new("/work/.git")), "work");
        assert_eq!(repository_name(Path::new("/")), "/");
    }

    #[test]
    fn directories_end_with_a_slash() {
        assert!(names_directory(Path::new("nested/")));
        assert!(!names_directory(Path::new("nested/file.rs")));
    }
}
