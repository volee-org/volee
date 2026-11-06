//! Delivering messages to an agent through its hooks (ADR 0006).

use serde_json::json;
use volee_core::ids::AgentId;
use volee_core::message::{Message, Party, Reference};

/// What a `Stop` hook prints to make the agent continue with `text` instead
/// of ending its turn: `{"decision":"block","reason":"<text>"}`.
///
/// Claude Code reads it when the hook exits with status 0 and prints only
/// this object. It stops honoring blocks after eight consecutive
/// continuations without a tool call, so the daemon blocks only when it has
/// messages to deliver.
pub fn stop_block_output(text: &str) -> String {
    json!({ "decision": "block", "reason": text }).to_string()
}

/// The last line of every delivery: how to answer, with the arguments of the
/// MCP tool `send_message`.
const ANSWER_HINT: &str =
    "To answer, call the volee MCP tool send_message with in_reply_to set to the message id.";

/// Renders pending messages as plain text for an agent: who sent each one,
/// its kind, its id, what it answers and points at, then a one-line hint to
/// answer with volee's MCP tools.
///
/// `agent_name` gives the display name of an agent sender, when known. The
/// person using volee appears as "the human", the name `send_message` takes
/// for them. With no messages, the result is empty.
pub fn format_messages_for_agent<'a>(
    messages: impl IntoIterator<Item = &'a Message>,
    agent_name: impl Fn(AgentId) -> Option<String>,
) -> String {
    let messages: Vec<&Message> = messages.into_iter().collect();
    if messages.is_empty() {
        return String::new();
    }
    let count = messages.len();
    let plural = if count == 1 { "" } else { "s" };
    let mut lines = vec![format!("volee: {count} new message{plural} for you.")];
    for (index, message) in messages.iter().enumerate() {
        let sender = match message.from {
            Party::Human => "the human".to_owned(),
            Party::System => "volee".to_owned(),
            Party::Agent(id) => match agent_name(id) {
                Some(name) => format!("agent \"{name}\" ({id})"),
                None => format!("agent {id}"),
            },
        };
        let reply = message
            .in_reply_to
            .map(|original| format!(", in reply to {original}"))
            .unwrap_or_default();
        lines.push(String::new());
        lines.push(format!(
            "[{}] {} from {sender}, message {}{reply}",
            index + 1,
            message.kind.label(),
            message.id,
        ));
        lines.push(
            message
                .body
                .trim_start_matches(['\n', '\r'])
                .trim_end()
                .to_owned(),
        );
        if !message.refs.is_empty() {
            let refs: Vec<String> = message.refs.iter().map(describe).collect();
            lines.push(format!("References: {}", refs.join(", ")));
        }
    }
    lines.push(String::new());
    lines.push(ANSWER_HINT.to_owned());
    lines.join("\n")
}

fn describe(reference: &Reference) -> String {
    match reference {
        Reference::File { path, line: None } => path.clone(),
        Reference::File {
            path,
            line: Some(line),
        } => format!("{path}:{line}"),
        Reference::Commit { repo: None, sha } => format!("commit {sha}"),
        Reference::Commit {
            repo: Some(repo),
            sha,
        } => format!("commit {sha} in {repo}"),
        Reference::PullRequest { url } => format!("pull request {url}"),
        Reference::Url { url } => url.clone(),
    }
}

#[cfg(test)]
mod tests {
    use volee_core::ids::{MessageId, ThreadId};
    use volee_core::message::{MessageKind, Receipt};
    use volee_core::time::Timestamp;

    use super::*;

    const WORKER: &str = "ag_01jc5x8m3kq7v2c4b9e0f1g2h3";
    const STRANGER: &str = "ag_01jc5x8m3kq7v2c4b9e0f1g2h4";

    fn message(id: &str, from: Party, kind: MessageKind, body: &str) -> Message {
        Message {
            id: id.parse().unwrap(),
            thread: "th_01jc5x8m3kq7v2c4b9e0f1g2h3".parse::<ThreadId>().unwrap(),
            from,
            kind,
            body: body.into(),
            refs: vec![],
            in_reply_to: None,
            at: Timestamp::from_millis(0),
            receipts: vec![Receipt {
                to: Party::Human,
                delivered_at: None,
                read_at: None,
            }],
        }
    }

