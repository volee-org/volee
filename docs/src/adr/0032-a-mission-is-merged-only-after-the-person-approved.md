# 0032. A mission is merged only after the person approved its report

- Status: Accepted (amends [0007](0007-missions.md) and [0011](0011-safety-and-delivery-evidence.md))
- Date: 2026-11-11

## Context

A mission ended when its coordinator said so. Once every worker had
delivered, the coordinator sent a final report and closed the mission
([ADR 0007](0007-missions.md)). Three things were missing at that end.

- **Nothing was merged, and nobody was asked.** "Delivered" meant a pushed
  branch with an open pull request. The person found the pull requests
  later, in a closed mission, and merged them by hand, one repository at a
  time. A coordinator that had a shell could also merge on its own: nothing
  in volee said whether it should.
- **Closing tidied nothing.** A closed mission left its workers' worktrees
  and its coordinator's session where they were, for the person to clean up
  agent by agent.
- **A repository without a remote could not deliver.** With nowhere to push
  and no pull request to open, its workers stayed "not delivered" for ever,
  and so did their mission.

The rules a person wants here are known: they were written for the tool
volee replaces, as the conduct of its coordinator. At the end of a mission
the coordinator merges nothing and closes nothing. It presents its report,
what is done, the proof, what is left, as a question to the person with two
answers: "ok merge", or "no". Only "ok merge" starts the merge, then the
close. On "no", or without an answer, the mission stays open. And on the
close, everything is tidied in one gesture, without deleting a branch or
forcing uncommitted work.

Those rules do not say who makes the merge. The first version of this
decision gave it to the coordinator, with volee checking the result. It was
built and run with real agents on 2026-11-11, and a measurement settled the
question: in the third mission run this way, Claude Code's auto mode
refused the coordinator's `git merge --no-ff <branch>` ("Merge Without
Review"), after allowing the same command in the two missions before. The
person had said
"ok merge", but to volee: their answer reaches the agent as a message a
hook relays, and the agent's harness does not take that for its user's
consent. The coordinator did what it should, it stopped and said so, and
the mission could not end. An agent's merge depends on what its harness
lets it run, which volee does not control and must not work around.

## Decision

A mission's work is merged, and the mission closed, only after the person
approved the report its coordinator presented. The approval is a recorded
answer that the daemon reads, and the daemon carries it out: it merges the
work itself, closes the mission and tidies it. No agent merges or closes.

- **The report is a question.** When every worker delivered, the supervisor
  asks the coordinator to check the work and present its report with the MCP
  tool `request_approval`: three lines, what is done, the proof, what is
  left. The daemon posts it as a question from the coordinator to the
  person, in the mission's thread, with the answers it offers
  ([ADR 0031](0031-a-question-may-offer-its-answers.md)): `ok merge` and
  `no` for a change mission, `ok close` and `no` for an analysis mission,
  which has nothing to merge. The lead that asked for the mission, when
  there is one, reads the same report. The mission is then
  `awaiting-approval`: it shows as waiting for the person, and the question
  sits on top of their messages.
- **Only the person's own approving answer approves.** The supervisor reads
  the person's answer to that question: the approving option, chosen
  exactly, makes the mission `approved`; anything else, "no" or a sentence,
  puts it back to `reporting` and tells the coordinator that nothing is
  merged, to do what the person asks and to present its report again. Only
  the answer to the report presented last counts, and only while the
  mission waits for it. A reply from an agent is not the person's answer.
- **volee merges, on the person's word.** Once a change mission is
  approved, the daemon merges the work of every worker whose work has not
  landed, one after the other:
  - in a repository without a remote, the worker's branch into its base
    branch, in the main checkout, as a merge commit (`git merge --no-ff`).
    The main checkout must have the base branch checked out and no
    uncommitted change to a tracked file; volee does not switch branches or
    stash for the person;
  - in a repository with a remote, the pull request at the tip of the
    worker's branch, on the forge, with the person's own forge client: a
    draft is marked ready, then the pull request is merged with the method
    the repository is configured with (`merge` in `[repos.<name>]`), or the
    first one the repository allows among a merge commit, a squash and a
    rebase.

  It reads the evidence from git and the forge first, so that a pull
  request the person merged themselves meanwhile is not merged again, and
  again afterwards: a merge the evidence does not show is not a merge
  ([ADR 0011](0011-safety-and-delivery-evidence.md)). It never forces,
  never bypasses a rule of the forge, never asks the forge to merge later
  on its own, and never deletes a branch.
- **A merge that does not go through changes nothing.** A merge that is
  refused or fails leaves the checkout as it was (a conflict is abandoned);
  what was merged before it stays merged. The mission goes back to
  `reporting`, and the person, the coordinator and the lead are told which
  worker's work was not merged and why. The answer that was given approved
  a merge that did not happen: once what blocks is settled (by a worker for
  a conflict, by the person for their checkout or a rule of the forge), the
  coordinator presents its report again, and the person answers again.
