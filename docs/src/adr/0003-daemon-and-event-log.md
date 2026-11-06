# 0003. A daemon with an event log is the single source of truth

- Status: Accepted
- Date: 2026-11-06

## Context

The prototype had four different ways to join its state file with live data,
and its CLI and its TUI could disagree about the same agent. Several clients
need the same view of the world: the TUI, the CLI, the MCP server used by
agents, and the hook command called by agents dozens of times per turn.

## Decision

- One daemon (`volee daemon`) owns all state. It is started on demand by any
  client and listens on a Unix socket with user-only permissions.
- The protocol is JSON-RPC 2.0, one JSON object per line. Clients can
  subscribe to state changes; nobody polls the daemon.
- State lives in SQLite (WAL mode). Facts arrive as **events** in an
  append-only table; the current state (agents, sessions, messages, missions)
  is a projection of those events, updated in the same transaction.
- `volee hook` must never slow an agent down: it sends its event with a short
  timeout and falls back to appending to a spool file when the daemon is
  unreachable. The daemon ingests the spool at start.

## Consequences

- One join rule, one place to test it, one answer everywhere.
- The event log gives the timeline view and debugging for free.
- The daemon becomes a component to supervise. It is small, restarts on
  demand, and loses nothing thanks to the spool.

## Alternatives considered

- **A shared state file with file locks** (the prototype). Every reader joins
  it with live data on its own, which is how the four joins appeared.
- **A server per client.** Duplicates reconciliation and work.
- **jsonrpsee or tarpc for IPC.** Heavier than needed, or Rust-only. Line
  delimited JSON-RPC is trivial to speak from any language and from `nc`.
