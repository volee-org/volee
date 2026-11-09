//! `claude --bg`: its command line, and what it prints.

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};

use crate::error::ParseError;
use crate::spec::{Completion, SpawnSpec};

/// The separator Claude Code prints between the fields of the `backgrounded`
/// line (a middle dot).
const SEPARATOR: char = '\u{b7}';

/// What `claude --bg` reported about the session it started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backgrounded {
    /// The session's id, which `claude agents` lists and `stop`, `rm`,
    /// `logs` and `attach` take (the first characters of its session UUID).
    pub id: String,
    /// The session's display name, when one was printed.
    pub name: Option<String>,
}

/// Reads the `backgrounded · <id> · <name>` line that `claude --bg` prints.
///
/// Other lines (Claude Code prints usage hints after it) and terminal escape
/// sequences are ignored. The name is everything after the second separator,
/// so it may itself contain ` · `.
pub fn parse_backgrounded(output: &str) -> Result<Backgrounded, ParseError> {
    for line in output.lines() {
        let line = strip_escapes(line);
        let line = line.trim();
        let Some(rest) = line
            .get(.."backgrounded".len())
            .filter(|word| word.eq_ignore_ascii_case("backgrounded"))
            .map(|word| &line[word.len()..])
        else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix(SEPARATOR) else {
            continue;
        };
        let (id, name) = match rest.split_once(SEPARATOR) {
            Some((id, name)) => (id.trim(), Some(name.trim())),
            None => (rest.trim(), None),
        };
        if !is_short_id(id) {
            return Err(ParseError::InvalidShortId(id.to_owned()));
        }
        return Ok(Backgrounded {
            id: id.to_owned(),
            name: name.filter(|name| !name.is_empty()).map(str::to_owned),
        });
    }
    Err(ParseError::NoBackgroundedLine)
}

/// Whether `id` looks like a session id that can safely be passed back to
/// `claude`. Claude Code 2.1.319 prints eight hexadecimal digits; any short
/// run of letters, digits and dashes is accepted so that a longer id in a
/// future version does not orphan the session it started.
fn is_short_id(id: &str) -> bool {
    (4..=36).contains(&id.len()) && super::is_argument_safe(id)
}

/// The arguments of `claude --bg` for `spec`, resuming `resume` when given.
///
/// Every value travels inside its option as `--opt=value` (checked against
/// Claude Code 2.1.319, which also rejects unknown options), so no value can
/// be read as an option and no variadic option (`--mcp-config`,
/// `--allowedTools`, `--disallowedTools`) can swallow the next argument. Tool
/// rules go in one comma-separated value, the list form the option
/// documents. The prompt comes last, after `--`.
pub(crate) fn args(resume: Option<&str>, spec: &SpawnSpec) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["--bg".into()];
    args.extend(common_args(resume, spec));
    args
}

/// The arguments of an interactive `claude` in a volee host: the same as
/// [`args`] without `--bg`, with `--session-id` for a new session (so the
/// launch knows its session before the first hook), and the prompt only when
/// there is one (an adopted agent resumes at its prompt).
pub(crate) fn host_args(
    resume: Option<&str>,
    session_id: Option<&str>,
    spec: &SpawnSpec,
) -> Vec<OsString> {
    let mut args = Vec::new();
    if let Some(id) = session_id {
        args.push(option("--session-id", id));
    }
    let mut rest = common_args(resume, spec);
    if spec.prompt.trim().is_empty() {
        // Drop the `--` and the empty prompt.
        rest.truncate(rest.len().saturating_sub(2));
    }
    args.extend(rest);
    args
}

