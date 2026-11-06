//! One-line summaries of tool calls, for people.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// The longest detail in a summary (a command, a path, a pattern), in
/// characters.
const MAX_DETAIL: usize = 80;

/// A short line describing a tool call, such as `Bash: cargo test -p
/// volee-core`, `Edit src/lib.rs`, `Grep "pattern"`, `Agent: review the
/// diff` or `WebFetch example.com`.
///
/// Paths are shown relative to `cwd` when they are inside it. Details are
/// cut to 80 characters (paths keep their end), control characters become
/// spaces, and a tool volee does not know, or a call without the expected
/// argument, is summarized by the tool's name alone.
pub fn tool_summary(tool: &str, input: &Value, cwd: Option<&Path>) -> String {
    let text = |field: &str| {
        input
            .get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let path = |field: &str| text(field).map(|path| format!(" {}", display_path(path, cwd)));
    let quoted = |field: &str| text(field).map(|value| format!(" \"{}\"", clip_end(value)));
    let detail = match tool {
        "Bash" => text("command")
            .and_then(|command| command.lines().map(str::trim).find(|line| !line.is_empty()))
            .map(|line| format!(": {}", clip_end(line))),
        "Read" | "Edit" | "Write" | "MultiEdit" => path("file_path"),
        "NotebookEdit" => path("notebook_path"),
        "Grep" | "Glob" => quoted("pattern"),
        "WebSearch" => quoted("query"),
        "Agent" | "Task" => {
            text("description").map(|description| format!(": {}", clip_end(description)))
        }
        "WebFetch" => text("url").map(|url| format!(" {}", clip_end(host(url)))),
        _ => None,
    };
    let name = clip_end(tool);
    match detail {
        Some(detail) => name + &detail,
        None => name,
    }
}

/// The files a tool call writes: the path given to `Edit`, `Write`,
/// `MultiEdit` or `NotebookEdit`, as the tool received it.
pub fn written_files(tool: &str, input: &Value) -> Vec<PathBuf> {
    let field = match tool {
        "Edit" | "Write" | "MultiEdit" => "file_path",
        "NotebookEdit" => "notebook_path",
        _ => return Vec::new(),
    };
    input
        .get(field)
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .into_iter()
        .collect()
}

/// `path` relative to `cwd` when it is inside it, cut from the start so that
/// the file name stays visible.
fn display_path(path: &str, cwd: Option<&Path>) -> String {
    let relative = cwd
        .and_then(|cwd| Path::new(path).strip_prefix(cwd).ok())
        .filter(|relative| !relative.as_os_str().is_empty())
        .map_or_else(
            || path.to_owned(),
            |relative| relative.display().to_string(),
        );
    let plain = single_line(&relative);
    let length = plain.chars().count();
    if length <= MAX_DETAIL {
        return plain;
    }
    let tail: String = plain.chars().skip(length - (MAX_DETAIL - 1)).collect();
    format!("\u{2026}{tail}")
}

/// The host (and port) of a web address, or the address itself when it has
/// none.
fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if host.is_empty() { url } else { host }
}

/// `text` on one line, cut to [`MAX_DETAIL`] characters with an ellipsis.
fn clip_end(text: &str) -> String {
    let plain = single_line(text);
    match plain.char_indices().nth(MAX_DETAIL) {
        Some(_) => {
            let kept: String = plain.chars().take(MAX_DETAIL - 1).collect();
            format!("{}\u{2026}", kept.trim_end())
        }
        None => plain,
    }
}

