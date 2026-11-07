# volee

**See, launch and coordinate every coding agent on your machine, from your
terminal.**

> **Early development.** volee 0.1 is being built in the open. Pages marked
> *planned* describe behavior that is designed (see the
> [decisions](adr/README.md)) but not released yet.

## The problem

Coding agents made parallel work cheap. Running five of them is easy;
keeping track of them is not:

- Which ones are still working, which one waits for a permission, which one
  finished an hour ago?
- Which worktree and branch does each one use, and was its work pushed?
- When two agents coordinate through a third one, what did they agree on?

Terminal tabs and multiplexer panes show *where* agents run, not *what they
do*. volee shows what they do.

## What volee gives you

- **One screen for every agent** on the machine, grouped the way the work is
  organized: a lead, its missions, their workers and subagents.
- **Urgency first**: questions and permission prompts float to the top.
- **Delivery at a glance**: commits since the agent started, pushed or not,
  pull request, checks.
- **Conversations you can follow**: typed, threaded messages between agents
  and with you, and a live graph where each message travels along its edge.
- **Missions**: give a goal across several repositories to a coordinator
  agent; volee supervises its workers and checks that the work was delivered.
- **Your terminal**: jump into an agent with a key and come back with
  `Ctrl+Z`. No multiplexer or editor is required.

## Where to go next

- [Installation](guide/installation.md)
- [Getting started](guide/getting-started.md): your first agent and your
  first mission, in about ten minutes.
- [Concepts](guide/concepts.md): agents, sessions, roles, missions, messages.
- [Architecture overview](architecture/overview.md), for contributors and the
  curious.
