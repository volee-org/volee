# 0022. A profile per harness says how to launch, resume, find and read it

- Status: Proposed
- Date: 2026-11-08

## Context

Until 0.7.0, volee launched only Claude Code, and other harnesses could only
join through MCP ([ADR 0014](0014-agents-of-other-harnesses-join-through-mcp.md)).
A host ([ADR 0019](0019-a-host-process-holds-each-agent-terminal.md)) runs any
command line in a terminal, so volee can launch and watch any harness. What
differs between harnesses is small and factual: the command that starts a
session with a first prompt, the one that resumes it, the process that shows
it is running, and where its state can be read.

The product owner's order: Claude Code, Codex CLI, Gemini CLI and Grok Build
first, then the most used ones (OpenCode, Copilot CLI, Aider, Amp, Cursor's
agent, Qwen Code), and any other command line.

## Decision

A **harness profile** is a value in `volee-harness`, with these parts:

| Part | What it says | Example for Claude Code |
| --- | --- | --- |
| `launch` | the command line for a new session: first prompt, model, name, volee's settings, worktree | `claude --name N --settings F --worktree=W -- "<task>"` |
| `resume` | the command line that continues a session, when there is one | `claude --resume <session id>` |
| `processes` | how to recognize its processes in the process table, and which of its own helpers to ignore | program `claude`; not `claude agents`, `claude logs`, its MCP servers |
| `session` | how to learn a session's id: hooks, a listing, a server, or none | hooks (`SessionStart`), `claude agents --json` |
| `states` | where states come from, best first | hooks, then `claude agents --json`, then the screen |
| `join` | how to give it volee's MCP tools | `--settings` (Claude), `--join` ([ADR 0014](0014-agents-of-other-harnesses-join-through-mcp.md)) for the others |

**States, best source first.** The vocabulary stays (working, needs you, idle,
done, failed, stopped). Each profile lists its sources in order:

1. the harness's hooks, for agents volee launched with its own settings;
2. a server or listing the harness documents (`claude agents --json`, Codex's
   `app-server`), for any agent;
3. the screen, as a last resort: output in the last seconds means working,
   the bell or a profile's prompt pattern means needs you, silence means idle.
   The host gives every watcher these signals (activity, title, bell) without
   a parser of its own.

**The generic profile** runs any command line. It has no resume, no session id
and only screen states. It is how a new harness is used before it has a
profile, and how a person runs a script or a REPL in a host.

**Profiles land one at a time**, each with a probe on the real program that
measures its commands, its processes and its states, written down in the
reference docs (as the Claude Code mechanics were). A profile states the
harness version it was measured on, and volee checks it at launch as it does
for Claude Code. What a probe did not measure stays out of the profile.

## Consequences

- Launching an agent of another harness is a choice in the launch dialog and a
  `--harness` flag, not new code paths through the daemon.
- States are as good as the harness allows: exact with hooks, coarse with a
  listing, guessed from the screen otherwise, and the interface says which.
- Each new harness costs a probe and a small profile; the generic profile
  covers the time in between.
- Harnesses change their flags often. A profile that fails its version check
  falls back to the generic profile for screen states and says so.

## Alternatives considered

- **ACP (the Agent Client Protocol) for every harness.** About forty agents
  speak it, natively or through an adapter, and it gives structured states.
  But the ACP client spawns and owns the agent and renders it itself: there is
  no terminal to take over, and it cannot observe a session started elsewhere.
  It stays a possible later source of states for hosted agents.
- **Screen parsing only.** Works everywhere, but guesses where hooks and
  listings know.
- **Profiles in the configuration file.** Lets people add a harness without a
  release, but the launch and resume rules need tests. The generic profile
  already covers "run this command"; user-defined profiles can come later.
