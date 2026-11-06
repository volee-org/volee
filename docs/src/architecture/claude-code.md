# Claude Code adapter

The `volee-harness` crate is how volee talks to Claude Code. It relies only
on documented commands and hooks, never on Claude Code's internal files
([ADR 0004](../adr/0004-sources-and-reconciliation.md)). Everything on this
page was measured on Claude Code 2.1.319; volee needs 2.1.300 or newer and
says so at startup when the installed version is older.

The daemon uses the adapter through the `Harness` trait, so other harnesses
can join later. Every command runs through a `CommandRunner` with a timeout,
which lets the tests replay recorded output instead of running `claude`.

## Commands

| Need | Command | Notes |
| --- | --- | --- |
| Version | `claude --version` | Checked when the daemon starts. |
| Every session | `claude agents --json --all` | Polled and reconciled by session id. |
| Launch | `claude --bg [flags] -- <prompt>` | Runs in the agent's worktree; prints `backgrounded · <id> · <name>`. |
| Resume | `claude --bg --resume <session-uuid> [flags] -- <prompt>` | Continues a stopped session; 2.1.319 creates a fork, linked to the same agent. |
| Stop | `claude stop <id>` | The conversation is kept. |
| Delete | `claude rm <id>` | Never with `--discard-unpushed` or `--force-remove-worktree`. |
| Preview | `claude logs <id>` | A raw replay of the session's terminal. |
| Take over | `claude attach <id>` | Runs in the TUI's terminal; `Ctrl+Z` returns to volee. |

`<id>` is the short id of a background session: the first eight characters of
its session UUID.

Every launch passes the agent's settings file (`--settings`, volee's hooks)
and its MCP configuration (`--mcp-config` with `--strict-mcp-config`, so the
agent loads volee's tools and nothing else). A permission mode is passed only
when the launch asks for one: volee never uses `bypassPermissions` on its own.
The prompt always comes after `--`, so no prompt can be read as an option, and
a one-word prompt gets a trailing newline so that it can never run a Claude
Code subcommand such as `claude update`.

## Session states

Background sessions report a `state`; interactive sessions only report the
`status` of their process, which is also the fallback when `state` is missing
or unknown.

| Claude Code reports | volee shows |
| --- | --- |
| `working` | working |
| `blocked`, waiting for `permission prompt` | needs input: permission |
| `blocked`, waiting for `input needed` | needs input: question |
| `blocked`, waiting for `dialog open` | needs input: dialog |
| `blocked`, waiting for anything else | needs input, with Claude Code's words |
| `done`, process alive | done (ready for the next prompt) |
| `done`, process gone | stopped |
| `failed` | failed |
| `stopped` | stopped |
| status `busy` | working |
| status `waiting` | needs input |
| status `idle` | idle |

Hooks refine these states for the agents volee launched, for example with the
tool that waits for permission.

## Hooks

The settings file of an agent adds one hook per event to the user's own
hooks; the user's settings file is never edited. Each hook runs
`volee hook <event> --agent <id>` with a ten-second timeout, and the `Stop`
event has a second handler that waits for messages while the agent is idle.
Claude Code gives that waiter 30 seconds more than its own wait, so it always
ends on its own:

```json
"Stop": [{ "hooks": [
  { "type": "command", "command": "/usr/local/bin/volee hook stop --agent ag_01jc...", "timeout": 10 },
  { "type": "command", "command": "/usr/local/bin/volee hook wait --agent ag_01jc... --timeout 3600",
    "asyncRewake": true, "timeout": 3630 }
]}]
```

| Event | What volee takes from it |
| --- | --- |
| `SessionStart` | The session id (to link the session to the agent), source, model and title. |
| `UserPromptSubmit` | The agent is working. |
| `PreToolUse` | A one-line summary such as `Bash: cargo test`; `AskUserQuestion` means a question for you. |
| `PostToolUse` | The summary and the files the tool wrote. |
| `Notification` | Permission prompts and dialogs; other notifications are ignored. |
| `Stop` | The end of the turn and the last message; pending messages are delivered here. |
| `SessionEnd` | The session ended, and why. |
| `SubagentStart`, `SubagentStop` | Subagents running inside the session. |

Messages reach a working agent through the `Stop` hook, which prints
`{"decision":"block","reason":"<messages>"}`, and an idle agent through the
waiter, which exits with status 2 and the messages on standard error
([ADR 0006](../adr/0006-messaging.md)).

## Tolerance

Claude Code evolves quickly, so the adapter reads its output defensively:

- Unknown fields are ignored, every event-specific field is optional, and a
  field with an unexpected type is treated as missing.
- A listing element that cannot be read is skipped and marks the listing as
  incomplete, so it can never make an agent look gone.
- Terminal escape sequences and extra lines around the `backgrounded` line
  are ignored.
