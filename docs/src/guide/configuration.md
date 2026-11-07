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

[worktrees]
root = "~/.local/share/volee/worktrees"

[repos.billing]
path = "~/code/billing"
base = "develop"
model = "sonnet"
setup = "bun install"

[tui]
theme = "catppuccin-mocha"

[tui.keys]
take-over = "ctrl-a"
```

## `[agents]`

| Key | Default | Meaning |
| --- | --- | --- |
| `model` | the harness default | Model for the agents volee launches, unless the repository sets one. |
| `permission_mode` | the harness default | `acceptEdits`, `auto`, `manual`, `dontAsk` or `plan`, passed to Claude Code. volee never passes `bypassPermissions`. |
| `wake_timeout` | `3600` | Seconds an idle agent stays reachable by the wake-up hook. Afterwards, a message resumes the session instead. |

## `[worktrees]`

| Key | Default | Meaning |
| --- | --- | --- |
| `root` | `~/.local/share/volee/worktrees` | Where volee creates worktrees, one folder per repository. |

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

## `[tui]`

| Key | Default | Meaning |
| --- | --- | --- |
| `theme` | `catppuccin-mocha` | A built-in theme or a file in `~/.config/volee/themes/`; see [Themes](themes.md). |
| `reduced_motion` | `false` | Stop animations: a new message lights up its line on the [network](interface.md#the-network) instead of travelling along it, and spinners stop. |
| `keys` | none | Key overrides by action name, such as `take-over = "ctrl-a"`; the [interface guide](interface.md#keys) lists the actions. |
