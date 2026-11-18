# 0021. Take over with `a` or `volee attach`, leave with Ctrl+Q, watch on a wall

- Status: Accepted; taking over from the interface amended by [0026](0026-edit-mode-types-into-an-agent-inside-volee.md), the wall by [0028](0028-one-emulator-and-the-wall-inside-the-interface.md)
- Date: 2026-11-08

Since [ADR 0028](0028-one-emulator-and-the-wall-inside-the-interface.md),
the interface's wall is a view of the interface itself, on every agent of
the list that runs, in a volee host or not. `volee host wall` stays the
wall described below.

Since [ADR 0026](0026-edit-mode-types-into-an-agent-inside-volee.md), `i`
or `a` in the interface enters edit mode inside volee's full-screen view
instead of handing over the whole terminal, and that view is live. `volee
attach`, the leave key, the size rules and the wall stay as below.

## Context

With a host per agent ([ADR 0019](0019-a-host-process-holds-each-agent-terminal.md)),
volee itself connects a terminal to an agent: there is no `claude attach` to
call any more. Three ways of looking at agents need rules:

- **take over**: type into the agent from the interface (`a`) or from any
  terminal, and come back;
- **watch**: the preview beside the list, and the full-screen view (`z`);
- **several at once**: the product owner wants a wall of live screens, to see
  what every agent is doing without opening each one.

Until 0.7.0, `Ctrl+Z` left a `claude attach` because Claude Code chose that
key. volee now chooses. A key that leaves must never reach the agent, must be
rare in agents' own shortcuts, and must survive the keyboard protocols agents
switch on: once a program enables the kitty keyboard protocol, the person's
terminal no longer sends `0x11` for Ctrl+Q but `ESC [ 113 ; 5 u`.

## Decision

**Take over.** `a` on an agent suspends the interface, connects its terminal
to the agent's host as the input owner, and comes back to the same row when
the person leaves. `volee attach <agent>` does the same from any terminal and
returns to the shell. Before switching, it prints one line saying how to come
back.

**Leave with Ctrl+Q.** The client reads the person's keys and removes Ctrl+Q
before they reach the host, in every encoding: the plain byte `0x11`, the
kitty keyboard protocol (`CSI 113 ; 5 u`, with any event type), and xterm's
`modifyOtherKeys` (`CSI 27 ; 5 ; 113 ~`). Agents never receive Ctrl+Q. The key
becomes configurable later if someone needs Ctrl+Q inside an agent.

**One owner, one size.** The input owner's terminal size is the agent's size:
the client sends it when it connects and each time the terminal is resized.
When the owner leaves, the size stays. Taking over from a second terminal
moves the input there; the first client is told and returns to its caller. No
one else ever resizes an agent.

**Clean entry and exit.** On entry the client switches the person's terminal
to the alternate screen, clears it and writes the host's snapshot. On exit it
turns off what the agent may have left on (mouse reporting, bracketed paste,
focus events, kitty keyboard flags, hidden cursor, the agent's own alternate
screen), then restores the person's screen.

**Watch.** The preview and the full-screen view are watchers: a socket to the
host, a `Screen` of `volee-term` fed by the snapshot and the stream, drawn
into a ratatui buffer cell by cell with its colors. An agent's screen is drawn
at the agent's size, never resized for the view. When the area has fewer
rows, runs of blank rows shrink to one (an agent sized by a tall terminal
leaves a gap between its conversation and its prompt), then the view keeps
the rows from the bottom of what is drawn, or the cursor, upwards. Columns
that do not fit are cut on the right.

**The wall of screens.** A fifth view (`5`, after the hierarchy, the
network, the repositories and the timeline) shows the agents of the current
list as tiles of live screens, as many as the terminal can hold at a readable
size (1, 2, 4, 6 or 9). Arrows or `hjkl` move between tiles, `a` takes over
the selected one, `z` shows it full screen, and the filter (`/`) narrows the
wall like the other views. Only visible tiles keep a watcher; tiles that scroll away
close theirs.

**Agents volee does not host.** Their screen comes from what their location
offers, drawn by the same emulator: a pane capture when a location adapter
knows their pane, the harness's replay (`claude logs`) for a Claude Code
background session, nothing otherwise. volee never types into them: `a`
focuses their pane when an adapter knows it, and otherwise offers to adopt
them ([ADR 0023](0023-every-agent-on-the-machine.md)).

## Consequences

- Taking over works the same for every harness and from any terminal, and
  leaving never depends on the agent's own shortcuts.
- Watching costs one socket and one emulator per visible screen, and never
  disturbs the agent: no resize, no redraw, no query.
- An agent sized by a wide terminal shows cut in a narrow tile. The wall
  favors seeing many agents over seeing all of one; `z` shows all of one.
- Ctrl+Q is taken from agents. None of the supported harnesses binds it
  today, and flow control (XON) is off in the raw mode clients use.

## Alternatives considered

- **Keep Ctrl+Z.** It suspends programs in every shell and some agents use it;
  intercepting it would surprise people more than Ctrl+Q does.
- **A prefix key, like tmux.** Costs a key press on every shortcut, for one
  action.
- **Scale screens down to fit a tile.** Text cannot be scaled in a terminal;
  dropping columns makes code unreadable. Cutting keeps what is shown exact.
- **The newest client sets the size, as `claude attach` does.** Measured: a
  watcher that connects resizes everybody, and the size sticks after it
  leaves. Only the person typing should decide it.

## A menu key (2026-11-18)

The person did not want a single key that leaves at once: he asked for a
key that opens a menu, as tmux's prefix does, and chose Control and Space.
Control and B, tmux's, was set aside: tmux and tuios take it before volee
sees it, and Claude Code uses it to send a command to the background.
Control and Space is free in Claude Code, tmux, tuios, zellij and the
shells; macOS uses it to switch keyboard layouts only where several are
installed.

The leave keys are now `ctrl-space q` by default, which reverses the
alternative set aside above (a prefix key, like tmux). In the interface,
Control and Space opens a small window over the agent's screen: `q` comes
back, `d` shows the agent's changes, `n` and `p` open the next or the
previous agent, Control and Space again is sent to the agent, and any other
key closes the menu and reaches no one. `volee attach`, `volee run` and
`volee host wall` read the same keys in the raw input: the filter of the
host's client recognizes Control and Space in every encoding (`0x00`, the
kitty protocol's `CSI 32 ; 5 u`, xterm's `CSI 27 ; 5 ; 32 ~`), arms a menu
that waits across reads, detaches on q, sends a second Control and Space as
it came and drops any other key. A single Control key that leaves at once
(`leave = "ctrl-q"`) is still accepted.

