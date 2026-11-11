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
    /// The answers a question to the person offers, in the order they are
    /// shown; empty for any other message. The person may still answer in
    /// their own words: only an answer that is one of these counts as that
    /// choice (see [`Message::chosen`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// When it was sent.
    pub at: Timestamp,
    /// One receipt per recipient.
    pub receipts: Vec<Receipt>,
}

/// The fewest answers a question may offer: with one, there is no choice.
pub const MIN_OPTIONS: usize = 2;

/// The most answers a question may offer, so that each one has a digit and
/// they all fit under the question.
pub const MAX_OPTIONS: usize = 6;

/// The longest answer a question may offer, in characters.
pub const MAX_OPTION_CHARS: usize = 40;

/// Why a list of answers cannot be offered. Each reason reads as a sentence
/// for whoever asked.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OptionsError {
    /// Fewer than [`MIN_OPTIONS`] or more than [`MAX_OPTIONS`].
    #[error(
        "a question offers between {min} and {max} answers, not {0}: ask a narrower question, \
         or ask it without options",
        min = MIN_OPTIONS,
        max = MAX_OPTIONS
    )]
    Count(usize),
    /// An answer is empty, on several lines, or longer than
    /// [`MAX_OPTION_CHARS`].
    #[error(
        "the answer {0:?} cannot be offered: an answer is a few words on one line, at most \
         {max} characters",
        max = MAX_OPTION_CHARS
    )]
    Shape(String),
    /// Two answers are the same, ignoring case.
    #[error("the answer {0:?} is offered twice")]
    Twice(String),
}

/// Checks the answers a question offers: [`MIN_OPTIONS`] to [`MAX_OPTIONS`]
/// of them, each a few words on one line without control characters, and
/// all different whatever their case. No answers at all is a free question,
/// which is fine.
pub fn check_options(options: &[String]) -> Result<(), OptionsError> {
    if options.is_empty() {
        return Ok(());
    }
    if !(MIN_OPTIONS..=MAX_OPTIONS).contains(&options.len()) {
        return Err(OptionsError::Count(options.len()));
    }
    for (index, option) in options.iter().enumerate() {
        let plain = !option.is_empty()
            && option.trim() == option
            && option.chars().count() <= MAX_OPTION_CHARS
            && !option.chars().any(char::is_control);
        if !plain {
            return Err(OptionsError::Shape(option.clone()));
        }
        if options[..index]
            .iter()
            .any(|earlier| earlier.eq_ignore_ascii_case(option))
        {
            return Err(OptionsError::Twice(option.clone()));
        }
    }
    Ok(())
}

impl Message {
    /// The answer this question offers that `answer` is: the same words,
    /// whatever their case and the blanks around them. Anything else, a
    /// longer sentence that contains an option included, chooses nothing.
    pub fn chosen(&self, answer: &str) -> Option<&str> {
        let answer = answer.trim();
        self.options
            .iter()
            .map(String::as_str)
            .find(|option| option.eq_ignore_ascii_case(answer))
    }

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
            options: Vec::new(),
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

    fn offering(options: &[&str]) -> Message {
        Message {
            id: MessageId::from_ulid(Ulid::from_parts(1, 1)),
            thread: ThreadId::from_ulid(Ulid::from_parts(1, 2)),
            from: Party::Agent(AgentId::from_ulid(Ulid::from_parts(1, 3))),
            kind: MessageKind::Question,
            body: "Merge?".into(),
            refs: vec![],
            in_reply_to: None,
            options: options.iter().map(|option| (*option).to_owned()).collect(),
            at: Timestamp::from_millis(0),
            receipts: vec![],
        }
    }

    #[test]
    fn an_answer_chooses_an_option_only_when_it_is_that_option() {
        let question = offering(&["ok merge", "no"]);
        assert_eq!(question.chosen("ok merge"), Some("ok merge"));
        // The person's spelling does not matter, nor the blanks around.
        assert_eq!(question.chosen("  OK Merge\n"), Some("ok merge"));
        assert_eq!(question.chosen("No"), Some("no"));
        // A sentence that contains an option is the person's own words.
        assert_eq!(question.chosen("ok merge, but only the first one"), None);
        assert_eq!(question.chosen("not ok merge"), None);
        assert_eq!(question.chosen("1"), None);
        assert_eq!(question.chosen(""), None);
        assert_eq!(offering(&[]).chosen("ok merge"), None);
    }

    #[test]
    fn offered_answers_are_few_short_and_different() {
        let offer = |options: &[&str]| {
            check_options(
                &options
                    .iter()
                    .map(|option| (*option).to_owned())
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(offer(&[]), Ok(()), "a free question");
        assert_eq!(offer(&["ok merge", "no"]), Ok(()));
        assert_eq!(offer(&["a", "b", "c", "d", "e", "f"]), Ok(()));
        assert_eq!(offer(&["yes"]), Err(OptionsError::Count(1)));
        assert_eq!(
            offer(&["a", "b", "c", "d", "e", "f", "g"]),
            Err(OptionsError::Count(7))
        );
        for odd in ["", " yes", "yes ", "yes\nno", "bell\u{7}"] {
            assert_eq!(
                offer(&["no", odd]),
                Err(OptionsError::Shape(odd.to_owned())),
                "{odd:?}"
            );
        }
        let long = "x".repeat(MAX_OPTION_CHARS + 1);
        assert_eq!(
            offer(&["no", &long]),
            Err(OptionsError::Shape(long.clone()))
        );
        assert_eq!(offer(&["no", &long[1..]]), Ok(()));
        assert_eq!(
            offer(&["Yes", "no", "yes"]),
            Err(OptionsError::Twice("yes".into()))
        );
        // Each refusal is a sentence for the agent that asked.
        let count = OptionsError::Count(1).to_string();
        assert!(count.contains("between 2 and 6 answers, not 1"), "{count}");
        let shape = OptionsError::Shape(long).to_string();
        assert!(shape.contains("at most 40 characters"), "{shape}");
    }

    #[test]
    fn messages_written_before_options_existed_still_read() {
        let question = offering(&[]);
        let json = serde_json::to_string(&question).unwrap();
        assert!(!json.contains("options"), "{json}");
        assert_eq!(serde_json::from_str::<Message>(&json).unwrap(), question);
        let offered = offering(&["ok merge", "no"]);
        let json = serde_json::to_string(&offered).unwrap();
        assert!(json.contains(r#""options":["ok merge","no"]"#), "{json}");
        assert_eq!(serde_json::from_str::<Message>(&json).unwrap(), offered);
    }
}
