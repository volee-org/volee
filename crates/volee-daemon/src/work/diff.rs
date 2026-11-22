//! The unified diff of a branch against its base, split per file and ordered
//! by what a reader should look at first.

// Style lints that cost more than they give in this parsing code.
#![allow(
    clippy::assert_is_empty,
    clippy::format_push_string,
    clippy::items_after_statements,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::case_sensitive_file_extension_comparisons
)]

use std::path::Path;
use std::process::Command;

use serde::Serialize;

/// A file's patch is cut at this many bytes.
const MAX_PATCH: usize = 150_000;
/// All the patches together are cut at this many bytes.
const MAX_TOTAL: usize = 3_000_000;
/// What `git diff` may print before volee stops reading it.
const MAX_OUTPUT: usize = 12_000_000;

/// How a file changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Status {
    Added,
    Modified,
    Deleted,
    Renamed,
}

/// What a file is for, which decides how soon a reader should see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Category {
    Source,
    Test,
    Docs,
    Config,
    /// A lock file: never read, collapsed.
    Lock,
    /// Generated or built output: collapsed.
    Generated,
}

impl Category {
    /// Whether the interface folds the file until asked.
    pub(crate) fn collapsed(self) -> bool {
        matches!(self, Self::Lock | Self::Generated)
    }
}

/// One file of the diff.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct FileDiff {
    pub(crate) path: String,
    /// Where a renamed file came from.
    pub(crate) old_path: Option<String>,
    pub(crate) status: Status,
    pub(crate) insertions: u32,
    pub(crate) deletions: u32,
    pub(crate) binary: bool,
    pub(crate) category: Category,
    pub(crate) collapsed: bool,
    /// The file's part of the unified diff, `diff --git` header included.
    pub(crate) patch: String,
    /// The patch was cut.
    pub(crate) truncated: bool,
}

/// What `git diff` gave.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Diff {
    pub(crate) files: Vec<FileDiff>,
    /// `git diff` printed more than volee reads.
    pub(crate) truncated: bool,
}

/// Runs `git diff base...branch` in `cwd` (the merge base, so only what the
/// branch changed).
pub(crate) fn run(cwd: &Path, base: &str, branch: &str) -> Result<Diff, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args([
            "-c",
            "core.quotepath=off",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "-M",
            "--end-of-options",
        ])
        .arg(format!("{base}...{branch}"))
        .output()
        .map_err(|error| format!("git could not run: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git diff failed: {}", stderr.trim()));
    }
    let cut = output.stdout.len() > MAX_OUTPUT;
    let bytes = if cut {
        &output.stdout[..MAX_OUTPUT]
    } else {
        &output.stdout[..]
    };
    let mut diff = split(&String::from_utf8_lossy(bytes));
    diff.truncated = cut;
    Ok(diff)
}

