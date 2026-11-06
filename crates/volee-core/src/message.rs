//! Messages between people and agents (ADR 0006).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, MessageId, MissionId, ThreadId};
use crate::time::Timestamp;

/// Someone who can send or receive a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "party", content = "agent")]
pub enum Party {
    /// The person using volee.
    Human,
    /// volee itself, for example the mission supervisor.
    System,
    /// An agent.
    Agent(AgentId),
}

impl fmt::Display for Party {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Human => f.write_str("you"),
            Self::System => f.write_str("volee"),
            Self::Agent(id) => write!(f, "{id}"),
        }
    }
}

/// What a message is for. The kind drives sorting and highlighting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessageKind {
    /// Asks something and expects an answer.
    Question,
    /// Answers a question.
    Answer,
    /// An agreement between agents, such as an API shape.
    Contract,
    /// A summary of finished work.
    Report,
    /// Something went wrong or needs attention.
    Alert,
    /// Anything else.
    Note,
}

impl MessageKind {
    /// The label shown to people.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::Answer => "answer",
            Self::Contract => "contract",
            Self::Report => "report",
            Self::Alert => "alert",
            Self::Note => "note",
        }
    }
}

/// Something a message points at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "ref")]
pub enum Reference {
    /// A file, optionally at a line.
    File {
        /// Path, relative to the agent's workspace when possible.
        path: String,
        /// One-based line number.
        line: Option<u32>,
    },
    /// A commit.
    Commit {
        /// Repository display name, when known.
        repo: Option<String>,
        /// Full or abbreviated commit id.
        sha: String,
    },
    /// A pull request.
    PullRequest {
        /// Web address.
        url: String,
    },
    /// Any web address.
    Url {
        /// Web address.
        url: String,
    },
}

/// When a recipient got and read a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// The recipient.
    pub to: Party,
    /// When the message reached the recipient.
    pub delivered_at: Option<Timestamp>,
    /// When the recipient read it.
    pub read_at: Option<Timestamp>,
}

/// A message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Its identifier; identifiers sort by creation time.
    pub id: MessageId,
    /// The conversation it belongs to.
    pub thread: ThreadId,
    /// The sender.
    pub from: Party,
    /// What it is for.
    pub kind: MessageKind,
    /// Markdown text.
    pub body: String,
    /// What it points at.
    pub refs: Vec<Reference>,
    /// The message it answers, if any.
    pub in_reply_to: Option<MessageId>,
    /// When it was sent.
    pub at: Timestamp,
    /// One receipt per recipient.
    pub receipts: Vec<Receipt>,
}

impl Message {
    /// The recipients.
    pub fn recipients(&self) -> impl Iterator<Item = Party> + '_ {
        self.receipts.iter().map(|receipt| receipt.to)
    }

    /// Whether `party` is a recipient.
    pub fn is_for(&self, party: Party) -> bool {
        self.recipients().any(|to| to == party)
    }

    /// The receipt of a recipient.
    pub fn receipt(&self, party: Party) -> Option<&Receipt> {
        self.receipts.iter().find(|receipt| receipt.to == party)
    }
}

/// A conversation: one per mission, one per pair of agents, and the inbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    /// Its identifier.
    pub id: ThreadId,
    /// A short title.
    pub title: String,
    /// The mission it belongs to, for mission threads.
    pub mission: Option<MissionId>,
    /// Who takes part.
    pub participants: Vec<Party>,
    /// When it was opened.
    pub opened_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ulid::Ulid;

    #[test]
    fn party_serializes_with_a_tag() {
        assert_eq!(
            serde_json::to_string(&Party::Human).unwrap(),
            r#"{"party":"human"}"#
        );
        let agent = Party::Agent(AgentId::from_ulid(Ulid::from_parts(1, 2)));
        let json = serde_json::to_string(&agent).unwrap();
        assert!(
            json.starts_with(r#"{"party":"agent","agent":"ag_"#),
            "{json}"
        );
        assert_eq!(serde_json::from_str::<Party>(&json).unwrap(), agent);
    }

    #[test]
    fn receipts_name_the_recipients() {
        let message = Message {
            id: MessageId::from_ulid(Ulid::from_parts(1, 1)),
            thread: ThreadId::from_ulid(Ulid::from_parts(1, 2)),
            from: Party::System,
            kind: MessageKind::Note,
            body: "hello".into(),
            refs: vec![],
            in_reply_to: None,
            at: Timestamp::from_millis(0),
            receipts: vec![Receipt {
                to: Party::Human,
                delivered_at: None,
                read_at: None,
            }],
        };
        assert!(message.is_for(Party::Human));
        assert!(!message.is_for(Party::System));
        assert_eq!(MessageKind::Contract.label(), "contract");
    }
}
