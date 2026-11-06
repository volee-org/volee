//! `claude --bg`: its command line, and what it prints.

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};

use crate::error::ParseError;
use crate::spec::SpawnSpec;

/// The separator Claude Code prints between the fields of the `backgrounded`
/// line (a middle dot).
const SEPARATOR: char = '\u{b7}';

/// What `claude --bg` reported about the session it started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backgrounded {
    /// The session's short id: the first characters of its session UUID.
    pub short_id: String,
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
            short_id: id.to_owned(),
            name: name.filter(|name| !name.is_empty()).map(str::to_owned),
        });
    }
    Err(ParseError::NoBackgroundedLine)
}

/// Whether `id` looks like a short session id that can safely be passed back
/// to `claude`. Claude Code 2.1.319 prints eight hexadecimal digits; any
/// short run of letters, digits and dashes is accepted so that a longer id
/// in a future version does not orphan the session it started.
fn is_short_id(id: &str) -> bool {
    (4..=36).contains(&id.len()) && super::is_argument_safe(id)
}

/// The arguments of `claude --bg` for `spec`, resuming `resume` when given.
///
/// Free text never reaches the parser as an option: the prompt comes after
/// `--`, and every variadic option (`--mcp-config`, `--allowedTools`,
/// `--disallowedTools`) is followed by another option or by `--`, so it
/// cannot swallow the prompt.
pub(crate) fn args(resume: Option<&str>, spec: &SpawnSpec) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["--bg".into()];
    if let Some(session) = resume {
        push_option(&mut args, "--resume", session);
    }
    if !spec.name.trim().is_empty() {
        push_option(&mut args, "--name", &spec.name);
    }
    if let Some(model) = &spec.model {
        push_option(&mut args, "--model", model);
    }
    push_option(&mut args, "--settings", &spec.settings_file);
    push_option(&mut args, "--mcp-config", &spec.mcp_config_file);
    args.push("--strict-mcp-config".into());
    if let Some(mode) = spec.permission_mode {
        push_option(&mut args, "--permission-mode", mode.as_str());
    }
    for (flag, rules) in [
        ("--allowedTools", &spec.allowed_tools),
        ("--disallowedTools", &spec.disallowed_tools),
    ] {
        if !rules.is_empty() {
            args.push(flag.into());
            args.extend(rules.iter().map(Into::into));
        }
    }
    if let Some(text) = &spec.append_system_prompt {
        push_option(&mut args, "--append-system-prompt", text);
    }
    args.push("--".into());
    args.push(prompt_operand(&spec.prompt).into());
    args
}

/// Appends an option and its value as two arguments. Claude Code's parser
/// always takes the argument after an option that needs a value as that
/// value, even when it starts with a dash.
fn push_option(args: &mut Vec<OsString>, name: &str, value: impl AsRef<OsStr>) {
    args.push(name.into());
    args.push(value.as_ref().to_os_string());
}

