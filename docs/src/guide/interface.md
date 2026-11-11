# The interface

`volee` opens one screen that shows every agent on your machine and what it
needs from you. To try it without any agent running, start the demo, a
simulated team of agents that changes every few seconds:

```sh
volee demo
```

![volee demo: the hierarchy of a lead agent, two missions and their workers](../images/demo.gif)

## Starting volee

```sh
volee
```

`volee` with no command connects to the [daemon](cli.md#the-daemon),
starting it in the background when it does not run yet, and shows every
agent it knows: the agents volee launched itself, each in its volee host,
and the Claude Code sessions and OpenCode agents it finds on the machine,
wherever they were started. The daemon looks
for sessions as soon as the interface opens, then while it stays open every
two seconds after a look that found something new, slowing down to every ten
seconds while nothing changes (every thirty seconds when no interface is
open). A session started elsewhere therefore shows up within ten seconds at
worst, and changes come in quick succession after that; agents volee
launched show up at once, through their hooks. Nothing needs to be
configured first.

Everything on the screen comes from the daemon, and so do the actions:

| What | What the daemon uses |
| --- | --- |
| The agents, their states and messages | Its own records, kept up to date as hosts and sessions come and go. |
| The preview of an agent's screen | The agent's volee host, which holds its exact screen: the interface follows it live while the agent is selected. For agents volee does not host: tuios or tmux, for a session in one of their panes (`capture-pane`); Claude Code, which replays a background session's output (`claude logs`). volee's setup log, for an agent whose session has not started yet. |
| An agent's changes (`d`) | git, in the agent's worktree or folder. |
| Opening an agent (`enter`, `i`, `a`, a double click) | A terminal on the agent, which the interface connects to: the volee host it runs in, or a view the daemon starts on the session where it runs (`claude attach` on a Claude Code background session, `tuios pane-view` on a tuios pane). tmux brings forward the pane of a session in one of its panes. |
| The live full-screen view (`z`) | The agent's volee host. |
| Adopting an agent (`A`) | Claude Code, which stops the background session (`claude stop`), then a new volee host, which resumes its conversation. |
| The wall of screens (`5`) | For each agent of the list that runs: its volee host, followed live, or what the preview uses for an agent volee does not host. |
| Writing to an agent (`m`) | Its own records and the agent's hooks. |

When the daemon cannot do something, because Claude Code or git is
missing, because the agent runs in a terminal of its own, or because the
daemon is older than the interface, volee says so in a sentence where the
result would be, such as "No preview: dashboard runs in a terminal of its
own: switch to that terminal to see or control it", and goes on working.
With no agent yet, the views say how agents get there; when the daemon
cannot use Claude Code at all (missing, or too old), they say so instead,
and `volee daemon status` says why. If the daemon stops while volee runs,
the top bar shows `offline` and the screen stops changing; start `volee`
again to reconnect.

The theme, the keys and reduced motion come from the `[tui]` table of the
[configuration](configuration.md), as for the demo.

## Layout

```text
  volee  1 Hierarchy  2 Network  3 Repos  4 Timeline      ! 2 need you · ▸ 6 working · 12 agents
╭ Agents ──────────────╮╭ Hierarchy · compact ─────────────────────────────────────────────────╮
│▌v ! invoice-tools    ││              ╭─ tower ──────── ⠹ ─╮                                   │
│    needs you · mcp-… ││              ╰─────────┬──────────╯                                   │
│ s ! invoice-schema   ││          ┌─────────────┴──────────────┐                               │
│    needs you · bill… ││ ╭─ invoicing ────────╮      ╭─ latency-audit ────╮                   │
│ x ▸ explore-traces   ││ ╰┬───────────────────╯      ╰┬───────────────────╯                   │
│    …                 ││  ├─┤ ! invoice-schema        ├─┤ ▸ trace-gateway                     │
╰──────────────────────╯╰───────────────────────────────────────────────────────────────────────╯
╭ Detail ──────────────╮╭ Preview · invoice-tools ──────────────────────────────────────────────╮
│ invoice-tools        ││ ⏺ Before I go on, one question:                                       │
│ ! needs you · 1 min  ││   Should create_invoice accept a draft flag?                          │
╰──────────────────────╯╰───────────────────────────────────────────────────────────────────────╯
 h/j/k/l move   gg/G first/last   s subtree   ' jump   / filter   n/N match   1-4 views
 m message   d diff   enter open   z full screen   I messages   ␣ more   ? help
```

- **Top bar.** The views, numbered (press the number to switch; `5` opens
  the [wall of screens](#the-wall-of-screens) over the whole terminal), and
  counters: questions waiting for your answer (`?`) and unread messages
  (`✉`) when there are any, then how many agents need you, how many work,
  how many there are.
- **Agents.** Every agent, the most urgent first: those that need you, then
  failed, working, launching, done, idle, stopped and lost. Each row shows
  the agent's jump letter, its state, its name, how long it has been in that
  state, and where it works (`repo › branch`, or its mission).
- **View.** What the tab shows: the [hierarchy](#the-hierarchy) of who
  launched whom, the [network](#the-network) of who talks to whom, the
  [repositories](#the-repositories) with their worktrees and what agents
  delivered, or the [timeline](#the-timeline) of what happened when. The
  selected agent is the same in every view and in the list.
- **Detail.** Facts about the selected agent: role, state and since when,
  its mission and the mission supervisor's verdict on its work, repository
  and branch, worktree, where it runs (`volee host`, `background`, a pane
  such as `tuios · work`, or `a terminal`) and with which model, whether it
  is held (you stopped it, or its mission is over: volee will not resume it
  on its own), delivery evidence, last activity and session.
- **Preview.** The selected agent's terminal screen, kept up to date. For
  an agent volee does not host, volee reads it again every two seconds
  while the agent works and every five seconds while it rests: a screen
  changes at rest too, when you type in its pane or when the last lines of
  a turn land late. An agent that ended keeps the screen it left. For an
  agent in a volee host, it is the exact screen the host holds, followed
  live, status line and prompt frame included, drawn at the agent's own
  size: looking at an agent never resizes it. The preview, the full-screen
  view and the agent's tile on the wall show the same cells of that screen,
  its cursor and its blank rows included. The bottom of the screen,
  where the latest output is, stays in view; scroll it with `j` and `k` when
  the panel has the keyboard. Lines wider than the panel are cut at its
  edge. `z` shows that screen full size, its
  whole conversation in view, from its newest lines: lines wider than the
  window go on over several lines, between words where they can, so that
  nothing is cut but the padding and frames the agent's interface draws to
  its own edge. For an agent in a volee host, that view is live: volee
  watches the host and draws each change as it comes. `j` and `k` scroll
  it, `enter` types into the agent ([edit mode](#edit-mode-typing-into-an-agent)),
  and `z` or `esc` comes back. For a session
  in a tuios or tmux pane, the screen is the pane's, row for row, as its
  multiplexer captures it. A Claude Code
  background session that volee does not host (one that volee 0.7
  launched, or a `claude --bg` you started by hand) is replayed from
  `claude logs`, which keeps only the session's recent output: in a session
  that has run for hours, parts of the screen drawn long ago and never
  redrawn since, such as a list that has not changed, can show gaps. A
  recent session shows whole. A background session draws its conversation
  at the top of its terminal and its prompt at the bottom: so that the end
  of the conversation stays in view, the preview and the wall show a run
  of three blank rows or more as one dim row that says how many it stands
  for (`⋯ 12 blank rows ⋯`). `z` shows every row of the screen, and
  [opening](#edit-mode-typing-into-an-agent) the session (`enter`) shows
  it live.
- **Hints.** The keys that apply to the panel that has the keyboard, on two
  rows from 20 rows up: the top one says how to move around (moves, first
  and last, the filter and its matches, panels and views), the bottom one
  what to do (message, diff, open, the leader, commands, quit),
  with `? help` at the right. Smaller terminals and the windows keep one
  row. They come from the same table as the help (`?`), so they are always
  right.

Panel sizes depend only on the size of the terminal and on the view: moving
around never makes anything jump. Below 100 columns or 28 rows, the view
makes room. On the hierarchy tab, the agents panel shows the hierarchy as an
indented tree, next to the detail and the preview; the network, the
repositories and the timeline take the right side of the screen, next to
the agents and the detail. The smallest supported terminal is 80x24.

## States

Every state has a glyph and a word, so you can read it without colors. A
spinner turns on the cards of working agents, as an extra, unless
`reduced_motion` is set.

| Glyph | Word | Meaning |
| --- | --- | --- |
| `!` | needs you | Waits for a person: a permission, a question or a dialog. |
| `▸` | working | Running a turn. |
| `…` | launching | Started by volee, not seen running yet. |
| `✓` | done | Finished its last task. |
| `·` | idle | Alive, waiting for its next instruction. |
| `✗` | failed | Ended with an error. |
| `■` | stopped | Its process stopped; the conversation can be resumed. |
| `∅` | lost | volee cannot find it any more. Never treated as done. |

## The hierarchy

The hierarchy shows who launched whom. Lead agents sit at the top, then the
coordinators of their missions, then the workers. Sessions volee did not
launch come last, on their own. Subagents hang under their parent's card as
one-line satellites. The path from the selected card up to its root is
highlighted.

A full card shows, from top to bottom: the name, the role and a spinner on
its border; the state, for how long, and a sparkline of the last minutes of
activity; the repository and branch (the mission, for a coordinator); what
the agent does or waits for; and its delivery (`3 commits · PR #42 ✓`), its
mission's progress for a coordinator, or its scope for a lead.

When a mission has more workers than the width allows, they are stacked in
a column under their coordinator, so the tree stays readable instead of
scrolling.

### Zoom

The hierarchy has three levels of detail: full **cards**, **compact** cards
with only the state, and **dots** with a glyph and a name. volee picks the
most detailed level that fits the panel; `+` and `_` move to more or less
detail and keep it, `0` lets volee choose again. When the tree is larger
than the panel, it scrolls to keep the selection in view, arrows on the
border tell where it goes on, and the mouse wheel scrolls it (with Shift,
sideways).

`Enter` shows only the selected agent and its descendants, with more room
for each card; `Esc` goes back to the whole hierarchy.

## The network

![The network of the demo: agents grouped by who talks to whom, a message on its way](../images/network.gif)

The network shows who talks to whom. Every agent is a node, whatever its
harness (Claude Code, Codex CLI...) or its mission, and whether volee
launched it or not; you and volee itself, the mission supervisor, join as
soon as they take part in a conversation.

- A **line** joins two parties that exchanged messages, in either
  direction. It is thicker when they talked a lot lately: each message
  counts less as it ages.
- A **faint dashed line** joins an agent to the agent that launched it.
- A question that **waits for its answer** is drawn in the question color,
  with a `?` next to the one who must answer. The title counts them.
- When a message is posted, a **pulse** `●` travels along its line from the
  sender to each recipient, in about a second.
- The selected agent's lines are highlighted and its neighbours' names are
  in bold. The line under the graph tells which questions it waits for or
  owes, who it talks with, who launched it and whom it launched.

Lines follow the routes volee allows
([ADR 0006](../adr/0006-messaging.md)): agents of a mission write to each
other freely; otherwise an agent writes to its parent, its children, its
mission's coordinator or the lead who asked for the mission, and to you. So
a conversation between two missions goes through their coordinators and
the lead, or through you, and a session volee did not launch talks with
you: the picture shows who relays what.

Agents that talk end up close together, whatever their mission: positions
come from a force-directed layout (linked agents attract each other, all
agents push each other away). It is computed only when an agent or a new
pair of correspondents appears, never while you watch, and it starts from
the previous positions, so a newcomer moves the others little. Agents that
do not match the filter are dimmed.

Like the hierarchy, the network has three levels of detail: **cards** with
the name and the state, **compact** labels with the state's glyph and the
name, and **dots** with the glyph and the agent's jump letter, the one in
the list, so `'` and that letter selects it. In dots, the selected agent's
name is written next to its dot. volee picks the most detailed level whose
nodes fit without overlapping; `+`, `_` and `0` work as in the hierarchy.
`h`, `j`, `k` and `l` move to the nearest agent in that direction, `g` and
`G` to the top-left and the bottom-right ones.

With `reduced_motion = true` in the [configuration](configuration.md#tui),
a new message lights up its whole line for a moment instead of travelling
along it, and spinners stop.

## The repositories

![The repositories of the demo: worktrees, delivery evidence, verdicts, and what is safe to tidy](../images/repos.gif)

The repositories view shows where agents work and what they delivered.
Repositories come first, sorted by name, then their worktrees (the main
checkout first) with the branch checked out, the branch it started from and
the worktree's directory; a repository with a single worktree takes one
line. Under each worktree come its agents, their subagents under them.
Coordinators and agents outside any repository come last.

Every agent shows its delivery evidence: commits ahead of its base, lines
added and removed, whether they are pushed, its pull request and the checks,
a merge, a delivered report, uncommitted changes. For a worker of a
mission, the supervisor's last **verdict** sits at the right (`✓ delivered`,
`! needs you`, `✗ not delivered`...); a coordinator shows its mission's
progress instead.

A worktree that [`volee tidy`](cli.md#tidying-worktrees) would remove is
marked **`✓ safe to tidy`**: its work landed (merged into its base, merged
through a pull request, or reported), it has no uncommitted changes, its
agent does not run, volee checked the work since the agent's last change,
and volee launched the agent in a worktree it created. It is the same rule,
from [ADR 0011](../adr/0011-safety-and-delivery-evidence.md). Otherwise the
full level says why the worktree is kept, such as `keep: busy, not landed`
or `keep: not launched by volee`. The view only tells; it removes nothing.
The title counts the worktrees that are safe to tidy.

Three levels of detail: **full** (a line for the agent, a line for its
evidence), **compact** (one line per agent, the evidence after the name)
and **names**. volee shows the evidence in full when it fits, on one line
otherwise; `+`, `_` and `0` change the level and give it back to volee.
`j` and `k` move through the agents in the view's order, `h` and `l` jump
to the previous or next repository.

## The timeline

![The timeline of the demo: a lane per agent, the cursor going back to a question](../images/timeline.gif)

The timeline shows what happened when. Each agent has a lane, in the
hierarchy's order so lanes never jump around, over the last half hour by
default. The present is at the right edge, and the picture slides left as
time passes, one column at a time.

A lane draws the agent's state as a bar: heavy while it works (`━`) or
waits for you (`━` in the color of that state), thin while it is idle or
done (`─`), dotted once it stopped (`┄`). Marks tell what happened; when
several fall in the same column, the one a person most needs to see wins:

| Mark | Meaning |
| --- | --- |
| `!` | The agent started to need you. |
| `✓`, `✗` | It finished, or failed. |
| `?` | It asked a question. |
| `◆`, `◇` | It sent a message, or received one. |
| `●` | Its delivery moved: commits, push, pull request, checks, merge, report. |
| `◉` | Its mission's supervisor judged its work. |

A **cursor** scrubs through time. `h` and `l` move it one column back or
forward, `[` and `]` jump to the selected agent's previous or next event,
and a click on a lane moves it there. The line under the lanes reads the
cursor: when, the selected agent's state then, and what it did at that
moment (the question it asked, the answer it received, the pull request
that merged...). Going back further than the lanes reach slides the window
back with the cursor; `Esc` brings it to the present again. `+` and `_`
make the time scale finer or coarser, from a second to an hour per column;
`0` comes back to the present and the default scale.

## The wall of screens

`5` (or `Space w`) opens the wall: the agents of the current list that run,
each as a tile of its screen, over the whole terminal. The filter (`/`)
applies, so filter first to watch one mission or one repository.

```text
 Wall · 5 agents run
┏ ! invoice-tools · needs you ━━━━━━┓╭ ▸ invoicing · working · live ────╮
┃ ⏺ Before I go on, one question:   ┃│ ⏺ The tests pass: 356 of 356.    │
┃   Should create_invoice accept a  ┃│                                  │
┃   draft flag?                     ┃│ ❯ commit and push                │
┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛╰──────────────────────────────────╯
 h/j/k/l move   z full screen   enter open   q back
```

Every agent that runs has its tile, wherever it runs:

- an agent in a volee host is followed **live** (its title says so);
- any other agent (a Claude Code background session, a session in a tuios
  or tmux pane) shows the screen volee reads for its preview, read again
  every five seconds while the agent works, every fifteen seconds while it
  rests, and at once when its state changes.

A tile shows the bottom of its agent's screen, where the prompt and the
questions are, cut on the right when the tile is narrower, under the
agent's name and state. The wall only looks: it never takes an agent's
input and never resizes its terminal.

A session in a terminal volee cannot read (Claude Code started by hand in
a terminal window, outside tuios and tmux) has no screen to show. Its tile
says so, then what volee knows of the agent: where it works, where it
runs, what it did last. Opening it says how to make it reachable.

A page holds as many tiles as your terminal shows at a readable size (at
least 40 columns by 14 rows each): two in an 80 by 24 terminal, nine in a
large one, up to sixteen in a very large one. Moving past the last tile of
a page shows the next page, and the first row says which page this is.

The wall's selection is the list's: the selected tile has a thick border,
and leaving the wall leaves the list on that agent.

| Keys | What it does |
| --- | --- |
| arrows, `h`, `j`, `k`, `l` | move between tiles |
| `g g`, `G` | the first and the last screen |
| `enter`, `i`, `a`, a double click | open the selected agent, as from the list ([edit mode](#edit-mode-typing-into-an-agent)); leaving it comes back to the wall |
| `z` | show the selected screen full size; `z` or `esc` comes back to the wall |
| a click | select a tile |
| `q`, `esc` | come back to the interface |

These are the defaults of the `wall` context: `[tui.keys.wall]` changes
them, like any other key ([Keys](#keys)).

From a shell, `volee host wall` shows every running volee host of the
machine as a wall of its own, whatever the hosts run and without the
daemon ([hosts](cli.md#hosts)).

## Moving around

- `j`, `k` and the arrows move through the list; `gg` and `G` go to its
  first and last agent, `Ctrl+d` and `Ctrl+u` half a page down and up. In the hierarchy, `h`,
  `j`, `k` and `l` move through the tree as drawn: down to satellites and
  children, up to the parent, sideways to the nearest card. In the network
  they move to the nearest agent on screen; in the repositories and the
  timeline, `j` and `k` follow the view's order.
- Every agent has a letter, shown in the list. Press `'` then the letter
  to jump to it, from anywhere: two keys to reach any agent.
- `-` goes back to the agent selected before, and again to return.
- `/` filters by name, repository, branch or mission, as you type. The
  hierarchy and the network dim the agents that do not match; the list,
  the repositories and the timeline hide them. `Enter` keeps the filter,
  `Esc` clears it. With a filter kept, `n` and `N` go to the next and
  previous agent that matches, round the list.
- `1` to `4` show a view; `H` and `L` the previous and the next one.
- `Tab` moves the keyboard to the next panel. Clicking a row, a card, a
  node, a lane or a tab selects it; a second click on an agent, soon after,
  opens it.

`Esc` closes or leaves one thing at a time (a popup, the timeline's
cursor, the subtree, the filter, then the panel), and never quits volee.

## Writing to an agent

`m` opens the composer on the selected agent. Write your message (Markdown
is fine), choose its kind with `Tab` (`note`, `question`, `answer`,
`contract` or `alert`) and send it with `Enter`. `Alt+Enter` or `Ctrl+J`
starts a new line, and pasting several lines keeps them.

When the agent asked you a question that you have not answered, the
composer opens as the answer to it and shows the question; volee then links
the two, so the question stops waiting. A message reaches an agent volee
launched at the end of its turn, and wakes it when it is idle. Sessions
started outside volee cannot receive messages yet; the composer says so.

`Esc` closes the composer and keeps what you wrote: the hint line shows
`✎ draft to <agent>`, and `m` brings it back, addressed to the agent
selected then. Quitting with a draft asks first.

## Messages

`I` (or `Space i`) opens your messages. Questions agents asked you and that you have not
answered come first, pinned until an answer refers to them; then every
thread you take part in, the latest activity first: mission threads,
threads with agents, and volee's own alerts. A `●` marks threads with
unread messages; the top bar counts both.

The selected thread shows on the right. `Enter` reads it: the keyboard
moves into the thread (`j` and `k` scroll it) and its messages are marked
read; `Esc` comes back to the list, then closes the window.

`r` replies in the selected thread: to a pinned question, the composer
opens as its answer, linked to it, so the question stops waiting;
elsewhere it writes to the last agent that spoke there (or the mission's
coordinator). Sending or closing the composer comes back to the messages.
volee's alerts take no reply.

`s` asks for a summary of the thread, written by a model. Summaries are
off unless you turn them on (`summaries = true` under `[messages]` in the
[configuration](configuration.md)); until then, the window says so.

## Changes

`d` shows the selected agent's changes against the branch it started
from: first a summary (`git diff --stat`), then the full patch with `f`. `j`
and `k` scroll, `r` loads them again, and `Esc` or `d` closes the window. A
very long patch stops at a line that says so; the rest is in the agent's
worktree.

## Your own tools

`o` opens the selected agent's worktree in another program, from the
agents or from the Changes window: `e` your editor, `d` your diff viewer,
`g` your git interface, `w` an editor with a window of its own. None is
required: volee offers what it finds (`$VISUAL` or `$EDITOR`, Neovim with
diffview.nvim, lazygit, VS Code) and what `[tui.tools]` names in the
[configuration](configuration.md#tuitools), and says how to add the
others. Terminal programs take over the terminal and the interface comes
back when they exit; VS Code opens its own window and volee stays.

## Launching agents and missions

`Space n` launches an agent in a new worktree: choose the repository (from
`[repos]` in the configuration, or type the path of a checkout when there
is none), name the new branch, write the task (its first prompt) and,
optionally, the model and the harness (`claude-code`, the default, or
`opencode`). `Space N` creates a mission: a title, its kind (`change`
delivers branches and pull requests, `analysis` reports), its repositories
and its goal; volee launches its coordinator.

`Tab` moves between fields, `←` and `→` choose, `space` picks a
repository, and `Enter` starts a new line in the task or the goal (it moves
to the next field from the others). `Ctrl+S` launches, from any field:
`Enter` never does, so a task you are still writing cannot start an agent.
If your terminal or multiplexer keeps `Ctrl+S` for itself (Zellij's scroll
mode does), move `launch` to another key under `[tui.keys]`. When volee
refuses (a repository Claude Code does not trust yet, a branch that
already exists...), the dialog stays open and says why, so you can fix it
and launch again. The new agent is selected as soon as it appears.

## Edit mode: typing into an agent

`enter` opens the selected agent and types into it without leaving volee
(`i`, `a` and a double click do the same). The full-screen view opens on
the agent's live screen, its frame takes the input colour, and its title
and the bottom line say **EDIT** and whose keys these are now:

```text
╭  EDIT  Screen · invoicing ─────────────────────────────────────────────╮
│ ⏺ The tests pass: 356 of 356.                                          │
│                                                                        │
│ ❯ commit and push                                                      │
╰────────────────────────────────────────────────────────────────────────╯
  EDIT   your keys go to invoicing   ctrl-q back to volee
```

Every key goes to the agent, `Esc`, `q` and `Ctrl+C` included, and so does
what you paste, in one piece (as a bracketed paste when the agent turned it
on). `Ctrl+Q` gives the keys back to volee: the screen stays, live, `i`
types again, and `z` or `esc` comes back to the list. `Ctrl+Q` never
reaches the agent; to use another key, set `leave` in
`[tui.keys.attached]` ([Keys](#keys)).

The mouse goes to the agent too, when its program asked for it (Claude Code
does): clicks, drags and the wheel land on the agent's own cells. For a
program that did not ask, the wheel scrolls back through what it wrote,
as it does in the full-screen view outside edit mode. A click on the frame
around the screen does nothing.

In edit mode the agent's terminal takes the size of the panel, so that
its screen is drawn cell for cell, cursor included, and follows when your
terminal changes size; it keeps that size when you leave. Only edit mode
and `volee attach` set an agent's size: the preview, the full-screen view
and the wall never resize it. For an agent in a volee host, the three show
the same live screen as edit mode: the preview follows the selected agent
as it writes, and its title says `live`. volee encodes your keys the way the agent
asked (the kitty keyboard protocol Claude Code turns on, or the usual
encoding), and `Ctrl+Z`, which suspends Claude Code, is followed by its
resumption: in a volee host there is no shell to bring it back.

`volee attach <agent>`, from a shell, takes an agent over in the whole
terminal instead; `Ctrl+Q` comes back there too. Taking an agent over in
one place ends edit mode in another: volee says so and watches the agent
again.

### Opening a session volee did not start

A session started outside volee opens the same way, where it runs, through
the door it already has
([ADR 0027](../adr/0027-open-without-owning.md)):

| Where it runs | What `enter` does |
| --- | --- |
| A Claude Code background session, running or finished | shows it with Claude Code's own `claude attach`, in a terminal inside volee |
| A tuios pane | shows that pane, with `tuios pane-view` |
| A tmux pane | brings the pane forward (below) |
| A terminal volee cannot reach | says so, on the bottom line |

Nothing is stopped, restarted or adopted for it: it is the same session, in
the same process, with its own tools and settings. `Ctrl+Q` closes the
terminal volee opened and comes back to the list, and the bottom line says
that the agent keeps running. While it is open, the session takes the size
of volee's panel; a tuios pane goes back to its own size when you leave.

A finished background session is still in the list, as in Claude Code's
own agents view: opening it starts it again, under the same session, as
`claude attach` does. Leaving the terminal the way Claude Code offers
(`Ctrl+Z`) closes it too, and the left arrow on the session's empty prompt
shows Claude Code's own agents view there, as in any terminal.

A Claude Code session in a terminal volee cannot reach becomes reachable
when you press the left arrow on its empty prompt, in that terminal:
Claude Code moves it to the background, where volee opens it.

Agents volee launched before 0.10 run in older hosts, which outlive an
upgrade: edit mode sends them the bytes a terminal sends, so typing works,
but a key only a keyboard protocol tells apart, such as `Shift+Enter`,
arrives as its plain form, and `Ctrl+Z` is not sent at all, since nothing
in such a host would resume Claude Code. Such an agent gets a new host the
next time it is stopped and resumed.

### Adopting an agent

Opening a session needs no adoption. Adopt one when you want volee to
drive it from now on, like the agents it launches: messages reach it,
`volee stop` stops it, a message resumes it, and it comes back after a
reboot.

`A` moves the selected agent's conversation into a volee host, as
`volee adopt` does. It asks first, on the bottom line, because it changes
the session: volee stops it (`claude stop`), so that two programs never
write the same conversation, then resumes the conversation in a new host,
in the same directory, with volee's settings in place of the session's own
(its hooks, its MCP servers, its permission mode). `y` adopts and opens the
agent as soon as its host runs; `n` or `Esc` leaves it alone. It works on
Claude Code background sessions, on agents whose session ended, and on
OpenCode agents started with `--session`. An agent that runs in a terminal
of its own must be quit there first: volee refuses it until then and says
so on the bottom line. The agent keeps its name and its history.

The conversation resumes in the agent's worktree. In a repository Claude
Code does not trust yet, it asks first, in the host: open the agent
(`enter`) to answer, once for the repository. When the resumed program fails, the agent is
`stopped` and its detail quotes the program's last line (Claude Code's
trust message, for example). The conversation is not lost: fix the cause
and send the agent a message, or resume it in the background yourself,
and volee shows it there again, ready to adopt.

### Agents in tuios panes

A Claude Code session you started by hand in a
[tuios](https://github.com/Gaurav-Gosain/tuios) pane is found in that pane
when tuios is installed and its server runs: the daemon asks tuios each
time it looks for sessions, and the Detail panel says where it runs
(`tuios · work`: tuios, and the session that holds the pane). Its preview
is the pane's screen, and `enter` opens the pane inside volee: the daemon
runs `tuios pane-view` on it, and what you type goes to the pane. While no
tuios window shows the pane's session, the pane takes the size of volee's
panel, and goes back to its own when you leave with `Ctrl+Q`. No pane is
opened, closed or moved, and the session in it is none the wiser.

The session shows in volee ten seconds after it starts, like any session
started outside volee. `/clear` in the pane starts a new session in the
same process: its row stays the same row.

An agent of another AI in a tuios pane is shown and opened the same way.
OpenCode is found in the process table, then in its pane. For a harness
volee has no list of its own for (Codex CLI, Gemini CLI, Aider), tuios
itself tells volee: it recognizes the program in the foreground of the
pane, and volee lists that pane as an agent
([Agents of other harnesses](other-harnesses.md#in-a-tuios-pane)).

volee uses the default tuios server, or the one `TUIOS_SERVER` names when
you run `volee daemon run` yourself. The commands that bring a pane forward
name that server, so they reach the pane even when volee runs in a pane of
another tuios server; the command volee shows then names it too, such as
`tuios --server lab attach work`.

tuios's own Claude Code hooks (`tuios integration install claude-code`)
tell volee which session runs in which pane; without them, volee follows
the session's process to its pane. Nothing is required: without tuios,
these sessions are listed as before, and `volee daemon status` says
whether volee found tuios
([ADR 0015](../adr/0015-tuios-location-adapter.md)).

### Agents in tmux panes

A Claude Code session you started by hand in a [tmux](https://github.com/tmux/tmux)
pane is found the same way, when tmux is installed and its server runs: volee
follows the session's process to the pane that runs it. The Detail panel
says `tmux · work`, the preview is the pane's screen, and `enter` makes the pane
current in its window and session, then switches your tmux client to it when
volee runs in a client of that same tmux server. From anywhere else (outside
tmux, or in a client of another server), volee switches no client, not even
one open in another terminal: it says how to see the pane instead,
`tmux attach -t =<session>` (the `=` asks for that exact session).

The daemon uses the default tmux server, or the one `TMUX` names when you
run `volee daemon run` yourself inside tmux, and every command it runs or
shows names that server. Zellij is not supported yet.

## Forgetting an agent

`F` forgets the selected agent, after asking on the bottom line: volee stops
listing it and ignores its sessions from then on, as `volee forget` does.
Nothing is stopped or deleted: its sessions, worktree and branch stay as
they are. volee refuses an agent it launched that still runs (stop it first
with `volee stop`) and an agent of an open mission, and the bottom line says
which.

## Keys

The keys follow the habits of vim and of terminal tools such as lazygit:
`h`, `j`, `k`, `l` and the arrows move, `gg` and `G` go to the ends,
`Ctrl+d` and `Ctrl+u` move half a page, `/` filters and `n` / `N` go
through the matches, `?` shows the keys, `:` runs a command by name, `Esc`
goes back and `q` quits. `q` quits at once, unless a message you started
would be lost: then volee asks first.

**The keys of this screen.** `?` opens a small popup at the bottom right
with the keys of the panel you are in and of everywhere, in columns, with
nothing to scroll. Press one of them and the popup closes and does it;
`?` again lists every key of every context; `Esc` closes it.

**The leader.** Less frequent actions sit behind the space bar, as in
LazyVim: press `Space` and a popup at the bottom right shows what can
follow, such as `n` for a new agent, `N` for a new mission, `t` for the
theme, `w` for the wall or `k` for every key. `Esc`, or a key that
continues nothing, drops the sequence. `gg` works the same way: after `g`,
the popup shows what can follow.

**The command palette.** `:` opens a prompt on the bottom line and lists
the commands of the current screen, the ones matching what you type first:
each action by its name and what it does, and `theme <name>` for every
theme. `Enter` runs the selected one, the arrows, `Tab` or `Ctrl+n` /
`Ctrl+p` move, `Esc` closes.

**The theme.** `Space t` (or `:theme`) opens the theme picker at the
bottom right: moving shows each theme at once, `Enter` keeps it in the
[configuration](configuration.md#tui), `Esc` goes back to the theme you
had. See [Themes](themes.md).

**Changing keys.** Every key in the tables below can be changed in the
`[tui.keys]` table of the [configuration](configuration.md#tuikeys): give
an action other keys in every context, or in one with a
`[tui.keys.<context>]` table (the context's name is in each title below),
and an empty list unbinds it. Keys are written like `q`, `ctrl-c`,
`shift-tab`, `enter` or `f5`, and sequences of up to three keys like
`g g` or `space n`:

```toml
[tui.keys]
take-over = "ctrl-a"           # opening an agent, everywhere it has a key
new-agent = ["c", "space n"]   # several keys
adopt = []                     # no key

[tui.keys.agents]
first = "home"                 # in the agents list only

[tui.keys.attached]
leave = "ctrl-g"               # leave edit mode with Ctrl+G
```

volee refuses, with the line of the configuration, two actions on the same
key where both apply, and a key that both acts and starts a longer
sequence (`g` alone would hide `gg`). The leave key (`leave`) must be
Control and a letter other than `h`, `i`, `j` or `m`: it is the only kind
of key every terminal sends the same way while an agent owns the keyboard.
Typing keys (letters, Backspace, the arrows in a text) are not shortcuts
and stay as they are. The tables are generated from the keymap, like the
popups and the hints.

<!-- keys:start -->
### Everywhere (`global`)

| Keys | What it does | Action |
| --- | --- | --- |
| `'` | jump to an agent: press ' then its letter | `jump` |
| `-` | back to the agent selected before | `jump-back` |
| `/` | filter agents by name, repository, branch or mission | `filter` |
| `n` | the next agent that matches the filter, round the list | `next-match` |
| `N` | the previous agent that matches the filter, round the list | `previous-match` |
| `m` | write a message to the selected agent | `message` |
| `d` | show the selected agent's changes | `diff` |
| `enter`, `i`, `a` | open the selected agent: its own terminal, full size inside volee, takes your keys; the leave key (ctrl-q) comes back and the agent keeps running | `take-over` |
| `z` | show the selected agent's screen full size | `full-screen` |
| `I`, `space i` | your messages, questions waiting for you first | `messages` |
| `o`, `space o` | open the selected agent's worktree in your editor, diff viewer or git tool | `open-with` |
| `A`, `space a` | move the selected agent into a volee host (asks first): its session is stopped and resumed there with volee's settings; opening it needs no adoption | `adopt` |
| `F`, `space f` | forget the selected agent: stop listing it (asks first) | `forget` |
| `space n` | launch an agent in a new worktree | `new-agent` |
| `space N` | create a mission, which launches its coordinator | `new-mission` |
| `space w`, `5` | show the screens of the agents that run, side by side, as a wall | `show-wall` |
| `space t` | pick a colour theme, with a live preview; volee keeps your choice | `pick-theme` |
| `space k` | list every key, in every context | `all-keys` |
| `:` | the command palette: type part of a command's name and press enter | `commands` |
| `tab` | focus the next panel | `next-pane` |
| `shift-tab` | focus the previous panel | `previous-pane` |
| `1` | show the hierarchy | `show-hierarchy` |
| `2` | show the network | `show-network` |
| `3` | show repositories and worktrees | `show-repos` |
| `4` | show the timeline | `show-timeline` |
| `H` | show the previous view | `previous-view` |
| `L` | show the next view | `next-view` |
| `?` | show the keys of this screen; ? again lists every key | `help` |
| `esc` | close, cancel or go back (never quits) | `back` |
| `q`, `ctrl-c` | quit volee (asks first when a draft would be lost) | `quit` |

### Agents list (`agents`)

| Keys | What it does | Action |
| --- | --- | --- |
| `h`, `←` | parent agent | `left` |
| `j`, `↓` | next agent | `down` |
| `k`, `↑` | previous agent | `up` |
| `l`, `→` | first child agent | `right` |
| `gg`, `home` | first agent | `first` |
| `G`, `end` | last agent | `last` |
| `ctrl-d`, `pgdn` | half a page of agents down | `half-page-down` |
| `ctrl-u`, `pgup` | half a page of agents up | `half-page-up` |
| `s` | show the agent's subtree in the hierarchy | `focus-subtree` |
| `+`, `=` | more detail in the view beside the list | `zoom-in` |
| `_` | less detail in the view beside the list | `zoom-out` |
| `0` | the view's detail chosen from the size again | `zoom-auto` |

### Hierarchy (`hierarchy`)

| Keys | What it does | Action |
| --- | --- | --- |
| `h`, `←` | card on the left | `left` |
| `j`, `↓` | card below | `down` |
| `k`, `↑` | card above | `up` |
| `l`, `→` | card on the right | `right` |
| `gg`, `home` | first card | `first` |
| `G`, `end` | last card | `last` |
| `s` | show only the selected agent and its descendants | `focus-subtree` |
| `+`, `=` | more detail on cards | `zoom-in` |
| `_` | less detail on cards | `zoom-out` |
| `0` | choose the detail level from the size | `zoom-auto` |

### Network (`network`)

| Keys | What it does | Action |
| --- | --- | --- |
| `h`, `←` | agent on the left | `left` |
| `j`, `↓` | agent below | `down` |
| `k`, `↑` | agent above | `up` |
| `l`, `→` | agent on the right | `right` |
| `gg`, `home` | first agent, top left | `first` |
| `G`, `end` | last agent, bottom right | `last` |
| `+`, `=` | more detail on agents | `zoom-in` |
| `_` | less detail on agents | `zoom-out` |
| `0` | choose the detail level from the size | `zoom-auto` |

### Repositories (`repos`)

| Keys | What it does | Action |
| --- | --- | --- |
| `j`, `↓` | next agent | `down` |
| `k`, `↑` | previous agent | `up` |
| `h`, `←` | previous repository | `left` |
| `l`, `→` | next repository | `right` |
| `gg`, `home` | first agent | `first` |
| `G`, `end` | last agent | `last` |
| `+`, `=` | more detail per agent | `zoom-in` |
| `_` | less detail per agent | `zoom-out` |
| `0` | choose the detail level from the size | `zoom-auto` |

### Timeline (`timeline`)

| Keys | What it does | Action |
| --- | --- | --- |
| `j`, `↓` | next agent | `down` |
| `k`, `↑` | previous agent | `up` |
| `h`, `←` | move the cursor back in time | `left` |
| `l`, `→` | move the cursor forward in time | `right` |
| `[` | cursor to the selected agent's previous event | `previous-event` |
| `]` | cursor to the selected agent's next event | `next-event` |
| `gg`, `home` | first agent | `first` |
| `G`, `end` | last agent | `last` |
| `+`, `=` | shorter time span, more detail | `zoom-in` |
| `_` | longer time span | `zoom-out` |
| `0` | back to now and the default time span | `zoom-auto` |

### Detail and preview (`detail`)

| Keys | What it does | Action |
| --- | --- | --- |
| `j`, `↓` | scroll the preview down | `down` |
| `k`, `↑` | scroll the preview up | `up` |
| `ctrl-d`, `pgdn` | scroll the preview half a page down | `half-page-down` |
| `ctrl-u`, `pgup` | scroll the preview half a page up | `half-page-up` |
| `r` | refresh the preview | `refresh` |

### Full screen (`screen`)

| Keys | What it does | Action |
| --- | --- | --- |
| `k`, `↑` | scroll up, to older lines | `up` |
| `j`, `↓` | scroll down, to newer lines | `down` |
| `ctrl-u`, `pgup` | half a page up, to older lines | `half-page-up` |
| `ctrl-d`, `pgdn` | half a page down, to newer lines | `half-page-down` |
| `gg`, `home` | the top of the screen | `first` |
| `G`, `end` | the bottom, the newest lines | `last` |
| `r` | refresh the screen | `refresh` |
| `enter`, `i`, `a` | open the agent: your keys go to it; the leave key (ctrl-q) comes back | `take-over` |
| `z`, `esc`, `q` | come back to the interface | `back` |
| `ctrl-c` | quit volee | `quit` |

### Filter prompt (`filter`)

| Keys | What it does | Action |
| --- | --- | --- |
| `enter` | keep the filter and close the prompt; n and N then go through the matches | `apply` |
| `esc` | clear the filter and close the prompt | `back` |
| `ctrl-c` | quit volee | `quit` |

### Message composer (`composer`)

| Keys | What it does | Action |
| --- | --- | --- |
| `enter` | send the message | `send` |
| `alt-enter`, `ctrl-j` | start a new line | `new-line` |
| `tab` | next kind of message | `next-kind` |
| `shift-tab` | previous kind of message | `previous-kind` |
| `esc` | close the composer and keep the draft | `back` |
| `ctrl-c` | quit volee (asks first) | `quit` |

### Changes (`diff`)

| Keys | What it does | Action |
| --- | --- | --- |
| `j`, `↓` | scroll down | `down` |
| `k`, `↑` | scroll up | `up` |
| `ctrl-d`, `pgdn` | half a page down | `half-page-down` |
| `ctrl-u`, `pgup` | half a page up | `half-page-up` |
| `gg`, `home` | top | `first` |
| `G`, `end` | bottom | `last` |
| `f` | switch between the summary and the full patch | `toggle-full` |
| `r` | load the changes again | `refresh` |
| `o` | open the worktree in your editor, diff viewer or git tool | `open-with` |
| `esc`, `d`, `q` | close the changes | `back` |
| `ctrl-c` | quit volee | `quit` |

### Questions (`confirm`)

| Keys | What it does | Action |
| --- | --- | --- |
| `y` | yes | `confirm` |
| `n`, `esc` | no | `back` |
| `ctrl-c` | quit volee without asking again | `quit` |

### Messages (`inbox`)

| Keys | What it does | Action |
| --- | --- | --- |
| `j`, `↓` | next row, or further down the thread you read | `down` |
| `k`, `↑` | previous row, or further up the thread you read | `up` |
| `ctrl-d`, `pgdn` | half a page down | `half-page-down` |
| `ctrl-u`, `pgup` | half a page up | `half-page-up` |
| `gg`, `home` | first row, or the start of the thread | `first` |
| `G`, `end` | last row, or the end of the thread | `last` |
| `enter` | read the thread, which marks its messages read | `apply` |
| `r` | reply: the answer to the question, else a note to the agent | `reply` |
| `s` | summarize the thread with a model, when summaries are on | `summarize` |
| `esc`, `q`, `I` | leave the thread, then close the messages | `back` |
| `ctrl-c` | quit volee | `quit` |

### Open with (`tools`)

| Keys | What it does | Action |
| --- | --- | --- |
| `e` | your editor ($VISUAL, $EDITOR or [tui.tools] editor) | `open-editor` |
| `d` | your diff viewer (Neovim with diffview, or [tui.tools] diff) | `open-diff` |
| `g` | your git tool (lazygit, or [tui.tools] git) | `open-git` |
| `w` | an editor with its own window (VS Code, or [tui.tools] ide) | `open-ide` |
| `esc`, `q`, `o` | close the menu | `back` |
| `ctrl-c` | quit volee | `quit` |

### Launch dialogs (`dialog`)

| Keys | What it does | Action |
| --- | --- | --- |
| `ctrl-s` | launch the agent, or create the mission | `launch` |
| `tab` | next field | `next-field` |
| `shift-tab` | previous field | `previous-field` |
| `enter`, `alt-enter`, `ctrl-j` | a new line in the task or the goal, the next field elsewhere | `new-line` |
| `esc` | cancel | `back` |
| `ctrl-c` | quit volee | `quit` |

### Choices in dialogs (`choice`)

| Keys | What it does | Action |
| --- | --- | --- |
| `←`, `h` | the previous option, or the cursor to the previous repository | `previous-choice` |
| `→`, `l` | the next option, or the cursor to the next repository | `next-choice` |
| `space` | pick or unpick the repository under the cursor; the next option of a single choice | `toggle-choice` |

### Text fields (`editing`)

| Keys | What it does | Action |
| --- | --- | --- |
| `ctrl-w`, `ctrl-backspace` | delete the word before the cursor | `delete-word` |
| `ctrl-u` | delete from the start of the line to the cursor | `delete-to-line-start` |
| `ctrl-a`, `home` | the cursor to the start of the line | `line-start` |
| `ctrl-e`, `end` | the cursor to the end of the line | `line-end` |

### Keys popup (`keys`)

| Keys | What it does | Action |
| --- | --- | --- |
| `?` | list every key, in every context | `all-keys` |
| `esc`, `q` | close the popup; any other key closes it and acts | `back` |

### All keys (`help`)

| Keys | What it does | Action |
| --- | --- | --- |
| `j`, `↓` | scroll down | `down` |
| `k`, `↑` | scroll up | `up` |
| `ctrl-d`, `pgdn` | half a page down | `half-page-down` |
| `ctrl-u`, `pgup` | half a page up | `half-page-up` |
| `gg`, `home` | top | `first` |
| `G`, `end` | bottom | `last` |
| `esc`, `?`, `q` | close the list | `back` |
| `ctrl-c` | quit volee | `quit` |

### Command palette (`commands`)

| Keys | What it does | Action |
| --- | --- | --- |
| `enter` | run the selected command | `apply` |
| `↓`, `ctrl-n`, `tab` | the next command | `down` |
| `↑`, `ctrl-p`, `shift-tab` | the previous command | `up` |
| `esc` | close the palette | `back` |
| `ctrl-c` | quit volee | `quit` |

### Theme picker (`themes`)

| Keys | What it does | Action |
| --- | --- | --- |
| `j`, `↓` | the next theme, shown at once | `down` |
| `k`, `↑` | the previous theme, shown at once | `up` |
| `enter` | keep the theme and save it in the configuration | `apply` |
| `esc`, `q` | back to the theme you had | `back` |
| `ctrl-c` | quit volee | `quit` |

### Wall of screens (`wall`)

| Keys | What it does | Action |
| --- | --- | --- |
| `h`, `←` | the screen on the left | `left` |
| `j`, `↓` | the screen below | `down` |
| `k`, `↑` | the screen above | `up` |
| `l`, `→` | the screen on the right | `right` |
| `gg`, `home` | the first screen | `first` |
| `G`, `end` | the last screen | `last` |
| `z` | the selected screen alone, full size; back comes back to the wall | `full-screen` |
| `enter`, `i`, `a` | open the selected agent: your keys go to it; the leave key (ctrl-q) comes back to the wall | `take-over` |
| `q`, `esc` | leave the wall | `back` |
| `ctrl-c` | quit volee; in `volee host wall`, leave the wall | `quit` |

### Edit mode (`attached`)

| Keys | What it does | Action |
| --- | --- | --- |
| `ctrl-q` | leave edit mode, or an agent taken over from a shell, and come back; Control and a letter only | `leave` |
<!-- keys:end -->
