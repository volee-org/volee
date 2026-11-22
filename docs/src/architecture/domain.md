# Data and events

What volee keeps, and what it computes each time. The rule behind it
([ADR 0003](../adr/0003-daemon-and-event-log.md)): keep what was said raw, and
decide what it means later, from the raw text.

## What is stored

Everything is in one SQLite database, `volee.db` in the state directory
(`~/.local/state/volee`). `volee events` reads it without the daemon.

| Table | Holds | Lives |
| --- | --- | --- |
| `hook_events` | Each Claude Code hook event as it arrived: time, event name, session id, working directory, raw payload. Append-only. | Kept |
| `journal` | One line per action: kind, branch, tip, ok, by hand or by a rule, a sentence. The only place an automatic action is visible. | Kept |
| `line_comments` | Your comments on a line of a diff, per repository and branch tip. | Until the next commit of that branch (a new tip starts clean) |
| `risk_overrides` | The risk you set by hand, per branch tip. | Until the next commit |
| `reviews`, `review_findings` | The pre-review of a branch tip: status, who reviewed, the findings. | Per `(repo, branch, tip)` |
| `branch_memory` | The last tip seen with commits of its own, so a later merge can be proven. | Kept |
| `pull_request_cache` | What the forge said about a branch's pull request. A merged one for good, another for five minutes, a failure for one. | Until stale |
| `events`, `meta` | The 0.x registry's event log and its metadata (messages, missions). See below. | Kept |

Nothing in the database is a secret. The token that authenticates hooks is a
file next to it (`hook-token`, readable by its owner only).

## What is computed

Everything the inbox shows is derived, not stored:

- **The agents view**: each Claude Code session joined to its hook events and to
  what git says of its worktree ([overview](overview.md#agents-and-finished-work)).
- **Finished work**: the `finished` rule of the Lua rules, or the built-in
  `is_finished_work` of `volee-core` (not busy, stopped after its last
  activity, not merged, at least one commit the base lacks).
- **The risk, layers, review plan and merge decision**: the rules, evaluated per
  repository and branch tip and cached until the rules files change.
- **The work screen**: git, the transcript, the diagram and the diff, built per
  request.

## Agent states

States come from Claude Code's listing, refined by hooks:

| State | Meaning |
| --- | --- |
| `launching` | Started, not seen running yet |
| `working` | Running a turn |
| `needs-input` | Waits for you: a permission, a question or a dialog |
| `idle` | Alive, waiting for its next instruction |
| `done` | Finished its last task |
| `failed` | Ended with an error |
| `stopped` | The process ended; the conversation can be resumed |
| `lost` | volee cannot find it any more; never treated as done |

Subagent and task events count as neither activity nor stop: on 2.1.330 the
subagent that writes the suggested next prompt stops a few seconds after the
turn's `Stop`, and counting it made a finished agent look busy.

## Held requests

A held permission or question lives in the daemon's memory, not in the database:
it is lost with the process, and Claude Code then asks in the terminal. The
journal records what you decided.

## The 0.x registry

`volee-core` still holds the 0.x model: agents with roles, sessions, missions,
threads and messages, rebuilt by replaying the `events` table
([ADR 0001](../adr/0001-agent-identity.md),
[ADR 0006](../adr/0006-messaging.md)). It serves `ask_human` and agents that join
over MCP. v2 does not feed it from Claude Code's listing any more: that is the
agents view's job. Its pieces (the mission supervisor, reconciliation of a
harness listing) are not part of the inbox and are on the list for removal; the
ADRs that describe them are kept as history.
