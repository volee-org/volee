# Getting started

This walk-through takes about ten minutes. It assumes volee is installed (see
[Installation](installation.md)) and that you already use Claude Code.

## 1. Check that volee finds Claude Code

```sh
volee daemon start
volee daemon status
```

Other commands start the daemon on their own when they need it; here it is
started by hand, because `volee daemon status` only reports. The status
names the Claude Code version it found, and whether `gh` is there to read
pull requests. If Claude Code is missing or too old, the line says why. Right
after the start, the `forge` line may say `gh not checked yet`: volee checks
`gh` in the background, so ask again a few seconds later.

## 2. See every agent on the machine

```sh
volee
```

The interface opens on every Claude Code session and every OpenCode agent
running on this machine, whoever started it: in a terminal, in the
background, or launched by another agent. The ones that need you come first.
Press `?` for the keys; `q` quits without stopping anything. `volee ls`
prints the same list without the interface.

Nothing here depends on how you started the sessions: volee reads Claude
Code's own list of sessions, looks for OpenCode in the process table and
asks tuios which of its panes run another AI, such as Codex CLI or Gemini
CLI (at once when the interface opens, then every two to ten seconds while
it is open, more often while sessions change, and every thirty seconds
otherwise),
and it hears the hooks of the agents it launches itself, which report at
once. The list is what runs now: a session you close leaves it after a
minute, a background session stays for as long as Claude Code keeps it, and
a session started outside volee shows once it has lived ten seconds.

Press `enter` on any of them, or double click it: volee opens it where it
runs, in a terminal inside the interface, and you type there (step 5).

## 3. Tell volee about your repositories

```sh
volee config init
$EDITOR "$(volee config path)"
```

Add one table per repository you want volee to launch agents in:

```toml
[repos.billing]
path = "~/code/billing"
base = "main"
```

Then run `volee config check`. Claude Code works only in folders you
trust, and each repository counts on its own: if you have never run `claude`
in a repository, do it once and accept the trust prompt before you launch an
agent there.

## 4. Launch an agent

```sh
volee spawn billing feat/invoices "Add an invoice model with its migration and tests." --name invoices
```

volee creates the branch `feat/invoices` from `main`, checks it out in a new
worktree at `billing/.claude/worktrees/feat-invoices`, and starts Claude Code
there with your task, in a volee host: a small process that holds the
agent's terminal, so the agent keeps running when you close the interface,
the terminal or the daemon. The agent appears at once as `invoices`, with its
repository and branch (without `--name`, it would be called
`billing-feat-invoices`). Your own checkout is never touched. Add
`--harness opencode` to start OpenCode instead.

The agent runs in Claude Code's `manual` permission mode unless you choose
another in the configuration: when it needs a permission, it shows up as
needing you, and you answer by taking it over.

## 5. Talk to it, or take it over

```sh
volee msg send invoices "Use cents, not floats, for amounts."
volee attach invoices       # Ctrl+Q comes back
volee diff invoices         # what it changed so far
volee stop invoices         # stop it; its conversation is kept
```

A message reaches the agent at the end of its turn, wakes it if it is idle,
and resumes it in a new host if it stopped. `volee attach` hands your
terminal to the agent, from any terminal; `Ctrl+Q` returns to where you were,
and the agent keeps running. In the interface, `enter` opens the agent
inside volee and sends it your keys until `Ctrl+Q`, `z` shows its screen
full size, and `5` shows the screens of the agents that run, side by side.
After `volee stop`, volee does not resume the agent on its own, even when
other agents write to it: your next message to it does.

The sessions you started yourself open the same way, where they run, and
stay what they are: `enter` (or `volee attach`) on a background session
runs Claude Code's own `claude attach` on it, and `enter` on a session in a
tuios pane shows that pane. Nothing is stopped or restarted, and `Ctrl+Q`
leaves the session running as it was. Messages are for the agents volee
launched: to say something to another session, open it and type. A session
in a terminal volee cannot reach says so; in Claude Code, the left arrow on
its empty prompt moves it to the background, where volee opens it.

