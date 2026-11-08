# 0018. A pane that shows a background session is its preview

- Status: Accepted
- Date: 2026-11-08

## Context

The preview of a Claude Code background session comes from `claude logs`,
which replays only the session's recent output. What was drawn before the
replay starts and never redrawn since, such as the frame of the prompt and
the status line, is missing: on a session that has run for hours, the
preview shows the conversation with gaps and no status line.

People often keep such a session open in a pane: `claude agents` shows the
session they pick, and `claude attach <id>` shows one session. That pane
holds the session's whole screen. The session runs in Claude Code's
background supervisor, not in the pane, so the location adapters of
[ADR 0015](0015-tuios-location-adapter.md), which walk from a session's
process up to a pane, never find it, and the daemon did not even ask them
about background sessions.

The only link between such a pane and its session is the pane's title:
Claude Code titles its terminal with the name of the session it shows,
after a status mark (`◑ fix the parser`). tuios 0.12.1 reports that title
for each agent pane it knows, and no session id (measured on this
machine: `agent_session_id` is empty for a pane that runs `claude agents`).

## Decision

After each discovery round, the daemon asks each multiplexer which panes
show the live background sessions, given by name. A pane shows a session
when its title, without the status mark, is the session's name, and only
names that no other live session shares are asked about. The answer, a
**mirror**, is kept in memory until the next round.

A mirror is not a location: the agent stays where it runs (`background`),
and no event is recorded, so moving around in `claude agents` changes
nothing in the registry or the timeline. The preview, and so the
full-screen view, reads the mirror's pane when there is one, and falls back
to `claude logs` when there is none or the pane does not answer.

tuios finds mirrors from `list-agents`; the other multiplexers find none
for now.

## Consequences

- A background session open in a tuios pane previews exactly as that pane
  shows it, status line and prompt included, and follows the pane: the
  mirror appears or goes within a discovery round (2 to 10 seconds while
  the interface is open).
- The match is by name. Two live sessions with one name are never
  mirrored, but a pane titled with a session's name that shows something
  else (another program setting the same title) would be read as its
  mirror. Claude Code's names are several words long, which makes this
  unlikely.
- Take over (`a`) still runs `claude attach` in the interface's terminal,
  even when a pane already shows the session.

## Alternatives considered

- **Make the pane the agent's location.** It would also send take over to
  the pane, but every visit to another session in `claude agents` would
  record two `LocationChanged` events, and the session does not run in the
  pane: stopping the pane's program does not stop it.
- **Match by process.** The pane's program (`claude agents`, or `claude
  attach` with an id) does not say which session `claude agents` shows,
  and `claude attach` clients are not the only way to show one.
- **Keep the replay and accumulate it.** Reading `claude logs` repeatedly
  and stitching the replays together still misses what was drawn before
  volee first read it.
