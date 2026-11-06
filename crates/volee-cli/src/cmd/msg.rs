//! `volee msg send`.

use anyhow::Result;
use volee_core::AgentId;
use volee_core::message::{MessageKind, Party};
use volee_proto::methods::{SendMessage, SendParams};

use crate::connect::connect_or_start;

/// Sends a message from the human to an agent.
pub(crate) async fn send(to: AgentId, body: String, kind: MessageKind) -> Result<()> {
    let client = connect_or_start().await?;
    let sent = client
        .call::<SendMessage>(SendParams {
            from: Party::Human,
            to: vec![Party::Agent(to)],
            kind,
            body,
            refs: vec![],
            in_reply_to: None,
            thread: None,
        })
        .await?;
    println!(
        "Sent {} to {to}. It reaches the agent at the end of its turn, or wakes it if it is idle.",
        sent.message
    );
    Ok(())
}
