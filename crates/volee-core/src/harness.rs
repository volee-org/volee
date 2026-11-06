//! Coding harnesses and their sessions.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The program that runs the model for an agent.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessKind {
    /// Anthropic's Claude Code.
    ClaudeCode,
    /// Any other harness, named by its adapter.
    Other(String),
}

impl fmt::Display for HarnessKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClaudeCode => f.write_str("claude-code"),
            Self::Other(name) => f.write_str(name),
        }
    }
}

/// Identifies one harness session, for example a Claude Code session UUID.
///
/// A session key is unique across harnesses because it carries its harness.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionKey {
    /// The harness that owns the session.
    pub harness: HarnessKind,
    /// The harness's own identifier for the session.
    pub id: String,
}

impl SessionKey {
    /// A Claude Code session, by its session UUID.
    pub fn claude(id: impl Into<String>) -> Self {
        Self {
            harness: HarnessKind::ClaudeCode,
            id: id.into(),
        }
    }

    /// The first eight characters of the session id, which Claude Code uses
    /// as the short id of a background session.
    pub fn short(&self) -> &str {
        self.id.get(..8).unwrap_or(&self.id)
    }
}

impl fmt::Display for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.harness, self.id)
    }
}

/// How a session came to belong to an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionOrigin {
    /// The session was started by volee for this agent.
    Started,
    /// The session continues a previous session of the same agent in place.
    Resumed,
    /// The session is a copy of a previous session of the same agent, with a
    /// new id and the same conversation.
    Forked,
    /// volee found the session already running.
    Discovered,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_id_matches_claude_background_ids() {
        let key = SessionKey::claude("3be7fdc8-3d9c-4ff8-b917-8089d57780b2");
        assert_eq!(key.short(), "3be7fdc8");
        assert_eq!(SessionKey::claude("abc").short(), "abc");
    }

    #[test]
    fn display_carries_the_harness() {
        let key = SessionKey::claude("3be7fdc8");
        assert_eq!(key.to_string(), "claude-code:3be7fdc8");
    }
}
