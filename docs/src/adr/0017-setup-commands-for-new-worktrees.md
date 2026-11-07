# 0017. Setup commands prepare a new worktree before its agent starts

- Status: Accepted
- Date: 2026-11-07

## Context

volee launches an agent in a new git worktree
([ADR 0013](0013-worktrees-live-in-their-repository.md)). A worktree holds
the tracked files only: no `node_modules`, no virtual environment, none of
the ignored files a project needs to run, such as `.env.local`. An agent
that starts there spends its first minutes, and tokens, installing
dependencies; in Claude Code's `manual` permission mode it waits for the
person to allow each install command; and some projects cannot run their
tests at all without a file that git ignores.

tuios-cockpit, the prototype volee replaces, had an `install` command per
repository for this. People moving to volee asked for the same thing, and
the configuration written from the cockpit's registry had to leave those
commands out.

## Decision

- **Where.** `setup` in a repository's table: a list of commands, such as
  `["cp \"$VOLEE_REPO_ROOT/.env.local\" .", "bun install"]`. They come from
  the person's own configuration file, which agents do not write.
- **When.** For every agent volee launches in a new worktree, including a
  worktree that a failed launch left untouched and that a new launch reuses
  ([ADR 0016](0016-what-a-failed-launch-left-untouched.md)): after the
  worktree exists and the agent is registered, before its session starts.
  The coordinator of a mission, which works in a main checkout, has none.
- **How.** The daemon runs them in order, each with `sh -c`, in the
  worktree, with standard input closed. Each gets the daemon's environment
  plus `VOLEE_REPO` (the repository's name), `VOLEE_REPO_ROOT` (its main
  checkout), `VOLEE_WORKTREE`, `VOLEE_BRANCH` and `VOLEE_BASE`. Each may run
  for 15 minutes.
- **What people see.** The agent is listed as launching, its detail naming
  the command that runs (`setting up: bun install`). What the commands print
  goes to `setup.log` in the agent's state directory; `volee spawn` names that
  file once the agent runs.
- **Failure.** The first command that exits with a failure status, or runs
  out of time, stops the setup. The launch then fails as a failed session
  start does: the agent is marked failed with the command and the last line
  it printed, no session starts, the worktree and branch stay
  ([ADR 0011](0011-safety-and-delivery-evidence.md)), and the same command
  can run again once the problem is fixed; the setup runs again then.

## Consequences

- An agent starts in a worktree that runs, with no install step of its own
  and no permission prompt for it.
- A launch takes as long as its setup. `volee spawn`, the launch dialog and an
  agent that asks volee for a worker over MCP wait for it; `volee spawn` says
  so before it waits.
- Setup commands should be safe to run twice: a launch that reuses a worktree
  runs them again. A setup that changes tracked files, or leaves files git
  does not ignore, makes the worktree no longer untouched, so it is not
  reused.
- A command that runs out of time is killed; processes it started in the
  background are not.

## Alternatives considered

- **Tell the agent to run the setup first.** It costs tokens on every launch,
  waits for permissions in `manual` mode, and depends on the agent doing it
  before anything else.
- **A git `post-checkout` hook.** It runs for every checkout in the
  repository, the person's own included, and volee would have to install it
  in their repository.
- **Start the session at once and set up beside it.** The agent would work
  in a worktree that is still being prepared, and see half-installed
  dependencies.