/// The prompt as the last argument of `claude`.
///
/// Claude Code's parser runs a subcommand when the first operand is its name,
/// even after `--`: a one-word prompt such as `update` or `purge` would run
/// `claude update` or `claude purge` instead of starting a session. A prompt
/// without whitespace gets a trailing newline, which no subcommand name has
/// and which the model does not notice.
fn prompt_operand(prompt: &str) -> String {
    if prompt.chars().any(char::is_whitespace) {
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
            permission_mode: None,
            allowed_tools: vec![],
            disallowed_tools: vec![],
            append_system_prompt: None,
        }
    }

    fn strings(args: &[OsString]) -> Vec<&str> {
        args.iter().map(|arg| arg.to_str().unwrap()).collect()
    }

    #[test]
    fn reads_the_backgrounded_line_among_hints() {
        let output = "backgrounded \u{b7} 4402670e \u{b7} neon billing investigation\n\
                      \n  claude attach 4402670e   open it here\n  claude logs 4402670e     see its output\n";
        assert_eq!(
            parse_backgrounded(output).unwrap(),
            Backgrounded {
                short_id: "4402670e".into(),
                name: Some("neon billing investigation".into()),
            }
        );
    }

    #[test]
    fn the_name_may_contain_the_separator() {
        let output = "\n  backgrounded \u{b7} b60aa3b9 \u{b7} api \u{b7} billing \u{b7} v2  \n";
        let background = parse_backgrounded(output).unwrap();
        assert_eq!(background.short_id, "b60aa3b9");
        assert_eq!(
            background.name.as_deref(),
            Some("api \u{b7} billing \u{b7} v2")
        );
    }

    #[test]
    fn the_name_is_optional() {
        let background = parse_backgrounded("backgrounded \u{b7} 4402670e\n").unwrap();
        assert_eq!(background.short_id, "4402670e");
        assert_eq!(background.name, None);
        let empty = parse_backgrounded("backgrounded \u{b7} 4402670e \u{b7} \n").unwrap();
        assert_eq!(empty.name, None);
    }

    #[test]
    fn colors_and_hyperlinks_are_ignored() {
        let output = "\u{1b}[1;32mbackgrounded\u{1b}[0m \u{b7} \u{1b}]8;;claude://4402670e\u{1b}\\4402670e\u{1b}]8;;\u{1b}\\ \u{b7} \u{1b}[2mfix\u{1b}[0m\n";
        let background = parse_backgrounded(output).unwrap();
        assert_eq!(background.short_id, "4402670e");
        assert_eq!(background.name.as_deref(), Some("fix"));
    }

    #[test]
    fn lines_that_merely_start_with_the_word_are_skipped() {
        let output = "backgrounded sessions are listed by claude agents\nBackgrounded \u{b7} 0a1b2c3d \u{b7} x\n";
        assert_eq!(parse_backgrounded(output).unwrap().short_id, "0a1b2c3d");
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
        for bad in ["--help", "a b", "x", "4402670e;rm"] {
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
                "--name",
                "alpha worker",
                "--settings",
                "/state/agents/ag_1/settings.json",
                "--mcp-config",
                "/state/agents/ag_1/mcp.json",
                "--strict-mcp-config",
                "--",
                "Fix the flaky test",
            ]
        );
    }

    #[test]
    fn bypassing_permissions_needs_an_explicit_request() {
        let defaults = args(None, &spec());
        assert!(
            !strings(&defaults)
                .iter()
                .any(|arg| arg.contains("ermission"))
        );
        let asked = SpawnSpec {
            permission_mode: Some(PermissionMode::BypassPermissions),
            ..spec()
        };
        let asked = args(None, &asked);
        let asked = strings(&asked);
        let at = asked
            .iter()
            .position(|arg| *arg == "--permission-mode")
            .unwrap();
        assert_eq!(asked[at + 1], "bypassPermissions");
    }

    #[test]
    fn an_empty_name_is_left_to_claude() {
        let unnamed = SpawnSpec {
            name: "  ".into(),
            ..spec()
        };
        assert!(!strings(&args(None, &unnamed)).contains(&"--name"));
    }

    #[test]
    fn a_one_word_prompt_cannot_become_a_subcommand() {
        assert_eq!(prompt_operand("update"), "update\n");
        assert_eq!(prompt_operand("purge"), "purge\n");
        assert_eq!(prompt_operand("fix the tests"), "fix the tests");
        assert_eq!(prompt_operand("line one\nline two"), "line one\nline two");
    }

    #[test]
    fn strip_escapes_leaves_plain_text_alone() {
        assert!(matches!(strip_escapes("plain"), Cow::Borrowed("plain")));
        assert_eq!(strip_escapes("a\u{1b}[31mb\u{1b}7c"), "abc");
        assert_eq!(strip_escapes("\u{1b}]0;title\u{7}text"), "text");
    }
}
