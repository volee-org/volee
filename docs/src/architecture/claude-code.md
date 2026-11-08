# Claude Code adapter

The `volee-harness` crate is how volee talks to Claude Code. It relies only
on documented commands and hooks, never on Claude Code's internal files
([ADR 0004](../adr/0004-sources-and-reconciliation.md)). Everything on this
page was measured on Claude Code 2.1.319 or taken from its documentation, as
marked; volee needs 2.1.319 or newer, the version it was measured on, and
says so at startup when the installed version is older.

The daemon uses the adapter through the `Harness` trait, so other harnesses
can join later. Every command runs through a `CommandRunner` with a timeout,
which lets the tests replay recorded output instead of running `claude`.

## Commands

| Need | Command | Notes |
| --- | --- | --- |
| Version | `claude --version` | Checked when the daemon starts. |
| Every session | `claude agents --json --all` | Polled and reconciled by session id. |
| Launch | `claude --bg --opt=value... -- <prompt>` | Runs in the agent's worktree; prints `backgrounded · <id> · <name>`. |
| Resume | `claude --bg --resume=<session-uuid> --opt=value... -- <prompt>` | Continues a stopped session; 2.1.319 creates a fork, linked to the same agent. |
| Stop | `claude stop <id>` | The conversation is kept. |
| Delete | `claude rm <id>` | Never with `--discard-unpushed` or `--force-remove-worktree`. |
| Preview | `claude logs <id>` | A raw replay of the session's terminal. |
| Take over | `claude attach <id>` | Runs in the TUI's terminal; `Ctrl+Z` returns to volee. |

