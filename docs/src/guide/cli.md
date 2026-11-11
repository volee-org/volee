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

The interface writes no log unless you ask: it owns your terminal, so it
has nowhere to print. With `VOLEE_LOG` set, for example
`VOLEE_LOG=volee_tui=debug volee`, it writes to
`~/.local/state/volee/logs/interface.log`, a file only you can read. In
edit mode that log says what became of each key (sent to the agent, kept
back, dropped before the host answered), never the key itself.

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
volee attach <agent>        # open it in this terminal, where it runs
volee adopt <agent>         # move a session volee did not start into a volee host
volee stop <agent>          # stop it; volee does not resume it until you message it
volee forget <agent>        # stop listing an agent that no longer runs
```

`volee logs` prints the agent's screen as it looks now, as plain text. For
an agent in a volee host, that is the exact screen the host holds; for a
session in a tuios or tmux pane, the pane's screen, row for row. For a
Claude Code background session, volee replays what `claude logs` recorded
and prints the screen that leaves, every row of it, blank rows included; in
a session that has run for hours it can miss what was drawn long ago and
never redrawn.
`volee diff` compares the agent's worktree, committed or not, with the merge
base of its base branch, and lists the files it created; a very long diff is
cut, and the rest is in the worktree.

`volee attach` hands this terminal to the agent, where it runs. For an
agent in a volee host, `Ctrl+Q` comes back; the agent keeps running after
you leave. `Ctrl+Q` never reaches the agent, whatever keyboard protocol the
agent turned on. While you type, your terminal's size is the agent's size,
and it stays when you leave; nothing else resizes an agent. Taking the same
agent over from a second terminal moves the input there, and the first
terminal returns to its shell.

For a Claude Code background session volee did not start, `volee attach`
runs Claude Code's own `claude attach` on it: it is the same session, with
nothing stopped or restarted, and `Ctrl+Z` comes back to your shell, as
Claude Code says. In the interface, `enter` opens it the same way inside
volee ([opening a session](interface.md#opening-a-session-volee-did-not-start)).

Where an agent runs decides what these commands can do:

| The agent runs | `volee logs` | `volee attach` | `volee stop` |
| --- | --- | --- | --- |
| In a volee host: launched or adopted by volee | the exact screen | takes it over | ends it, like a closed terminal |
| As a Claude Code background session: launched by volee 0.7, or by hand with `claude --bg` | a replay of `claude logs` | opens it with `claude attach` | stops it if volee launched it |
| Interactive, in a tuios or tmux pane | the pane's screen | brings the pane forward ([panes](interface.md#agents-in-tuios-panes)); in the interface, `enter` opens a tuios pane inside volee | refused |
| Interactive, in another terminal (Claude Code or OpenCode) | refused: only that terminal shows it | refused, with what makes it reachable | refused |

The list is what runs now ([ADR 0027](../adr/0027-open-without-owning.md)).
A session volee did not start is one row for as long as its harness lists
it: a finished background session stays, like in Claude Code's own agents
view, and a session that is gone leaves the list a minute later. A session
started outside volee, in a terminal, shows once it has lived ten seconds,
so that a program's one-off call never becomes a row.

volee never stops or restarts a session it did not start: you stop it where
it was started. `volee adopt` is for a session you want volee to drive from
now on: volee stops a background session first (`claude stop`), so that two
programs never write the same conversation, then resumes its conversation
in a new host, in the same directory, with volee's settings in place of the
session's own, where it waits at its prompt. It also adopts
an agent whose session ended. An agent that runs in a terminal of its own
must be quit there first: volee refuses it until then. An OpenCode agent can
be adopted when it was started with `--session`, once you quit it. The agent
keeps its name and history in volee. Once adopted, it is handled like the
agents volee launches: messages reach it (they wake it when it waits at its
prompt), `volee stop` stops it, and a message resumes it in a new host.
The conversation resumes in the agent's worktree; in a repository Claude
Code does not trust yet, it asks there first (`volee attach` to answer).
When the resumed program fails, the agent's detail quotes its last line.

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

A launch on a branch where an agent already works is refused, and names
that agent: what there is to add goes to it, with `volee msg send`.

## A lead

```sh
volee lead                                   # here, and talk to it
volee lead "Add invoices to billing and to the API."
volee lead --name tower --model sonnet
```

A lead is the agent you talk to. It launches agents and missions for you,
hears their reports, and never codes
([ADR 0033](../adr/0033-a-lead-makes-others-work.md)). `volee lead` starts
one in the current directory, in a volee host, and opens it in your
terminal at once; `Ctrl+Q` gives the terminal back and the lead keeps
running. Without a terminal, it says how to open it. `enter` on its row in
the interface, or `volee attach <name>`, opens it again. It is named
`lead-<directory>` unless `--name` says otherwise, and uses the configured
model unless `--model` does.

A lead has no worktree and Claude Code's editing tools are turned off for
it. It gets two more MCP tools than the other agents: `spawn_agent`, for one
task in one repository, and `start_mission`, for work across several. What
it launches reports to it; it tells you in a line what it launched, and in
three lines what came back. You can also write to it without opening it:
`volee msg send <lead> "..."` from a shell, or `m` on its row in the
interface. It takes that as what you say, the same as what you type in its
terminal. `volee lead` typed again where that lead runs is refused, and
says how to open it; give another lead a name of its own with `--name`.

`volee lead`, `volee spawn` and `volee mission new` refuse to run in the
shell of an agent volee launched, like the commands that speak for you: an
agent launches with its own tools, so that what it starts reports to it.

## Running any program as an agent

```sh
volee run -- codex                      # any command line, after --
volee run --name invoices -- gemini     # with a name of your choice
```

`volee run [--name <name>] -- <command>` starts the command in a volee
host, in the current directory and with this terminal's environment, makes
it an agent of the list, and puts this terminal on it: you use the program
as if you had typed the command alone. `ctrl-q` (the leave key) gives the
terminal back, and the program keeps running under volee: `volee ls` and
the interface list it, `volee attach <name>` and `enter` open it,
`volee logs <name>` prints its screen and `volee stop <name>` ends it. The
agent is named after the directory unless `--name` says otherwise. Without
a terminal on standard input and output, the command starts the program
and prints how to open it. A command that cannot start (a program that is
not found) is no agent, and the reason is printed.

Such an agent is a program volee holds the terminal of, and nothing more:
its state is read on its screen (working while the screen changes, idle
after a few seconds without a change), nothing hands it a message, and
nothing resumes it once it ended, whether by itself, by `volee stop` or
with a reboot. Its row then shows the end for a minute and leaves the
list. See [Agents of other harnesses](other-harnesses.md#any-program-volee-run)
for when to use it, and [ADR 0029](../adr/0029-volee-run.md) for the
decision.

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

### Questions

An agent that needs a decision asks you. Its question waits, on top of your
[messages](interface.md#messages) in the interface, until you answer it:

```sh
volee msg questions
```

```text
msg_x7k2pq  coordinator-invoicing asks, 2 min ago:
  Both pull requests are green and reviewed. Merge them?
  Answers: 1 ok merge · 2 no
  Answer with `volee msg answer msg_x7k2pq <number>`.
