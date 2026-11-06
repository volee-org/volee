# 0006. Typed, threaded messages delivered through documented hooks

- Status: Accepted
- Date: 2026-11-06

## Context

Understanding how agents interact is a core goal. Messages must be readable
by humans (who asked what, who answered, what was agreed) and must reach
agents reliably whether they are working, idle or stopped.

Claude Code 2.1.319 has no supported way to type a prompt into a running
background session. Probes on 2026-11-06 established what works:

| Probe | Result |
| --- | --- |
| `Stop` hook returning `{"decision":"block","reason":"..."}` | The agent continued its turn with the message. |
| `Stop` hook with `asyncRewake: true` that waits, then exits 2 with the message on stderr | An idle agent woke up with the message, same session id. |
| `claude stop`, then `claude --bg --resume <session> "<message>"` | The conversation continued in a new forked session. |
| `PostToolUse` `additionalContext` | Not honored in the probe; not relied upon. |

## Decision

- A **message** has a sender (human, agent or system), recipients, a kind
  (`question`, `answer`, `contract`, `report`, `alert`, `note`), a markdown
  body, references (file and line, commit, pull request, URL), a thread, and
  per-recipient delivery and read timestamps.
- **Threads**: one per mission, one per pair of agents outside missions, and
  the human inbox. Questions to the human stay pinned until answered.
- **Routing**: inside a mission every member can write to every member;
  outside a mission an agent can write to its parent, its coordinator and the
  human. The human can write to anyone. Policies are configurable.
- **Delivery** to a Claude Code agent launched by volee:
  1. working: queued, delivered by the `Stop` block at the end of the turn;
  2. idle: delivered by the `asyncRewake` waiter armed at every `Stop`;
  3. stopped or reaped: delivered by `--bg --resume`, and the new session is
     linked to the same agent (ADR 0001).
- Agents that volee did not launch can be messaged only through an explicit
  "wake" action by the human, because it forks their session.
- Thread summaries by an LLM are opt-in and show their cost.

## Consequences

- Messages are never typed into a terminal, so pasting multi-line text is
  safe and no agent is interrupted mid-tool.
- A message to a working agent waits for its turn to end; urgent messages
  can be flagged in the TUI, which may offer a wake action.
- Only one `asyncRewake` waiter per agent may be active; the daemon retires
  older ones.

## Alternatives considered

- **Typing into panes** (the prototype). Interrupts agents, breaks on paste,
  needs a multiplexer.
- **Claude Code cross-session inbox socket.** Promising, but the message
  format is undocumented; kept as a possible future adapter.
- **Channels (research preview).** Requires flags and a plugin allowlist not
  available to every user.
