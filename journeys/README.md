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
- Python 3.9 or later (the scripts use the standard library only).
- A **sandbox**: a git repository that Claude Code already trusts (run
  `claude` in it once and accept). The journeys make throwaway branches and
  worktrees in it, named `journey-...`, and remove them.

## Running them

```sh
cargo build --release -p volee-cli
python3 journeys/open.py      --volee target/release/volee --sandbox ~/code/sandbox
python3 journeys/pane.py      --volee target/release/volee --sandbox ~/code/sandbox
python3 journeys/edit_mode.py --volee target/release/volee --sandbox ~/code/sandbox
python3 journeys/old_host.py  --volee target/release/volee --old-volee ~/bin/volee-0.9.1 --sandbox ~/code/sandbox
python3 journeys/installed.py --sandbox ~/code/sandbox --expect 0.11.0
```

Each prints one `PASS` or `FAIL` line per check, the screens it saw at the
steps that matter, and exits with status 0 only when every check passed.
`--keep` leaves the tuios session, the agent and the state in place so that
you can look (`tuios attach volee-journey-...`).

| Journey | What it does | Whose daemon |
| --- | --- | --- |
| `open.py` | Starts a Claude Code background session by hand, compares volee's list with what Claude Code lists (every session one row, no row without a session), opens the session by Enter, types into it, checks that it is the same session in the same process, leaves, opens it again once stopped, and removes it. | Its own, isolated |
| `pane.py` | Starts Claude Code by hand in a tuios pane: it shows in the list after ten seconds, Enter opens the pane inside volee, typing reaches it, `/clear` keeps its row, and quitting it takes it off the list. | Its own, isolated |
| `edit_mode.py` | Launches an agent, shows it full screen, types into it in edit mode, sends Esc, Ctrl+Z and a two-line paste, leaves with Ctrl+Q, then checks in a pseudo-terminal that the agent follows the panel's size. | Its own, isolated |
| `old_host.py` | The same typing on an agent whose host an older release started (hosts outlive upgrades). | Its own, started by the older binary |
| `installed.py` | The installed volee with your own daemon and your own list: compares the list with what Claude Code lists, launches one throwaway agent and types into it, starts one background session by hand and opens it by Enter, then removes both. Run it after installing a release. | Yours |

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
- Ask the agent for answers that cannot be on the screen already (a word in
  uppercase), and check the answer, not the prompt.
- Add every new thing the journey starts to `agents`, `branches`,
  `background` or `by_hand`, so that `cleanup()` removes it.