fn common_args(resume: Option<&str>, spec: &SpawnSpec) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    if let Some(session) = resume {
        args.push(option("--resume", session));
    }
    if !spec.name.trim().is_empty() {
        args.push(option("--name", &spec.name));
    }
    if let Some(model) = &spec.model {
        args.push(option("--model", model));
    }
    args.push(option("--settings", &spec.settings_file));
    args.push(option("--mcp-config", &spec.mcp_config_file));
    args.push("--strict-mcp-config".into());
    args.push(option("--permission-mode", spec.permission_mode.as_str()));
    for (flag, rules) in [
        ("--allowedTools", &spec.allowed_tools),
        ("--disallowedTools", &spec.disallowed_tools),
    ] {
        if !rules.is_empty() {
            args.push(option(flag, rules.join(",")));
        }
    }
    if let Some(text) = &spec.append_system_prompt {
        args.push(option("--append-system-prompt", text));
    }
    if let Some(name) = &spec.worktree {
        args.push(option("--worktree", name));
    }
    args.push("--".into());
    args.push(prompt_operand(&spec.prompt).into());
    args
}

/// The arguments of `claude -p` for a one-off answer: text output, no saved
/// session, no tools, no MCP server, no skills or commands, and only the
/// settings local to the directory it runs in (so not the person's hooks),
/// with the question last, after `--`.
pub(crate) fn completion_args(request: &Completion) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "-p".into(),
        option("--output-format", "text"),
        "--no-session-persistence".into(),
        option("--tools", ""),
        "--strict-mcp-config".into(),
        option("--mcp-config", r#"{"mcpServers":{}}"#),
        "--disable-slash-commands".into(),
        option("--setting-sources", "local"),
    ];
    if let Some(model) = &request.model {
        args.push(option("--model", model));
    }
    args.push(option("--system-prompt", &request.system_prompt));
    args.push("--".into());
    args.push(prompt_operand(&request.prompt).into());
    args
}

/// One `--name=value` argument.
fn option(name: &str, value: impl AsRef<OsStr>) -> OsString {
    let mut arg = OsString::from(name);
    arg.push("=");
    arg.push(value.as_ref());
    arg
}

/// The prompt as the last argument of `claude`.
///
/// Claude Code's parser runs a subcommand when the first operand is its name,
/// even after `--`: a one-word prompt such as `update` or `purge` would run
/// `claude update` or `claude purge` instead of starting a session. A prompt
/// without whitespace gets a trailing newline, which no subcommand name has.
/// A prompt starting with `-` gets a leading newline, so that it can never
/// be read as an option either. The model does not notice either newline.
fn prompt_operand(prompt: &str) -> String {
    if prompt.starts_with('-') {
        format!("\n{prompt}")
    } else if prompt.chars().any(char::is_whitespace) {
        prompt.to_owned()
    } else {
        format!("{prompt}\n")
    }
}

