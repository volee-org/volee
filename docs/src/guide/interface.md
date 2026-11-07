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
agent it knows: the Claude Code sessions it finds on the machine, wherever
they were started, and the agents volee launched itself. New sessions show
up within a couple of seconds; nothing needs to be configured first.

Everything on the screen comes from the daemon, and so do the actions:

| What | What the daemon uses |
| --- | --- |
| The agents, their states and messages | Its own records, kept up to date as sessions come and go. |
| The preview of an agent's screen | Claude Code, which replays the session's output (`claude logs`). |
| An agent's changes (`d`) | git, in the agent's worktree or folder. |
| Taking over an agent (`a`) | Claude Code, which attaches the session (`claude attach`). |
| Writing to an agent (`m`) | Its own records and the agent's hooks. |

When the daemon cannot do something, because Claude Code or git is
missing, because the agent runs in a terminal of its own, or because the
daemon is older than the interface, volee says so in a sentence where the
result would be, such as "No preview: dashboard runs in a terminal of its
own: switch to that terminal to see or control it", and goes on working. If
the daemon stops while volee runs, the top bar shows `offline` and the
screen stops changing; start `volee` again to reconnect.

The theme and the keys come from the `[tui]` table of the
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
 h/j/k/l move   enter subtree   ' jump   / filter   m message   a take over   q quit     ? help
```

- **Top bar.** The views, numbered (press the number to switch), and
  counters: how many agents need you, how many work, how many there are.
- **Agents.** Every agent, the most urgent first: those that need you, then
  failed, working, launching, done, idle, stopped and lost. Each row shows
  the agent's jump letter, its state, its name, how long it has been in that
  state, and where it works (`repo › branch`, or its mission).
- **View.** The hierarchy for now. The network, repositories and timeline
  views arrive in the next milestone.
- **Detail.** Facts about the selected agent: role, state and since when,
  its mission and the mission supervisor's verdict on its work, repository
  and branch, worktree, where it runs and with which model, delivery
  evidence, last activity and session.
- **Preview.** The selected agent's terminal screen, kept up to date while
  it works. The bottom of the screen, where the latest output is, stays in
  view; scroll it with `j` and `k` when the panel has the keyboard.
- **Hints.** The keys that apply to the panel that has the keyboard. They
  come from the same table as the help (`?`), so they are always right.

Panel sizes depend only on the size of the terminal: moving around never
makes anything jump. Below 100 columns or 28 rows, the view makes room: the
agents panel shows the hierarchy as an indented tree, next to the detail and
the preview. The smallest supported terminal is 80x24.

## States

Every state has a glyph and a word, so you can read it without colors. A
spinner turns on the cards of working agents, as an extra.

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

## Moving around

- `j`, `k` and the arrows move through the list; in the hierarchy, `h`,
  `j`, `k` and `l` move through the tree as drawn: down to satellites and
  children, up to the parent, sideways to the nearest card.
- Every agent has a letter, shown in the list. Press `'` then the letter
  to jump to it, from anywhere: two keys to reach any agent.
- `-` goes back to the agent selected before, and again to return.
- `/` filters by name, repository, branch or mission, as you type. Cards
  that do not match are dimmed; `Enter` keeps the filter, `Esc` clears it.
- `Tab` moves the keyboard to the next panel. Clicking a row, a card or a
  tab selects it.

`Esc` closes or leaves one thing at a time (the help, the subtree, the
filter, then the panel), and never quits volee.

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

## Changes

`d` shows the selected agent's changes against the branch it started
from: first a summary (`git diff --stat`), then the full patch with `f`. `j`
and `k` scroll, `r` loads them again, and `Esc` or `d` closes the window. A
very long patch stops at a line that says so; the rest is in the agent's
worktree.

## Taking over an agent

`a` takes over the selected agent in this terminal: volee asks the daemon
how, steps aside and runs the command it gives, such as `claude attach` on
the agent's session, after printing how to come back. Leave the session
with `Ctrl+Z` and volee comes back, on the same agent. Agents running in
another terminal cannot be attached from here; volee tells you where they
are instead.

## Keys

`q` quits at once, unless a message you started would be lost: then volee
asks first. The tables below are generated from the keymap, like the help
and the hints.

