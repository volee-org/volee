# 0005. Agents talk to volee through MCP and the CLI

- Status: Accepted
- Date: 2026-11-06

## Context

Agents need to find each other, send messages, report progress and, for
coordinators, spawn workers. The prototype gave its agents multiplexer
commands in their prompts (more than fifty calls across two prompts), which
tied every agent to that multiplexer.

Users run more than one AI. An agent of one harness should be able to message
an agent of another one on the same machine.

## Decision

- volee exposes an MCP server over stdio (`volee mcp`). Every agent volee
  launches gets it through `--mcp-config` and `--strict-mcp-config`, so a
  project `.mcp.json` approval can never stall a background session.
- The tools are harness-neutral: `whoami`, `list_agents`, `send_message`,
  `read_messages`, `report`, and for coordinators `spawn_worker` and
  `close_mission`.
- The same operations exist as CLI commands for scripts and for harnesses
  without MCP.
- Prompts given to agents never mention a multiplexer.

## Consequences

- Any MCP-capable harness can join the conversation; only process launching
  and state detection need a per-harness adapter.
- The MCP surface is a public API: it is versioned and documented.

## Alternatives considered

- **Multiplexer commands in prompts.** Couples agents to one terminal stack.
- **Claude Code cross-session messaging.** Claude-only, and its wire format is
  not published.