/// Splits a unified diff per file, sorts the files for reading and applies
/// the size limits.
pub(crate) fn split(unified: &str) -> Diff {
    let mut chunks: Vec<Vec<&str>> = Vec::new();
    for line in unified.lines() {
        if line.starts_with("diff --git ") {
            chunks.push(vec![line]);
        } else if let Some(chunk) = chunks.last_mut() {
            chunk.push(line);
        }
    }
    let mut files: Vec<FileDiff> = chunks.iter().map(|chunk| file_of(chunk)).collect();
    files.sort_by(|a, b| {
        a.category
            .cmp(&b.category)
            .then((b.insertions + b.deletions).cmp(&(a.insertions + a.deletions)))
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut total = 0;
    for file in &mut files {
        if total >= MAX_TOTAL {
            file.patch.clear();
            file.truncated = true;
        }
        total += file.patch.len();
    }
    Diff {
        files,
        truncated: false,
    }
}

fn file_of(chunk: &[&str]) -> FileDiff {
    let header = chunk.first().copied().unwrap_or_default();
    let mut old_path = None;
    let mut new_path = None;
    let mut status = Status::Modified;
    let mut binary = false;
    let (mut insertions, mut deletions) = (0, 0);
    let mut in_hunk = false;
    for line in chunk.iter().skip(1) {
        if in_hunk {
            match line.as_bytes().first() {
                Some(b'+') => insertions += 1,
                Some(b'-') => deletions += 1,
                _ => {}
            }
            continue;
        }
        if line.starts_with("@@") {
            in_hunk = true;
        } else if line.starts_with("new file mode") {
            status = Status::Added;
        } else if line.starts_with("deleted file mode") {
            status = Status::Deleted;
        } else if let Some(from) = line.strip_prefix("rename from ") {
            status = Status::Renamed;
            old_path = Some(from.to_owned());
        } else if let Some(to) = line.strip_prefix("rename to ") {
            new_path = Some(to.to_owned());
        } else if let Some(path) = line.strip_prefix("+++ b/") {
            new_path = Some(path.to_owned());
        } else if let Some(path) = line.strip_prefix("--- a/") {
            old_path.get_or_insert_with(|| path.to_owned());
        } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            binary = true;
        }
    }
    let path = new_path
        .or_else(|| {
            // A deleted file has no `+++ b/` line.
            (status == Status::Deleted)
                .then(|| old_path.clone())
                .flatten()
        })
        .or_else(|| header_path(header))
        .unwrap_or_else(|| "(unknown)".to_owned());
    let old_path = old_path.filter(|old| *old != path);
    let category = categorize(&path);
    let mut patch = chunk.join("\n");
    patch.push('\n');
    let truncated = patch.len() > MAX_PATCH;
    if truncated {
        let mut cut = MAX_PATCH;
        while !patch.is_char_boundary(cut) {
            cut -= 1;
        }
        patch.truncate(cut);
        if let Some(end) = patch.rfind('\n') {
            patch.truncate(end + 1);
        }
    }
    FileDiff {
        path,
        old_path,
        status,
        insertions,
        deletions,
        binary,
        category,
        collapsed: category.collapsed(),
        patch,
        truncated,
    }
}

/// The path out of `diff --git a/x b/x`, for a change with no `+++` line
/// (a mode change, a binary file).
fn header_path(header: &str) -> Option<String> {
    let rest = header.strip_prefix("diff --git a/")?;
    let (_, new) = rest.rsplit_once(" b/")?;
    Some(new.to_owned())
}

/// What a path is for.
pub(crate) fn categorize(path: &str) -> Category {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    const LOCKS: [&str; 12] = [
        "cargo.lock",
        "package-lock.json",
        "bun.lock",
        "bun.lockb",
        "yarn.lock",
        "pnpm-lock.yaml",
        "poetry.lock",
        "gemfile.lock",
        "composer.lock",
        "go.sum",
        "flake.lock",
        "uv.lock",
    ];
    if LOCKS.contains(&name) || name.ends_with(".lock") {
        return Category::Lock;
    }
    let generated_dir = lower.split('/').any(|part| {
        matches!(
            part,
            "dist" | "build" | "generated" | "node_modules" | "target"
        )
    });
    if generated_dir
        || name.contains(".min.")
        || name.ends_with(".map")
        || name.ends_with(".pb.go")
        || name.ends_with(".generated.ts")
    {
        return Category::Generated;
    }
    let in_test_dir = lower.split('/').any(|part| {
        matches!(
            part,
            "test" | "tests" | "__tests__" | "spec" | "specs" | "journeys"
        )
    });
    if in_test_dir
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.contains("_test.")
        || name.starts_with("test_")
        || name.ends_with(".snap")
    {
        return Category::Test;
    }
    let extension = name.rsplit_once('.').map(|(_, extension)| extension);
    if matches!(extension, Some("md" | "mdx" | "txt" | "rst" | "adoc"))
        || lower.starts_with("docs/")
        || name.starts_with("license")
    {
        return Category::Docs;
    }
    if matches!(
        extension,
        Some("toml" | "yaml" | "yml" | "json" | "ini" | "cfg" | "env" | "lua")
    ) && !lower.starts_with("src/")
        || name.starts_with('.')
        || matches!(name, "justfile" | "makefile" | "dockerfile")
    {
        return Category::Config;
    }
    Category::Source
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "diff --git a/Cargo.lock b/Cargo.lock\n\
index 1..2 100644\n--- a/Cargo.lock\n+++ b/Cargo.lock\n@@ -1,2 +1,3 @@\n a\n+b\n+c\n d\n\
diff --git a/tests/api.rs b/tests/api.rs\nnew file mode 100644\nindex 0..1\n--- /dev/null\n+++ b/tests/api.rs\n@@ -0,0 +1,2 @@\n+fn t() {}\n+fn u() {}\n\
diff --git a/src/lib.rs b/src/lib.rs\nindex 3..4 100644\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,3 @@\n-old\n+new\n--- not a header, a removed line\n keep\n\
diff --git a/src/gone.rs b/src/gone.rs\ndeleted file mode 100644\nindex 5..0\n--- a/src/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n\
diff --git a/old.md b/new.md\nsimilarity index 90%\nrename from old.md\nrename to new.md\n\
diff --git a/img.png b/img.png\nnew file mode 100644\nBinary files /dev/null and b/img.png differ\n";

    #[test]
    fn files_are_split_counted_and_ordered_for_reading() {
        let diff = split(SAMPLE);
        let order: Vec<&str> = diff.files.iter().map(|f| f.path.as_str()).collect();
        // Source first (largest change first), then tests, docs, locks last.
        assert_eq!(
            order,
            [
                "src/lib.rs",
                "src/gone.rs",
                "img.png",
                "tests/api.rs",
                "new.md",
                "Cargo.lock"
            ]
        );
    }

    #[test]
    fn counts_statuses_and_flags_are_right() {
        let diff = split(SAMPLE);
        let get = |path: &str| diff.files.iter().find(|f| f.path == path).unwrap();
        let lib = get("src/lib.rs");
        assert_eq!(
            (lib.insertions, lib.deletions),
            (1, 2),
            "a removed '--' line counts"
        );
        assert_eq!(lib.status, Status::Modified);
        assert_eq!(get("tests/api.rs").status, Status::Added);
        assert_eq!(get("tests/api.rs").insertions, 2);
        assert_eq!(get("src/gone.rs").status, Status::Deleted);
        let renamed = get("new.md");
        assert_eq!(renamed.status, Status::Renamed);
        assert_eq!(renamed.old_path.as_deref(), Some("old.md"));
        assert!(get("img.png").binary);
        let lock = get("Cargo.lock");
        assert!(lock.collapsed);
        assert_eq!(lock.category, Category::Lock);
        assert!(!lib.collapsed);
        assert!(lib.patch.starts_with("diff --git a/src/lib.rs"));
    }

    #[test]
    fn categories_follow_the_path() {
        assert_eq!(categorize("src/main.rs"), Category::Source);
        assert_eq!(categorize("ui/src/App.svelte"), Category::Source);
        assert_eq!(categorize("ui/tests/ui.test.ts"), Category::Test);
        assert_eq!(categorize("crates/x/src/a_test.rs"), Category::Test);
        assert_eq!(categorize("README.md"), Category::Docs);
        assert_eq!(categorize("docs/src/adr/0001.md"), Category::Docs);
        assert_eq!(categorize("Cargo.toml"), Category::Config);
        assert_eq!(categorize(".github/workflows/ci.yml"), Category::Config);
        assert_eq!(categorize("ui/package-lock.json"), Category::Lock);
        assert_eq!(categorize("ui/dist/app.js"), Category::Generated);
        assert_eq!(categorize("static/app.min.js"), Category::Generated);
    }

    #[test]
    fn a_huge_patch_is_cut_on_a_line() {
        let mut text = String::from(
            "diff --git a/big.rs b/big.rs\n--- a/big.rs\n+++ b/big.rs\n@@ -0,0 +1 @@\n",
        );
        for _ in 0..30_000 {
            text.push_str("+let value = 1;\n");
        }
        let diff = split(&text);
        let file = &diff.files[0];
        assert!(file.truncated);
        assert!(file.patch.len() <= MAX_PATCH);
        assert!(file.patch.ends_with('\n'));
        assert_eq!(file.insertions, 30_000, "counts come from the whole diff");
    }

    #[test]
    fn an_empty_diff_has_no_files() {
        assert!(split("").files.is_empty());
        assert!(split("garbage\nlines\n").files.is_empty());
    }
}
