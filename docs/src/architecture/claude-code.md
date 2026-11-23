# Claude Code adapter

volee v2 works with Claude Code through four documented channels and one
tolerated file format. All of it is measured on Claude Code 2.1.330. The rule
of [ADR 0004](../adr/0004-sources-and-reconciliation.md) stands, with one
exception recorded there: volee never parses Claude Code's internal files
(`~/.claude/jobs`, `~/.claude/sessions`), and reads transcripts only through
one tolerant module.

| Channel | Gives volee | Used for |
| --- | --- | --- |
| **Hooks over `http`** | Every event as it happens, and a way to answer | Events, held permissions and questions |
| **`claude agents --json`** | Which sessions exist and their state | The agents view |
| **Transcripts** `~/.claude/projects/<dir>/<session>.jsonl` | What the agent was asked, planned, ran and said | The plan, the open points, the reasons per file |
| **`/code-review`** | Findings on a branch | The pre-review |
| **The session's messaging socket, or `claude --bg --resume`** | A way to say something to a session | A fix request you ask for |

volee never types into a terminal, and Claude Code has no public API for it.

## Hooks

`volee doctor` writes sixteen `http` hook entries into one settings file (see
[the overview](overview.md#the-hook-settings)). A hook posts the event's JSON
with the per-install token; the daemon stores it raw in SQLite. `SessionStart`
takes no `http` hook, so it gets a silent `curl` command hook.

Held decisions use two of them (measured, see
[ADR 0038](../adr/0038-a-permission-or-a-question-is-held-until-the-person-decides.md)):

- **`PermissionRequest`** waits for the daemon's answer (a long, explicit
  timeout). Claude Code shows its own dialog at the same time, so it is a
  concurrent decider: if the terminal answers first the daemon learns it from
  the next events of the session and drops the card. The request carries no id,
  so it is matched by session, tool name and tool input. On timeout Claude Code
  falls back to its dialog, never to a deny.
- **`PreToolUse` with the matcher `AskUserQuestion`** answers in place of the
  person: `permissionDecision: allow` and an `updatedInput` that echoes the
  original `questions` and adds `answers`. `allow` without `updatedInput` fails
  ("Invalid tool parameters"). While the hook runs the terminal shows only a
  spinner, which is why the hold is short.

A session must not depend on volee being up. A refused connection, a timeout and
any non-`2xx` answer are non-blocking errors for Claude Code, and volee never
sets `onFailure: "block"`.

## The agents listing

`claude agents --json` gives, per session: id, session id, name, working
directory, kind, start time, state, status, what it waits for and the process
id. Interactive sessions and background sessions are listed; a `claude -p` run
is not, even while it runs. Hook payloads carry the session id but no process
id, so the join between hooks and the listing is by session id.

A session started with the Agent SDK does not appear in the listing (only its
transcript exists), so volee does not see it. The SDK is not used: hooks cover
every card of v1, and the SDK is a later, opt-in option (framing note, T3).

## Transcripts

`crates/volee-daemon/src/transcript.rs` is the only code that knows the format.
A line that does not parse, a line of an unknown type, a missing field: skipped
and counted. An unreadable or missing file gives "transcript unavailable" and
the work screen shows git and hook facts only. It is tested against a real
recorded session and synthetic odd lines. See
[ADR 0040](../adr/0040-a-finished-change-is-read-in-layers.md).

## The pre-review

`/code-review` runs read-only in the agent's worktree in a session named
`volee-review-...`, with no hooks, which the agents view ignores. The program is
`claude.command` of `config.lua`; the model and number of reviewers come from the
rules. Findings are stored per `(repo, branch, tip)`. See
[ADR 0039](../adr/0039-the-pre-review-is-claude-codes-code-review-and-an-opinion.md).

## A fix request

volee reads whether the session is alive again when the request is sent (a
fresh `claude agents --json --all` and a check that the process exists). For a
live session (a process id in the listing) one JSON line is written to its
Unix socket `/tmp/cc-socks/<pid>.sock`; the socket must be owned by you and not
reachable by others, or volee does not write to it and resumes the session
instead (a session that still runs answers "already running"). For a stopped session it runs
`claude --bg --resume <session id> "<text>"` and no other option, since any
other option starts a copy. It never runs `claude --resume -p` on a session that
is open. A write is not a delivery: volee confirms by the transcript or by the
agent turning to `working`, else it says "NOT delivered". The socket's line
schema is not documented by Claude Code; it is kept in one function
(`actions::fix::socket_line`) so a change is one edit. See
[ADR 0041](../adr/0041-actions-on-a-finished-change-and-how-a-fix-reaches-the-agent.md).

## What changes with Claude Code

An update can change what volee shows without any change in volee: the hook
payloads, the listing, the transcript format and the socket line are Claude
Code's. That is why each is read leniently (unknown fields and events are
ignored), why the transcript degrades to hooks and git, and why the proof of a
release runs a real Claude Code.
