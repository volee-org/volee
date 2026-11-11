# Roadmap

volee is young. This page lists what exists, what comes next and what is
deliberately out of scope. Dates are targets, not promises.

## 0.1: see everything, launch and talk

| Milestone | Content | Status |
| --- | --- | --- |
| Bootstrap | Workspace, CI, documentation site, contributor guides, agent rules | Done |
| Foundation | Domain model, SQLite event store, daemon with JSON-RPC over a Unix socket, `volee ls`, discovery of every Claude Code session | Done |
| Launch and observe | `volee spawn` (worktree + `claude --bg` + per-agent hooks), hook ingestion, git delivery state, `volee diff` | Done |
| Interface | Hierarchy view, preview, take over with `claude attach`, messages and composer, changes, your own tools, launch dialogs, keymap and help, themes, semantic zoom | Done |
| Messaging | MCP server for agents, typed threads, delivery to working, idle and stopped agents, inbox, message pulses on the graph | Done |
| Missions | Supervisor plus coordinator agent, change and analysis missions, delivery evidence, `volee tidy` | Done |
| More views | Network, repositories and worktrees, timeline | Done |
| Release | Prebuilt binaries for macOS and Linux, shell installer, demo recordings | Done: v0.1.0 |

## 0.2: other harnesses and tuios panes

| Milestone | Content | Status |
| --- | --- | --- |
| Other harnesses | Agents of any MCP-capable harness join volee and its missions with `volee mcp --join` ([ADR 0014](../adr/0014-agents-of-other-harnesses-join-through-mcp.md)) | Done |
| tuios panes | Claude Code sessions in tuios panes are shown, previewed and reached from the interface ([ADR 0015](../adr/0015-tuios-location-adapter.md)) | Done |
| Launch recovery | A failed launch runs again without a git step ([ADR 0016](../adr/0016-what-a-failed-launch-left-untouched.md)); `volee forget` removes an agent that no longer runs | Done |
| Release | v0.2.0 | Done: v0.2.0 |

## 0.3: setting up and tidying

| Milestone | Content | Status |
| --- | --- | --- |
| Shell completions | `volee completions <shell>` prints a completion script | Done |
| Setup commands | Commands each new worktree runs before its agent starts, such as `bun install` ([ADR 0017](../adr/0017-setup-commands-for-new-worktrees.md)) | Done |
| Forget from the interface | The interface forgets an agent that no longer runs, like `volee forget` | Done |
| Fixes from the 0.2 review | The forge line of `volee daemon status`, focus steps for tuios panes on the daemon's server | Done |

## 0.4: more places agents run

| Milestone | Content | Status |
| --- | --- | --- |
| tmux panes | Claude Code sessions in tmux panes are shown, previewed and reached from the interface, like tuios panes | Done: v0.4.0 |
| Setup in sight | The preview of an agent whose session has not started shows the end of its setup log | Done: v0.4.0 |
| Stalled launches | A launch whose session can never start ends as failed, ready to run again, instead of launching forever | Done: v0.4.1 |

## 0.5 and 0.6: reading an agent's conversation

| Milestone | Content | Status |
| --- | --- | --- |
| Full screen | `z` shows an agent's screen across the whole window, scrolled with `j` and `k`; `a` takes it over from there | Done: v0.5.0 |
| Wide sessions | The preview of a Claude Code session wider than 200 columns replays without garbling, and the end of its conversation stays in view | Done: v0.5.1 |
| Long lines | The full-screen view wraps lines wider than the window; the preview keeps cutting them | Done: v0.6.0 |

## 0.8: a host for every agent

| Milestone | Content | Status |
| --- | --- | --- |
| Hosts | Every agent volee launches runs in a volee host that holds its terminal and its exact screen, emulated by libghostty-vt, and survives the interface, the daemon and the terminal that started it ([ADR 0019](../adr/0019-a-host-process-holds-each-agent-terminal.md), [ADR 0020](../adr/0020-libghostty-vt-emulates-terminals.md)) | Done |
| Take over and watch | `a` and `volee attach` take an agent over from any terminal, `Ctrl+Q` comes back; the preview and `z` show the exact screen without resizing the agent ([ADR 0021](../adr/0021-take-over-watch-and-the-wall-of-screens.md)) | Done |
| Wall of screens | `5` shows the hosted agents of the list as tiles of live screens; `volee host wall` opens it from a shell | Done |
| OpenCode | `volee spawn --harness opencode` and the launch dialog launch OpenCode agents in hosts, follow their state from their screen and resume them ([ADR 0022](../adr/0022-harness-profiles.md)) | Done |
| Every agent on the machine | OpenCode agents started elsewhere are found in the process table; `A` or `volee adopt` moves an agent volee does not host into a host ([ADR 0023](../adr/0023-every-agent-on-the-machine.md)) | Done |
| Back after a reboot | The daemon resumes the agents a reboot interrupted, in new hosts; `volee daemon install` starts it at login ([ADR 0024](../adr/0024-agents-come-back-after-a-reboot.md)) | Done |
| Hosts by hand | `volee host spawn`, `ls`, `peek`, `attach`, `stop` and `wall` run any command line in a host, without the daemon | Done |
| Mirrors removed | The 0.7.0 previews of background sessions read from tuios panes by their title are gone; tuios and tmux still show the sessions started by hand in their panes | Done |
| Release | v0.8.0 | Done: v0.8.0 |