msg_8fq0zd  billing asks, 12 min ago:
  Are amounts stored with or without tax?
  Answer with `volee msg answer msg_8fq0zd "<your answer>"`.
```

```sh
volee msg answer msg_x7k2pq 1              # the first answer it offers
volee msg answer msg_x7k2pq "ok merge"     # the same, by its words
volee msg answer billing "Without tax."    # the agent's name, when it asked one question
```

A question may offer the answers to choose from. Name one by its number or
by its words, whatever their case: the agent then reads exactly the answer it
offered. Anything else is sent as your own words, and volee says that it is
not one of the offered answers: an agent that asked for a yes or a no must
not take a sentence for a yes. The answer is linked to the question, which
stops waiting.

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
volee mission close invoicing         # yourself, once every worker delivered
```

A repository is a name from the [configuration](configuration.md) or a
directory. A mission is named by its title, its short id (`ms_x7k2pq`) or its
id. While a mission is open, another one under the same title is refused,
whatever its case: what there is to add goes to its coordinator.

Creating a mission starts its coordinator: a Claude Code session in the main
checkout of the mission's first repository, with Claude Code's editing tools
turned off. It plans the work and launches one worker per task with the MCP
tool `spawn_worker`, each in its own worktree. Without Claude Code (missing,
or too old), `volee mission new` refuses before recording anything, and
`volee daemon status` says why.

