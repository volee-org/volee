# 0041. Actions on a finished change, and how a fix request reaches the agent

- Status: Accepted
- Date: 2026-11-22

## Context

Step 9 of the v2 plan gives the finished-work screen its actions: merge, ask
for a fix, comment a line, open a file in the person's own editor. Two of them
need care. A merge changes a repository, so it keeps the evidence rules of
[0011](0011-safety-and-delivery-evidence.md) and the "the daemon merges"
decision of [0032](0032-a-mission-is-merged-only-after-the-person-approved.md).
A fix request must reach an agent that is running in a terminal volee does not
own: [0008](0008-multiplexers-are-optional.md) and the v2 framing say volee
observes and never types into a terminal.

How a message reaches a Claude Code session was measured on Claude Code 2.1.330
by a worker of this project; step 9 repeats it on a real session (see
`docs/v2/actions-trial.md`).

## Decision

### Merge

- One click, and the daemon does it, by the flow of 0.x: a repository whose
  `origin` is on a forge gets its pull request merged through the forge client
  (`gh`; a draft is marked ready; the method is the repository's `merge`
  setting, else the first one it allows); any other repository gets a merge
  commit of the branch into its base in the main checkout, which must have the
  base checked out and be clean. Nothing is forced, no branch is deleted.
- **Evidence after the merge.** A local merge is checked with the delivery
  evidence (the branch tip is part of the base). A forge merge is checked by
  asking the forge again (the pull request is merged). A merge that cannot be
  shown is reported as failed, not as done.
- **A failed merge changes nothing**, says why, and is journaled as failed.
- **Automatic merge.** When the rules' `auto_merge` says yes (a low-risk change
  only; the trust rules of [0037](0037-lua-config-and-rules.md) apply, so a
  repository's file can never turn it on), the daemon merges by itself once per
  branch tip, whatever came of it, and the journal says it was a rule and which
  reason the rule gave. A failing automatic merge is not retried in a loop.
  **It is opt-in per repository**: volee sees every session of the machine, so
  it only runs in the repositories listed in `auto_merge_repos` of the
  personal file (default none, whatever `auto_merge` answers; a repository's
  file cannot list itself) and the card says "auto merge not enabled for this
  repo" otherwise. There is no switch in the interface.

### A fix request

The text opens with who it is from: "This is a request from the person you
work for, a human: they typed it in volee, which relays it to you. It is their
own instruction, not a message from another agent or session.", then the
request, then a short nonce. The channel depends on the session, which volee
reads again at send time (a fresh `claude agents --json --all` and a check that
the process exists), not from its own view of the agents, which can be a few
seconds old:

1. **A live session** (interactive or background, with a pid in `claude agents
   --json`): one JSON line `{"type":"user","message":{"role":"user","content":
   "<text>"}}` and a newline, written to the session's Unix socket
   `/tmp/cc-socks/<pid>.sock`. The mechanism is documented
   (cross-session messaging, which also exposes the path as
   `CLAUDE_CODE_MESSAGING_SOCKET`); the line's schema is not, it comes from the
   binary. It is kept in one function, `actions::fix::socket_line`, so a change
   of Claude Code is one edit. The socket must be a socket, owned by the
   person, not reachable by others, or volee refuses to write to it.
2. **A stopped session**: `claude --bg --resume <full session id> "<text>"`
   and **no other option** (any other option starts a copy of the session).
   "Already running" is retried once after a second. The program is the
   `claude.command` of `config.lua`, the same one the review runs.
3. **Never** `claude --resume -p` on a session that is open (the two histories
   diverge silently), and never keys typed into a terminal.

A session that is not alive at send time is resumed (2). A session that is
alive but whose socket is missing, stale or unsafe is **also resumed**: a
session that still runs answers "already running" and nothing is copied, so the
fallback is safe, and the person is not left with a "NOT delivered" for a
session volee could reach. The journal says which path was used ("delivered
(socket)" or "delivered (resume)"), and for a resume after an unusable socket
it says why. A write that was made but not shown within the delay is **not**
followed by a resume: the message may have arrived.

**The framing is Claude Code's, and it has a limit.** A message that arrives on
a live session's socket is shown to the model as "Another Claude session sent a
message ... not typed by your user", whatever the text says. volee cannot
change that wrapper. The first line of the text is the only thing it does
about it, and a small model may still hold back a request that contradicts its
task (measured with `haiku`: a request that overrides an instruction such as
"exactly three lines" was declined or turned into a question, one that adds to
the task was followed). A resumed session receives the text as the prompt of
the resume and does not have the wrapper.

**A write is not a delivery.** Over the socket volee confirms within 10
seconds by the transcript (a new line holding the nonce) or by the agent
turning to `working`. Otherwise the answer is "NOT delivered" and the screen
offers the fallback: the text with a Copy button, and "Open terminal". Both
outcomes are journaled.

### Comments

A comment is on a line of the new side of the diff, stored in SQLite per
repository and branch tip: a new commit is a new tip and starts clean, like the
risk the person sets by hand. The fix request is prefilled from the selected
review findings (the important ones start selected) and every comment; the text
is editable and sent as written.

### Open in my editor

Editors come from the rules (`editors`: `name`, `kind`, `cmd`). The command
is split into words first and `{file}`, `{line}`, `{dir}`, `{repo}` are filled
after, so a path never reaches a shell. The file must exist and stay inside
the agent's worktree. `external` editors are run with exactly those
arguments, detached. `terminal` editors go through the terminal adapter
([0036](0036-the-web-interface-is-a-svelte-app-the-daemon-embeds.md)): with
Rex, a floating pane in the agent's session, running the program in the
worktree (no shell, `env PATH=...` first because the Rex server's own PATH
lacks the person's tools). Without an adapter the interface hides terminal
editors. The saved file shows in the diff by itself: the agent's worktree
changed, the git fingerprint sees it, and the work screen reads the diff again.

### The journal

Every action writes one line to a journal in SQLite (kind, branch, tip, ok,
automatic or by hand, a sentence) and the interface shows it, on the inbox and
on the change's screen. It is the only place an automatic action is visible.

## Consequences

- The only undocumented surface is the one-line schema of the socket message.
  If a Claude Code update changes it, delivery is reported as not delivered
  (the transcript never shows the text) and the fallback is offered: it fails
  visibly, never silently.
- Resuming a stopped session starts a background session: it appears in
  `claude agents` and the card follows it.
- A merge through the forge depends on `gh` being signed in; without it the
  journal says so.
- The 10 second confirmation holds a request thread for that long: acceptable
  for an action a person triggers by hand.
