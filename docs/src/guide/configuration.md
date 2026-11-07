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
setup = "bun install"

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
| `permission_mode` | the harness default | `acceptEdits`, `auto`, `manual`, `dontAsk` or `plan`, passed to Claude Code. volee never passes `bypassPermissions`. |
| `wake_timeout` | `3600` | Seconds an idle agent stays reachable by the wake-up hook. Afterwards, a message resumes the session instead. |

## Worktrees

volee creates each agent's worktree inside its repository, at
`.claude/worktrees/<branch>`, where Claude Code keeps its own, and adds that
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
| `path` | yes | The main checkout. `~` is expanded. |
| `base` | no | The branch work starts from. The repository's default branch when unset. |
| `model` | no | Model for this repository's agents. |
| `setup` | no | A first step agents run in a new worktree, such as `bun install`. |

## `[messages]`

| Key | Default | Meaning |
| --- | --- | --- |
| `summaries` | `false` | Lets volee summarize a thread with a model when you ask (`volee summary`). Each summary runs Claude Code with your account, usually for a few cents. |
| `summary_model` | `haiku` | The model summaries use. |

A summary runs `claude -p` with no tools, no MCP server, none of your hooks
and no saved session, in a directory of volee's own: the model reads the
thread and nothing else.

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
the worktree and `{base}` for the branch the agent started from. An empty
string turns a tool off, even when it is installed:

```toml
[tui.tools]
editor = "hx"                   # instead of $EDITOR
diff = "git difftool -d {base}"
git = ""                        # never offer lazygit
ide = "zed {dir}"
```

Tools that run in the terminal take it over, as taking over an agent does,
and the interface comes back when they exit.