volee checks each worker when it stops. A change worker has delivered when
its commits are pushed with an open pull request, or merged; in a repository
without a remote, when its work is committed on a clean worktree. An
analysis worker has delivered when it sent its report. The coordinator hears
about every verdict.

Once every worker delivered, the coordinator presents its report (what is
done, the proof, what is left) as a [question](#questions) to you, with the
answers `ok merge` and `no` (`ok close` and `no` for an analysis mission).
Nothing is merged or closed before you answer, and `volee mission ls` shows
the mission as `waiting for you`:

```sh
volee msg questions
volee msg answer coordinator-invoicing 1     # ok merge
```

Only that answer merges the work, and volee makes the merges itself, one
worker after the other: no agent merges
([ADR 0032](../adr/0032-a-mission-is-merged-only-after-the-person-approved.md)).

- With a remote, it merges the worker's pull request on GitHub with your
  `gh`: a draft is marked ready first, and the method is the repository's
  [`merge` setting](configuration.md#reposname), or the first one the
  repository allows among a merge commit, a squash and a rebase. GitHub's
  rules still apply: volee does not bypass a required review or a failing
  check, and does not wait for them.
- Without a remote, it merges the worker's branch into its base branch in
  the main checkout, as a merge commit. The main checkout must have the base
  branch checked out, and no uncommitted change to a tracked file: volee
  does not switch branches or stash for you.

It then closes the mission. When a merge cannot go through, nothing is
forced and a conflict is abandoned: what was already merged stays merged,
the mission is open again, and a message tells you, the coordinator and the
lead which work was not merged and why. Once that is settled, the
coordinator presents its report again and you answer again. On any answer
other than `ok merge` the mission stays open, and the coordinator is told
what you said.

A closed mission is tidied in one gesture: volee removes the worktree of
every worker whose work landed, keeps its branch, forgets the agent, stops
the coordinator, and tells you in your messages what is merged, what it
tidied and what it kept, with the reason (uncommitted changes, work that did
not land). It never forces and never deletes a branch.

`volee mission close` is you closing a mission yourself, without the
coordinator and without a merge: it refuses a mission whose workers have
not all delivered and says which; `--force` closes it anyway. It tidies the
same way, so a worker whose work you did not merge is kept. Aborting
tells every agent of the mission to stop and deletes nothing: branches,
worktrees and pull requests stay where they are.

The commands that speak for you, `volee msg send`, `volee msg answer` and
`volee mission close`, refuse to run in the shell of an agent volee
launched: an agent asks you, it does not answer for you.

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
agent in `volee ls`; to have a program in the list,
[`volee run`](#running-any-program-as-an-agent) starts it in a host and
makes it an agent. `volee host ls` shows each host as `running`, `exited`
with its code, `killed` with its signal, or `interrupted` when its host is
gone without recording an end.

Every command takes `--dir` to use another directory of hosts than
`<state>/hosts` (`~/.local/state/volee/hosts` by default). Hosts keep there
a socket, a record and a log each, readable only by you.
`volee host wall --ids <id>,<id>` shows only these hosts. The interface
has a wall of its own, on every agent of its list that runs, in a volee
host or not ([the wall](interface.md#the-wall-of-screens)).

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
