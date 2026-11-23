# Configuration

volee is configured in Lua, in two files in `~/.config/volee/` (or
`$XDG_CONFIG_HOME/volee`, or `$VOLEE_CONFIG_DIR`):

| File | Holds | Read |
| --- | --- | --- |
| `config.lua` | The machine's settings: repositories, the hook port, the review program, git fetching. | When the daemon starts. Restart it after a change. |
| `rules.lua` | How much care a change needs: risk, the layers a card opens with, the review plan, merge, how long a question is held. | Live: the daemon reloads it when it changes. |

They are two files on purpose. The settings are read once and apply to the
whole machine; the rules are decisions about a change, change often, and a
repository can bring its own (`<repo>/.volee/rules.lua`, see
[Rules files](../reference/rules.md)), so they must never be mixed with the
settings a repository file is not allowed to touch. Both run in the same
sandbox (no `io`, no `require`, no network, limited time and memory); a
configuration file that fails to run is an error that says the file and the
line, and the daemon starts with the defaults.

Every setting is optional and a missing file means the defaults. Unknown keys
are errors, so a typo never becomes a setting that silently does nothing.

```sh
volee config init    # write a commented example config.lua
volee config check   # run the file and show what it sets
volee config path    # print where the file is
volee doctor         # also reports errors in the rules files
```

A leftover `config.toml` from volee 0.x is not read and not converted: volee
prints a one-line warning that it is ignored.

## Example

```lua
return {
  repos = {
    billing = {
      path = "~/code/billing",
      base = "develop",
      merge = "squash",
    },
  },

  hooks = { port = 47835, permission_timeout = 3600 },

  review = { enabled = true, auto = true },
  claude = { command = "/Users/me/.local/bin/claude" },
}
```

Because it is Lua, a value can be computed:

```lua
local repos = {}
for _, name in ipairs({ "api", "web", "docs" }) do
  repos[name] = { path = "~/code/" .. name }
end
return { repos = repos }
```

The file is data: a function in it is an error. Decisions belong in
`rules.lua`.

## `repos`

Optional. One entry per repository you want to name. The key is a name made of
letters, digits, `-`, `_` and `.`. volee finds the repository of every session
by its working directory whether it is listed or not; an entry only adds a
name and settings.

| Key | Meaning |
| --- | --- |
| `path` | The main checkout. `~` is expanded. Required. |
| `base` | The branch work is compared with, and merged into. The repository's default branch when unset. |
| `merge` | How a pull request of this repository is merged: `"merge"` (a merge commit), `"squash"` or `"rebase"`. When unset, the first of the three the repository allows on GitHub. A repository without a remote is always merged with a merge commit. |

## `hooks`

| Key | Default | Meaning |
| --- | --- | --- |
| `port` | `47835` | The port on `127.0.0.1` where the daemon receives Claude Code's `http` hooks and serves the interface. `volee doctor` writes it into the hook settings: run it again after a change. |
| `permission_timeout` | `3600` | Seconds a permission request waits for your decision on the page. Written as the hook's timeout by `volee doctor`; afterwards Claude Code asks in the terminal as usual. |

How long a **question** (`AskUserQuestion`) is held is not here: it is the
`held_question_timeout` setting of the rules, so a repository or you can set it
per project (ten minutes by default). `volee doctor` writes it, plus a margin,
as the hook's timeout, so run it again after changing it.

## `review`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Whether the daemon reviews finished work at all. |
| `auto` | `true` | `true`: a branch is reviewed as soon as its agent has finished work. `false`: only when asked (the Run button of the review layer). |
| `timeout` | `300` | Seconds one reviewer may run before it is stopped. |
| `concurrency` | `1` | Reviews running at once. |

The review is Claude Code's `/code-review`, run read-only in the agent's
worktree. Its findings are an opinion on the card, never a gate. The model and
the number of reviewers come from the rules (`review.model`,
`review.reviewers`, and `review.high_reviewers` for a high risk change), not
from this table. A branch tip is reviewed once; see
[ADR 0039](../adr/0039-the-pre-review-is-claude-codes-code-review-and-an-opinion.md).

## `claude`

| Key | Default | Meaning |
| --- | --- | --- |
| `command` | `"claude"` | The Claude Code program volee runs itself: the review, and the `claude --bg --resume` of a fix request for a stopped session. Set it when `claude` is not on the daemon's `PATH`. |

## `git`

| Key | Default | Meaning |
| --- | --- | --- |
| `fetch` | `"launched"` | Which repositories the daemon fetches in the background: `"launched"`, `"all"` or `"never"`. |

This setting concerns the agents that the registry carried over from 0.x
still tracks (those a mission or an MCP join made). The sessions volee observes
through Claude Code are your own: volee reads their repository and never fetches
it, so a merge is as current as your last `git fetch` (or, for a squash, as the
pull request lookup, see [Git](../architecture/git.md)). When it does fetch, it
runs `git fetch --prune origin` at most every five minutes, in batch mode: a
fetch that needs a password fails instead of waiting, and only your
remote-tracking branches change.

## `agents`

| Key | Default | Meaning |
| --- | --- | --- |
| `poll_interval` | `5` | Seconds between two listings of Claude Code's sessions (`claude agents --json`). A stored hook event asks for a new listing after 300 ms anyway. |

## `messages`

| Key | Default | Meaning |
| --- | --- | --- |
| `summaries` | `false` | Lets `volee summary` summarize a thread with a model. Each summary runs Claude Code with your account. |
| `summary_model` | `haiku` | The model summaries use. |

## Keys of 0.x that are gone

volee 0.x had keys that v2 never reads: `agents.model`, `agents.permission_mode`,
`agents.wake_timeout`, `agents.resume_after_reboot`, `repos.<name>.model`,
`repos.<name>.setup` and `missions`. They were launch settings, and v2 does not
launch agents. They are no longer accepted: like any unknown key, one of them
makes `config.lua` fail to load with a message that names it (`unknown field
`wake_timeout``, with the file), so delete it from an old file. `volee config` reports
the error; the daemon logs a warning and runs with the defaults until the file
is fixed.
