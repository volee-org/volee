# Journeys

A journey is what a person does with volee, done for real: the real `volee`
interface, driven by keyboard, with a real coding agent behind it. Each script
types keys, reads the screen, and says which checks passed.

The test suite checks the parts. The journeys check the trip: a release whose
parts all pass can still fail a person at the first key. No version of volee
is released unless the journeys pass on a real machine
([RELEASING.md](../RELEASING.md)).

They never run in CI: they need a logged-in Claude Code, a terminal
multiplexer to type through, and a few short prompts on a small model.

## What you need

- [tuios](https://github.com/Gaurav-Gosain/tuios) 0.12.1 or later. The
  interface runs in a window of a headless tuios session; the scripts type
  with `tuios send-keys` and read with `tuios capture-pane`. Your own sessions
  are not touched.
- Claude Code, logged in.
- For `other_ai.py` and `run.py`, [OpenCode](https://opencode.ai) on `PATH`.
  It is only started and typed into: no model is asked anything, so it
  needs no account.
- Python 3.9 or later (the scripts use the standard library only).
- A **sandbox**: a git repository that Claude Code already trusts (run
  `claude` in it once and accept). The journeys make throwaway branches and
  worktrees in it, named `journey-...`, and remove them.
- For `mission.py`, a second such repository (`--other-sandbox`, by default
  the directory `beta` next to the sandbox), neither with a remote, both
  with nothing uncommitted. The mission merges a one-line file into the main
  branch of each; the journey then puts each main branch back at the commit
  it started from, which it does only when every commit above it is its own.
  Its agents run in Claude Code's `auto` permission mode, since they commit
  and merge without a person at their keyboard.

## Running them

```sh
cargo build --release -p volee-cli
python3 journeys/open.py      --volee target/release/volee --sandbox ~/code/sandbox
python3 journeys/pane.py      --volee target/release/volee --sandbox ~/code/sandbox
python3 journeys/edit_mode.py --volee target/release/volee --sandbox ~/code/sandbox
python3 journeys/wall.py      --volee target/release/volee --sandbox ~/code/sandbox
python3 journeys/other_ai.py  --volee target/release/volee --sandbox ~/code/sandbox
python3 journeys/other_ai.py  --stand-in codex --volee target/release/volee --sandbox ~/code/sandbox
python3 journeys/run.py       --volee target/release/volee --sandbox ~/code/sandbox
python3 journeys/question.py  --volee target/release/volee --sandbox ~/code/sandbox
python3 journeys/mission.py   --volee target/release/volee --sandbox ~/code/sandbox --other-sandbox ~/code/sandbox-two
python3 journeys/old_host.py  --volee target/release/volee --old-volee ~/bin/volee-0.9.1 --sandbox ~/code/sandbox
python3 journeys/installed.py --sandbox ~/code/sandbox --expect 0.12.0
```

Each prints one `PASS` or `FAIL` line per check, the screens it saw at the
steps that matter, and exits with status 0 only when every check passed.
`--keep` leaves the tuios session, the agent and the state in place so that
you can look (`tuios attach volee-journey-...`).

| Journey | What it does | Whose daemon |
| --- | --- | --- |
| `open.py` | Starts a Claude Code background session by hand, compares volee's list with what Claude Code lists (every session one row, no row without a session), opens the session by Enter, types into it, checks that it is the same session in the same process, leaves, opens it again once stopped, and removes it. | Its own, isolated |
| `pane.py` | Starts Claude Code by hand in a tuios pane: it shows in the list after ten seconds, Enter opens the pane inside volee, typing reaches it, text typed in the pane while the agent rests shows in the preview and on the wall, the pane's own screen is found row for row in the preview, on the wall, full size and in `volee logs`, `/clear` keeps its row, and quitting it takes it off the list. | Its own, isolated |
| `edit_mode.py` | Launches an agent, shows it full screen, types into it in edit mode, sends Esc, Ctrl+Z and a two-line paste, leaves with Ctrl+Q, then checks in a pseudo-terminal that the agent follows the panel's size. | Its own, isolated |
| `wall.py` | Launches an agent and starts a background session by hand, in a window the size of a large terminal: the preview of the first is its live screen, `5` shows both screens side by side, each opens from the wall with Enter or `i`, takes what is typed and comes back to the wall, `z` shows one full size, and the wall opens no terminal and resizes nothing. Last, the same screen is compared cell by cell between the preview, the full-screen view and the wall, for both. | Its own, isolated |
| `other_ai.py` | Starts OpenCode by hand in a tuios pane, in a directory of its own: it shows in the list as an agent of another harness, in its pane; its preview and its tile on the wall are the pane's screen, row for row; Enter opens the pane inside volee, typing reaches it, the leave key comes back and it is the same process; quit, it stops and leaves the list. Nothing is asked of a model. With `--stand-in codex`, the same trip for a harness volee has no listing for: a copy of `cat` named `codex`, which tuios takes for Codex CLI by its name and volee finds through tuios. | Its own, isolated |
| `run.py` | Types `volee run -- opencode` in a terminal (a tuios pane), in a directory of its own: the terminal is on the program, volee lists it once in a volee host though it also finds it in the process table, and its preview is the screen the person sees, row for row; the leave key gives the terminal back and it keeps running; from the interface Enter opens it, typing reaches it, and its tile on the wall shows its screen; `volee attach` opens it in a terminal again; quit there, it stops, volee says that nothing resumes it, and it leaves the list. Then `volee run -- claude`: one row, though Claude Code lists its session. Nothing is asked of a model. | Its own, isolated |
| `question.py` | An agent of another harness joins through volee's MCP server and asks the person a question that offers its answers: the messages window shows it on top with its answers numbered; the number of an answer asks before anything is sent, `n` takes it back, `y` sends it, and the agent reads exactly the answer it offered, in reply to its question, which stops waiting. Two more questions are answered from a shell with `volee msg answer`, by the number of an answer and in the person's own words. The journey itself speaks MCP as that agent's harness would: nothing is asked of a model. | Its own, isolated |
| `mission.py` | Starts a lead with `volee lead`, opens it in the interface and tells it, in a sentence, what is wanted across two repositories without a remote. The lead starts a mission in its own name, says what it launched and hands back. The mission's coordinator, which answers to the lead, launches one worker per repository, each commits its work on a branch of its own, and the coordinator merges nothing: it presents its report and the mission waits for the person. In the interface the report sits on top of the messages with its two answers; `1` then `y` says "ok merge". Only then is each branch merged into the main branch of its repository, by volee itself, with a merge commit; the mission closes, the workers' worktrees are removed, their branches kept, the coordinator stopped, and one message says so, which the lead hears and tells the person. Last, the person asks the lead for one task in one repository, by a message from a shell this time (`volee msg send`): it launches one agent itself, which commits on its branch and reports to it, and nobody merges that work. Real agents do all of it, on a small model. The journey then puts both repositories back where they were. | Its own, isolated |
| `old_host.py` | The same typing on an agent whose host an older release started (hosts outlive upgrades). | Its own, started by the older binary |
| `installed.py` | The installed volee with your own daemon and your own list: compares the list with what Claude Code lists, launches one throwaway agent, types into it and shows it on the wall, starts one background session by hand and opens it by Enter, then removes both. Run it after installing a release. | Yours |

## What a journey leaves behind

Nothing. At the end it stops and forgets its agents, removes the background
sessions it started, stops its daemon, kills its tuios session, and removes
its worktrees and branches from the sandbox. The isolated journeys keep their
state under a new directory in `/tmp` (`--workdir` chooses another; keep it
short, because Unix socket paths are limited to about 100 characters).

A journey never presses a key on an agent it did not start: it finds its own
agent by a unique name, checks the selected row, and stops if it is not that
agent's.

## Writing one

- Start from the person's words: "I press `i` and I type", not from a
  function. One journey, one trip.
- Type as a person or a tool does: `type()` sends a burst, `keys()` named
  keys, `paste()` a paste. Waiting for a screen is `sees()`, never a fixed
  sleep followed by one look.
- Ask the agent for answers that cannot be on the screen already, and check
  the answer, not the prompt: `said()` looks for the word in a line the
  agent wrote, whatever its case. Asked for a word in uppercase, a small
  model wrote it in lowercase once in some hundred answers, and a journey
  that waited for the uppercase word failed for that.
- Keep apart what volee does and what the agent says. `answered()` tells
  that the typed text became a turn and that the agent wrote under it; what
  it wrote is another check, which says the answer it got. An agent that
  answers something else is not a defect of volee, and the run must say so
  at a glance.
- Never tell the agent, in one message, how to answer the next ones: "reply
  only OK" in a first message once made a small model answer OK to the
  question that followed.
- A screen is not still because its agent rests: for some twenty seconds
  after a turn, Claude Code shows a hint on its last row, then takes it
  away. And a view of a screen volee does not hold reads it every few
  seconds. To compare a view with a screen, wait for the screen to be still
  (`still()`), give the view the time of a few readings to show it, and
  count the comparison only when the screen has not changed since. A view
  read once, right after the screen, was compared with a screen it had read
  two seconds earlier, and failed one run in several.
- To change an agent's screen without asking the model for anything, type
  in its pane and leave the text in the input box; a line feed (`ctrl+j`)
  inside the text leaves a blank row on the screen. What a model writes
  changes from run to run, and so does how long it takes: Claude Code
  follows a shell command typed with `!` with a turn of the model, which
  comments its output in words of its own.
- A window of a headless tuios session has 80 columns and 24 rows.
  `resize()` gives the interface the size of the terminal the trip needs.
- Add every new thing the journey starts to `agents`, `branches`,
  `background` or `by_hand`, so that `cleanup()` removes it.
- Check what the trip rests on before you make it. A journey that needs an
  older daemon, a session in a pane or an agent in a host says so with a
  check of its own, and stops when it does not hold: a check that fails
  later for that reason reads as a defect of volee, and is not one.
- Run no `volee` command before the daemon you started listens: any command
  starts a daemon when none answers, and it would be the binary under test.
  `start_daemon()` waits on the socket for that reason.
