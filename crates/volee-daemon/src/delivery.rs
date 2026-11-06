//! Turning pending messages into text an agent reads (ADR 0006).

use std::fmt::Write as _;

use volee_core::Registry;
use volee_core::message::{Message, Party};

/// Renders messages for an agent, oldest first, with who sent them and why.
pub(crate) fn render_for_agent(registry: &Registry, messages: &[&Message]) -> String {
    let mut text = String::new();
    let count = messages.len();
    let plural = if count == 1 { "" } else { "s" };
    let _ = writeln!(text, "[volee] {count} new message{plural}:");
    for message in messages {
        let _ = writeln!(text);
        let _ = writeln!(
            text,
            "From {} ({}, id {}):",
            sender_label(registry, message.from),
            message.kind.label(),
            message.id
        );
        for line in message.body.lines() {
            let _ = writeln!(text, "  {line}");
        }
    }
    let _ = writeln!(text);
    let _ = write!(
        text,
        "Reply with the volee MCP tools (send_message, with in_reply_to set to the message id)."
    );
    text
}

/// A short description of a sender: `the human`, `volee`, or the agent's name
/// and role.
pub(crate) fn sender_label(registry: &Registry, party: Party) -> String {
    match party {
        Party::Human => "the human".to_owned(),
        Party::System => "volee".to_owned(),
        Party::Agent(id) => registry.agent(id).map_or_else(
            || id.to_string(),
            |agent| format!("{} ({})", agent.name, agent.role),
        ),
    }
}

/// The JSON a Claude Code `Stop` hook prints to hand `text` to the agent
/// instead of letting it stop.
pub(crate) fn stop_block_output(text: &str) -> String {
    serde_json::json!({ "decision": "block", "reason": text }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_block_output_is_one_json_object() {
        let output = stop_block_output("hello \"world\"\nbye");
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["decision"], "block");
        assert_eq!(value["reason"], "hello \"world\"\nbye");
        assert!(!output.contains('\n'));
    }
}
