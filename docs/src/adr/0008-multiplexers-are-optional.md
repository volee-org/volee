# 0008. Multiplexers are optional location adapters

- Status: Accepted
- Date: 2026-11-06

## Context

Developers use tmux, Zellij, herdr, tuios, WezTerm or Ghostty tabs. Some
agents are started by hand inside a pane; volee must show them and let the
user jump to them. But volee itself must not require any multiplexer.

## Decision

- A **location adapter** answers two questions for agents that live in a
  pane: "where is this session?" (by process id or session id) and "focus it".
  Optionally it can capture the pane for a preview.
- Adapters are optional and detected at runtime. The first ones are tmux and
  tuios; Zellij follows.
- Without an adapter, a pane-hosted agent is still listed with its state;
  taking it over is offered only when its harness supports attaching from
  another terminal.

## Consequences

- volee works in a plain terminal; multiplexer users get one more shortcut.
- Adapters stay small (a handful of commands each) and easy to contribute.

## Alternatives considered

- **Build on one multiplexer.** Rejected: excludes most users and brings back
  pane identity.
