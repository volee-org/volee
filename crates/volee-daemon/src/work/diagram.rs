//! A Mermaid diagram of a change, made from facts only: the files grouped by
//! directory, and an edge where a changed file gained an import of another
//! changed file. No model is asked; the same diff gives the same diagram.

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

use std::collections::BTreeMap;

use super::diff::{FileDiff, Status};

/// Above this many files the diagram shows directories, not files.
const MAX_FILE_NODES: usize = 36;
/// Above this many directories they are grouped by their first segment.
const MAX_DIR_NODES: usize = 30;

/// The diagram, as Mermaid `flowchart` source.
pub(crate) fn mermaid(files: &[FileDiff]) -> String {
    if files.is_empty() {
        return "flowchart LR\n  none[\"no file changed\"]\n".to_owned();
    }
    let edges = import_edges(files);
    if files.len() <= MAX_FILE_NODES {
        by_file(files, &edges)
    } else {
        by_directory(files, &edges)
    }
}

fn by_file(files: &[FileDiff], edges: &[(usize, usize)]) -> String {
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, file) in files.iter().enumerate() {
        groups.entry(dir_of(&file.path)).or_default().push(index);
    }
    let mut out = String::from("flowchart LR\n");
    for (group, (dir, members)) in groups.iter().enumerate() {
        let title = if dir.is_empty() { "(root)" } else { dir };
        out.push_str(&format!("  subgraph d{group}[\"{}\"]\n", label(title)));
        for &index in members {
            let file = &files[index];
            let name = file.path.rsplit('/').next().unwrap_or(&file.path);
            out.push_str(&format!(
                "    f{index}[\"{}  +{} -{}\"]\n",
                label(name),
                file.insertions,
                file.deletions
            ));
        }
        out.push_str("  end\n");
    }
    for (from, to) in edges {
        out.push_str(&format!("  f{from} --> f{to}\n"));
    }
    out.push_str(&classes(
        files
            .iter()
            .enumerate()
            .map(|(i, f)| (format!("f{i}"), f.status)),
    ));
    out
}

fn by_directory(files: &[FileDiff], edges: &[(usize, usize)]) -> String {
    let mut dirs: Vec<String> = files.iter().map(|f| dir_of(&f.path)).collect();
    let distinct = {
        let mut seen = dirs.clone();
        seen.sort();
        seen.dedup();
        seen.len()
    };
    if distinct > MAX_DIR_NODES {
        for dir in &mut dirs {
            *dir = dir.split('/').next().unwrap_or_default().to_owned();
        }
    }
    let mut nodes: BTreeMap<String, (usize, u32, u32)> = BTreeMap::new();
    for (file, dir) in files.iter().zip(&dirs) {
        let entry = nodes.entry(dir.clone()).or_default();
        entry.0 += 1;
        entry.1 += file.insertions;
        entry.2 += file.deletions;
    }
    let ids: BTreeMap<&String, usize> = nodes.keys().enumerate().map(|(i, k)| (k, i)).collect();
    let mut out = String::from("flowchart LR\n");
    for (dir, (count, added, removed)) in &nodes {
        let title = if dir.is_empty() { "(root)" } else { dir };
        out.push_str(&format!(
            "  d{}[\"{}  {count} files  +{added} -{removed}\"]\n",
            ids[dir],
            label(title)
        ));
    }
    let mut drawn: Vec<(usize, usize)> = Vec::new();
    for (from, to) in edges {
        let pair = (ids[&dirs[*from]], ids[&dirs[*to]]);
        if pair.0 != pair.1 && !drawn.contains(&pair) {
            drawn.push(pair);
            out.push_str(&format!("  d{} --> d{}\n", pair.0, pair.1));
        }
    }
    out
}

fn classes(nodes: impl Iterator<Item = (String, Status)>) -> String {
    let (mut added, mut deleted, mut renamed) = (Vec::new(), Vec::new(), Vec::new());
    for (id, status) in nodes {
        match status {
            Status::Added => added.push(id),
            Status::Deleted => deleted.push(id),
            Status::Renamed => renamed.push(id),
            Status::Modified => {}
        }
    }
    let mut out = String::from(
        "  classDef added stroke:#3fb950,stroke-width:2px\n  classDef deleted stroke:#f85149,stroke-width:2px,stroke-dasharray:4 3\n  classDef renamed stroke:#d29922,stroke-width:2px\n",
    );
    for (name, ids) in [("added", added), ("deleted", deleted), ("renamed", renamed)] {
        if !ids.is_empty() {
            out.push_str(&format!("  class {} {name}\n", ids.join(",")));
        }
    }
    out
}

fn dir_of(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(dir, _)| dir.to_owned())
        .unwrap_or_default()
}

/// Text that is safe inside a quoted Mermaid label.
fn label(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '"' => '\'',
            '<' | '>' | '&' | '#' | ';' | '`' | '\\' => ' ',
            c => c,
        })
        .collect()
}

/// `(importer, imported)` for every changed file that gained an import of
/// another changed file. Only relative imports of JavaScript and TypeScript
/// and `mod x;` of Rust are understood; anything else gives no edge.
fn import_edges(files: &[FileDiff]) -> Vec<(usize, usize)> {
    let mut edges = Vec::new();
    for (from, file) in files.iter().enumerate() {
        for target in added_imports(file) {
            for (to, other) in files.iter().enumerate() {
                if to != from
                    && !edges.contains(&(from, to))
                    && matches_target(&other.path, &target)
                {
                    edges.push((from, to));
                }
            }
        }
    }
    edges
}

