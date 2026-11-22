//! Errors of the rules, always carrying where they happened.

use std::fmt;

/// A load or run-time failure of a rules file, with the file and line when
/// Lua said them. It never aborts anything: the caller shows it and the
/// rules answer with the safest outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleError {
    /// The rules file, as the caller named it (a path).
    pub file: Option<String>,
    /// The line in that file.
    pub line: Option<u32>,
    /// The rule function that failed (`risk`, `layers`...), if it was one.
    pub function: Option<String>,
    /// What went wrong.
    pub message: String,
}

impl RuleError {
    pub(crate) fn plain(message: impl Into<String>) -> Self {
        Self {
            file: None,
            line: None,
            function: None,
            message: message.into(),
        }
    }

    pub(crate) fn in_function(mut self, name: &str) -> Self {
        self.function = Some(name.to_owned());
        self
    }

    /// Reads an `mlua` error. `chunks` maps the chunk ids given to Lua
    /// (`volee-repo`...) to the file names shown to people.
    pub(crate) fn from_lua(err: &mlua::Error, chunks: &[(String, String)]) -> Self {
        let text = lua_message(err);
        let first = text.lines().next().unwrap_or_default().trim();
        for (id, file) in chunks {
            let prefix = format!("{id}:");
            let Some(at) = first.find(&prefix) else {
                continue;
            };
            let rest = &first[at + prefix.len()..];
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            let Ok(line) = digits.parse::<u32>() else {
                continue;
            };
            let message = rest[digits.len()..].trim_start_matches(':').trim();
            return Self {
                file: Some(file.clone()),
                line: Some(line),
                function: None,
                message: message.to_owned(),
            };
        }
        Self::plain(first)
    }
}

fn lua_message(err: &mlua::Error) -> String {
    match err {
        mlua::Error::CallbackError { cause, .. } | mlua::Error::WithContext { cause, .. } => {
            lua_message(cause)
        }
        mlua::Error::RuntimeError(text) => text.clone(),
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        mlua::Error::MemoryError(_) => "the rule used more memory than it may".to_owned(),
        other => other.to_string(),
    }
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.file, self.line) {
            (Some(file), Some(line)) => write!(f, "{file}:{line}: ")?,
            (Some(file), None) => write!(f, "{file}: ")?,
            _ => {}
        }
        if let Some(function) = &self.function {
            write!(f, "in {function}(): ")?;
        }
        f.write_str(&self.message)
    }
}

impl std::error::Error for RuleError {}
