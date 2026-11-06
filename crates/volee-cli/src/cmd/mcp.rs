//! `volee mcp --agent <id>`: the MCP server of one agent, over stdio.
//!
//! Harnesses start this command for the agents volee launches. Stdout is the
//! MCP channel, so nothing else may be printed on it.

use anyhow::{Context, Result};
use rmcp::ServiceExt;
use volee_core::AgentId;
use volee_mcp::VoleeMcp;

use crate::connect::connect_or_start;

/// Serves the MCP tools for `agent` until the harness closes stdin.
pub(crate) async fn run(agent: AgentId) -> Result<()> {
    let client = connect_or_start().await?;
    let server = VoleeMcp::connect(agent, client).await?;
    let running = server
        .serve(rmcp::transport::stdio())
        .await
        .context("cannot start the MCP session on stdio")?;
    running
        .waiting()
        .await
        .context("the MCP session ended with an error")?;
    Ok(())
}
