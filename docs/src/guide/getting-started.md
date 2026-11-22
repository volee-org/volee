# Getting started

This takes about five minutes. You need volee installed (see
[Installation](installation.md)), Claude Code 2.1.330 or later, and a
repository where you run Claude Code.

## 1. Start the daemon

```sh
volee daemon start
volee daemon status
```

The daemon is a small local process. It receives Claude Code's hooks on
`127.0.0.1` (port 47835 by default), keeps what it learns in SQLite, and
serves the inbox. `volee daemon status` says whether it runs, and whether it
found Claude Code and `gh`.

## 2. Hook Claude Code to the daemon

Claude Code only tells volee what happens if its settings say where to post.
In the project you work in:

```sh
cd ~/code/my-project
volee doctor
```

`volee doctor` writes the hook entries into that project's
`.claude/settings.local.json` (the file Claude Code keeps out of version
control) and checks that the daemon answers. It prints what it wrote, never
touches another file, and keeps every other setting and every hook that is not
volee's. To cover every project of your user in one go, run
`volee doctor --user`, which writes `~/.claude/settings.json` instead.
`volee doctor --remove` takes the entries out again.

Hooks are read when a session starts: sessions that were already open keep
working without volee until they restart.

## 3. Open the inbox

```sh
volee ui
```

This opens a page in your browser at `http://127.0.0.1:47835/`. The link
carries a token in its fragment, so the page can talk to the daemon, and only
works on this machine. Keep the tab open: it updates by itself.

With nothing to decide it says "Nothing needs you". Start a Claude Code
session in the project and ask it for something that needs a permission, for
example to run a shell command. A permission card appears within a second,
with the tool and its input. Allow it from the page: the agent goes on, and you
did not touch its terminal.

## 4. See what a finished change looks like

Ask an agent to make a small change in a git worktree (for example
`claude --worktree my-change`) and commit it. When it stops, a **finished work**
card appears. **Read the work** opens it in layers; the
[interface guide](interface.md) walks through them. Merging is one click, or
none, if you decided so in your [rules](../reference/rules.md).

## 5. Tune it

Nothing here is required, and the defaults are cautious:

```sh
volee config init     # a commented config.lua: the machine's settings
```

Then write a `rules.lua` to say what is risky in your repositories:

```lua
-- ~/.config/volee/rules.lua
return {
  sensitive_paths = { "src/payments/**", "infra/**" },
  auto_merge = true,                   -- only where you list it, see below
  auto_merge_repos = { "~/code/docs-site" },
}
```

The [rules reference](../reference/rules.md) has every setting.
`volee doctor` reports errors in the rules files, and the page shows a banner
when one is broken.

## From the command line

The inbox is the main screen, but everything it knows is reachable from the
shell:

```sh
volee agents          # Claude Code's sessions, their state and finished work
volee events          # the hook events the daemon stored
volee diff <agent>    # an agent's changes against its base branch
```

See the [command line reference](cli.md).

## What volee never does

- It never starts an agent, holds a terminal or types into one.
- It never edits your Claude Code settings except the one file `volee doctor`
  names, and only on your command.
- It never answers a permission or a question without you, unless you wrote a
  rule that merges low risk changes in a repository you listed. A question or a
  permission nobody answers is handed back to Claude Code's own dialog in the
  terminal.
- It never deletes a branch, and never forces a merge.
- It never sends anything off your machine: the daemon listens on the loopback
  interface only. (The AI review runs Claude Code itself, with your account.)
