# Claude Code adapter

The `volee-harness` crate is how volee talks to Claude Code. It relies only
on documented commands and hooks, never on Claude Code's internal files
([ADR 0004](../adr/0004-sources-and-reconciliation.md)). Everything on this
page was measured on Claude Code 2.1.319 or taken from its documentation, as
marked; volee needs 2.1.319 or newer, the version it was measured on, and
says so at startup when the installed version is older.

The daemon uses the adapter through the `Harness` trait; the same crate
holds the OpenCode profile ([ADR 0022](../adr/0022-harness-profiles.md),
[Agents of other harnesses](../guide/other-harnesses.md#opencode)). Every
command runs through a `CommandRunner` with a timeout, which lets the tests
replay recorded output instead of running `claude`.

## Commands

Every agent volee launches or adopts runs `claude` interactive inside a
volee host ([ADR 0019](../adr/0019-a-host-process-holds-each-agent-terminal.md),
[Hosts](hosts.md)), started through `volee host spawn`:

| Need | Command | Notes |
| --- | --- | --- |
| Version | `claude --version` | Checked when the daemon starts. |
| Every session | `claude agents --json --all` | Polled and reconciled by session id. |
| Launch | `claude --session-id=<uuid> --name=<name> --settings=<file> --worktree=<name> --opt=value... -- <prompt>`, in a new host | Runs from the repository's main checkout. volee chooses the session id, so it knows the session before its first hook. |
| Resume | `claude --resume=<session-uuid> --worktree=<name> --opt=value... -- <prompt>`, in a new host | Continues a stopped session: for a message, after a reboot, or to adopt a session (then without a prompt, so it waits at its prompt). Linked to the same agent. |
| Preview | the host's screen | Exact, status line and prompt frame included; read without resizing the agent. |
| Take over | `volee host attach <host>` | Runs in the client's terminal; `Ctrl+Q` returns. |
| Stop | through the host: `SIGHUP` to the program's process group, `SIGKILL` five seconds later | Like a closed terminal; the conversation is kept. |

Background sessions (`claude --bg`) are the ones volee 0.7 launched, and any
you start by hand. volee shows them and does not drive them:

| Need | Command | Notes |
| --- | --- | --- |
| Preview | `claude logs <id>` | A raw replay of the session's terminal, see below. |
| Stop | `claude stop <id>` | Before an adoption, and for the sessions volee 0.7 launched. The conversation is kept. |
| Delete | `claude rm <id>` | Never with `--discard-unpushed` or `--force-remove-worktree`. |

A background session volee did not start is opened with `claude attach
<id>`, in the person's terminal (`volee attach`) or in a view inside the
interface ([ADR 0027](../adr/0027-open-without-owning.md)): the same
session in the same process, running or finished. The view runs in the
session's directory, so that Claude Code's own agents view, one key away in
`claude attach`, opens in a directory the person already trusts it with.
Adopting a session ([ADR 0023](../adr/0023-every-agent-on-the-machine.md))
is something else: it stops it with `claude stop` and resumes its
conversation in a host. A conversation whose worktree still exists resumes in
that worktree rather than with `--worktree` from the main checkout: a
repository Claude Code trusts covers its worktrees, and in one it does not
trust yet Claude Code asks in the host instead of exiting.

`<id>` is the id of a background session, kept exactly as `claude agents`
lists it (the session's *handle*). Interactive sessions started by hand have
no handle: volee shows them but cannot stop or delete them, and previews
and reaches only the ones that run in a tuios or tmux pane, through the
multiplexer ([ADR 0015](../adr/0015-tuios-location-adapter.md)).

`claude logs` replays the session's recent drawing, not a log of its text:
rows placed by cursor moves from the top left of the session's own terminal,
which can be far wider than 200 columns, some of them (the lines of a diff)
padded with colored blanks to its right edge, and nothing for what was drawn
before the replay starts, such as the frame of the prompt. The interface
replays it in a terminal emulator at least as wide as the widest row it
writes, so that no row wraps onto the next, and keeps a run of blank rows as
one, so that the end of the conversation stays in view
(`crates/volee-tui/src/preview.rs`; measured on 2.1.319 with a session 308
columns wide). It is one of the reasons volee no longer launches background
sessions ([ADR 0019](../adr/0019-a-host-process-holds-each-agent-terminal.md)):
a host holds the whole screen from the start.

Every launch passes the agent's settings file (`--settings`: volee's hooks,
a rule that allows volee's own MCP tools, and a status line that tells volee
how much of its context the agent uses, then shows the person's own,
[ADR 0035](../adr/0035-the-context-an-agent-uses-comes-from-its-status-line.md))
and its MCP configuration
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
example with the tool that waits for permission. The agents volee runs in
hosts are interactive sessions, so their hooks carry most of their state.

## Hooks

The settings file of an agent adds one hook per event to the user's own
hooks; the user's settings file is never edited. Each hook runs
`volee hook <event> --agent <id>` with a ten-second timeout (except
`SessionEnd`, which keeps Claude Code's short exit budget), and the `Stop`
and `SessionStart` events have a second handler that waits for messages
while the agent is idle: after a turn, and from the start of a session that
waits at its prompt without one (an adopted agent). The daemon keeps one
waiter per agent, the newest. Claude Code gives that waiter 30 seconds more
than its own wait, which is at most a day, so it always ends on its own:

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
  directory. In a host, `claude` runs interactive, with a terminal to ask
  in; the guides still ask you to run `claude` once in each repository
  before volee launches agents there.
- **Trust and worktrees** (measured with 2.1.319 in a trusted sandbox
  repository, clean environment). Trust is recorded per path, and a
  repository nested in a trusted folder does not inherit it. A worktree made
  with `git worktree add` is untrusted wherever it lives, even under a
  trusted repository's `.claude/worktrees/`. From the trusted main checkout,
  `claude --bg --worktree=<name>` works, and when `.claude/worktrees/<name>`
  already exists Claude Code reuses it on its current branch and locks it.
  That is how volee launches every agent, now without `--bg`, in a host
  (`SpawnSpec::worktree`, [ADR
  0013](../adr/0013-worktrees-live-in-their-repository.md)).
- **Resuming** (measured). From the trusted main checkout, `claude --bg
  --worktree=<name> --resume=<session id>` continues a stopped session's
  conversation in its worktree, as a fork with a new id. volee 0.7 resumed
  agents this way to deliver messages (ADR 0006); volee now runs the same
  command without `--bg`, in a new host. Once two sessions record the same
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
- **Choosing the session id** (documented). `--session-id <uuid>` is
  documented as a general option. volee passes a new random UUID with every
  launch in a host, so the daemon knows the session before its first hook.
  Its effect with `--bg` is neither documented nor measured, so the
  background launches of volee 0.7 did not use it: the daemon learned a new
  session's full id from its first hook or the next listing.

## Tolerance

Claude Code evolves quickly, so the adapter reads its output defensively:

- Unknown fields are ignored, every event-specific field is optional, and a
  field with an unexpected type is treated as missing.
- A listing element that cannot be read is skipped and marks the listing as
  incomplete, so it can never make an agent look gone.
- Terminal escape sequences and extra lines around the `backgrounded` line
  are ignored.