`<id>` is the id of a background session, kept exactly as `claude agents`
lists it (the session's *handle*). Interactive sessions have no id: volee
shows them but cannot stop or delete them, and previews and takes over only
the ones that run in a tuios pane, through tuios
([ADR 0015](../adr/0015-tuios-location-adapter.md)).

`claude logs` replays the session's recent drawing, not a log of its text:
rows placed by cursor moves from the top left of the session's own terminal,
which can be far wider than 200 columns, some of them (the lines of a diff)
padded with colored blanks to its right edge, and nothing for what was drawn
before the replay starts, such as the frame of the prompt. The interface
replays it in a terminal emulator at least as wide as the widest row it
writes, so that no row wraps onto the next, and keeps a run of blank rows as
one, so that the end of the conversation stays in view
(`crates/volee-tui/src/preview.rs`; measured on 2.1.319 with a session 308
columns wide). When a tuios pane shows the session (`claude agents` or
`claude attach`, which title the terminal with the session's name), the
preview reads that pane instead, which holds the whole screen
([ADR 0018](../adr/0018-panes-that-show-background-sessions.md)).

Every launch passes the agent's settings file (`--settings`: volee's hooks,
and a rule that allows volee's own MCP tools) and its MCP configuration
(`--mcp-config` with `--strict-mcp-config`, so the agent loads volee's tools
and nothing else). The permission mode and tool rules come from volee's
configuration only, never from an agent's request, and there is no way to
ask for `bypassPermissions`. Every launch and resume passes
`--permission-mode`: the configured mode, else `manual`, never the person's
own default, which may skip every check.

Every value travels inside its option (`--name=alpha`, not `--name alpha`),
so no value can be read as an option and no list option can swallow the next
argument; Claude Code 2.1.319 accepts this form and rejects unknown options
(measured on `claude agents`, read-only). The prompt comes last, after `--`.
A one-word prompt gets a trailing newline and a prompt starting with `-` a
leading one, so that no prompt can run a Claude Code subcommand such as
`claude update` or be read as an option.

## Environment and directories

Claude Code sets variables in the environment of the processes it starts
(shell commands, hooks, MCP servers, background sessions) that tie them to
one session: `CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_ENTRYPOINT`,
`CLAUDE_CODE_MESSAGING_SOCKET`, `CLAUDE_CODE_MESSAGING_TOKEN`,
`CLAUDE_JOB_DIR`, `CLAUDE_PID`, `CLAUDE_EFFORT` and a few more. volee removes
all of them from every `claude` it runs, so a daemon started from inside an
agent never launches or controls sessions as that agent. The user's own
configuration variables, such as `CLAUDE_CODE_SUBAGENT_MODEL`, are kept.

Queries and controls (`--version`, `agents`, `stop`, `rm`, `logs`) run from
`/`, a directory that always exists and holds no project settings; launches
run in the agent's worktree.

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

Each listed session also carries its id, its process id while it runs, and
when it started. Hooks refine these states for the agents volee launched, for
example with the tool that waits for permission.

## Hooks

The settings file of an agent adds one hook per event to the user's own
hooks; the user's settings file is never edited. Each hook runs
`volee hook <event> --agent <id>` with a ten-second timeout (except
`SessionEnd`, which keeps Claude Code's short exit budget), and the `Stop`
event has a second handler that waits for messages while the agent is idle.
Claude Code gives that waiter 30 seconds more than its own wait, which is at
most a day, so it always ends on its own:

```json
"Stop": [{ "hooks": [
  { "type": "command", "command": "/usr/local/bin/volee hook stop --agent ag_01jc...", "timeout": 10 },
  { "type": "command", "command": "/usr/local/bin/volee hook wait --agent ag_01jc... --timeout 3600",
    "asyncRewake": true, "timeout": 3630 }
]}],
"permissions": { "allow": ["mcp__volee"] }
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

## Messages

Messages reach a working agent through the `Stop` hook, which prints
`{"decision":"block","reason":"<messages>"}`, and an idle agent through the
waiter, which exits with status 2 and the messages on standard error
([ADR 0006](../adr/0006-messaging.md)).

The text cannot impersonate anyone: only volee writes the `[n] question from
...` lines, every line of a message body is quoted with `> `, and sender
names and references are cut to one line without control characters. A
delivery holds at most 8,000 characters (Claude Code drops hook output
beyond 10,000); messages that do not fit are counted, and the agent is told
to read them with the MCP tool `read_messages`.

## Trust, deletion and session ids

What the adapter knows, and how it knows it:

- **Untrusted directories** (documented). `claude --bg` checks workspace
  trust before it starts. Without a terminal to ask in, it refuses with
  ``Workspace not trusted. Run `claude` in <dir> once and accept the trust
  prompt, then retry.`` (two variants name the home directory and a directory
  that cannot be found). volee reports it as `UntrustedDirectory` with the
  directory.
- **Trust and worktrees** (measured with 2.1.319 in a trusted sandbox
  repository, clean environment). Trust is recorded per path, and a
  repository nested in a trusted folder does not inherit it. A worktree made
  with `git worktree add` is untrusted wherever it lives, even under a
  trusted repository's `.claude/worktrees/`. From the trusted main checkout,
  `claude --bg --worktree=<name>` works, and when `.claude/worktrees/<name>`
  already exists Claude Code reuses it on its current branch and locks it.
  That is how volee launches every agent (`SpawnSpec::worktree`, [ADR
  0013](../adr/0013-worktrees-live-in-their-repository.md)).
- **Resuming** (measured). From the trusted main checkout, `claude --bg
  --worktree=<name> --resume=<session id>` continues a stopped session's
  conversation in its worktree, as a fork with a new id. volee resumes agents
  this way to deliver messages (ADR 0006). Once two sessions record the same
  worktree, `claude rm` refuses to delete it and offers no way to force it.
  The resumed session starts its MCP servers again, but the conversation it
  continues may say that the volee server disconnected (it stopped with the
  old session). A resumed model took that for the current state and wrote
  that it could not answer, with its tools connected, so volee's resume
  prompt starts by saying that the tools work in the new session.
- **Unknown sessions** (measured). `claude stop` and `claude logs` answer
  `No job matching '<id>'. Run 'claude agents' to list running sessions.`,
  `claude rm` answers `No job matching '<id>'`, all with exit status 1.
  volee reports `UnknownSession`.
- **Deleting a session** (measured for the cases below). `claude rm` deletes
  a worktree Claude Code created for the session. For a worktree that existed
  before the session (every agent volee launches), it refused while the
  branch had commits it could not find on a remote: it kept the session and
  the worktree, still locked, said `kept <id>` (volee reports `SessionKept`)
  and offered `--discard-unpushed <token>`, which volee never passes.
  volee still refuses to delete a session whose directory is in a linked git
  worktree unless the daemon passes `WorktreeRemoval::Delivered`, which it
  does only after `tidy_verdict` found delivery evidence
  ([ADR 0011](../adr/0011-safety-and-delivery-evidence.md)).
- **Choosing the session id** (not measured). `--session-id <uuid>` is
  documented as a general option, but its effect with `--bg` is neither
  documented nor measured, so the adapter does not use it: the daemon learns
  a new session's full id from its first hook or the next listing.

## Tolerance

Claude Code evolves quickly, so the adapter reads its output defensively:

- Unknown fields are ignored, every event-specific field is optional, and a
  field with an unexpected type is treated as missing.
- A listing element that cannot be read is skipped and marks the listing as
  incomplete, so it can never make an agent look gone.
- Terminal escape sequences and extra lines around the `backgrounded` line
  are ignored.
