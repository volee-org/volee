# 0023. volee shows every agent on the machine, and adopts the ones it did not start

- Status: Accepted (supersedes [0018](0018-panes-that-show-background-sessions.md); amends [0004](0004-sources-and-reconciliation.md) and [0015](0015-tuios-location-adapter.md))
- Date: 2026-11-08

## Context

volee's promise is to show every coding agent on the machine. Until 0.7.0 it
found agents through `claude agents --json` only, so a Codex CLI or Gemini CLI
started in a terminal was invisible unless it joined through MCP
([ADR 0014](0014-agents-of-other-harnesses-join-through-mcp.md)). With hosts
([ADR 0019](0019-a-host-process-holds-each-agent-terminal.md)) volee's own
agents are easy to see; the others need a source that works for every
harness.

Every running agent is a process. Its program says which harness it is, and
its working directory gives the repository and branch through git. Reading the
process table (`ps`) and a process's directory (`/proc` on Linux, `lsof` on
macOS) uses documented system interfaces, not a harness's internal files,
which [ADR 0004](0004-sources-and-reconciliation.md) forbids.

Mirrors ([ADR 0018](0018-panes-that-show-background-sessions.md)) matched a
pane to a session by its title. The product owner rejected them.

## Decision

**Sources.** The daemon reconciles four sources into one list:

1. **hosts**: volee's own agents, exact and live;
2. **the process table**: every process of the current user that a harness
   profile ([ADR 0022](0022-harness-profiles.md)) recognizes, without its
   helpers, with its directory, and so its repository and branch;
3. **harness listings and servers** where they exist (`claude agents --json`,
   Claude Code hooks, Codex's `app-server`), which add session ids and states;
4. **location adapters** (tuios, tmux), now only a read-only source of screens
   for agents that run in a pane, matched by the multiplexer's own record of
   the session id or by process ancestry, never by title.

Joins use, in order: the host's own record, the harness session id, the
process id with its start time. A process seen in the table and in a listing
is one agent.

**Identity.** [ADR 0001](0001-agent-identity.md) does not change: an agent
started elsewhere gets an agent on first sight, with the role `independent`,
keyed by its session id when its profile gives one, else by its process and
start time.

**Read only.** volee never types into, resizes or stops an agent it does not
host. Its screen comes from its location, as
[ADR 0021](0021-take-over-watch-and-the-wall-of-screens.md) says.

**Adopt.** For a harness whose profile can resume, `A` on an agent started
elsewhere moves its conversation into a host:

1. the original must end first, so that two programs never write the same
   conversation: volee stops a background session with the harness's own
   command (`claude stop`), and asks the person to quit an agent that runs in
   a terminal, then waits for its process to end;
2. volee runs the profile's resume command (`claude --resume`, `codex resume`,
   `gemini --resume`) in the same directory, in a new host;
3. the agent keeps its volee identity; the resumed session joins its lineage.

**Mirrors are removed** in the release that brings hosts, with their code
(`mirrors` in the daemon, `Multiplexer::mirrors`).

**Cost.** One `ps` per discovery round (about 17 ms measured on this machine),
and one directory lookup per process seen for the first time, cached by
process id and start time.

## Consequences

- An agent of any known harness appears within a discovery round, wherever it
  was started, with its repository and branch.
- Agents started elsewhere have the states their harness exposes: exact for a
  Claude Code session (listing), coarse or none for others until adopted.
- Adopting is the one way to take full control of an agent volee did not start,
  and it never runs two copies of a conversation.
- A harness that renames its program, or runs as a generic interpreter, needs
  its profile updated; the profile's probe says what to match.
- Only the current user's processes are read.

## Alternatives considered

- **`claude agents --json` only.** Sees Claude Code only.
- **Every harness joins through MCP.** Needs the person to configure each
  harness, and shows nothing for an agent that was not.
- **Read harnesses' session files.** Rich, but undocumented and unstable, as
  ADR 0004 says.
- **Match panes by title** (ADR 0018). Ambiguous, and tied to a multiplexer.
- **Adopt without stopping the original.** Two programs would write the same
  conversation.
