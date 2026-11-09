# 0024. Agents interrupted by a reboot come back in new hosts

- Status: Accepted
- Date: 2026-11-08

## Context

A host ([ADR 0019](0019-a-host-process-holds-each-agent-terminal.md)) keeps
its agent alive while the machine runs. A reboot, a logout or a crash of the
host ends the program, but the conversation is saved by the harness and the
worktree is on disk. The product owner wants volee to bring such agents back
by itself.

Some agents must stay down: one stopped with `volee stop` comes back only when
someone writes to it, and the agents of a mission that is over never come back
([ADR 0007](0007-missions.md)). An agent that crashes as soon as it starts
must not be restarted in a loop.

## Decision

- **Interrupted.** A host record whose program has no exit status, whose host
  process is gone, and whose agent was not stopped by volee, is an interrupted
  agent. A machine that stopped leaves exactly that.
- **Resume.** When the daemon starts and finds interrupted agents, it runs
  each one's resume command ([ADR 0022](0022-harness-profiles.md)) in a new
  host, in the same directory and worktree, with the same settings. The
  resumed session joins the agent's lineage ([ADR 0001](0001-agent-identity.md)).
- **Not resumed:** agents stopped by volee, agents of a mission that is over,
  agents whose harness cannot resume (the generic profile: they are shown as
  interrupted, with a relaunch action), and agents interrupted twice within
  ten minutes (shown as failed, with the reason).
- **When.** The daemon starts the first time volee is used after the reboot,
  as today. `volee daemon install` registers it to start at login (a launchd
  agent on macOS, a systemd user unit on Linux), so that agents come back
  without opening volee. volee never installs it unasked.
- **Setting.** `[agents] resume_after_reboot = true` by default; `false`
  shows interrupted agents with a resume action instead.

## Consequences

- After a reboot, every agent that was working comes back with its
  conversation, in its worktree, and the timeline shows the interruption.
- A resumed agent has lost what it was doing in the middle of a turn (a
  command running, a half-written answer): the harness resumes from the last
  saved message. The resume prompt says the machine restarted.
- Without the login item, nothing comes back until volee runs once.
- Records of hosts accumulate in `<state>/hosts/`; the daemon removes the ones
  of agents that are forgotten or tidied.

## Alternatives considered

- **Resume nothing, offer a button.** Simpler and safe, but the product owner
  wants agents to continue on their own.
- **Keep agents alive across reboots.** Impossible without the harness; its
  saved conversation is what survives.
- **Install the login item on first run.** Changes the person's system
  without asking.
