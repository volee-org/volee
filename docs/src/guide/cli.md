# Command line

Everything in volee goes through one daemon per user. The commands below
start it when they need it, so most of the time you never think about it.

## The daemon

| Command | What it does |
| --- | --- |
| `volee daemon start` | Starts the daemon in the background, unless it already runs. |
| `volee daemon status` | Shows whether it runs, its version, uptime and counters. Never starts it. |
| `volee daemon stop` | Asks the running daemon to stop. Agents keep running: they belong to their harness. |
| `volee daemon run` | Runs the daemon in the foreground, logging to the terminal. Useful to debug. |

The daemon logs to `~/.local/state/volee/logs/daemon.log`. Set `VOLEE_LOG`
to change the level, for example `VOLEE_LOG=debug volee daemon run`.

A command that starts the daemon may run inside an agent's session or a
multiplexer pane. The daemon starts in its state directory and drops the
variables that identify that session or pane (such as `CLAUDE_CODE_SESSION_ID`
or `TMUX_PANE`), so the agents it launches never inherit them. Your settings,
such as `ANTHROPIC_API_KEY` or `CLAUDE_CODE_USE_BEDROCK`, are kept.

## Agents

```sh
volee ls          # every agent, the ones that need you first
volee ls --json   # the same, as JSON for scripts
```

The list includes every live Claude Code session on the machine, in a
terminal or in the background, whoever started it. Agents volee launched
report through their hooks the moment something happens. The daemon finds
the other sessions by listing them: every two seconds while the
[interface](interface.md) is open, and every thirty seconds otherwise, so an
idle daemon costs next to nothing. Opening the interface lists them at once.
Finished sessions you never saw through volee are left out; `volee daemon
status` tells you whether Claude Code was found.

Each line shows a symbol and a word for the state, so the list reads without
color:

| Symbol | State |
| --- | --- |
| `!` | needs you (permission, question, dialog) |
| `▸` | working |
| `…` | launching |
| `✓` | done |
| `·` | idle |
| `✗` | failed |
| `■` | stopped |
| `∅` | lost |

Each agent also has a short id such as `ag_x7k2pq`. Wherever a command asks
for an agent, you can type its name, that short id, or its full id.

### Acting on an agent

```sh
volee logs <agent>          # its screen as it looks now
volee diff <agent>          # its changes against the branch it started from, by file
volee diff <agent> --full   # the same changes, as a full patch
volee attach <agent>        # take it over in this terminal; Ctrl+Z comes back
volee stop <agent>          # stop it; volee does not resume it until you message it
```

`volee logs` prints the agent's screen as it looks now, as plain text: the
harness records every frame its interface drew, and volee replays them to
keep only the last one.
`volee diff` compares the agent's worktree, committed or not, with the merge
base of its base branch, and lists the files it created; a very long diff is
cut, and the rest is in the worktree. `volee attach` hands this terminal to
the agent until you press `Ctrl+Z`; the agent keeps running after you leave.

These commands work on every background session, including the ones you
started yourself. A session running in a terminal of its own (an interactive
`claude`) is shown by `volee ls`, but only that terminal can show or control
it. `volee stop` only stops the agents volee launched: stop the others where
they were started. A stopped agent keeps its conversation, and volee does not
resume it on its own, even when other agents write to it, until you send it a
message yourself. `volee stop` on an agent that is not running only records
that.

## Launching agents

```sh
volee spawn billing feat/invoices "Add invoices to the billing module."
volee spawn . fix/flaky-test "Make the payment test stable." --base develop --model sonnet
```

`volee spawn <repository> <branch> <task>` creates the branch from the base
(`--base`, else the repository's `base` in the configuration, else its
default branch), checks it out in a new worktree at
`.claude/worktrees/<name>` inside the repository (the branch with `/` and
other unusual characters replaced by `-`), and starts a Claude Code
background session there with the task as its first prompt. `--name` sets
its display name, `<repository>-<name>` otherwise. The agent gets volee's
hooks and MCP tools, so it appears at once in `volee ls` and can send you
messages.

Claude Code only starts background sessions in repositories you trust: run
`claude` once in each repository and accept its trust prompt. volee says so
when it is missing, and starts nothing.

## Tidying worktrees

```sh
volee tidy <agent>             # remove its worktree if its work landed
volee tidy --all               # the same for every agent volee launched
volee tidy --all --dry-run     # say what would be removed, touch nothing
```

`volee tidy` removes the worktree of an agent only when the evidence says
its work landed: its branch is merged into its base branch, its pull request
is merged, or, for analysis work, its report was delivered. The worktree
must also be clean, the agent must not be running, and volee must have
checked the agent's work since its last change. It only touches agents volee
launched, in the worktrees it created under `.claude/worktrees/`. It never
deletes a branch and has no way to force a removal: when something is
missing, it says what, for example:

```text
Kept billing-feat-invoices: its work has not landed: not merged into its base branch, no merged pull request, no report.
```

Once the worktree is gone, the agent leaves `volee ls`; its history stays in
volee's event log, and its branch stays in the repository.

## Messages

```sh
volee msg send billing "Also update the changelog."
volee msg send ag_x7k2pq "Use TTC." --kind answer
```

A message reaches a working agent at the end of its turn, and wakes an idle
agent at once. When the session of an agent volee launched has ended (Claude
Code stops idle background sessions after about an hour), volee resumes it in
its worktree with the message as its prompt; if that fails, it tells you once.
Two cases are never resumed on their own:

- an agent you stopped with `volee stop`: messages from agents and from volee
  wait, and your next message to it resumes it with all of them;
- an agent whose mission is over (closed or aborted): its messages wait until
  it runs again, which only you can decide, with `claude --resume` in its
  worktree.

Agents you did not launch through volee cannot receive messages yet.
`volee msg send` says, after sending, which of these cases applies.

## Summaries

```sh
volee summary invoicing      # a mission's thread
volee summary billing        # your thread with an agent
```

A model reads the thread and answers in a few bullet points: what was
decided, what was delivered (with its links), the questions still waiting and
who must answer them, what is blocked. Summaries are off until you set
`summaries = true` under `[messages]` in the
[configuration](configuration.md#messages), because each one runs Claude Code
with your account.

## Missions

A mission is one goal carried out across one or more repositories by a
coordinator agent and its workers (see [Concepts](concepts.md)).

```sh
volee mission new "Invoicing" "Add invoices to billing and expose them in the API" \
  --repo billing --repo api           # or --repo billing,api
volee mission new "Slow queries" "Find the ten slowest queries" --repo . --kind analysis
volee mission ls                      # open missions; --all adds closed ones, --json for scripts
volee mission show invoicing          # agents, verdicts on workers, the mission's thread
volee mission abort invoicing --reason "wrong approach"
volee mission close invoicing         # once every worker delivered
```

A repository is a name from the [configuration](configuration.md) or a
directory. A mission is named by its title, its short id (`ms_x7k2pq`) or its
id.

Creating a mission starts its coordinator: a Claude Code session in the main
checkout of the mission's first repository, with Claude Code's editing tools
turned off. It plans the work and launches one worker per task with the MCP
tool `spawn_worker`, each in its own worktree.

volee checks each worker when it stops. A change worker has delivered when
its commits are pushed with an open pull request, or merged; an analysis
worker has delivered when it sent its report. The coordinator hears about
every verdict, and once every worker delivered it is asked for the final
report, which it sends before calling `close_mission`.

`volee mission close` refuses a mission whose workers have not all delivered
and says which; `--force` closes it anyway. Aborting tells every agent of the
mission to stop. Neither deletes anything: branches, worktrees and pull
requests stay where they are.

## The interface

```sh
volee                             # the interface, on every agent of this machine
volee demo                        # the interface, on a simulated team of agents
volee demo --theme terminal       # with another theme than the configured one
```

`volee` with no command starts the daemon when it does not run, then shows
the agents it knows until you quit. `volee demo` needs no daemon and touches
nothing: it shows a fictional lead, two missions and their workers, and two
sessions volee did not launch, whose states change and who write to each
other every few seconds. Both take their theme, keys and `reduced_motion`
setting from the `[tui]` table of the configuration. The
[interface guide](interface.md) describes the screen, what needs the daemon,
and every key.

## Isolated runs

Every path can be moved with an environment variable, which is handy for
trying volee without touching your real state:

| Variable | Default |
| --- | --- |
| `VOLEE_CONFIG_DIR` | `$XDG_CONFIG_HOME/volee`, else `~/.config/volee` |
| `VOLEE_STATE_DIR` | `$XDG_STATE_HOME/volee`, else `~/.local/state/volee` |
| `VOLEE_DATA_DIR` | `$XDG_DATA_HOME/volee`, else `~/.local/share/volee` |
| `VOLEE_SOCKET` | `$XDG_RUNTIME_DIR/volee.sock`, else `<state>/volee.sock` |

Put a custom `VOLEE_SOCKET` in a directory only you can enter, as the
defaults are: the daemon restricts the socket to you right after creating
it, and a directory that others can enter would let them connect in
between (the daemon logs a warning then). The daemon replaces a stale
socket left by a crash, but never another kind of file.