`volee adopt <agent>` (`A` in the interface, which asks first) is for a
session you want volee to drive from now on: it stops the session and
resumes its conversation in a volee host, with volee's settings, where it
receives messages and comes back after a reboot.

## 6. Run a mission

```sh
volee mission new "Invoicing" \
  "Add invoices to billing and expose them in the API, with tests." \
  --repo billing --repo api
volee mission show invoicing
```

A coordinator agent plans the work in the first repository, without editing
it, and launches one worker per task, each in its own worktree of one of the
mission's repositories (eight at most, unless you raise
`[missions] max_workers`). When a worker
stops, volee checks its work itself: commits pushed and a pull request open
for a change (commits alone in a repository without a remote), a report sent
for an analysis. The coordinator hears every verdict and asks workers to
finish what is missing.

Once every worker delivered, the coordinator merges nothing and closes
nothing: it presents its report, and asks you. The question waits on top of
your [messages](interface.md#messages) (`I`), and the mission shows as
`waiting for you`:

```text
coordinator-invoicing → you · question
Mission "Invoicing" is ready to merge.
Done: invoices in billing, exposed in the API.
Proof: billing#42 and api#17, tests green.
Left: nothing
  1 ok merge
  2 no
```

Press `1` then `y` (or run `volee msg answer coordinator-invoicing 1`):
volee merges each worker's work itself, its pull request on GitHub, or its
branch into the base branch in a repository without a remote, and the
mission closes. No agent merges. Closing tidies the mission in one gesture:
the workers' worktrees are removed, their branches kept, the coordinator
stopped, and one message tells you what is merged, what was tidied and what
was kept, with the reason. On `no`, or on an answer in your own words,
nothing is merged and the mission stays open. If a merge cannot go through
(a required review is missing, your main checkout has uncommitted changes),
nothing is forced: volee tells you why, and the coordinator presents its
report again once it is settled.

```sh
volee mission ls --all
volee mission abort invoicing --reason "wrong approach"   # nothing is deleted
```

## 7. Or talk to a lead

```sh
cd ~/code
volee lead
```

A lead is the agent you talk to: it launches the others. `volee lead` starts
one in the current directory and opens it in your terminal; `Ctrl+Q` leaves
it running, and `enter` on its row in the interface, or `volee attach`,
comes back to it. Tell it what you want in your own words:

```text
Add invoices to billing and expose them in the API, with tests.
```

It looks at what already runs, launches an agent for a task in one
repository or a mission for several, tells you in one line what it
launched, and gives the hand back. It never codes, and it does not watch
its agents: volee wakes it when one reports, and it tells you in three
lines what is done, the proof and what is left. Before a merge you are
asked, as in the section above; a lead never answers for you.

`volee lead "Add invoices to billing and to the API."` starts it with the
request at once.

## After a reboot

A reboot ends every host, but not the conversations: the first time the
daemon starts afterwards, it resumes each agent the reboot interrupted in a
new host, in its worktree: one that was working is told that the machine
restarted, one that waited at its prompt comes back waiting. Agents you
stopped, agents of a finished mission and agents that were done stay as they
were. To have the
daemon start at login, so that agents come back without opening volee, run
once:

```sh
volee daemon install
```

## What volee never does

- It never deletes a branch, and never removes a worktree whose work is not
  delivered.
- It never edits your Claude Code settings: the agents it launches get their
  hooks and tools on their own command line.
- It never resumes an agent you stopped until you write to it, and never
  resumes the agents of a finished mission.
- It never stops, restarts or reconfigures a session it did not start: it
  opens it where it runs, and what you type there is yours. Only `volee
  adopt`, which asks, moves one into a volee host.
- It never installs anything that starts at login unless you run
  `volee daemon install`.
- It never forces anything on you: no multiplexer, editor or git client is
  required.

Next: [Concepts](concepts.md) explains the words used here, and
[The interface](interface.md) every view and key.
