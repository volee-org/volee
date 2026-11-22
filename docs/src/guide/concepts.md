# Concepts

volee uses a handful of words with precise meanings. Knowing them makes the
inbox and the documentation easier to read.

## Agent

A Claude Code session that volee sees through `claude agents --json`: an
interactive session in a terminal or a background session. An agent has a
name, a **state** (working, idle, blocked or waiting for you), a working
directory and, when that directory is in a git repository, a **branch** and the
**base branch** it started from. An agent is not a terminal and not a process
volee owns: volee never starts one. A `claude -p` run does not appear, because
Claude Code does not list it.

## Card

One thing in the inbox that needs you. There are three kinds:

| Card | What triggers it | What you decide |
| --- | --- | --- |
| **Permission** | Claude Code asks permission to use a tool (`PermissionRequest` hook) | Allow once, allow with one of the rules Claude Code suggests, or deny with a message the model reads |
| **Question** | The agent asks you to choose or write (`AskUserQuestion`) | Pick the proposed answers or write your own |
| **Finished work** | The agent stopped and its branch holds commits that are not in the base branch | Read it, then merge, ask for a fix, comment, or leave it |

An agent that waits for something volee cannot hold (a dialog of another kind)
shows as a plain **waiting** card. Every other running agent is only counted.

## Held decision

A permission or a question that volee holds open: the hook request stays
unanswered until you decide on the page. Claude Code shows its own dialog in
the terminal at the same time for a permission, and whoever answers first wins;
the card goes in every case. A question is held for a short time (ten minutes by
default, `held_question_timeout` in your rules), then handed back to the
terminal. "Ask in terminal" hands either kind back at once. If the daemon stops,
Claude Code asks in the terminal: nothing is decided and nothing is lost.

## Risk and layers

Every finished change gets a **risk** (`low`, `medium` or `high`) with the
reasons, computed by your [rules](../reference/rules.md) from the paths touched,
the size, migrations, dependency files, untested code and review findings. The
risk picks the **layers** the work screen opens with, the review plan (the
model and how many reviewers) and whether a merge may be automatic. You can
raise or lower the risk of a change by hand on its card; the choice lasts until
the next commit.

## Layers

A finished change is read in layers, each opened only if needed: the summary,
the AI review, the plan followed, the open points, the files and why, a diagram
of the change and the guided diff.

## Pre-review

Claude Code's own `/code-review`, run read-only in the agent's worktree when
finished work appears. Its findings are an **opinion**, shown on the card and
under the lines of the diff. They never block a merge by themselves.

## Journal

Every action volee takes for you (a merge, a fix request, an editor opened) and
every merge it did by itself writes one line: what, which branch and tip,
whether it worked, whether it was you or a rule, and a sentence. It is the only
place an automatic action is visible.

## Delivery evidence

What volee can prove about an agent's work: commits since the base, whether
the branch is pushed, whether its pull request was merged, whether its tip is
part of the base. Merging and tidying rely on evidence, never on the absence of
a window or a process. See [Git](../architecture/git.md).

## Rules and configuration

Two Lua files, on purpose: `config.lua` holds the machine's settings and is read
when the daemon starts; `rules.lua` (one per repository, one personal) holds
decisions about a change and is reloaded live. See
[Configuration](configuration.md) and [Rules files](../reference/rules.md).
