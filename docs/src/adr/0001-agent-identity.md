# 0001. An agent is a unit of work, not a pane or a process

- Status: Accepted
- Date: 2026-11-06

## Context

volee grew out of a prototype that identified an agent by the terminal pane
it ran in. A review of that prototype on 2026-11-06 found that this single
choice caused most of its visible bugs:

- When a pane closed, the agent was shown as gone while its session kept
  running.
- When a session was attached from another pane, the same agent appeared
  twice.
- A stale record whose pane had disappeared took over the state of an
  unrelated agent in the same multiplexer session.

Meanwhile, coding harnesses moved on. Claude Code background sessions exist
independently of any terminal, can be attached from anywhere, and are reaped
and restarted by their supervisor. Resuming a stopped session can create a new
session (a fork) that continues the same conversation.

## Decision

- An **agent** is a unit of work with a volee identifier (`AgentId`, a
  sortable ULID rendered as `ag_...`). It has a role, an optional parent, an
  optional mission, a workspace (repository, worktree, branch, base) and a
  lifecycle state.
- An agent has one or more **harness sessions** (for Claude Code: session
  UUIDs). Sessions form a lineage: start, resume, fork. Exactly one session is
  current.
- A **location** (background supervisor, multiplexer pane, plain terminal) is
  an attribute that can change at any time and is never used as identity.
- Sessions that volee did not launch get an agent on first sight, keyed by
  their session id, with the role `independent`.

## Consequences

- Closing, moving or re-attaching a terminal never changes what volee shows.
- A resumed or forked session stays the same agent in the TUI, with its
  history.
- Every join between sources uses session ids, never names or positions.
- volee must store the session lineage; this is one more table.

## Alternatives considered

- **Pane as identity** (the prototype). Rejected for the reasons above.
- **Harness session id as identity.** Simple, but a resume that forks would
  create a new agent and lose its history, missions and messages.
- **Process id.** Changes on every restart and is reused by the OS.
