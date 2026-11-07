//! The MCP server through which coding agents talk to volee and to each
//! other (ADR 0005).
//!
//! Every agent volee launches gets this server (`volee mcp --agent <id>`)
//! in its MCP configuration. The tools are harness-neutral, so any harness
//! that speaks MCP can join the conversation:
//!
//! | Tool | Purpose |
//! | --- | --- |
//! | `whoami` | The calling agent's identity, role, mission and workspace. |
//! | `list_agents` | Every agent on the machine and whether the caller may write to it. |
//! | `send_message` | Write to agents or to the human, by id or by name. |
//! | `read_messages` | Read messages addressed to the caller. |
//! | `report` | Tell whoever launched the caller that its task is done, blocked or failed. |
//!
//! The server keeps a replica of the daemon's registry, fed by an event
//! subscription, and sends every change through the daemon, which applies
//! the routing rule.

mod server;
mod view;

pub use server::{McpError, VoleeMcp};

/// The argument types of the tools, for callers that invoke tools directly.
pub mod tools {
    pub use crate::server::{
        KindArg, ListArgs, OutcomeArg, ReadArgs, RefArg, ReportArgs, SendArgs, SpawnArgs,
    };
}