To move an action to other keys, name it in the `[tui.keys]` table of the
[configuration](configuration.md). An override replaces every key of the
action, in every panel; keys are written like `q`, `ctrl-c`, `shift-tab`,
`enter` or `f5`, and two actions cannot share a key in the same panel:

```toml
[tui.keys]
take-over = "ctrl-a"
```

<!-- keys:start -->
### Everywhere

| Keys | What it does | Action |
| --- | --- | --- |
| `'` | jump to an agent: press ' then its letter | `jump` |
| `-` | back to the agent selected before | `jump-back` |
| `/` | filter agents by name, repository, branch or mission | `filter` |
| `m` | write a message to the selected agent | `message` |
| `d` | show the selected agent's changes | `diff` |
| `a` | take over the selected agent in this terminal | `take-over` |
| `tab` | focus the next panel | `next-pane` |
| `shift-tab` | focus the previous panel | `previous-pane` |
| `1` | show the hierarchy | `show-hierarchy` |
| `2` | show the network | `show-network` |
| `3` | show repositories and worktrees | `show-repos` |
| `4` | show the timeline | `show-timeline` |
| `?` | show or hide the keys | `help` |
| `esc` | close, cancel or go back (never quits) | `back` |
| `q`, `ctrl-c` | quit volee (asks first when a draft would be lost) | `quit` |

### Agents list

| Keys | What it does | Action |
| --- | --- | --- |
| `h`, `←` | parent agent | `left` |
| `j`, `↓` | next agent | `down` |
| `k`, `↑` | previous agent | `up` |
| `l`, `→` | first child agent | `right` |
| `g`, `home` | first agent | `first` |
| `G`, `end` | last agent | `last` |
| `enter` | show the agent's subtree in the hierarchy | `focus-subtree` |

### Hierarchy

| Keys | What it does | Action |
| --- | --- | --- |
| `h`, `←` | card on the left | `left` |
| `j`, `↓` | card below | `down` |
| `k`, `↑` | card above | `up` |
| `l`, `→` | card on the right | `right` |
| `g`, `home` | first card | `first` |
| `G`, `end` | last card | `last` |
| `enter` | show only the selected agent and its descendants | `focus-subtree` |
| `+`, `=` | more detail on cards | `zoom-in` |
| `_` | less detail on cards | `zoom-out` |
| `0` | choose the detail level from the size | `zoom-auto` |

### Detail and preview

| Keys | What it does | Action |
| --- | --- | --- |
| `j`, `↓` | scroll the preview down | `down` |
| `k`, `↑` | scroll the preview up | `up` |
| `r` | refresh the preview | `refresh` |

### Filter prompt

| Keys | What it does | Action |
| --- | --- | --- |
| `enter` | keep the filter and close the prompt | `apply` |
| `esc` | clear the filter and close the prompt | `back` |
| `ctrl-c` | quit volee | `quit` |

### Message composer

| Keys | What it does | Action |
| --- | --- | --- |
| `enter` | send the message | `send` |
| `alt-enter`, `ctrl-j` | start a new line | `new-line` |
| `tab` | next kind of message | `next-kind` |
| `shift-tab` | previous kind of message | `previous-kind` |
| `esc` | close the composer and keep the draft | `back` |
| `ctrl-c` | quit volee (asks first) | `quit` |

### Changes

| Keys | What it does | Action |
| --- | --- | --- |
| `j`, `↓` | scroll down | `down` |
| `k`, `↑` | scroll up | `up` |
| `g`, `home` | top | `first` |
| `G`, `end` | bottom | `last` |
| `f` | switch between the summary and the full patch | `toggle-full` |
| `r` | load the changes again | `refresh` |
| `esc`, `d`, `q` | close the changes | `back` |
| `ctrl-c` | quit volee | `quit` |

### Questions

| Keys | What it does | Action |
| --- | --- | --- |
| `y` | yes | `confirm` |
| `n`, `esc` | no | `back` |
| `ctrl-c` | quit volee without asking again | `quit` |

### Help

| Keys | What it does | Action |
| --- | --- | --- |
| `j`, `↓` | scroll down | `down` |
| `k`, `↑` | scroll up | `up` |
| `esc`, `?`, `q` | close the help | `back` |
| `ctrl-c` | quit volee | `quit` |
<!-- keys:end -->
