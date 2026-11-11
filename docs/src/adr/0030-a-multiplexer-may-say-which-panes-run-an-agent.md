# 0030. A multiplexer may say which of its panes run an agent

- Status: Accepted (amends [0023](0023-every-agent-on-the-machine.md) and [0015](0015-tuios-location-adapter.md))
- Date: 2026-11-11

## Context

volee finds the agents of a harness when it can read a list of them: the
sessions Claude Code lists, the OpenCode processes of the process table
([ADR 0023](0023-every-agent-on-the-machine.md)). A profile says how to
recognize a harness's processes, and a profile is only written from a probe
on the real program ([ADR 0022](0022-harness-profiles.md)). For Codex CLI,
Gemini CLI and the others there is none yet: started by hand in a pane, such
an agent is not in volee's list, though the pane is one volee could show and
open ([ADR 0027](0027-open-without-owning.md)).

tuios already knows. It keeps a manifest of harnesses and recognizes the
program in the foreground of each pane. Measured on tuios 0.12.1 with
programs named `codex`, `gemini`, `aider` and `opencode` started by hand in
panes, no hook installed: `list-agents` reports each pane with its harness
(`codex`, `gemini-cli`, `aider`, `opencode`), the name of its foreground
program, its directory, `source: detect`, `confidence: strong`; a program it
does not know is reported with no harness. `get-window` gives the process
the pane started.

[ADR 0023](0023-every-agent-on-the-machine.md) made the location adapters
"only a read-only source of screens". That sentence was written when every
agent volee could show came from a listing of its own.

## Decision

A location adapter may also say which of its panes run an agent, and of
which harness. volee asks it at each round, and makes an agent of each pane
whose harness it has no list of its own for: every harness tuios names
except Claude Code and OpenCode, which volee finds itself.

- **The agent is a process, not a pane.** It is the process in the
  foreground of the pane's terminal: the pane's own process when the pane
  was started with the program, the foreground job of its shell otherwise
  (the terminal's foreground process group, which `ps` gives). The agent is
  keyed by that process, like an OpenCode agent found without a session id
  ([ADR 0001](0001-agent-identity.md): a location is never an identity). A
  pane whose process cannot be found makes no agent.
- **Only what tuios sees now.** A pane counts when tuios recognizes the
  program in its foreground (`source: detect`, with strong evidence: the
  program's name). A harness that a hook once reported in a pane says what
  ran there, not what runs there now.
- **Its harness is the one the multiplexer names**, as written
  (`codex`, `gemini-cli`). volee has no profile for it: nothing is
  launched, resumed or read beyond the pane.
- **It is an agent volee only watches**, in its pane: listed with the pane's
  directory, shown by the pane's screen in the preview, on the wall and
  full size, and opened by Enter where it runs
  ([ADR 0027](0027-open-without-owning.md)). Nothing is typed into it,
  stopped or restarted on volee's own account.
- **It stays for as long as the multiplexer names it.** When its process
  ends or the pane's foreground program is no longer a harness, the agent
  stops, then leaves the list a minute later.
- **Its state** is `idle` while it runs: volee reads no state for it. The
  multiplexer's own guess of what the pane does is not taken, since it is
  drawn from the same screen volee shows.

## Consequences

- An agent of Codex CLI, Gemini CLI, Aider or any harness tuios knows shows
  in volee and opens there when it runs in a tuios pane, without a profile
  in volee and without the person doing anything.
- Outside tuios nothing changes: in tmux or in a plain terminal such an
  agent is not found. `volee run` puts it under volee
  ([ADR 0029](0029-volee-run.md)).
- volee trusts tuios's manifest for what counts as an agent. A program that
  merely carries the name of a harness is listed as one.
- The mechanism was measured with stand-in programs carrying the harnesses'
  names: Codex CLI and Gemini CLI were not installed on the machine it was
  built on. What their real interfaces do in a pane is not known here.

## Alternatives considered

- **Profiles for each harness in volee.** The right end state, but a profile
  needs a probe on the real program, one at a time. tuios's manifest covers
  the wait, for panes.
- **Taking tuios's states too** (working, needs input). They come from
  tuios's hooks when the person installed them, and from its reading of the
  screen otherwise. volee would repeat a guess as a fact; a later decision
  can take the hook-reported ones.
- **Keying the agent by its pane.** Simple, and wrong the day a pane runs a
  second program: the row would carry the history of another process.
