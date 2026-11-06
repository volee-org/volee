# Command line

Everything in volee goes through one daemon per user. The commands below
start it when they need it, so most of the time you never think about it.

## The daemon

| Command | What it does |
| --- | --- |
| `volee daemon start` | Starts the daemon in the background, unless it already runs. |
| `volee daemon status` | Shows whether it runs, its version, uptime and counters. Never starts it. |
| `volee daemon stop` | Asks the running daemon to stop. Agents keep running: they belong to their harness. |
| `volee daemon run` | Runs the daemon in the foreground, logging to the terminal. Useful to debug. |

The daemon logs to `~/.local/state/volee/logs/daemon.log`. Set `VOLEE_LOG`
to change the level, for example `VOLEE_LOG=debug volee daemon run`.

## Agents

```sh
volee ls          # every agent, the ones that need you first
volee ls --json   # the same, as JSON for scripts
```

Each line shows a symbol and a word for the state, so the list reads without
color:

| Symbol | State |
| --- | --- |
| `!` | needs you (permission, question, dialog) |
| `▸` | working |
| `…` | launching |
| `✓` | done |
| `·` | idle |
| `✗` | failed |
| `■` | stopped |
| `∅` | lost |

## Messages

```sh
volee msg send ag_01jc5… "Also update the changelog."
volee msg send ag_01jc5… "Use TTC." --kind answer
```

A message reaches a working agent at the end of its turn, and wakes an idle
agent at once. Agents you did not launch through volee cannot receive
messages yet.

## Isolated runs

Every path can be moved with an environment variable, which is handy for
trying volee without touching your real state:

| Variable | Default |
| --- | --- |
| `VOLEE_CONFIG_DIR` | `$XDG_CONFIG_HOME/volee`, else `~/.config/volee` |
| `VOLEE_STATE_DIR` | `$XDG_STATE_HOME/volee`, else `~/.local/state/volee` |
| `VOLEE_DATA_DIR` | `$XDG_DATA_HOME/volee`, else `~/.local/share/volee` |
| `VOLEE_SOCKET` | `$XDG_RUNTIME_DIR/volee.sock`, else `<state>/volee.sock` |
