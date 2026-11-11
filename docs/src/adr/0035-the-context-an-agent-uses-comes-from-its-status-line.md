# 0035. The context an agent uses comes from its status line

- Status: Accepted
- Date: 2026-11-11

## Context

The person compared volee's main screen with the cockpit it replaces. Each
card of the cockpit showed how much of its context window an agent's
conversation used (`ctx 15%`), the warning that a conversation is about to
be compacted. volee did not know it.

Claude Code gives it in one documented place: the JSON it passes on stdin to
a session's status line command (`context_window.used_percentage`, with the
model, the directory and the rate limits), each time the status line is
drawn again. Nothing else carries it: not `claude agents --json`, not the
hooks' payloads. volee does not read Claude Code's internal files
([ADR 0004](0004-sources-and-reconciliation.md)).

The person has a status line of their own, set in their Claude Code
settings: a script that shows the context, the rate limits and warnings.
Whatever volee does must leave what they see under the prompt as it is.

## Decision

**The agents volee launches run volee's status line.** Their per-agent
settings, passed with `--settings` as their hooks are
([ADR 0026](0026-edit-mode-types-into-an-agent-inside-volee.md)), set the
status line to `volee status-line --agent <id>`. The command reads the JSON
Claude Code gives it and:

1. tells the daemon the share of context the conversation uses
   (`agents.context`), with a timeout of 300 ms and never starting a daemon:
   a daemon that does not answer misses one refresh, and the next one tells
   it again;
2. runs the status line the person configured for Claude Code, as Claude
   Code would without volee's settings: the project's local settings
   (`.claude/settings.local.json`), else the project's
   (`.claude/settings.json`), else the person's own
   (`~/.claude/settings.json`), through a shell, with the same input, in the
   project's directory, for five seconds at most; and prints what it
   prints. Without one, it prints nothing. volee's own command is never run
   as the person's.

volee reads those settings files and never writes them.

**The daemon records a change, not a refresh.** `agents.context` commits an
event `context-used` only when the share differs from the one the agent
has: a conversation that grows from empty to full makes about a hundred
events, not one per refresh. The agent carries the last share.

**The cards show it.** A worker's card, and a session's, show the share as a
bar in the agent's color, in the color of what waits past 80 %, with its
percentage. Where volee does not know it, the card says `ctx ?`: the
sessions volee did not launch, and the agents launched by a volee older
than 0.16 until they are launched again.

## Consequences

- The person sees their own status line in the agents volee launches, as
  in any session; volee's command adds a call to the daemon to each
  refresh.
- A status line that depends on its own process (one that keeps state
  between refreshes in memory) still works: it is run anew at each refresh,
  as Claude Code runs it.
- The sessions volee did not launch have no context in volee. tuios can
  know it, when the person's status line feeds it
  (`tuios agent-statusline`); reading it from tuios is left for later.
- Adopted agents run in a volee host with volee's settings, and get the
  status line too.

## Alternatives considered

- **A status line of volee's own** (the model and the context). It would
  have replaced the person's status line in every agent volee launches.
- **Reading the conversation's transcript** to count the context. It is an
  internal file of Claude Code ([ADR 0004](0004-sources-and-reconciliation.md)).
- **An event at every refresh.** Thousands of events a day for nothing.
