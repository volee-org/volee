# Concepts

volee uses a handful of words with precise meanings. Knowing them makes the
interface and the documentation easier to read.

## Agent

A unit of work done by an AI coding harness, such as a Claude Code session
fixing a bug in one repository. An agent has:

- a **role** (below);
- a **workspace**: repository, worktree, branch and the base branch it started
  from;
- a **state**: working, needs you, idle, done, failed, or lost;
- a **parent** (the agent that launched it) and, optionally, a **mission**.

An agent is not a terminal pane and not a process. If you close every window,
the agent is still there. If its session is resumed or forked, it is still the
same agent, with its history.

## Session

One run of a harness, such as a Claude Code session with its session id. An
agent has one current session and keeps the lineage of the previous ones.

## Roles

| Role | Who | Typical work |
| --- | --- | --- |
| **Lead** | An agent you launch yourself, with `volee spawn` | Works on your request in its own worktree and reports back to you. Leads that start missions and workers come later. |
| **Coordinator** | The agent of a mission | Splits the goal, writes tasks for workers, answers their questions, writes the final report |
| **Worker** | An agent working in one repository, in its own worktree | Codes, tests, opens a pull request, or writes an analysis |
| **Subagent** | A helper an agent starts by itself (for example a search) | Short tasks inside its parent's session |
| **Independent** | A session volee did not launch | Anything you started by hand; volee shows it, with a coarser state |

## Mission

A goal that spans one or more repositories, run by a coordinator and delivered
by workers. A **change** mission delivers branches and pull requests; an
**analysis** mission delivers reports. volee's supervisor checks delivery
evidence before a mission is considered done.

## Message

A typed note from one party (you, an agent, or volee itself) to others:
`question`, `answer`, `contract`, `report`, `alert` or `note`. Messages are
grouped in threads (one per mission, one per pair of agents, and your inbox)
and can point at a file and line, a commit or a pull request.

## Delivery evidence

What volee can prove about an agent's work: commits since it started, branch
pushed, pull request opened or merged, checks passed, report delivered. Cleanup
commands rely on evidence, never on the absence of a window or a process.

volee keeps this evidence current by itself, for the sessions you started
without volee too: it reads git in each agent's directory, fetches each
repository with an agent (`git fetch --prune origin`, at most every five
minutes) and, when the GitHub CLI is signed in, asks it about each agent's
branch at most once a minute. A fetch only updates your remote-tracking
branches. See [Git](../architecture/git.md).
