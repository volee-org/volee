# 0002. The harness owns agent processes

- Status: Superseded by [0019](0019-a-host-process-holds-each-agent-terminal.md)
- Date: 2026-11-06

## Context

Someone has to keep agent processes alive, give them a terminal, survive the
user closing windows, and let the user take over an agent. Terminal
multiplexers do this; so does Claude Code itself since its background
sessions (`claude --bg`), its supervisor daemon and `claude attach`.

Measured on Claude Code 2.1.319: `claude --bg` returns at once with a short
id; `claude agents --json` lists every session with its state; `claude
attach <id>` opens the session in the current terminal and `Ctrl+Z` returns
control to the calling program with exit status 0.

## Decision

volee never owns agent processes or their terminals. For Claude Code, every
agent volee launches is a background session started with `claude --bg`
inside its worktree. Taking over an agent from the TUI suspends the TUI, runs
`claude attach <id>` in the same terminal, and resumes the TUI on the same
row when the user leaves with `Ctrl+Z`.

## Consequences

- Agents survive the TUI, the daemon and every terminal window being closed.
- volee needs no PTY code, no scrollback, no resize handling for agents.
- volee depends on the harness's supervisor features. They are new and
  labelled research preview, so volee checks the harness version at startup
  and degrades to read-only discovery when a feature is missing.
- Agents running in a multiplexer pane (started by hand) are still visible;
  taking them over goes through a location adapter (ADR 0008).

## Alternatives considered

- **Own the processes in PTYs** (like ccmanager). Re-implements what the
  harness and multiplexers already do, and dies with the TUI.
- **Require a multiplexer** (like the prototype on tuios, or tools built on
  tmux). Ties volee to one terminal stack and brings back pane identity.