## 0.9 and 0.10: keys, and typing inside volee

| Milestone | Content | Status |
| --- | --- | --- |
| Keys | A popup of the keys of each screen, a leader key, vim keys, every key configurable, a theme picker ([ADR 0025](../adr/0025-key-sequences-a-leader-and-which-key-popups.md)) | Done: v0.9.0 |
| Edit mode | The full-screen view of an agent in a volee host is live, and typing goes to the agent inside volee's frame until `Ctrl+Q` ([ADR 0026](../adr/0026-edit-mode-types-into-an-agent-inside-volee.md)) | Done: v0.10.0 |

## 0.11: open every agent where it runs

An audit on 2026-11-10 found that volee owned the agents it showed, where it
should plug into them: typing into a session volee did not start needed an
adoption, and the list kept sessions that were gone. The plan that followed
has five steps, each released only once a person can make its trip by
keyboard ([journeys](https://github.com/volee-org/volee/tree/main/journeys)).

| Milestone | Content | Status |
| --- | --- | --- |
| Journeys | Scripts that drive the real interface by keyboard with a real agent are the gate of every release | Done |
| The list is what runs | One row per session the harness lists, finished background sessions included; a session that is gone leaves the list; a one-off run of a few seconds never shows ([ADR 0027](../adr/0027-open-without-owning.md)) | Done |
| Open without owning | `enter`, a double click or `i` opens any agent where it runs, inside volee: `claude attach` on a background session, `tuios pane-view` on a tuios pane; nothing is stopped or restarted ([ADR 0027](../adr/0027-open-without-owning.md)) | Done |
| Release | v0.11.0 | Done: v0.11.0 |

## 0.12: one emulator, and a wall of every agent

| Milestone | Content | Status |
| --- | --- | --- |
| One emulator | The preview, the full-screen view, edit mode and the wall read every screen through libghostty-vt; `vt100` is gone ([ADR 0028](../adr/0028-one-emulator-and-the-wall-inside-the-interface.md)) | Done |
| Live preview | The preview of an agent in a volee host is its live screen, on the connection the full-screen view and edit mode use | Done |
| The mouse | In edit mode, clicks, drags and the wheel reach a program that asked for them, on its own cells | Done |
| The wall, inside | `5` shows the screen of every agent of the list that runs, in a volee host or not, as many to a page as the terminal shows at a readable size; what is opened from the wall comes back to it | Done |
| Release | v0.12.0 | Done: v0.12.0 |
| The same screen in every view | The proof the plan asked for, run after the release: the preview and the wall dropped an agent's cursor block and showed a run of blank rows as one. Fixed, and a test and a journey now compare the three views cell by cell ([ADR 0028](../adr/0028-one-emulator-and-the-wall-inside-the-interface.md#the-proof-and-what-it-found)) | Done: v0.12.1 |
| A screen at rest stays current | The screen of an agent volee does not host was read again only while the agent worked: at rest it went stale, as after a command typed in its pane. The preview now reads it again every five seconds and a tile of the wall every fifteen ([ADR 0028](../adr/0028-one-emulator-and-the-wall-inside-the-interface.md#a-screen-at-rest)) | Done: v0.12.2 |
| A pane's screen, row for row | The daemon says what it read: the screen itself (a volee host's snapshot, with the size of its terminal; a multiplexer's capture of a pane) or the replay of a session's output. The preview, the wall, the full-screen view and `volee logs` show a pane's screen row for row, blank rows included ([ADR 0028](../adr/0028-one-emulator-and-the-wall-inside-the-interface.md#the-proof-and-what-it-found)) | Done: v0.12.2 |
| An exact preview everywhere | Met for an agent in a volee host, a view open on a session and a multiplexer pane. For a Claude Code background session that is not open, volee has its replay, which is its screen as far as measured (50 rows of 50): the full-screen view and `volee logs` show every row of it, and the views that crop show a run of blank rows as one row that says how many it stands for, where they used to hide it without a word. Decided while the project's owner was away, and his to overturn ([ADR 0028](../adr/0028-one-emulator-and-the-wall-inside-the-interface.md#the-fold-said)). The size of a replay is still told from what it writes: nothing gives it | Done |

## 0.13: agents of other AIs

| Milestone | Content | Status |
| --- | --- | --- |
| Another AI, in its pane | An OpenCode agent started by hand in a tuios or tmux pane is placed in its pane; its preview and its tile are the pane's screen, and in a tuios pane `enter` opens it inside volee | Done |
| `volee run` | `volee run -- <command>` runs any program as an agent of the list, in a volee host, from the terminal it is typed in; the leave key gives the terminal back and the program keeps running ([ADR 0029](../adr/0029-volee-run.md)) | Done |
| Agents tuios recognizes | An agent of a harness volee has no listing for (Codex CLI, Gemini CLI, Aider...) is found through tuios when it runs in one of its panes ([ADR 0030](../adr/0030-a-multiplexer-may-say-which-panes-run-an-agent.md)). Checked with stand-in programs carrying the harnesses' names: the real ones were not available | Done |
| Release | v0.13.0 | Done: v0.13.0 |

## 0.14: missions as a lead runs them

| Milestone | Content | Status |
| --- | --- | --- |
| A question with its answers | An agent asks the person a question that offers its answers; the messages window numbers them, a number then `y` answers, and `volee msg answer` does it from a shell ([ADR 0031](../adr/0031-a-question-may-offer-its-answers.md)) | Done |
| Your approval before a merge | A mission's coordinator presents its report as a question. On "ok merge" volee merges the work itself (the pull request on GitHub, or a merge commit in a repository without a remote), closes the mission and tidies it in one gesture; a merge that cannot go through changes nothing and says why. No agent merges ([ADR 0032](../adr/0032-a-mission-is-merged-only-after-the-person-approved.md)) | Done |
| A lead | `volee lead` starts the agent you talk to: it launches agents and missions for you, never codes, does not wait in a loop, and tells you in three lines what came back; duplicates are refused with whom to write to ([ADR 0033](../adr/0033-a-lead-makes-others-work.md)) | Done |
| Release | v0.14.0 | Done: v0.14.0 |
| Writing to a lead | A lead does what you write to it through volee (`volee msg send`, or `m` in the interface), the same as what you type in its terminal: a message volee hands an agent as from the human is the human's own word. Found by a mission run on a real daemon after 0.14.0 was installed | Done: v0.14.1 |

## 0.15: a session typed in any terminal

| Milestone | Content | Status |
| --- | --- | --- |
| A session in a terminal of its own | `enter` on a Claude Code session typed in a terminal tab no longer fails. It says which key to press there (the left arrow on its empty prompt, which has Claude Code move the conversation to the background), waits, and opens the conversation by itself, on the same row. It is the same in every terminal application: none is asked for anything ([ADR 0034](../adr/0034-a-session-in-its-own-terminal-opens-once-it-left-it.md)) | Done |
| Release | v0.15.0 | Done: v0.15.0 |

## 0.16: the cockpit's main screen

| Milestone | Content | Status |
| --- | --- | --- |
| The tree | The tree of the selected agent, children side by side, and the sessions started on their own in rows. Cards in the agent's own color, with its state as a filled badge, its model, its activity over five minutes, the lines it changed and its delivery | Done |
| The frame | The list grouped by kind (leads, missions, terminals, background) with the detail under it; from 160 columns, what waits for you and a journal at the right; the selected agent's actions on its screen's border; agents counted by role; keys drawn as keys | Done |
| The context an agent uses | On its card, the share of its context window an agent's conversation uses, read from its status line, which still shows the person's own ([ADR 0035](../adr/0035-the-context-an-agent-uses-comes-from-its-status-line.md)) | Done |
| The terminal's background | Every theme draws on the terminal's own background, unless `[tui] background = "theme"` | Done |
| Found on the way to the release | A lead no longer takes its own row of `list_agents` for a duplicate of the mission it is asked to start (the row is marked `you`). A coordinator's name shows whole in the list. A mission's worker is judged only on evidence read after it stopped: one that committed and stopped within seconds was once said to have no commits. What is typed in edit mode before the agent's host answers is sent once it does, instead of being dropped | Done |
| Release | v0.16.0 | Done: v0.16.0 |

## Next

Nothing is scheduled: the plan that ran from 0.11 to 0.14 is complete. What
comes next is chosen among the items below.

## Later

- A location adapter for Zellij (jump to a pane-hosted agent); tuios and tmux
  have one ([ADR 0015](../adr/0015-tuios-location-adapter.md)).
- Profiles for Codex CLI, Gemini CLI, Grok Build and others, each measured on
  the real program ([ADR 0022](../adr/0022-harness-profiles.md)). Until
  then, `volee run` runs any of them as an agent, with a state read on its
  screen, and agents of any MCP-capable harness can
  [join](../guide/other-harnesses.md) through `volee mcp --join`.
- Finding agents of harnesses other than OpenCode in the process table, so
  that they show outside tuios too.
- Messaging sessions started outside volee.
- The context of sessions volee did not launch: Claude Code gives it only to
  a session's status line, which volee sets for the agents it launches
  ([ADR 0035](../adr/0035-the-context-an-agent-uses-comes-from-its-status-line.md)).
- Pressing the left arrow for the person where a terminal application has a
  door for it (Ghostty 1.4 on macOS was measured to have one), and waiting
  from `volee attach` as the interface does
  ([ADR 0034](../adr/0034-a-session-in-its-own-terminal-opens-once-it-left-it.md)).
- A Homebrew tap and a crates.io release.

## Not planned

- Running agents inside tmux or another multiplexer: hosts hold their
  terminals ([ADR 0019](../adr/0019-a-host-process-holds-each-agent-terminal.md)).
- Windows support, until the harnesses and Unix-socket replacements make it
  reasonable.
- A web or desktop interface.