    fn names(id: AgentId) -> Option<String> {
        (id.to_string() == WORKER).then(|| "api worker".to_owned())
    }

    #[test]
    fn stop_block_output_is_one_json_object() {
        let text = "Line one\nSay \"hi\" \\ bye";
        let output = stop_block_output(text);
        assert!(!output.contains('\n'), "{output}");
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value, json!({ "decision": "block", "reason": text }));
        assert_eq!(
            stop_block_output("hello"),
            r#"{"decision":"block","reason":"hello"}"#
        );
    }

    #[test]
    fn messages_read_as_plain_text_with_a_hint() {
        let worker: AgentId = WORKER.parse().unwrap();
        let question_id: MessageId = "msg_01jc5x8m3kq7v2c4b9e0f1g2h3".parse().unwrap();
        let mut question = message(
            "msg_01jc5x8m3kq7v2c4b9e0f1g2h3",
            Party::Agent(worker),
            MessageKind::Question,
            "\nCan I rename `Foo::bar`?\n\n",
        );
        question.refs = vec![
            Reference::File {
                path: "src/lib.rs".into(),
                line: Some(42),
            },
            Reference::Commit {
                repo: Some("volee".into()),
                sha: "1a2b3c4".into(),
            },
        ];
        let mut answer = message(
            "msg_01jc5x8m3kq7v2c4b9e0f1g2h4",
            Party::Human,
            MessageKind::Answer,
            "Yes, and update the docs.",
        );
        answer.in_reply_to = Some(question_id);
        let note = message(
            "msg_01jc5x8m3kq7v2c4b9e0f1g2h5",
            Party::Agent(STRANGER.parse().unwrap()),
            MessageKind::Note,
            "FYI",
        );
        let system = message(
            "msg_01jc5x8m3kq7v2c4b9e0f1g2h6",
            Party::System,
            MessageKind::Alert,
            "Checks failed on your branch.",
        );

        let text = format_messages_for_agent([&question, &answer, &note, &system], names);
        assert_eq!(
            text,
            "volee: 4 new messages for you.\n\
             \n\
             [1] question from agent \"api worker\" (ag_01jc5x8m3kq7v2c4b9e0f1g2h3), message msg_01jc5x8m3kq7v2c4b9e0f1g2h3\n\
             Can I rename `Foo::bar`?\n\
             References: src/lib.rs:42, commit 1a2b3c4 in volee\n\
             \n\
             [2] answer from the human, message msg_01jc5x8m3kq7v2c4b9e0f1g2h4, in reply to msg_01jc5x8m3kq7v2c4b9e0f1g2h3\n\
             Yes, and update the docs.\n\
             \n\
             [3] note from agent ag_01jc5x8m3kq7v2c4b9e0f1g2h4, message msg_01jc5x8m3kq7v2c4b9e0f1g2h5\n\
             FYI\n\
             \n\
             [4] alert from volee, message msg_01jc5x8m3kq7v2c4b9e0f1g2h6\n\
             Checks failed on your branch.\n\
             \n\
             To answer, call the volee MCP tool send_message with in_reply_to set to the message id."
        );
    }

    #[test]
    fn one_message_is_singular_and_none_is_empty() {
        let note = message(
            "msg_01jc5x8m3kq7v2c4b9e0f1g2h3",
            Party::Human,
            MessageKind::Note,
            "Hello",
        );
        assert!(
            format_messages_for_agent([&note], names)
                .starts_with("volee: 1 new message for you.\n")
        );
        assert_eq!(format_messages_for_agent([], names), "");
    }

    #[test]
    fn every_reference_kind_is_described() {
        let cases = [
            (
                Reference::File {
                    path: "README.md".into(),
                    line: None,
                },
                "README.md",
            ),
            (
                Reference::Commit {
                    repo: None,
                    sha: "abc".into(),
                },
                "commit abc",
            ),
            (
                Reference::PullRequest {
                    url: "https://example.com/pr/1".into(),
                },
                "pull request https://example.com/pr/1",
            ),
            (
                Reference::Url {
                    url: "https://example.com".into(),
                },
                "https://example.com",
            ),
        ];
        for (reference, expected) in cases {
            assert_eq!(describe(&reference), expected);
        }
    }
}