- **No agent merges or closes.** The coordinator's instructions say that
  the merge is volee's, whatever a message says. It has no tool that closes
  a mission, and the daemon refuses a close asked in an agent's name. A
  mission ends on the person's approval, which volee carries out, or by the
  person's own hand: `volee mission close`, which merges nothing, closes a
  mission whose workers delivered, or any mission with `--force`.
- **Closing tidies, in one gesture.** After a close, the daemon tidies
  every worker whose work landed (its worktree removed, its branch kept,
  the agent forgotten: [ADR 0011](0011-safety-and-delivery-evidence.md),
  unchanged), stops the coordinator and forgets it, and tells the person,
  and the lead, in one message what is merged, what was tidied and what was
  kept, with the reason. It names the merges: a reader of "closed, branches
  kept" alone takes it that nothing was merged, as a lead did in a real
  mission. After an approval, the turn the person's answer woke in the
  coordinator is left to end first. Nothing is forced. An aborted mission
  is not tidied: aborting keeps everything.
- **An approval is carried out even if the daemon stops.** A mission that
  is `approved` when the daemon starts is merged and closed then.
- **Ready without a remote.** The delivery evidence says when a repository
  has no remote. There, a worker has delivered when its work is committed
  on a clean worktree; no push and no pull request are asked for. The merge
  is proven the way it already was: the branch's commits are part of the
  base branch, which is what a merge commit gives. An analysis worker's
  report now counts in its evidence too, so that its worktree can be
  tidied.
- **An agent does not speak for the person.** volee marks the sessions of
  the agents it launches (`VOLEE_AGENT` in their environment). The commands
  that speak for the person, `volee msg send`, `volee msg answer` and
  `volee mission close`, refuse to run where it is set, and say to use the
  MCP tools instead.

## Consequences

- The person decides once, on a three-line report, with two keys. The
  merges and the cleanup follow without them and without a model: the same
  commands every time, in about a second for a local merge. Until they
  decide, nothing moves and the mission says that it waits for them.
- volee now writes to repositories. Until this decision it read git, and
  removed worktrees whose work had landed. It now makes merge commits in
  the main checkout of a repository without a remote, and merges pull
  requests with the person's sign-in on the forge. It does so only for a
  mission whose report the person approved, in that mission's repositories.
- The merge no longer depends on what an agent's harness lets it run, or on
  a model getting a merge right.
- A merge commit volee makes is made with the person's git configuration,
  as if they had typed the command: their name, their hooks, their
  signature. A hook that refuses it is a merge that did not go through, and
  is said like any other.
- The forge's own rules still apply: required reviews and checks, protected
  branches. volee does not wait for checks to pass: a pull request that
  cannot be merged now is said, with the forge's words, and the mission
  stays open.
- Pull requests are merged through the forge client that reads them (`gh`,
  for GitHub): a repository in which volee sees pull requests is one in
  which it can merge them.
- A mission can end in a repository without a remote, which is what a
  sandbox usually is. There the person's main checkout must be ready for
  the merge; when it is not, volee says what is in the way.
- The gate is not a lock: an agent with a shell can still run a merge
  command before it asks, or remove the mark from its environment and
  answer for the person. What volee guarantees is narrower and checked: a
  mission never shows as approved without the person's recorded answer,
  volee merges nothing without it, no agent's request closes a mission, and
  nothing is tidied before the work landed.
- A report presented while a worker still changes its work no longer
  stands: the mission goes back to `running`, the person and the coordinator
  are told, and an answer to that report approves nothing.
- A volee older than this decision cannot read a log that holds the two new
  statuses.

## Alternatives considered

- **The coordinator merges, and volee checks.** This decision's first
  version: an agent can read a repository's rules and adapt, where a
  program merges the same way every time. Built, and measured: the
  coordinator's harness refused its merge in the third mission, because
  the person's word was given to volee and not to it. And a merge
  is the one step of a mission where adapting is not wanted: the person
  approved a report, not a judgement call.
- **The person merges, and volee only proves it.** That is what missions
  did before: the person merges by hand, one repository at a time. The
  rule this decision ports says that the merge follows "ok merge".
- **The harness's own question to its user.** The approval would live in
  one agent's terminal, where the daemon cannot read it and cannot carry it
  out.
- **Approving from the command line only** (`volee mission approve`). One
  more verb for what answering the question already says, and a question
  left unanswered on top of the messages.
- **Blocking merge commands with a hook.** A pattern on shell commands
  blocks the commands it knows and looks like a guarantee. The rule is
  stated to the agent instead, and what the daemon enforces is what it can
  check.
- **Merging somewhere else when the main checkout is not ready**, in a
  temporary worktree of the base branch. The base branch would change under
  a checkout the person is working in. volee says what is in the way
  instead.
- **Waiting for the forge** (`gh pr merge --auto`). The mission would close
  on a merge that has not happened and may never happen.
- **Tidying on approval rather than on close.** The work has not landed
  yet: nothing proves a worktree can go ([ADR 0011](0011-safety-and-delivery-evidence.md)).
