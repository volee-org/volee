# Configuration

volee is configured in Lua, in two files in `~/.config/volee/` (or
`$XDG_CONFIG_HOME/volee`, or `$VOLEE_CONFIG_DIR`):

| File | Holds | Read |
| --- | --- | --- |
| `config.lua` | The machine's settings: repositories, the hook port, the review service, git fetching. | When the daemon starts. Restart it after a change. |
| `rules.lua` | How much care a change needs: risk, the layers a card opens with, the review plan, merge, how long a question is held. | Live: the daemon reloads it when it changes. |

They are two files on purpose. The settings are read once and apply to the
whole machine; the rules are decisions about a change, change often, and a
repository can bring its own (`<repo>/.volee/rules.lua`, see
[Rules files](../reference/rules.md)), so they must never be mixed with the
settings a repository file is not allowed to touch. Both run in the same
sandbox (no `io`, no `require`, no network, limited time and memory); a
configuration file that fails to run is an error that says the file and the
line, and the daemon starts with the defaults.

Every setting is optional and a missing file means the defaults. Unknown
keys are errors, so a typo never becomes a setting that silently does
nothing.

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
  agents = {
    model = "opus",
    permission_mode = "auto",
    wake_timeout = 3600,
  },

  repos = {
    billing = {
      path = "~/code/billing",
      base = "develop",
      model = "sonnet",
      setup = { "bun install" },
    },
  },

  hooks = { port = 47835 },
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

## `agents`