/// Removes terminal escape sequences (colors, cursor moves, hyperlinks).
fn strip_escapes(text: &str) -> Cow<'_, str> {
    const ESC: char = '\u{1b}';
    const BEL: char = '\u{7}';
    if !text.contains(ESC) {
        return Cow::Borrowed(text);
    }
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != ESC {
            plain.push(c);
            continue;
        }
        match chars.next() {
            // Control sequence: parameters, then one final byte in `@..=~`.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // Operating system command: until BEL or ESC \.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == BEL {
                        break;
                    }
                    if c == ESC {
                        chars.next_if_eq(&'\\');
                        break;
                    }
                }
            }
            // Any other escape is two characters long.
            _ => {}
        }
    }
    Cow::Owned(plain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::PermissionMode;

    fn spec() -> SpawnSpec {
        SpawnSpec {
            cwd: "/work/alpha".into(),
            name: "alpha worker".into(),
            model: None,
            prompt: "Fix the flaky test".into(),
            settings_file: "/state/agents/ag_1/settings.json".into(),
            mcp_config_file: "/state/agents/ag_1/mcp.json".into(),
            permission_mode: PermissionMode::Manual,
            allowed_tools: vec![],
            disallowed_tools: vec![],
            append_system_prompt: None,
            worktree: None,
        }
    }

    fn strings(args: &[OsString]) -> Vec<&str> {
        args.iter().map(|arg| arg.to_str().unwrap()).collect()
    }

    #[test]
    fn a_worktree_is_named_before_the_prompt() {
        let spec = SpawnSpec {
            cwd: "/work/alpha".into(),
            worktree: Some("feat-invoices".into()),
            ..spec()
        };
        let args = args(None, &spec);
        let args = strings(&args);
        let at = args.iter().position(|arg| *arg == "--").unwrap();
        assert!(args[..at].contains(&"--worktree=feat-invoices"), "{args:?}");
    }

    #[test]
    fn reads_the_backgrounded_line_among_hints() {
        let output = "backgrounded \u{b7} 7d3c1f0a \u{b7} refactor the parser\n\
                      \n  claude attach 7d3c1f0a   open it here\n  claude logs 7d3c1f0a     see its output\n";
        assert_eq!(
            parse_backgrounded(output).unwrap(),
            Backgrounded {
                id: "7d3c1f0a".into(),
                name: Some("refactor the parser".into()),
            }
        );
    }

    #[test]
    fn the_name_may_contain_the_separator() {
        let output = "\n  backgrounded \u{b7} c4e5a6b7 \u{b7} api \u{b7} billing \u{b7} v2  \n";
        let background = parse_backgrounded(output).unwrap();
        assert_eq!(background.id, "c4e5a6b7");
        assert_eq!(
            background.name.as_deref(),
            Some("api \u{b7} billing \u{b7} v2")
        );
    }

    #[test]
    fn the_name_is_optional() {
        let background = parse_backgrounded("backgrounded \u{b7} 7d3c1f0a\n").unwrap();
        assert_eq!(background.id, "7d3c1f0a");
        assert_eq!(background.name, None);
        let empty = parse_backgrounded("backgrounded \u{b7} 7d3c1f0a \u{b7} \n").unwrap();
        assert_eq!(empty.name, None);
    }

    #[test]
    fn colors_and_hyperlinks_are_ignored() {
        let output = "\u{1b}[1;32mbackgrounded\u{1b}[0m \u{b7} \u{1b}]8;;claude://7d3c1f0a\u{1b}\\7d3c1f0a\u{1b}]8;;\u{1b}\\ \u{b7} \u{1b}[2mfix\u{1b}[0m\n";
        let background = parse_backgrounded(output).unwrap();
        assert_eq!(background.id, "7d3c1f0a");
        assert_eq!(background.name.as_deref(), Some("fix"));
    }

    #[test]
    fn lines_that_merely_start_with_the_word_are_skipped() {
        let output = "backgrounded sessions are listed by claude agents\nBackgrounded \u{b7} 0a1b2c3d \u{b7} x\n";
        assert_eq!(parse_backgrounded(output).unwrap().id, "0a1b2c3d");
    }

    #[test]
    fn a_missing_line_or_an_unsafe_id_is_an_error() {
        assert!(matches!(
            parse_backgrounded("Error: not logged in\n"),
            Err(ParseError::NoBackgroundedLine)
        ));
        assert!(matches!(
            parse_backgrounded(""),
            Err(ParseError::NoBackgroundedLine)
        ));
        for bad in ["--help", "a b", "x", "7d3c1f0a;rm"] {
            let output = format!("backgrounded \u{b7} {bad} \u{b7} name");
            assert!(
                matches!(
                    parse_backgrounded(&output),
                    Err(ParseError::InvalidShortId(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_minimal_spec_passes_only_what_volee_always_needs() {
        assert_eq!(
            strings(&args(None, &spec())),
            [
                "--bg",
                "--name=alpha worker",
                "--settings=/state/agents/ag_1/settings.json",
                "--mcp-config=/state/agents/ag_1/mcp.json",
                "--strict-mcp-config",
                "--permission-mode=manual",
                "--",
                "Fix the flaky test",
            ]
        );
    }

    #[test]
    fn every_value_travels_inside_its_option() {
        let full = SpawnSpec {
            model: Some("opus".into()),
            permission_mode: PermissionMode::AcceptEdits,
            allowed_tools: vec!["Bash(cargo test *)".into(), "Edit".into()],
            disallowed_tools: vec!["WebFetch".into()],
            append_system_prompt: Some("-You are a volee worker.".into()),
            ..spec()
        };
        assert_eq!(
            strings(&args(Some("7d3c1f0a-2b4e-4c6d-9e8f-0a1b2c3d4e5f"), &full)),
            [
                "--bg",
                "--resume=7d3c1f0a-2b4e-4c6d-9e8f-0a1b2c3d4e5f",
                "--name=alpha worker",
                "--model=opus",
                "--settings=/state/agents/ag_1/settings.json",
                "--mcp-config=/state/agents/ag_1/mcp.json",
                "--strict-mcp-config",
                "--permission-mode=acceptEdits",
                "--allowedTools=Bash(cargo test *),Edit",
                "--disallowedTools=WebFetch",
                "--append-system-prompt=-You are a volee worker.",
                "--",
                "Fix the flaky test",
            ]
        );
    }

    #[test]
    fn the_permission_mode_is_always_passed_and_never_skips_checks() {
        let defaults = args(None, &spec());
        let defaults = strings(&defaults);
        assert!(
            defaults.contains(&"--permission-mode=manual"),
            "{defaults:?}"
        );
        assert!(
            !defaults
                .iter()
                .any(|arg| arg.contains("bypass") || arg.contains("dangerously")),
            "{defaults:?}"
        );
    }

    #[test]
    fn an_empty_name_is_left_to_claude() {
        let unnamed = SpawnSpec {
            name: "  ".into(),
            ..spec()
        };
        assert!(
            !strings(&args(None, &unnamed))
                .iter()
                .any(|arg| arg.starts_with("--name"))
        );
    }

    #[test]
    fn a_one_word_prompt_cannot_become_a_subcommand() {
        assert_eq!(prompt_operand("update"), "update\n");
        assert_eq!(prompt_operand("purge"), "purge\n");
        assert_eq!(prompt_operand("fix the tests"), "fix the tests");
        assert_eq!(prompt_operand("line one\nline two"), "line one\nline two");
    }

    #[test]
    fn a_prompt_cannot_become_an_option() {
        assert_eq!(prompt_operand("-p"), "\n-p");
        assert_eq!(prompt_operand("--help me"), "\n--help me");
        assert_eq!(prompt_operand("--"), "\n--");
    }

    #[test]
    fn strip_escapes_leaves_plain_text_alone() {
        assert!(matches!(strip_escapes("plain"), Cow::Borrowed("plain")));
        assert_eq!(strip_escapes("a\u{1b}[31mb\u{1b}7c"), "abc");
        assert_eq!(strip_escapes("\u{1b}]0;title\u{7}text"), "text");
    }

    #[test]
    fn a_completion_has_no_tools_no_servers_no_hooks_and_no_saved_session() {
        let request = Completion {
            cwd: "/state/summaries".into(),
            model: Some("haiku".into()),
            system_prompt: "Summarize.".into(),
            prompt: "--- a thread that starts with dashes".into(),
        };
        let args = completion_args(&request);
        let args = strings(&args);
        for expected in [
            "-p",
            "--output-format=text",
            "--no-session-persistence",
            "--tools=",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--setting-sources=local",
            "--model=haiku",
            "--system-prompt=Summarize.",
        ] {
            assert!(args.contains(&expected), "{expected} in {args:?}");
        }
        let at = args.iter().position(|arg| *arg == "--").unwrap();
        assert_eq!(args.len(), at + 2, "the question comes last");
        assert!(
            !args[at + 1].starts_with('-'),
            "it cannot read as an option"
        );
    }
}
