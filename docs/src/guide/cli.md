# Command line

The inbox is in the browser, but the daemon and what it knows are reachable
from the shell. `volee --help` lists every command, and `volee <command> --help`
its options. Commands that need the daemon start it when it is not running.

## The daemon

```sh
volee daemon start      # in the background, if it is not running
volee daemon run        # in the foreground (logs on the terminal)
volee daemon status     # whether it runs, its counters, and what it found: Claude Code, gh
volee daemon stop
volee daemon install    # start it at login (a launchd agent on macOS, a systemd user unit on Linux)
volee daemon uninstall
```

A held permission or question is lost when the daemon stops: Claude Code then
asks in the terminal, and nothing is decided. `daemon install` is optional and
not done for you.

## Connecting Claude Code

```sh
volee doctor                  # hooks into <current dir>/.claude/settings.local.json
volee doctor --project DIR    # ... into another project
volee doctor --user           # ... into ~/.claude/settings.json: every session of this user
volee doctor --remove         # take volee's entries out again
```

`volee doctor` merges `http` hook entries for sixteen events into **one**
settings file, keeps everything else in it, replaces its own entries on a rerun
and never overwrites a file that is not valid JSON. It writes the long timeout
of the `PermissionRequest` hook and a second `PreToolUse` entry for
`AskUserQuestion` from your settings, so run it again after changing
`hooks.permission_timeout` or `held_question_timeout`. It then checks that the
daemon answers and reports errors in the rules files. See
[the Claude Code adapter](../architecture/claude-code.md).

## The interface

```sh
volee ui              # open the inbox in the browser
volee ui --no-open    # only print the link (it carries the access token)
```

## What volee sees

```sh
volee agents          # Claude Code's sessions: state, repository, branch, distance from the base, finished work
volee agents --all    # also sessions whose process ended
volee agents --json
volee diff <agent>    # changes against the branch it started from, by file
volee diff <agent> --full
volee events          # the hook events stored, oldest first
volee events --session <id> --limit 50 --json
```

`volee events` reads the database directly, so it works with the daemon
stopped. An `<agent>` is a name, a short id or an id.

`volee ls` lists the agents of the 0.x registry (the ones that need you first);
for Claude Code sessions use `volee agents`.

## Tidying

```sh
volee tidy <agent>      # remove the worktree of an agent whose work landed; the branch is kept
volee tidy --all --dry-run
volee forget <agent>    # stop listing an agent that no longer runs; sessions, worktree and branch stay
```

`tidy` only removes a worktree on delivery evidence (the work is in the base
branch or its pull request merged), refuses a main checkout, a locked
worktree and one with uncommitted changes, and never deletes a branch
([ADR 0011](../adr/0011-safety-and-delivery-evidence.md)).

## Configuration

```sh
volee config init     # write a commented example config.lua, unless it exists
volee config check    # run the file and summarize what it sets
volee config path
```

## Completions

```sh
volee completions fish > ~/.config/fish/completions/volee.fish
```

bash, elvish, fish, PowerShell and zsh are supported;
[Installation](installation.md) says where each goes.

## Carried over from 0.x

These commands belong to the 0.x registry, which still runs in the daemon. They
are not part of the inbox, and v2 does not launch agents any more, so what is
left of them is limited to agents that joined volee over MCP (see the
[MCP reference](../reference/mcp.md)). They may be removed or reworked before
v2.0.0.

```sh
volee msg send <to> "text"      # a message from you to an agent
volee msg questions             # the questions agents asked you (ask_human), with their offered answers
volee msg answer <question|agent> <number or words>
volee mission new|ls|show|abort|close
volee summary <mission|agent>   # a model's summary of a thread (off unless messages.summaries is set)
volee mcp --agent <id> | --join <name>   # the MCP server of an agent, started by a harness
volee hook <event> --agent <id>          # called by hooks of agents the old tool launched; not for people
```
