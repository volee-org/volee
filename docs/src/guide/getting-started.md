# Getting started

This walk-through takes about ten minutes. It assumes volee is installed (see
[Installation](installation.md)) and that you already use Claude Code.

## 1. Check that volee finds Claude Code

```sh
volee daemon status
```

The daemon starts on its own the first time a command needs it. Its status
names the Claude Code version it found, and whether `gh` is there to read
pull requests. If Claude Code is missing or too old, the line says why.

## 2. See every agent on the machine

```sh
volee
```

The interface opens on every Claude Code session running on this machine,
whoever started it: in a terminal, in the background, or launched by another
agent. The ones that need you come first. Press `?` for the keys; `q` quits
without stopping anything. `volee ls` prints the same list without the
interface.

Nothing here depends on how you started the sessions: volee reads Claude
Code's own list of sessions every two seconds, and the hooks of the agents it
launches itself.

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

Then run `volee config check`. Claude Code only starts background sessions
in folders you trust, and each repository counts on its own: if you have
never run `claude` in a repository, do it once and accept the trust prompt.

## 4. Launch an agent

```sh
volee spawn billing feat/invoices "Add an invoice model with its migration and tests."
```

volee creates the branch `feat/invoices` from `main`, checks it out in a new
worktree at `billing/.claude/worktrees/feat-invoices`, and starts a Claude
Code background session there with your task. The agent appears at once,
with its repository and branch. Your own checkout is never touched.

## 5. Talk to it, or take it over

```sh
volee msg send billing "Use cents, not floats, for amounts."
volee attach billing        # Ctrl+Z comes back
```

A message reaches the agent at the end of its turn, wakes it if it is idle,
and resumes its session if it ended. Questions the agent asks you show up
pinned in the interface until you answer. `volee attach` hands your terminal
to the agent's session; `Ctrl+Z` returns to where you were, and the agent
keeps running.

## 6. Run a mission

```sh
volee mission new "Invoicing" \
  "Add invoices to billing and expose them in the API, with tests." \
  --repo billing --repo api
volee mission show invoicing
```

A coordinator agent plans the work in the first repository, without editing
it, and launches one worker per task, each in its own worktree. When a worker
stops, volee checks its work itself: commits pushed and a pull request open
for a change, a report sent for an analysis. The coordinator hears every
verdict, asks workers to finish what is missing, and once every worker
delivered, sends you the final report with links to every pull request and
closes the mission.

```sh
volee mission ls --all
volee mission abort invoicing --reason "wrong approach"   # nothing is deleted
```

## What volee never does

- It never deletes a branch, and never removes a worktree whose work is not
  delivered.
- It never edits your Claude Code settings: the agents it launches get their
  hooks and tools on their own command line.
- It never forces anything on you: no multiplexer, editor or git client is
  required, and the ones you use are optional integrations.

Next: [Concepts](concepts.md) explains the words used here, and
[The interface](interface.md) every view and key.
