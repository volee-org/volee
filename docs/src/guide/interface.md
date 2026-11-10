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
| The preview of an agent's screen | The agent's volee host, which holds its exact screen. For agents volee does not host: tuios or tmux, for a session in one of their panes (`capture-pane`); Claude Code, which replays a background session's output (`claude logs`). volee's setup log, for an agent whose session has not started yet. |
| An agent's changes (`d`) | git, in the agent's worktree or folder. |
| Taking over an agent (`a`) | The agent's volee host (`volee host attach`); tuios or tmux, which bring forward the pane of a session in one of their panes. |
| Adopting an agent (`A`) | Claude Code, which stops the background session (`claude stop`), then a new volee host, which resumes its conversation. |
| The wall of screens (`5`) | The volee hosts of the agents in the list. |
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
 h/j/k/l move   gg/G first/last   enter subtree   ' jump   / filter   n/N match   1-4 views
 m message   d diff   a take over   z full screen   i messages   A adopt   ␣ more   ? help
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
- **Preview.** The selected agent's terminal screen, kept up to date while
  it works. For an agent in a volee host, it is the exact screen the host
  holds, status line and prompt frame included, drawn at the agent's own
  size: looking at an agent never resizes it. The bottom of the screen,
  where the latest output is, stays in view; scroll it with `j` and `k` when
  the panel has the keyboard. Lines wider than the panel are cut at its
  edge. `z` (or `enter` in the preview) shows that screen full size, its
  whole conversation in view, from its newest lines: lines wider than the
  window go on over several lines, between words where they can, so that
  nothing is cut but the padding and frames the agent's interface draws to
  its own edge. `j` and `k` scroll it, `a` takes the agent over, and `z` or
  `esc` comes back. For a session
  in a tuios or tmux pane, the screen is the pane's. A Claude Code
  background session that volee does not host (one that volee 0.7
  launched, or a `claude --bg` you started by hand) is replayed from
  `claude logs`, which keeps only the session's recent output: in a session
  that has run for hours, parts of the screen drawn long ago and never
  redrawn since, such as a list that has not changed, can show gaps. A
  recent session shows whole, and [adopting](#adopting-an-agent) it moves
  it into a host, whose screen is exact.
- **Hints.** The keys that apply to the panel that has the keyboard, on two
  rows from 20 rows up: the top one says how to move around (moves, first
  and last, the filter and its matches, panels and views), the bottom one
  what to do (take over, message, diff, adopt, the leader, commands, quit),
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

`5` (or `Space w`) opens the wall: the agents of the current list that run in a volee
host, each as a tile of its live screen, over the whole terminal. The
filter (`/`) applies, so filter first to watch one mission or one
repository. A page holds up to nine tiles, laid out as one, two, four, six
or nine depending on how many agents there are; moving past the last tile
of a page shows the next one.

Each tile shows its agent's screen at the agent's own size, cut on the
right when the tile is narrower, and never resizes the agent; its title is
the agent's name and the title its program set. Agents volee does not host
are not on the wall: [adopt](#adopting-an-agent) one to see it there.

| Keys | What it does |
| --- | --- |
| arrows, `h`, `j`, `k`, `l` | move between tiles |
| `z` | show the selected tile alone, over the whole wall; `z` or `esc` brings the wall back |
| `a`, `enter` | take over the selected agent; the leave key (`Ctrl+Q`) comes back to the wall |
| `q`, `esc` | come back to the interface |

These are the defaults of the `wall` context: `[tui.keys.wall]` changes
them, like any other key ([Keys](#keys)).

The wall runs as `volee host wall`, the interface stepping aside while it
is open. From a shell, `volee host wall` opens it on every running host
([hosts](cli.md#hosts)).

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
  node, a lane or a tab selects it.

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

`i` opens your messages. Questions agents asked you and that you have not
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

`n` launches an agent in a new worktree: choose the repository (from
`[repos]` in the configuration, or type the path of a checkout when there
is none), name the new branch, write the task (its first prompt) and,
optionally, the model and the harness (`claude-code`, the default, or
`opencode`). `N` creates a mission: a title, its kind (`change`
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

## Taking over an agent

`a` takes over the selected agent in this terminal: volee steps aside,
prints how to come back, and connects your terminal to the agent's volee
host, which draws the agent's current screen at once. You type into the
agent as if it ran there. `Ctrl+Q` comes back to volee, on the same agent,
and the agent keeps running. `Ctrl+Q` never reaches the agent, in whatever
form your terminal sends it once the agent turned on a keyboard protocol
(the kitty keyboard protocol or xterm's modifyOtherKeys). On the way back,
volee turns off what the agent left on in your terminal, such as mouse
reporting or bracketed paste, and restores your screen.

The last row of your terminal keeps a volee bar while you type, so you
always know where you are: the agent's name, its repository and branch,
whether it is working, and the key that comes back:

```text
 volee │ billing · shop › feat/invoices │ ▸ working │ Ctrl+Q back to volee
```

The agent gets every row above the bar; its scrolling never reaches it.

While you type, your terminal's size, less the bar's row, is the agent's
size, and it stays when you leave. Only the terminal that takes over sets it: the preview, the
full-screen view and the wall never resize an agent. Taking the same agent
over from a second terminal, with `volee attach`, moves the input there,
and the first terminal comes back to volee.

volee types only into the agents it hosts. On a Claude Code background
session that volee does not host, `a` asks first, on the bottom line,
whether to move it into a volee host: `y` adopts it (below) and takes it
over as soon as its host runs, `n` or `Esc` leaves it alone. In a
repository Claude Code does not trust yet, the agent's screen then shows
Claude Code's trust question: answer it there, once. Agents running in another terminal cannot be taken over
from here; volee tells you where they are instead, and brings forward the
pane of an agent in a tuios or tmux pane (below).

### Adopting an agent

`A` moves the selected agent's conversation into a volee host, as
`volee adopt` does, so that you can take it over and see its exact screen.
It works on Claude Code background sessions (the agents volee 0.7 launched
with `claude --bg`, and any `claude --bg` you started by hand), on agents
whose session ended, and on OpenCode agents started with `--session`. volee
first stops the background session (`claude stop`), so that two programs
never write the same conversation, then resumes the conversation in a new
host, in the same directory, where the agent waits at its prompt. An agent
that runs in a terminal of its own must be quit there first: volee refuses
it until then and says so on the bottom line. The agent keeps its name and
its history; once adopted, `a` takes it over, and volee handles it like the
agents it launches: messages reach it, `volee stop` stops it, and a message
resumes it.

The conversation resumes in the agent's worktree. In a repository Claude
Code does not trust yet, it asks first, in the host: take the agent over
(`a`) to answer, once for the repository. When the resumed program fails, the agent is
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
is the pane's screen, and `a` brings the pane forward instead of taking the
agent over: tuios focuses it, shows its workspace and, when volee runs in a
tuios pane itself, switches your tuios window to the pane's session. volee
stays where it was. When no tuios window shows that session (none is
attached), volee says the agent is in front in its session and how to see
it: `tuios attach <session>`. Nothing is typed into the pane, and no pane is
opened, closed or moved.

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
says `tmux · work`, the preview is the pane's screen, and `a` makes the pane
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
take-over = "ctrl-a"           # everywhere it has a key
new-agent = ["c", "space n"]   # several keys
adopt = []                     # no key

[tui.keys.agents]
first = "home"                 # in the agents list only

[tui.keys.attached]
leave = "ctrl-g"               # leave a taken-over agent with Ctrl+G
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
| `a` | take over the selected agent in this terminal | `take-over` |
| `z` | show the selected agent's screen full size | `full-screen` |
| `i` | your messages, questions waiting for you first | `messages` |
| `o`, `space o` | open the selected agent's worktree in your editor, diff viewer or git tool | `open-with` |
| `A`, `space a` | adopt the selected agent into a volee host, so that you can take it over | `adopt` |
| `F`, `space f` | forget the selected agent: stop listing it (asks first) | `forget` |
| `space n` | launch an agent in a new worktree | `new-agent` |
| `space N` | create a mission, which launches its coordinator | `new-mission` |
| `space w`, `5` | show the agents in volee hosts as a wall of live screens | `show-wall` |
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
| `enter` | show the agent's subtree in the hierarchy | `focus-subtree` |
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
| `enter` | show only the selected agent and its descendants | `focus-subtree` |
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
| `enter` | show the agent's screen full size | `full-screen` |

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
| `a` | take over the agent in this terminal | `take-over` |
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
| `esc`, `q`, `i` | leave the thread, then close the messages | `back` |
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
| `z` | the selected screen alone, full size, and back | `full-screen` |
| `a`, `enter` | take over the selected agent; the leave key comes back to the wall | `take-over` |
| `q`, `esc` | leave the wall | `back` |
| `ctrl-c` | leave the wall | `quit` |

### Taken over (`attached`)

| Keys | What it does | Action |
| --- | --- | --- |
| `ctrl-q` | leave the agent you took over and come back; Control and a letter only | `leave` |
<!-- keys:end -->
