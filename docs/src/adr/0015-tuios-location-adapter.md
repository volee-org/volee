# 0015. tuios is the first location adapter

- Status: Accepted
- Date: 2026-11-07

## Context

[ADR 0008](0008-multiplexers-are-optional.md) plans location adapters: for an
agent that lives in a multiplexer pane, find the pane, focus it, and
optionally capture it for a preview. volee 0.1 has none. A Claude Code
session started by hand (an interactive session) is listed, but its preview
and its take over are refused: "switch to that terminal". Most interactive
sessions of the product owner run in tuios panes.

Measured on 2026-11-07 with tuios 0.12.1 and Claude Code 2.1.319, from a
throwaway tuios session:

- `claude agents --json --all` gives every live session its process id.
  Interactive sessions have no handle, so `claude logs` and `claude attach`
  cannot reach them.
- `tuios call list-agents '{"all_sessions":true}'` lists the agent panes of
  every session in one call of about 11 ms. Each one carries its session, its
  window id, its harness and `agent_session_id`. tuios's own Claude Code
  hooks (`tuios integration install claude-code`) fill that id with the
  exact Claude Code session id.
- Without those hooks, `tuios call resolve-pane '{"pids":[...]}'` names the
  pane whose process is the nearest ancestor of a process, in about 11 ms.
  For a process outside every pane, it answers `window_not_found`. One
  `ps -axo pid=,ppid=` (about 17 ms) gives the ancestors.
- Working directories do not tell panes apart: the person's own pane and the
  probe both ran in `~/Developer`.
- A background session that a pane shows through `claude agents` is not in
  that pane's process tree. It stays a background session, which
  `claude attach` takes over.
- `tuios call capture-pane` with `styled` returns the pane's rows with their
  colors.
- `tuios focus-window -s S W` focuses a pane and shows its workspace.
  `tuios switch-session S`, run from a pane, switches the client that shows
  that pane's session; from outside every pane it switches the only attached
  client, and needs a client to be named when there are several.

## Decision

- **Where.** A `Multiplexer` trait in a new crate, `volee-mux`, answers four
  questions: which panes some sessions run in, what a pane shows, how to bring
  it forward, and whether the multiplexer can be used now. `Tuios` is the
  first implementation. tmux and Zellij can follow behind the same trait. It
  starts programs through volee-harness's `CommandRunner`, which gives
  timeouts and a fake for tests.
- **Optional.** `volee daemon run` uses tuios only when `tuios` is on the
  `PATH`. Each call needs a running tuios server. Without one, a round changes
  nothing and the next round asks again. There is nothing to configure, and
  `volee daemon status` says what it found.
- **Matching.** After each discovery round, for live interactive sessions
  only:
  1. tuios's own record: the agent pane whose harness is `claude-code` and
     whose `agent_session_id` is the session's id;
  2. otherwise, the session's process: its pid and its ancestors, given to
     `resolve-pane`, which says whether a pane runs it;
  3. never the working directory.

  A match is recorded as `Location::Pane { multiplexer: "tuios", target:
  "<session>:<window id>" }`, with a `LocationChanged` event. A session that
  is no longer in a pane goes back to `Terminal`. A failed round changes
  nothing. Every adapter writes its addresses as `<session>:<pane>`, so the
  interface can name the session.
- **Preview.** `agents.preview` of an agent in a pane captures the pane, and
  returns its rows placed with cursor moves. The interface draws them as it
  draws `claude logs`.
- **Take over.** `agents.attach` answers with `focus`: the commands that
  bring the pane forward (`tuios focus-window -s S W`, then
  `tuios switch-session S`). The client runs them in its own environment,
  without giving up its terminal. When volee runs in a tuios pane, tuios then
  switches the person's own client, even when several are attached.
- **Read and focus only.** volee reads (`list-agents`, `resolve-pane`,
  `capture-pane`) and focuses (`focus-window`, `switch-session`). It never
  types into a pane, never creates, closes or moves one, and never installs
  tuios's hooks.

## Consequences

- An interactive session in a tuios pane can be watched from volee and
  reached in one key; nothing changes without tuios.
- A round costs one `tuios` call when tuios's hooks report session ids. Each
  interactive session they leave unmatched adds one `ps` and one call, and a
  session found outside every pane is not asked about again.
- The client runs commands the daemon sends, as it already does for
  `claude attach`.
- A pane found once is followed one round late: a focus in between fails and
  says why. A session run in tmux inside a tuios pane is not matched: its
  terminal is tmux's, and that is a tmux adapter's job.

## Alternatives considered

- **The interface finds and focuses panes itself.** Each client would need
  the adapters and the process table, and two clients could disagree. The
  daemon matches once and records the location for every client.
- **The daemon focuses the pane.** It runs outside every pane, so tuios
  would switch "the only attached client": the wrong one, or none, as soon as
  two are attached.
- **Matching on the working directory or the pane title.** Ambiguous, as
  measured.
- **A switch in the configuration.** ADR 0008 keeps adapters optional and
  detected; detection costs one `PATH` lookup.
