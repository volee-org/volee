# Command line

Everything in volee goes through one daemon per user. The commands below
start it when they need it, so most of the time you never think about it.

## The daemon

| Command | What it does |
| --- | --- |
| `volee daemon start` | Starts the daemon in the background, unless it already runs. |
| `volee daemon status` | Shows whether it runs, its version, uptime and counters. Never starts it. |
| `volee daemon stop` | Asks the running daemon to stop. Agents keep running: each one lives in its own volee host. |
| `volee daemon run` | Runs the daemon in the foreground, logging to the terminal. Useful to debug. |
| `volee daemon install` | Starts the daemon at login from now on, so that agents a reboot interrupted come back without opening volee. `--print` shows the file and where it would go, and installs nothing. |
| `volee daemon uninstall` | Stops starting the daemon at login. |

The daemon logs to `~/.local/state/volee/logs/daemon.log`. Set `VOLEE_LOG`
to change the level, for example `VOLEE_LOG=debug volee daemon run`.

A command that starts the daemon may run inside an agent's session or a
multiplexer pane. The daemon starts in its state directory and drops the
variables that identify that session or pane (such as `CLAUDE_CODE_SESSION_ID`
or `TMUX_PANE`), so the agents it launches never inherit them. Your settings,
such as `ANTHROPIC_API_KEY` or `CLAUDE_CODE_USE_BEDROCK`, are kept.

### Starting at login

```sh
volee daemon install           # start the daemon at login
volee daemon install --print   # show the file, install nothing
volee daemon uninstall         # undo it
```

