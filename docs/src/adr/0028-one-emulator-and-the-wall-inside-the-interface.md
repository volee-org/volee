# 0028. One emulator for every screen, and the wall inside the interface

- Status: Accepted (completes [0020](0020-libghostty-vt-emulates-terminals.md); amends [0021](0021-take-over-watch-and-the-wall-of-screens.md), [0026](0026-edit-mode-types-into-an-agent-inside-volee.md) and [0027](0027-open-without-owning.md))
- Date: 2026-11-11

## Context

The audit of 2026-11-10 ([ADR 0027](0027-open-without-owning.md)) counted
three ways of drawing an agent's screen in volee 0.10:

- the preview, through the `vt100` crate, from a snapshot the daemon read
  every two seconds while the agent worked;
- the full-screen view and edit mode, through `vt100` too, from a
  connection to the agent's volee host;
- the wall of screens, through libghostty-vt, as a program of its own
  (`volee host wall`) that the interface started and stepped aside for.

[ADR 0020](0020-libghostty-vt-emulates-terminals.md) had decided that
libghostty-vt reads every terminal, and `vt100` stayed in the interface.
Two emulators meant two readings of the same bytes: a screen could differ
between the preview, the full-screen view and the wall of one agent.

The wall had a second problem, which
[ADR 0027](0027-open-without-owning.md) made plain. It showed only the
agents in volee hosts. On the product owner's machine that day, ten
sessions ran, one of them in a volee host: the wall showed one tile, or a
sentence that said it had nothing to show. It was the same defect as the
adoption rule: volee showed what it owned.

And the mouse did nothing in edit mode. An agent's interface that takes
clicks and the wheel (Claude Code does, for its own lists and its
transcript) got none of them inside volee.

## Decision

**One emulator.** Every screen the interface draws goes through
`volee_term::Screen`, which wraps libghostty-vt, and `vt100` leaves the
workspace:

- a live screen (the interface connected to a volee host or to a view)
  keeps a `Screen` of the program's size, fed the bytes the host sends: a
  snapshot, then the output;
- a screen the daemon read (a multiplexer pane, the replay of a background
  session) is fed to a `Screen` once, of the size the output tells.

`volee-term` gains what the interface needs from the emulator: wide
characters as two cells drawn once, a view that scrolls back through the
history and stays on its lines while the program writes, the cursor-key and
paste modes for the hosts that do not encode keys themselves, and the
mouse.

**One live screen, three ways to fit it.** The interface draws a screen
cell for cell where it fits (edit mode gives the agent the panel's size),
wrapped where it is read (the full-screen view of a wider screen), and
cropped to its bottom where it is glanced at (the preview, a tile of the
wall). The cells for the last two are read from the emulator once per
change of the screen, not once per frame.

**The preview of an agent in a volee host is its live screen.** The
interface follows the selected agent's host with a connection that only
watches, opened on the tick after the selection settles, so that moving
through the list opens no connection per row. That one connection serves
the preview, the full-screen view and edit mode. A host that does not
answer is not asked again until the selection changes. Agents volee does
not host keep the preview the daemon reads.

**The mouse reaches an opened agent.** In edit mode, a program that turned
mouse reporting on gets clicks, drags, moves and the wheel on its own
cells, encoded in the format it asked for. The interface encodes them with
its own copy of the agent's screen, which knows the modes the program set,
and sends the bytes as input: the host protocol does not change, and hosts
from earlier versions take them. A click on the frame around the screen is
volee's and goes nowhere. For a program that did not ask for the mouse,
the wheel scrolls back through what it wrote, as in a terminal.

**The wall is a view of the interface, on every agent that runs.** `5` or
`space w` opens it over the whole interface but the hint line. It holds
the agents of the list that run (the filter applies), wherever they run:

| Where the agent runs | Its tile |
| --- | --- |
| A volee host | followed live, on a connection that only watches |
| Anywhere else | the screen the daemon reads, as for the preview: again every five seconds while the agent works, and once when its state changes |

- A tile shows the bottom of the screen, cut on the right, under the
  agent's name and state. It never takes an agent's input and never
  resizes its terminal: no view ([ADR 0027](0027-open-without-owning.md))
  is opened for a tile.
- A page holds as many tiles as the terminal shows at a readable size, at
  least 40 columns by 14 rows each: two in an 80 by 24 terminal, nine in a
  large one, and a fourth row or column only where each tile keeps 72
  columns or 20 rows.
- The wall's selection is the list's. `h`, `j`, `k`, `l` and the arrows
  move it, `gg` and `G` go to the first and last screen, a click selects a
  tile. `enter`, `i`, `a` or a double click open the selected agent,
  `z` shows its screen full size, exactly as from the list.
- What is opened from the wall comes back to the wall. While something
  covers it, the wall follows nothing: its connections close and open
  again when it shows.
- `q` or `esc` leaves the wall, on the agent it was on.

`volee host wall` stays what it was: the wall of the volee hosts of the
machine, whatever they run, from a shell, without the daemon.

## Consequences

- A screen reads the same in the preview, full size, on the wall and in
  the host: the same emulator, fed the same bytes.
- The wall is useful on a machine where volee hosts nothing: it shows the
  sessions that run in the background and in multiplexer panes.
- A tile of an agent volee does not host is up to five seconds late, and a
  page of such tiles costs a read per tile every five seconds while those
  agents work (`claude logs` for a background session, `capture-pane` for a
  pane). An exact, live tile would need a client on the session, which
  resizes it ([ADR 0027](0027-open-without-owning.md)).
- A tile shows the bottom of a screen: with a status line of several rows
  under the prompt, a tile of fourteen rows shows a few rows of the
  conversation. Which rows of a screen matter is a harness's business, and
  belongs with the harness profiles
  ([ADR 0022](0022-harness-profiles.md)).
- The interface holds one connection per live tile of the page shown, at
  most sixteen, and one for the selected agent outside the wall.
- The mouse works in edit mode for the programs that ask for it; for the
  others, the wheel reads back.
- Building volee needs Zig for every crate that draws a screen, the
  interface included: there is no emulator in pure Rust left to fall back
  on.

## Alternatives considered

- **Keep the wall on volee hosts only.** It contradicts
  [ADR 0027](0027-open-without-owning.md): the wall would show what volee
  owns, on machines where it owns little.
- **A view per tile**, for exact and live tiles of sessions volee does not
  host. Each view is a client of the session: `claude attach` resizes a
  background session to the tile and starts a stopped one again. Looking
  must not change what is looked at.
- **The wall as a fifth tab**, inside the view panel. A third of the
  terminal for nine screens leaves tiles nobody can read.
- **A mouse frame in the host protocol**, encoded by the host as keys are
  ([ADR 0026](0026-edit-mode-types-into-an-agent-inside-volee.md)). The
  interface already holds a copy of the screen with its modes; bytes work
  with every host that runs today, and keys stay named because the hosts
  of earlier versions cannot be asked to encode what they never knew.
- **Skip an agent interface's prompt and status lines in a tile**, to show
  more of the conversation in small tiles. It needs to know each harness's
  screen; a wrong guess hides a question.
