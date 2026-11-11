# 0028. One emulator for every screen, and the wall inside the interface

- Status: Accepted (completes [0020](0020-libghostty-vt-emulates-terminals.md); amends [0021](0021-take-over-watch-and-the-wall-of-screens.md), [0026](0026-edit-mode-types-into-an-agent-inside-volee.md) and [0027](0027-open-without-owning.md))
- Date: 2026-11-11; corrected the same day by [its proof](#the-proof-and-what-it-found)

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

Fitting never changes a cell. A view that crops shows the last rows of the
screen as they are, down to the last one that shows something, and each
row down to its last cell that shows something. A cell shows nothing only
when it is a blank with no background and no attribute that draws on a
blank: reverse video, an underline and a strike do, and a blank in reverse
video is how an agent's interface draws its cursor.

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
| Anywhere else | the screen the daemon reads, as for the preview: again every five seconds while the agent works, every fifteen seconds while it rests, and at once when its state changes |

- A tile shows the bottom of the screen, cut on the right, under the
  agent's name and state. It never takes an agent's input and never
  resizes its terminal: no view ([ADR 0027](0027-open-without-owning.md))
  is opened for a tile.
- An agent whose screen the daemon cannot read (a session in a terminal
  that is neither a multiplexer pane nor a background session) keeps its
  tile, which says why and shows the facts of the detail panel. It is
  asked for again when the agent's state or place changes.
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

- A screen volee holds reads the same in the preview, full size, on the
  wall and in the host, cell for cell: [the proof](#the-proof-and-what-it-found)
  says how far that goes, and where a screen is only a replay.
- The wall is useful on a machine where volee hosts nothing: it shows the
  sessions that run in the background and in multiplexer panes.
- A tile of an agent volee does not host is up to five seconds late while
  its agent works and up to fifteen while it rests, and a page of such
  tiles costs a read per tile at that pace (`claude logs` for a background
  session, `capture-pane` for a pane). A live tile would need a client on
  the session, which resizes it ([ADR 0027](0027-open-without-owning.md)).
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

## The proof, and what it found

The plan that set this component asked for one proof before any release:
the same screen, compared cell by cell between the preview, the full-screen
view and the wall. Version 0.12.0 was released without it. Run the same
day, it found two differences, both older than this decision and both kept
by it:

- A blank in reverse video at the end of a row was not drawn by the views
  that crop: they left out whatever a row ended with that was blank and
  had no background. The preview and the wall lost an agent's cursor. For
  a session volee does not host, all three views lost it, which a
  comparison of the views with one another cannot see: each view is also
  compared with the emulator's own cell.
- A run of blank rows was shown as one row by the preview and the wall,
  for every screen. That repairs a replay, which leaves blank what it never
  redrew. On a screen volee holds, it moved rows.

Both are fixed in 0.12.1, and the proof stays in the repository:

- `crates/volee-tui/src/same_screen.rs` draws the three views of one
  screen, for an agent in a volee host and for a session read through the
  source (since 0.12.2 twice for the latter: a multiplexer's capture,
  which is a screen, and a replay), and compares every cell a view shows
  (character, colors, attributes) with the emulator's cell and with the
  same cell of the two other views. Its screen holds what made the views
  differ and what could: wide characters on a view's edge, a background to
  the end of a row, a gap of blank rows, a cursor block.
- `journeys/wall.py` makes the comparison by keyboard on real agents, as
  far as a capture of the screen tells (the characters, not the colors),
  and counts it only when the screen did not change meanwhile.
  `journeys/pane.py` makes it for a session in a tuios pane, against the
  pane's own screen as tuios gives it, which volee has no part in.

What the proof establishes depends on where a screen comes from:

| Where the agent runs | What the three views show | Is it the agent's screen? |
| --- | --- | --- |
| A volee host | the host's screen, row for row | yes, and live |
| A view open on the session | the view's screen, row for row | yes, while the view is open |
| A multiplexer pane | what the multiplexer captured, row for row (0.12.1 read it as a replay: a run of blank rows showed as one) | yes, as of the last reading |
| A Claude Code background session, not open | the replay of `claude logs`: every row of it full size and in `volee logs`; in the preview and on the wall, a run of three blank rows or more as one row that says how many it stands for (until 0.12.3: as one blank row, in every view, without a word) | its cells and its rows, as far as measured; what a view that crops folds, it says |

The plan's words were "an exact preview everywhere", with the size
guessing and the blank-row repair gone. In 0.12.1 the last two rows did
not meet them, and for both the reason was in volee, not in what it reads.

A pane's capture is exact, and 0.12.1 read it with a replay's repairs:
the daemon did not say which kind of reading it returned. Since 0.12.2 it
does. `agents.preview` says whether what it returns is the screen itself
(a volee host's snapshot, a multiplexer's capture of a pane) or the replay
of a session's output, and the interface keeps a screen row for row: the
preview, the wall, the full-screen view and `volee logs` show a pane's
blank rows as the pane holds them.

The daemon also gives the size of a host's terminal, which a snapshot
needs. A snapshot redraws its screen in a terminal of that size only, and
the interface told the size from what the snapshot wrote, fifty rows at
least: the snapshot of a host taller than that lost its first rows, and
that of a shorter host, which carries the host's history for a client
that scrolls back, showed rows of that history above the screen
(`volee logs`, and the preview until its live connection opened). On a
real machine, `volee logs` printed fifty rows for an agent whose terminal
holds forty-five. A capture needs no size: each of its rows is placed
with a cursor move, so it draws the same cells in any terminal large
enough.

The last row stayed open until 0.12.3.

For a background session, this decision first said that an exact screen
needs a client on the session. That was written before it was measured,
and it is wrong. Measured on 2026-11-11 with Claude Code 2.1.322, on two
throwaway background sessions, each read three ways in a terminal of 200
by 50, the size Claude Code gives a background session: its real screen
through a client (`claude attach` in a volee host), the raw replay of
`claude logs` in a volee host, and volee's preview.

| Session | Raw replay against the real screen | What volee's preview leaves out |
| --- | --- | --- |
| Fresh: one short turn | the same on 50 rows of 50 once a client has attached; on 49 of 50 read before any client | 32 blank rows between the conversation and the prompt |
| One turn that wrote 400 lines | the same on 50 of 50 after a client; on 49 of 50 before | nothing: the screen holds no run of blank rows |

The row that differs before a client attaches is one that attaching
changes: in one session a hint that the session then shows, in the other a
row that held something past its hundredth column and then held nothing.
The only way to see a session's screen is a client, and a client makes the
session draw again, so a replay cannot be compared with the screen of a
session nobody looks at more closely than that.

So the replay is the screen, to that extent, with no client. What volee
changed was its own: it counted a run of blank rows as one, in every view
and without a word, and it tells the size from what the replay writes. A
background session draws its conversation at the top of its terminal and
its prompt at the bottom: shown as it is, the last rows of its screen can
be blank rows and the prompt, and a small view then shows nothing of what
the agent said. Counting the blank rows as one keeps the end of the
conversation in view, and is not the screen.

Two real background sessions, read the same day through `claude logs`,
give the measure of it. One, an hour old and at rest, never has two blank
rows in a row: counting a run as one changes nothing. The other, hours old
and at work, holds twelve blank rows between the last line of its
conversation and its footer: shown as it is, a preview of twelve rows
would hold blank rows and the footer, and nothing of what the agent is
doing. Both terminals are some 210 columns wide, not 200: a session keeps
the width of its last client, so the size cannot be assumed either.

### The fold, said

Since 0.12.3:

- A replay keeps its rows as they are. The full-screen view and
  `volee logs` show every one of them.
- The views that crop, the preview and a tile of the wall, show a run of
  three blank rows or more as one row, in volee's own dim style, which says
  how many rows it stands for: `⋯ 12 blank rows ⋯`. One or two blank rows
  are how an interface spaces what it writes, and stay as they are.
- A screen itself (a volee host, a view, a multiplexer's capture) never
  folds: its views show it row for row.

What a view shows of a replay is then the screen's own cells, and what it
leaves out, it says. The size of the replay's terminal is still told from
what the replay writes: `claude logs` does not say it, and the two
sessions above show that it varies.

This was decided on 2026-11-11 while the project's owner was away, on a
question put to him and not answered yet. It is the smallest change that
hides nothing, and it is his to overturn: showing a replay's rows as they
are in every view means removing the fold, and a live, exact preview of a
session that is not open needs a client on it, which resizes it
([ADR 0027](0027-open-without-owning.md)).

Not measured: the replay of a session that has run for hours against its
screen through a client, which would resize a session someone works in.
Earlier versions showed gaps there in what had been drawn long before.

### A screen at rest

Comparing a pane's own screen with volee's views by keyboard found what a
comparison of cells at one moment cannot: a screen read through the daemon
was read again only while its agent worked (the preview), or while it
worked and once when its state changed (a tile). At rest it went stale.
After a command typed in a pane, the preview still showed the screen of
the session's start, until the selection moved.

An agent at rest still has a screen that changes: a person types in its
pane, the last lines of a turn land after the reading its end asked for,
a turn runs that is too short for volee to see as work, and Claude Code
itself takes a hint away from its last row some twenty seconds after a
turn (measured on background sessions). Since 0.12.2 the
preview reads the screen of a selected agent at rest again every five
seconds, and a tile every fifteen: a page of tiles costs a read per tile,
and one read of a background session takes Claude Code about 0.6 second
(0.17 second of processor time, measured on an idle session). An agent
that ended keeps the screen it left, which nothing reads again.

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