Without it, the daemon starts the first time you use volee after a reboot,
and that is when it resumes the agents the reboot interrupted
([after a reboot](#after-a-reboot)). `volee daemon install` makes that
happen at login instead: on macOS it writes a launchd agent,
`~/Library/LaunchAgents/org.volee.daemon.plist`, and on Linux a systemd user
unit, `~/.config/systemd/user/volee.service`; both run `volee daemon run`,
and the command loads them at once. The file records the `PATH` of the shell
you run it from, so that the daemon finds `claude`, `git` and `gh`, and the
`VOLEE_*` variables you set: run it again after you change them. volee never
installs it on its own.

### What the daemon does in the background

While it runs, the daemon keeps what volee shows current by itself. None of
it waits for you, and none of it changes your work:

| What | How often | What it runs |
| --- | --- | --- |
| Find sessions | While the [interface](interface.md) is open, every two seconds while sessions change, slowing down to every ten seconds while nothing does; every thirty seconds when no interface is open; at once when the interface opens or `volee ls` asks | `claude agents --json --all` |
| Read each agent's work | When it finishes a turn or stops, and every minute while it runs | git queries in its directory, which never change the repository |
| Fetch | At most every five minutes per repository, for the repositories of agents volee launched | `git fetch --prune origin`; the [`[git] fetch`](configuration.md#git) setting chooses which repositories, or none |
| Look up pull requests | At most once a minute per branch, and right after its agent stops | `gh pr list`, with your own `gh` sign-in, when the GitHub CLI is installed and signed in |
| Find sessions in tuios panes | After each look for sessions, while Claude Code sessions run by hand | `tuios call list-agents`, then `ps` and `tuios call resolve-pane` for a session tuios's hooks did not report; only when `tuios` was on the `PATH` when the daemon started ([ADR 0015](../adr/0015-tuios-location-adapter.md)) |
| Find sessions in tmux panes | After each look for sessions, while Claude Code sessions run by hand | `tmux list-panes -a`, then `ps` for a session not found before; only when `tmux` was on the `PATH` when the daemon started |
| Read the screens of hosted agents without hooks | After each look for sessions | a snapshot of each OpenCode agent's host: a screen that changed is working, one unchanged for about six seconds is idle, a program that ended is stopped |
| Find OpenCode agents started elsewhere | After each look for sessions | `ps`, then `lsof` for the directories of the `opencode` processes it finds (not the background service, `opencode serve --service`) |
| Resume agents a reboot interrupted | Once, when the daemon starts | each agent's resume command, in a new host ([after a reboot](#after-a-reboot)) |

Pull request look-ups use your GitHub account's API allowance, a few
requests a minute with several agents running. Without `gh`, or signed out,
volee skips them and `volee daemon status` says why. Without a tuios or tmux
server, the search for panes changes nothing, and `volee daemon status` says
so on its `panes` lines.

## Agents

```sh
volee ls          # every agent, the ones that need you first
volee ls --json   # the same, as JSON for scripts
```

The list includes every agent volee launched, each in its volee host, and
every live Claude Code session and OpenCode agent on the machine, in a
terminal or in the background, whoever started it. Claude Code agents volee
launched report through their hooks the moment something happens. The
daemon finds the other sessions by listing them (and OpenCode in the process
table), and each listing costs a little CPU, so it lists as often as things
change. While the [interface](interface.md) is
open, it lists every two seconds after a listing that found something new,
and waits twice as long after each listing that found nothing, up to ten
seconds. With no interface open, it lists every thirty seconds, so an idle
daemon costs next to nothing. Opening the interface lists them at once, and
so does `volee ls`, which waits for that listing (three seconds at most)
before it prints. Finished sessions you never saw through volee are left
out; `volee daemon status` tells you whether Claude Code was found.

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
volee attach <agent>        # take it over in this terminal; Ctrl+Q comes back
volee adopt <agent>         # move an agent volee does not host into a volee host
volee stop <agent>          # stop it; volee does not resume it until you message it
volee forget <agent>        # stop listing an agent that no longer runs
```

`volee logs` prints the agent's screen as it looks now, as plain text. For
an agent in a volee host, that is the exact screen the host holds. For a
Claude Code background session, volee replays what `claude logs` recorded
and keeps the last frame, which can miss what was drawn long ago and never
redrawn.
`volee diff` compares the agent's worktree, committed or not, with the merge
base of its base branch, and lists the files it created; a very long diff is
cut, and the rest is in the worktree.

`volee attach` hands this terminal to the agent until you press `Ctrl+Q`;
the agent keeps running after you leave. `Ctrl+Q` never reaches the agent,
whatever keyboard protocol the agent turned on. While you type, your
terminal's size is the agent's size, and it stays when you leave; nothing
else resizes an agent. Taking the same agent over from a second terminal
moves the input there, and the first terminal returns to its shell.

Where an agent runs decides what these commands can do:

| The agent runs | `volee logs` | `volee attach` | `volee stop` |
| --- | --- | --- | --- |
| In a volee host: launched or adopted by volee | the exact screen | takes it over | ends it, like a closed terminal |
| As a Claude Code background session: launched by volee 0.7, or by hand with `claude --bg` | a replay of `claude logs` | refused: adopt it first | stops it if volee launched it |
| Interactive, in a tuios or tmux pane | the pane's screen | brings the pane forward ([panes](interface.md#agents-in-tuios-panes)) | refused |
| Interactive, in another terminal (Claude Code or OpenCode) | refused: only that terminal shows it | refused | refused |

volee only shows the agents it did not start: it never types into them,
resizes or stops them, and you stop them where they were started.
`volee adopt` moves one into a volee host, so that you can take it over:
volee stops a background session first (`claude stop`), so that two programs
never write the same conversation, then resumes its conversation in a new
host, in the same directory, where it waits at its prompt. It also adopts
an agent whose session ended. An agent that runs in a terminal of its own
must be quit there first: volee refuses it until then. An OpenCode agent can
be adopted when it was started with `--session`, once you quit it. The agent
keeps its name and history in volee.

A stopped agent keeps its conversation, and volee does not resume it on its
own, even when other agents write to it, until you send it a message
yourself. `volee stop` on an agent that is not running only records that.

`volee forget` removes an agent from volee's lists, for example one whose
launch failed or a session you no longer care about; volee then ignores its
sessions. Nothing else changes: its worktree and branch stay, so remove them
with git when you no longer need them. volee refuses to forget an agent it
launched that still runs (stop it first) and an agent of an open mission.

## Launching agents

```sh
volee spawn billing feat/invoices "Add invoices to the billing module."
volee spawn . fix/flaky-test "Make the payment test stable." --base develop --model sonnet
volee spawn billing feat/export "Export invoices as CSV." --harness opencode
```

`volee spawn <repository> <branch> <task>` creates the branch from the base
(`--base`, else the repository's `base` in the configuration, else its
default branch), checks it out in a new worktree at
`.claude/worktrees/<name>` inside the repository (the branch with `/` and
other unusual characters replaced by `-`), and starts Claude Code there, in
a volee host, with the task as its first prompt. `--name` sets its display
name, `<repository>-<name>` otherwise. The agent gets volee's hooks and MCP
tools, so it appears at once in `volee ls` and can send you messages.

`--harness opencode` starts OpenCode instead, in the same kind of worktree
and host (`opencode --session <id> --prompt <task>`, with `--auto` when
`permission_mode = "auto"`). `--model` does not apply to it. OpenCode takes
no hooks, so volee reads its state from its screen, and messages wait until
it stops, then resume it; see
[Agents of other harnesses](other-harnesses.md#opencode).

When the repository has `setup` commands in the configuration, such as
`bun install`, they run in the new worktree before the session starts, and
`volee spawn` says so before it waits; the agent is listed as launching,
with the command that runs. A failing command fails the launch and keeps the
worktree, like the cases below
([setup commands](configuration.md#setup-commands)).

Claude Code works only in repositories you trust: run `claude` once in each
repository and accept its trust prompt before volee launches agents there.
When a launch fails before its session started, the worktree and the branch
it had already made stay, untouched: once the cause is fixed, run the same
`volee spawn` again, and volee reuses them, as long as nothing was done in
them ([ADR 0016](../adr/0016-what-a-failed-launch-left-untouched.md)). That
includes a launch the daemon could not finish because it stopped, and one
whose session Claude Code never listed within two minutes: volee marks those
failed at its next look for sessions.

## After a reboot

A reboot, a logout or a crash of a host ends the agent's program, but its
conversation and its worktree stay. The first time the daemon starts
afterwards, it looks for agents whose host is gone without recording an end,
and resumes each one in a new host, in the same directory and worktree. An
agent that was working gets a first prompt that says the machine restarted,
so that it checks where it was; an agent that waited at its prompt comes
back waiting, without one. A resumed agent has lost what it was doing in the
middle of a turn, such as a command that was running: the harness resumes
from the last saved message.

Some agents are not resumed: the ones you stopped, the ones of a mission
that is over, the ones that were done or had failed, and an agent whose host
came from such a resume and is gone again less than ten minutes after it
started, which volee takes for a crash rather than a reboot, so that it
never restarts an agent in a loop: it marks it failed, with the reason. Set
[`resume_after_reboot = false`](configuration.md#agents) to resume none of
them: they show as stopped, and your next message resumes one.

The daemon starts when you first use volee, or at login once you ran
[`volee daemon install`](#starting-at-login).

## Tidying worktrees

```sh
volee tidy <agent>             # remove its worktree if its work landed
volee tidy --all               # the same for every agent volee launched
volee tidy --all --dry-run     # say what would be removed, touch nothing
```

`volee tidy` removes the worktree of an agent only when the evidence says
its work landed: its branch is merged into its base branch, its pull request
is merged, or, for analysis work, its report was delivered. It also removes
the worktree of a launch that failed before any session started, when
nothing was ever done in it: no change, no untracked file, no commit of its
own. The worktree
must also be clean, the agent must not be running, and volee must have
checked the agent's work since its last change. It only touches agents volee
launched, in the worktrees it created under `.claude/worktrees/`. Files git
ignores (a copied `.env`, local data) are not work git can see, so they go
with the worktree: copy out anything you want to keep first. It never
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

A message reaches a working Claude Code agent at the end of its turn, and
wakes an idle one at once. When an agent volee launched has stopped (its
program ended, or Claude Code stopped an idle background session after about
an hour), volee resumes it in a new volee host, in its worktree, with the
message as its prompt: Claude Code with `--resume`, OpenCode with
`--session`. If that fails, it tells you once. An OpenCode agent has no hooks
to hand it a message while it runs: the message waits until it stops.
Two cases are never resumed on their own:

- an agent you stopped with `volee stop`: messages from agents and from volee
  wait, and your next message to it resumes it with all of them;
- an agent whose mission is over (closed or aborted): its messages wait until
  it runs again, which only you can decide, with `claude --resume` in its
  worktree.

Claude Code sessions you did not launch through volee cannot receive
messages yet. Agents of other harnesses that [joined](other-harnesses.md)
through volee's MCP server read their messages themselves, when they call
`read_messages`; a message sent while one is stopped waits for its next
start. `volee msg send` says, after sending, which of these cases applies.

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
tool `spawn_worker`, each in its own worktree. Without Claude Code (missing,
or too old), `volee mission new` refuses before recording anything, and
`volee daemon status` says why.

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
other and to you every few seconds, along the routes the daemon allows.
Both take their theme, keys and `reduced_motion`
setting from the `[tui]` table of the configuration. The
[interface guide](interface.md) describes the screen, what needs the daemon,
and every key.

## Hosts

Every agent volee launches runs in a volee host, a small process of its own
that holds the agent's terminal and its exact screen
([how a host works](../architecture/hosts.md)). The `volee host` commands
work on hosts directly, without the daemon, and run any command line in one:

```sh
volee host spawn -- htop                     # start a program in a new host; prints its id
volee host spawn --cwd ~/code/api --name api-tests -- cargo test
volee host ls                                # hosts, running or ended; --json for scripts
volee host peek <id>                         # its screen, as plain text
volee host attach <id>                       # take it over; Ctrl+Q comes back
volee host stop <id>                         # end its program
volee host wall                              # every running host as a wall of live screens
```

`volee host spawn` starts the host detached from your terminal, waits until
it answers, and prints its id; closing the terminal does not end it.
`--cwd` sets the program's directory (this one otherwise), `--cols` and
`--rows` its size until someone takes it over (this terminal's otherwise),
`--name` the name its clients show, and `--linger` how many seconds the last
screen stays after the program ends (600). A host started this way has no
harness profile: volee does not resume it after a reboot, and it is not an
agent in `volee ls`. `volee host ls` shows each host as `running`, `exited`
with its code, `killed` with its signal, or `interrupted` when its host is
gone without recording an end.

Every command takes `--dir` to use another directory of hosts than
`<state>/hosts` (`~/.local/state/volee/hosts` by default). Hosts keep there
a socket, a record and a log each, readable only by you.
`volee host wall --ids <id>,<id>` shows only
these hosts; the interface opens the wall this way on the agents of its
list ([the wall](interface.md#the-wall-of-screens)).

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
