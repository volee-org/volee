# 0006. Typed, threaded messages delivered through documented hooks

- Status: Accepted (amended by [0031](0031-a-question-may-offer-its-answers.md): a
  question to the person may offer its answers; and on 2026-11-17 by
  [a resting agent past its wake-up hook](#a-resting-agent-past-its-wake-up-hook))
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
     linked to the same agent (ADR 0001). The resume starts from the
     repository's main checkout with `--worktree` (ADR 0013), carries the
     pending messages as its prompt and the agent's instructions again, and
     the daemon links the new session by the handle the harness printed,
     before any listing could take it for a stranger. Two holds prevent a
     resume: the human stopped the agent (lifted by the human's next message
     to it, never by an agent's), or its mission is over (never lifted). An
     aborted mission's notice goes only to its live agents.
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

## A resting agent past its wake-up hook

Added on 2026-11-17, in 0.18.

An agent's wake-up hook waits for its messages for `[agents] wake_timeout`
seconds, an hour by default, then ends. The resumer only acts on an agent
whose session ended. An agent still at its prompt past that hour therefore
had nothing to hand it a message: it stayed pending for as long as the agent
rested. Measured on the person's machine: a question sent at night to an
agent whose last turn had ended over an hour before was never delivered, and the
messages window did not say so.

volee holds the terminal of an agent in a volee host (ADR 0019), so the
daemon types the pending messages at its prompt, the same text a hook would
hand it, then Enter, and marks them delivered. It does so only when the agent
is Claude Code, idle or done for twenty seconds at least, no hook of it waits
for messages, and nobody types into it: a watcher's welcome now says whether
a client has the host's input (`owned`), so volee never takes the input from
a person. When Claude Code's vim mode shows normal mode, volee presses `i`
first. A host started before 0.18 cannot say whether someone types into it,
and is left alone. A session volee does not host gets no message, as before,
and the window that writes to it says so.

The messages window shows, on each message the person wrote to agents,
whether it was delivered (`✓ delivered`) or waits (`⋯ not delivered yet`).
`journeys/deliver.py` sends a message to an agent thirty seconds after its
hook stopped waiting, five seconds after its turn, and the agent answers it.
