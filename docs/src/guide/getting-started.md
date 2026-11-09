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
Code's own list of sessions and looks for OpenCode in the process table (at
once when the interface opens, then every two to ten seconds while it is
open, more often while sessions change, and every thirty seconds otherwise),
and it hears the hooks of the agents it launches itself, which report at
once. The agents you did not start through volee are shown read only: volee
never types into them until you adopt one (step 5).

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
and the agent keeps running. In the interface, `a` does the same, `z` shows
the agent's screen full size, and `5` shows every agent of the list as a
wall of live screens. After `volee stop`, volee does not resume the agent on
its own, even when other agents write to it: your next message to it does.

A Claude Code session you started yourself with `claude --bg`, or one that
volee 0.7 launched, is shown but not taken over. `volee adopt <agent>` (`A`
in the interface) stops it and resumes its conversation in a host, where it
waits at its prompt; then `volee attach` works on it. Quit a session that
runs in a terminal of its own before you adopt it.

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
for a change, a report sent for an analysis. The coordinator hears every
verdict, asks workers to finish what is missing, and once every worker
delivered, sends you the final report with links to every pull request and
closes the mission.

```sh
volee mission ls --all
volee mission abort invoicing --reason "wrong approach"   # nothing is deleted
```

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
- It never types into, resizes or stops an agent it did not start, until
  you adopt it.
- It never installs anything that starts at login unless you run
  `volee daemon install`.
- It never forces anything on you: no multiplexer, editor or git client is
  required.

Next: [Concepts](concepts.md) explains the words used here, and
[The interface](interface.md) every view and key.
