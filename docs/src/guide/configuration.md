# Configuration

volee reads `~/.config/volee/config.toml` (or `$XDG_CONFIG_HOME/volee`, or
`$VOLEE_CONFIG_DIR`). Every setting is optional and a missing file means the
defaults.

```sh
volee config init    # write a commented example
volee config check   # validate the file and show what it sets
volee config path    # print where the file is
```

Unknown keys are errors, reported with their line, so a typo never turns into
a setting that silently does nothing.

## Example

```toml
[agents]
model = "opus"
permission_mode = "auto"
wake_timeout = 3600

[repos.billing]
path = "~/code/billing"
base = "develop"
model = "sonnet"
setup = ["bun install"]

[tui]
theme = "catppuccin-mocha"

[tui.keys]
take-over = "ctrl-a"

[tui.tools]
git = "gitui"
```

## `[agents]`

| Key | Default | Meaning |
| --- | --- | --- |
| `model` | the harness default | Model for the agents volee launches, unless the repository sets one. |
| `permission_mode` | `manual` | `acceptEdits`, `auto`, `manual`, `dontAsk` or `plan`, passed to Claude Code with every launch and resume. `manual` asks before every tool that needs permission: a background agent then waits until you take it over and answer. volee never passes `bypassPermissions`, and never leaves the mode to your own Claude Code default. |
| `wake_timeout` | `3600` | Seconds an idle agent stays reachable by the wake-up hook. Afterwards, a message resumes the session instead. |

## Worktrees

volee creates each agent's worktree inside its repository, at
`.claude/worktrees/<name>`, where Claude Code keeps its own. `<name>` is the
branch with every character other than a letter, a digit, `.`, `_` or `-`
replaced by `-`: `feat/invoices` becomes `feat-invoices`. volee adds that
folder to the repository's local exclude file so `git status` stays clean.
The location is not configurable: Claude Code only trusts a background
session started from a repository you trust, and never a worktree elsewhere
(see [ADR 0013](../adr/0013-worktrees-live-in-their-repository.md)). Run
`claude` once in each repository you list below and accept its trust prompt.

## `[repos.<name>]`

One table per repository volee may launch agents in. `<name>` is what you type
on the command line and what agents see; it uses letters, digits, `-`, `_`
and `.`.

| Key | Required | Meaning |
| --- | --- | --- |
| `path` | yes | The main checkout. `~` is expanded. A folder inside another repository is refused, so that agents never land in the repository around it. |
| `base` | no | The branch work starts from. The repository's default branch when unset. |
| `model` | no | Model for this repository's agents. |
| `setup` | no | Commands that prepare each new worktree before its agent starts, such as `["bun install"]`. See below. |

### Setup commands

A new worktree holds the tracked files only: no dependencies, and none of
the files git ignores. `setup` prepares it before the agent starts
([ADR 0017](../adr/0017-setup-commands-for-new-worktrees.md)):

```toml
[repos.shop]
path = "~/code/shop"
setup = [
  "cp \"$VOLEE_REPO_ROOT/.env.local\" .",
  "bun install",
]
```

The daemon runs the commands in order, each with `/bin/sh -c`, in the new
worktree. Besides the daemon's environment, they get `VOLEE_REPO` (the
repository's name), `VOLEE_REPO_ROOT` (its main checkout), `VOLEE_WORKTREE`,
`VOLEE_BRANCH` and `VOLEE_BASE`. Each may run for 15 minutes, after which it
is killed with everything it started. Meanwhile the agent is listed as
launching, with the command that runs; what the commands print goes to
`setup.log` in the agent's folder of volee's state directory as they print
it, so `tail -f` follows it, and `volee spawn` says where it is.

If a command fails, the launch fails: no session starts, the worktree and
branch stay, and the agent is listed as failed with the command and the last
line it printed. Fix the cause and run the same `volee spawn` again: volee
reuses the worktree while git sees it clean, and runs the setup again, so
keep the commands safe to run twice. Commands that write only files git
ignores, such as dependencies or a `.env.local` listed in `.gitignore`, keep
the worktree clean; if the setup left other files, remove the worktree and
its branch before launching again. If the agent's mission ends during the
setup, the launch stops and no session starts. A mission's coordinator works
in the main checkout and runs no setup.