/// Where an added import points, as normalised path stems (no extension).
fn added_imports(file: &FileDiff) -> Vec<Vec<String>> {
    let dir = dir_of(&file.path);
    let stem = file.path.rsplit('/').next().unwrap_or_default();
    let mut found = Vec::new();
    for line in file.patch.lines() {
        let Some(added) = line.strip_prefix('+') else {
            continue;
        };
        if line.starts_with("+++") {
            continue;
        }
        let added = added.trim();
        if let Some(spec) = js_specifier(added).filter(|spec| spec.starts_with('.')) {
            found.push(vec![strip_extension(&join(&dir, &spec))]);
        } else if let Some(name) = added
            .strip_prefix("pub(crate) mod ")
            .or_else(|| added.strip_prefix("pub mod "))
            .or_else(|| added.strip_prefix("mod "))
            .and_then(|rest| rest.strip_suffix(';'))
        {
            let name = name.trim();
            let own = stem.strip_suffix(".rs").unwrap_or(stem);
            let mut candidates = vec![join(&dir, name), join(&dir, &format!("{name}/mod"))];
            if !matches!(own, "lib" | "main" | "mod") {
                candidates.push(join(&join(&dir, own), name));
                candidates.push(join(&join(&dir, own), &format!("{name}/mod")));
            }
            found.push(candidates);
        }
    }
    found
}

fn matches_target(path: &str, candidates: &[String]) -> bool {
    let stem = strip_extension(path);
    candidates
        .iter()
        .any(|candidate| stem == *candidate || stem == format!("{candidate}/index"))
}

/// The quoted module name of `import ... from "x"`, `import "x"`,
/// `export ... from "x"` or `require("x")`.
fn js_specifier(line: &str) -> Option<String> {
    let after = if let Some(at) = line.rfind(" from ") {
        &line[at + 6..]
    } else if let Some(rest) = line.strip_prefix("import ") {
        rest
    } else {
        &line[line.find("require(")? + 8..]
    };
    let after = after.trim_start();
    let quote = after.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let inner = &after[1..];
    Some(inner[..inner.find(quote)?].to_owned())
}

/// `dir/spec` with `.` and `..` resolved.
fn join(dir: &str, spec: &str) -> String {
    let mut parts: Vec<&str> = dir.split('/').filter(|part| !part.is_empty()).collect();
    for part in spec.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

fn strip_extension(path: &str) -> String {
    const EXTENSIONS: [&str; 9] = [
        ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".svelte", ".vue", ".rs",
    ];
    for extension in EXTENSIONS {
        if let Some(stem) = path.strip_suffix(extension) {
            return stem.to_owned();
        }
    }
    path.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work::diff::split;

    fn change(path: &str, added: &[&str]) -> String {
        let mut text =
            format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n");
        for line in added {
            text.push_str(&format!("+{line}\n"));
        }
        text
    }

    #[test]
    fn files_are_grouped_by_directory_and_imports_become_edges() {
        let diff = split(
            &[
                change("src/math.ts", &["export const mul = 1;"]),
                change(
                    "tests/math.test.ts",
                    &["import { mul } from \"../src/math\";"],
                ),
                change("src/App.svelte", &["import x from './lib/util.ts'"]),
                change("src/lib/util.ts", &["export {}"]),
            ]
            .concat(),
        );
        let text = mermaid(&diff.files);
        assert!(text.starts_with("flowchart LR"));
        assert!(text.contains("subgraph"));
        assert!(text.contains("\"src\""));
        assert!(text.contains("\"tests\""));
        // tests/math.test.ts -> src/math.ts, App.svelte -> lib/util.ts
        let index = |path: &str| diff.files.iter().position(|f| f.path == path).unwrap();
        assert!(text.contains(&format!(
            "f{} --> f{}",
            index("tests/math.test.ts"),
            index("src/math.ts")
        )));
        assert!(text.contains(&format!(
            "f{} --> f{}",
            index("src/App.svelte"),
            index("src/lib/util.ts")
        )));
    }

    #[test]
    fn rust_mod_declarations_become_edges() {
        let diff = split(
            &[
                change("crates/a/src/lib.rs", &["mod work;", "pub mod other;"]),
                change("crates/a/src/work/mod.rs", &["fn f() {}"]),
            ]
            .concat(),
        );
        let text = mermaid(&diff.files);
        assert_eq!(text.matches("-->").count(), 1);
    }

    #[test]
    fn added_files_are_marked_and_labels_are_safe() {
        let diff = split(
            "diff --git a/a\"b<c>.ts b/a\"b<c>.ts\nnew file mode 100644\n--- /dev/null\n+++ b/a\"b<c>.ts\n@@ -0,0 +1 @@\n+x\n",
        );
        let text = mermaid(&diff.files);
        assert!(text.contains("class f0 added"));
        assert!(!text.contains("<c>"));
        assert!(!text.contains("a\"b"));
    }

    #[test]
    fn a_big_change_shows_directories_only() {
        let mut unified = String::new();
        for i in 0..50 {
            unified.push_str(&change(&format!("src/d{}/f{i}.rs", i % 3), &["x"]));
        }
        let text = mermaid(&split(&unified).files);
        assert!(text.contains("files"));
        assert!(!text.contains("subgraph"));
        assert_eq!(text.matches("[\"").count(), 3);
    }

    #[test]
    fn no_file_gives_a_placeholder() {
        assert!(mermaid(&[]).contains("no file changed"));
    }
}
