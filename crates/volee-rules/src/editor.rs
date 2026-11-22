//! Editor commands: how the person's own editor opens a file at a line.

use serde::{Deserialize, Serialize};

/// Where an editor runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditorKind {
    /// Needs a terminal (`nvim`, `hx`): volee opens it in a terminal pane.
    Terminal,
    /// Opens its own window (`code`, `zed`): volee just runs it.
    External,
}

/// A configured editor: `{ name = "nvim", kind = "terminal", cmd = "nvim +{line} {file}" }`.
///
/// Placeholders: `{file}` (absolute path), `{line}`, `{dir}` (the worktree)
/// and `{repo}` (the repository name).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Editor {
    /// The name shown in the interface.
    pub name: String,
    /// Terminal or external.
    pub kind: EditorKind,
    /// The command line, with placeholders.
    pub cmd: String,
}

/// What to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorTarget {
    /// Absolute path of the file.
    pub file: String,
    /// 1-based line.
    pub line: u32,
    /// The worktree directory.
    pub dir: String,
    /// The repository name.
    pub repo: String,
}

/// An editor command that cannot be used.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EditorError {
    /// Nothing to run.
    #[error("editor {0:?} has an empty cmd")]
    Empty(String),
    /// A placeholder that does not exist.
    #[error("editor {name:?} uses the unknown placeholder {{{placeholder}}}")]
    UnknownPlaceholder {
        /// The editor.
        name: String,
        /// The placeholder, without braces.
        placeholder: String,
    },
    /// A quote that never closes.
    #[error("editor {0:?} has an unclosed quote in cmd")]
    Unclosed(String),
}

const PLACEHOLDERS: [&str; 4] = ["file", "line", "dir", "repo"];

impl Editor {
    /// Checks the command: not empty, quotes closed, known placeholders only.
    pub fn validate(&self) -> Result<(), EditorError> {
        let words =
            split_words(&self.cmd).ok_or_else(|| EditorError::Unclosed(self.name.clone()))?;
        if words.is_empty() {
            return Err(EditorError::Empty(self.name.clone()));
        }
        for word in &words {
            for placeholder in placeholders(word) {
                if !PLACEHOLDERS.contains(&placeholder) {
                    return Err(EditorError::UnknownPlaceholder {
                        name: self.name.clone(),
                        placeholder: placeholder.to_owned(),
                    });
                }
            }
        }
        Ok(())
    }

    /// The program and arguments to run. The template is split into words
    /// first and the placeholders are filled after, so a path with spaces or
    /// shell characters stays one argument and is never interpreted.
    pub fn command(&self, target: &EditorTarget) -> Result<Vec<String>, EditorError> {
        self.validate()?;
        let words = split_words(&self.cmd).unwrap_or_default();
        let line = target.line.to_string();
        Ok(words
            .into_iter()
            .map(|word| {
                word.replace("{file}", &target.file)
                    .replace("{line}", &line)
                    .replace("{dir}", &target.dir)
                    .replace("{repo}", &target.repo)
            })
            .collect())
    }
}

fn placeholders(word: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = word;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                found.push(&after[..close]);
                rest = &after[close + 1..];
            }
            None => break,
        }
    }
    found
}

/// Splits on whitespace, honouring single and double quotes. `None` when a
/// quote never closes.
fn split_words(text: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            (None, c) => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if in_word {
        words.push(current);
    }
    Some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> EditorTarget {
        EditorTarget {
            file: "/w/my dir/a b.rs; rm -rf".into(),
            line: 42,
            dir: "/w".into(),
            repo: "volee".into(),
        }
    }

    fn editor(cmd: &str) -> Editor {
        Editor {
            name: "e".into(),
            kind: EditorKind::Terminal,
            cmd: cmd.into(),
        }
    }

    #[test]
    fn placeholders_are_filled_after_splitting_so_paths_stay_one_argument() {
        let argv = editor("nvim +{line} {file}").command(&target()).unwrap();
        assert_eq!(argv, ["nvim", "+42", "/w/my dir/a b.rs; rm -rf"]);
    }

    #[test]
    fn quotes_group_words_in_the_template() {
        let argv = editor("open -a 'Visual Studio Code' {dir}")
            .command(&target())
            .unwrap();
        assert_eq!(argv, ["open", "-a", "Visual Studio Code", "/w"]);
    }

    #[test]
    fn bad_commands_are_refused() {
        assert_eq!(editor("  ").validate(), Err(EditorError::Empty("e".into())));
        assert_eq!(
            editor("nvim {nope}").validate(),
            Err(EditorError::UnknownPlaceholder {
                name: "e".into(),
                placeholder: "nope".into()
            })
        );
        assert_eq!(
            editor("nvim 'x").validate(),
            Err(EditorError::Unclosed("e".into()))
        );
    }
}