## `[messages]`

| Key | Default | Meaning |
| --- | --- | --- |
| `summaries` | `false` | Lets volee summarize a thread with a model when you ask (`volee summary`). Each summary runs Claude Code with your account, usually for a few cents. |
| `summary_model` | `haiku` | The model summaries use. |

A summary runs `claude -p` with no tools, no MCP server, none of your hooks
and no saved session, in a directory of volee's own: the model reads the
thread and nothing else.

## `[missions]`

| Key | Default | Meaning |
| --- | --- | --- |
| `max_workers` | `8` | The most agents one mission may launch besides its coordinator, counting those that already stopped. |

A coordinator launches workers with its `spawn_worker` tool, without asking
you, so this limit is what keeps a coordinator gone wrong from starting
sessions without end. Launches that run at the same time all count. A
coordinator that reaches the limit gets a refusal that tells it to ask you.
Workers also stay in the repositories the mission names: volee compares their
main checkouts, never their directory names.

## `[git]`

| Key | Default | Meaning |
| --- | --- | --- |
| `fetch` | `"launched"` | Which repositories the daemon fetches in the background: `"launched"` (those of agents volee launched), `"all"` (also those of sessions volee only discovered) or `"never"`. |

To tell whether an agent's commits are pushed, and whether its branch was
merged, volee compares the branch with `origin`. The daemon therefore runs
`git fetch --prune origin` in an agent's repository at most every five
minutes, in batch mode: a fetch that needs a password fails instead of
waiting, and only your remote-tracking branches change (`--prune` forgets the
ones deleted on the remote; `FETCH_HEAD` is left alone).

By default it fetches only the repositories of agents volee launched: their
remote is part of the work volee started. A session you started yourself is
yours, and so is its remote: volee reads its repository but leaves fetching
to you, and compares with what your last fetch brought. Set `"all"` to have
those repositories fetched too, or `"never"` to fetch only by hand (for
example on a metered connection, or with remotes that need a hardware key).
With fetches off, push states and merges are as current as your last fetch,
and new agents start from `origin/<base>` as that fetch left it.

## `[tui]`

| Key | Default | Meaning |
| --- | --- | --- |
| `theme` | `catppuccin-mocha` | A built-in theme or a file in `~/.config/volee/themes/`; see [Themes](themes.md). |
| `reduced_motion` | `false` | Stop animations: a new message lights up its line on the [network](interface.md#the-network) instead of travelling along it, and spinners stop. |
| `keys` | none | Key overrides by action name, such as `take-over = "ctrl-a"`; the [interface guide](interface.md#keys) lists the actions. |
| `tools` | found on the machine | Programs `o` opens an agent's worktree with; see below. |

### `[tui.tools]`

The interface can open an agent's worktree in your own tools. None is
required and none is forced: volee offers the ones it finds, and each key
below replaces what it would find.

| Key | Found when unset | Runs |
| --- | --- | --- |
| `editor` | `$VISUAL`, then `$EDITOR`, with `.` | in this terminal |
| `diff` | `nvim -c 'DiffviewOpen {base}'`, when diffview.nvim is installed | in this terminal |
| `git` | `lazygit`, when installed | in this terminal |
| `ide` | `code {dir}`, when VS Code's `code` is installed | in a window of its own, in the background |

Each value is a command line, split like a shell would (quotes, no
variables or pipes) and run in the agent's worktree. `{dir}` stands for
the worktree and `{base}` for the branch the agent started from, each
passed as one argument. Keep them out of `sh -c '...'`: the base may come
from a coordinator agent (volee only accepts a plain branch name, but a
shell is still the wrong place for text an agent chose). An empty string
turns a tool off, even when it is installed:

```toml
[tui.tools]
editor = "hx"                   # instead of $EDITOR
diff = "git difftool -d {base}"
git = ""                        # never offer lazygit
ide = "zed {dir}"
```

Tools that run in the terminal take it over, as taking over an agent does,
and the interface comes back when they exit.
