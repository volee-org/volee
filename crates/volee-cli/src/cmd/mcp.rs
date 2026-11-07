//! `volee mcp`: the MCP server of one agent, over stdio.
//!
//! Harnesses start this command: with `--agent <id>` for the agents volee
//! launches, and with `--join <name>` for an agent of another harness that
//! joins volee (ADR 0014). Stdout is the MCP channel, so nothing else may be
//! printed on it.

use anyhow::{Context, Result};
use rmcp::ServiceExt;
use volee_core::AgentId;
use volee_mcp::VoleeMcp;
use volee_proto::methods::JoinParams;

use crate::connect::connect_or_start;
use crate::resolve;

/// Serves the MCP tools for `agent` until the harness closes stdin.
pub(crate) async fn run(agent: AgentId) -> Result<()> {
    let client = connect_or_start().await?;
    let server = VoleeMcp::connect(agent, client).await?;
    serve(server).await
}

/// Joins volee as `name`, an agent of `harness` working in the current
/// directory, optionally in `mission`, then serves its tools until the
/// harness closes stdin. volee knows the agent while this runs.
pub(crate) async fn join(name: &str, harness: &str, mission: Option<&str>) -> Result<()> {
    let client = connect_or_start().await?;
    let mission = match mission {
        Some(mission) => {
            let registry = resolve::registry(&client).await?;
            Some(resolve::mission(&registry, mission)?)
        }
        None => None,
    };
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    let params = JoinParams {
        name: name.to_owned(),
        harness: harness.to_owned(),
        cwd,
        mission,
    };
    let server = VoleeMcp::join(params, client)
        .await
        .context("cannot join volee")?;
    serve(server).await
}

async fn serve(server: VoleeMcp) -> Result<()> {
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