/// `text` with control characters (newlines, tabs, escapes) replaced by
/// spaces, trimmed.
fn single_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn summary(tool: &str, input: &Value) -> String {
        tool_summary(tool, input, Some(Path::new("/work/volee")))
    }

    #[test]
    fn bash_shows_the_first_line_of_the_command() {
        assert_eq!(
            summary(
                "Bash",
                &json!({"command": "cargo test -p volee-core", "description": "Run tests"})
            ),
            "Bash: cargo test -p volee-core"
        );
        assert_eq!(
            summary(
                "Bash",
                &json!({"command": "\n  git add -A\ngit commit -m wip\n"})
            ),
            "Bash: git add -A"
        );
    }

    #[test]
    fn long_details_are_cut_to_eighty_characters() {
        let command = format!("echo {}", "x".repeat(200));
        let line = summary("Bash", &json!({ "command": command }));
        let detail = line.strip_prefix("Bash: ").unwrap();
        assert_eq!(detail.chars().count(), MAX_DETAIL);
        assert!(detail.ends_with('\u{2026}'));
        let exact = "y".repeat(MAX_DETAIL);
        assert_eq!(
            summary("Bash", &json!({ "command": exact })),
            format!("Bash: {exact}")
        );
    }

    #[test]
    fn file_tools_show_paths_relative_to_the_working_directory() {
        assert_eq!(
            summary(
                "Edit",
                &json!({"file_path": "/work/volee/src/lib.rs", "old_string": "a", "new_string": "b"})
            ),
            "Edit src/lib.rs"
        );
        assert_eq!(
            summary("Read", &json!({"file_path": "/work/volee/README.md"})),
            "Read README.md"
        );
        assert_eq!(
            summary(
                "Write",
                &json!({"file_path": "/tmp/notes.txt", "content": "x"})
            ),
            "Write /tmp/notes.txt"
        );
        assert_eq!(
            summary(
                "MultiEdit",
                &json!({"file_path": "/work/volee/Cargo.toml", "edits": []})
            ),
            "MultiEdit Cargo.toml"
        );
        assert_eq!(
            summary(
                "NotebookEdit",
                &json!({"notebook_path": "/work/volee/a.ipynb"})
            ),
            "NotebookEdit a.ipynb"
        );
        assert_eq!(
            tool_summary("Read", &json!({"file_path": "/work/volee/README.md"}), None),
            "Read /work/volee/README.md"
        );
    }

    #[test]
    fn long_paths_keep_their_end() {
        let path = format!("/work/volee/{}main.rs", "deep/".repeat(30));
        let line = summary("Read", &json!({ "file_path": path }));
        assert!(line.starts_with("Read \u{2026}"), "{line}");
        assert!(line.ends_with("/main.rs"), "{line}");
        assert_eq!(line.chars().count(), "Read ".len() + MAX_DETAIL);
    }

    #[test]
    fn searches_quote_their_pattern() {
        assert_eq!(
            summary(
                "Grep",
                &json!({"pattern": "fn main", "path": "/work/volee"})
            ),
            "Grep \"fn main\""
        );
        assert_eq!(
            summary("Glob", &json!({"pattern": "**/*.rs"})),
            "Glob \"**/*.rs\""
        );
        assert_eq!(
            summary("WebSearch", &json!({"query": "ratatui scrollbar"})),
            "WebSearch \"ratatui scrollbar\""
        );
    }

    #[test]
    fn agents_show_their_description() {
        assert_eq!(
            summary(
                "Agent",
                &json!({"description": "Review the diff", "prompt": "...", "subagent_type": "Explore"})
            ),
            "Agent: Review the diff"
        );
        assert_eq!(
            summary("Task", &json!({"description": "Find callers"})),
            "Task: Find callers"
        );
    }

    #[test]
    fn web_fetches_show_the_host() {
        let fetch = |url: &str| summary("WebFetch", &json!({"url": url, "prompt": "summarize"}));
        assert_eq!(
            fetch("https://example.com/docs/page?x=1"),
            "WebFetch example.com"
        );
        assert_eq!(
            fetch("http://user:secret@localhost:3000/api"),
            "WebFetch localhost:3000"
        );
        assert_eq!(fetch("example.org/path"), "WebFetch example.org");
        assert_eq!(fetch("https://"), "WebFetch https://");
    }

    #[test]
    fn unknown_tools_and_missing_arguments_fall_back_to_the_name() {
        assert_eq!(summary("TodoWrite", &json!({"todos": []})), "TodoWrite");
        assert_eq!(
            summary("mcp__volee__send_message", &json!({"to": "x"})),
            "mcp__volee__send_message"
        );
        assert_eq!(summary("Edit", &json!({})), "Edit");
        assert_eq!(summary("Bash", &json!({"command": "  \n "})), "Bash");
        assert_eq!(summary("Read", &Value::Null), "Read");
        assert_eq!(summary("Grep", &json!({"pattern": 42})), "Grep");
    }

    #[test]
    fn control_characters_never_reach_the_line() {
        assert_eq!(
            summary("Grep", &json!({"pattern": "a\tb\u{1b}[31m"})),
            "Grep \"a b [31m\""
        );
        assert_eq!(
            summary("Edit", &json!({"file_path": "/work/volee/odd\nname.rs"})),
            "Edit odd name.rs"
        );
    }

    #[test]
    fn only_writing_tools_list_files() {
        let path = "/work/volee/src/lib.rs";
        for tool in ["Edit", "Write", "MultiEdit"] {
            assert_eq!(
                written_files(tool, &json!({ "file_path": path })),
                [PathBuf::from(path)],
                "{tool}"
            );
        }
        assert_eq!(
            written_files("NotebookEdit", &json!({"notebook_path": "/work/a.ipynb"})),
            [PathBuf::from("/work/a.ipynb")]
        );
        assert_eq!(
            written_files("Read", &json!({ "file_path": path })),
            Vec::<PathBuf>::new()
        );
        assert_eq!(
            written_files("Bash", &json!({"command": "touch x"})),
            Vec::<PathBuf>::new()
        );
        assert_eq!(written_files("Edit", &json!({})), Vec::<PathBuf>::new());
    }
}