| Key | Default | Meaning |
| --- | --- | --- |
| `model` | the harness default | Model for the agents volee launches, unless the repository sets one. |
| `permission_mode` | `manual` | `acceptEdits`, `auto`, `manual`, `dontAsk` or `plan`, passed to Claude Code with every launch and resume. `manual` asks before every tool that needs permission: the agent then waits until you take it over and answer. volee never passes `bypassPermissions`, and never leaves the mode to your own Claude Code default. OpenCode agents get `--auto` when the mode is `auto`, and nothing otherwise. |
| `wake_timeout` | `3600` | Seconds an idle agent's wake-up hook waits for messages. Afterwards, volee types a message at the prompt of an agent in a volee host, once it has rested twenty seconds and nobody types into it; a session that ended is resumed with its messages. |
| `resume_after_reboot` | `true` | Resume, in new volee hosts, the agents a reboot interrupted, the first time the daemon starts afterwards ([after a reboot](cli.md#after-a-reboot)). `false`: they show as stopped, and your next message to one resumes it. |

## Worktrees

volee creates each agent's worktree inside its repository, at
`.claude/worktrees/<name>`, where Claude Code keeps its own. `<name>` is the
branch with every character other than a letter, a digit, `.`, `_` or `-`
replaced by `-`: `feat/invoices` becomes `feat-invoices`. volee adds that
folder to the repository's local exclude file so `git status` stays clean.
The location is not configurable: Claude Code only trusts a session started
from a repository you trust, and never a worktree elsewhere
(see [ADR 0013](../adr/0013-worktrees-live-in-their-repository.md)). Run
`claude` once in each repository you list below and accept its trust prompt.

## `repos`

One entry per repository volee may launch agents in. The key is what you type
on the command line and what agents see; it uses letters, digits, `-`, `_`
and `.`.

| Key | Required | Meaning |
| --- | --- | --- |
| `path` | yes | The main checkout. `~` is expanded. A folder inside another repository is refused, so that agents never land in the repository around it. |
| `base` | no | The branch work starts from. The repository's default branch when unset. |
| `model` | no | Model for this repository's agents. |
| `setup` | no | Commands that prepare each new worktree before its agent starts, such as `["bun install"]`. See below. |
| `merge` | no | How volee merges this repository's pull requests once you approved a mission: `"merge"` (a merge commit), `"squash"` or `"rebase"`. When unset, the first of the three the repository allows on GitHub. A repository without a remote is always merged with a merge commit. |

### Setup commands

A new worktree holds the tracked files only: no dependencies, and none of
the files git ignores. `setup` prepares it before the agent starts
([ADR 0017](../adr/0017-setup-commands-for-new-worktrees.md)):

```lua
return { repos = { shop = {
  path = "~/code/shop",
  setup = {
    'cp "$VOLEE_REPO_ROOT/.env.local" .',
    "bun install",
  },
} } }
```

The daemon runs the commands in order, each with `/bin/sh -c`, in the new
worktree. Besides the daemon's environment, they get `VOLEE_REPO` (the
repository's name), `VOLEE_REPO_ROOT` (its main checkout), `VOLEE_WORKTREE`,
`VOLEE_BRANCH` and `VOLEE_BASE`. Each may run for 15 minutes, after which it
is killed with everything it started. Meanwhile the agent is listed as
launching, with the command that runs; what the commands print goes to
`setup.log` in the agent's folder of volee's state directory as they print
it, so `tail -f` follows it, and `volee spawn` says where it is. Until the
agent's session starts, its preview in the interface and `volee logs` show
the end of that log.

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

Some setups, each checked with `volee config check`:

```lua
-- Bun, and a .env.local that git ignores, from the main checkout.
setup = { 'cp "$VOLEE_REPO_ROOT/.env.local" .', "bun install" }

-- npm, exactly as the lockfile says.
setup = { "npm ci" }

-- Python with uv.
setup = { "uv sync" }

-- A monorepo with a .env.local in several apps and packages.
setup = {
  [[cd "$VOLEE_REPO_ROOT" && find apps packages -maxdepth 2 -name .env.local | while read -r file; do mkdir -p "$VOLEE_WORKTREE/${file%/*}" && cp "$file" "$VOLEE_WORKTREE/$file"; done]],
  "bun install",
}
```

Package managers also run scripts: the project's own and its dependencies'
(`postinstall`, `prepare`). They run in the new worktree, which shares the
repository's git configuration with your main checkout, so a script that runs
`git config` (to set `core.hooksPath`, for example) changes it for every
checkout of the repository. If that matters, skip the scripts
(`bun install --ignore-scripts`, `npm ci --ignore-scripts`) and run the ones
you need as setup commands of their own.

## `messages`

| Key | Default | Meaning |
| --- | --- | --- |
| `summaries` | `false` | Lets volee summarize a thread with a model when you ask (`volee summary`). Each summary runs Claude Code with your account, usually for a few cents. |
| `summary_model` | `haiku` | The model summaries use. |

A summary runs `claude -p` with no tools, no MCP server, none of your hooks
and no saved session, in a directory of volee's own: the model reads the
thread and nothing else.

## `missions`

| Key | Default | Meaning |
| --- | --- | --- |
| `max_workers` | `8` | The most agents one mission may launch besides its coordinator, counting those that already stopped. |

A coordinator launches workers with its `spawn_worker` tool, without asking
you, so this limit is what keeps a coordinator gone wrong from starting
sessions without end. Launches that run at the same time all count. A
coordinator that reaches the limit gets a refusal that tells it to ask you.
Workers also stay in the repositories the mission names: volee compares their
main checkouts, never their directory names.

## `review`

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Whether the daemon reviews finished work at all. |
| `auto` | `true` | `true`: a branch is reviewed as soon as its agent has finished work. `false`: only when asked (`POST /api/agents/{id}/review`). |
| `timeout` | `300` | Seconds one reviewer may run before it is stopped. |
| `concurrency` | `1` | Reviews running at once. |
| `command` | `"claude"` | The Claude Code program to run. |

The review is Claude Code's `/code-review`, run read-only in the agent's
worktree. Its findings are an opinion on the card, never a gate. The model
and the number of reviewers come from the rules (`review.model`,
`review.reviewers` in `rules.lua`; more reviewers when the risk is high), not
from this table. A branch tip is reviewed once; see
[ADR 0039](../adr/0039-the-pre-review-is-claude-codes-code-review-and-an-opinion.md).

## `git`

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

## `hooks`

| Key | Default | Meaning |
| --- | --- | --- |
| `port` | `47835` | The port on `127.0.0.1` where the daemon receives Claude Code's `http` hooks. `volee doctor` writes it into the hook settings: run it again after a change. |
| `permission_timeout` | `3600` | Seconds a permission request waits for your decision in the browser. Written as the hook's timeout by `volee doctor`; afterwards Claude Code asks in the terminal as usual. |

How long a **question** (`AskUserQuestion`) is held is not here any more: it
is the `held_question_timeout` setting of the rules, so a repository or you
can set it per project. `volee doctor` writes it, plus a margin, as the hook's
timeout, so run it again after changing it.
