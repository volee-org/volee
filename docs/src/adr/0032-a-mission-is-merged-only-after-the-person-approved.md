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

## Decision

A mission's work is merged, and the mission closed, only after the person
approved the report its coordinator presented. The approval is a recorded
answer that the daemon reads; the close then tidies the mission.

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
- **The coordinator merges, and volee checks.** Once approved, the
  coordinator merges each worker's work into its base branch: a pull
  request with the forge's own command; in a repository without a remote, a
  plain merge of the worker's branch in the main checkout. It never forces
  and never deletes a branch; a merge that fails is the person's to hear
  about. volee merges nothing itself: it proves merges
  ([ADR 0011](0011-safety-and-delivery-evidence.md)).
- **The close needs both.** `close_mission` by the coordinator is refused
  unless the mission is approved and, for a change mission, the work of
  every worker is merged, by the evidence. The daemon reads that evidence
  again at that moment, from git and the forge, whatever the state of the
  workers: a merge made a second ago by another agent is seen. The person
  can still close a mission themselves once its workers delivered, or any
  mission with `--force`.
- **Closing tidies, in one gesture.** After a close, by the coordinator or
  by the person, the daemon tidies every worker whose work landed (its
  worktree removed, its branch kept, the agent forgotten:
  [ADR 0011](0011-safety-and-delivery-evidence.md), unchanged), stops the
  coordinator once the turn that closed the mission ended and forgets it,
  and tells the person in one message what was tidied and what was kept,
  with the reason. Nothing is forced. An aborted mission is not tidied:
  aborting keeps everything.
- **Ready without a remote.** The delivery evidence says when a repository
  has no remote. There, a worker has delivered when its work is committed
  on a clean worktree; no push and no pull request are asked for. The merge
  is proven the way it already was: the branch's commits are part of the
  base branch. A squash is not such a merge, and the coordinator is told to
  make a plain one. An analysis worker's report now counts in its evidence
  too, so that its worktree can be tidied.
- **An agent does not speak for the person.** volee marks the sessions of
  the agents it launches (`VOLEE_AGENT` in their environment). The commands
  that speak for the person, `volee msg send`, `volee msg answer` and
  `volee mission close`, refuse to run where it is set, and say to use the
  MCP tools instead.

## Consequences

- The person decides once, on a three-line report, with two keys; the
  merges and the cleanup follow without them. Until they decide, nothing
  moves and the mission says that it waits for them.
- A mission can end in a repository without a remote, which is what a
  sandbox usually is.
- The coordinator's last turn is no longer a report nobody answers: it is a
  merge it must get right. It does it with a shell, in the repositories of
  the mission, including a merge into the base branch of a main checkout
  when there is no remote. That is what "ok merge" means there, and the
  question says "merge".
- The gate holds an agent that follows its instructions and one that slips.
  It is not a lock: an agent with a shell can run a merge command before it
  asks, or remove the mark from its environment and answer for the person.
  What volee guarantees is narrower and checked: a mission never shows as
  approved without the person's recorded answer, the coordinator's close is
  refused without it, and nothing is tidied before the work landed.
- A report presented while a worker still changes its work no longer
  stands: the mission goes back to `running`, the person and the coordinator
  are told, and an answer to that report approves nothing.
- A volee older than this decision cannot read a log that holds the two new
  statuses.

## Alternatives considered

- **volee merges.** A program would merge the same way every time, and that
  is the problem: merge methods, protected branches, required checks and
  conflicts differ by repository, and an agent can read the rules and
  adapt. volee keeps the part a program does better: reading the person's
  answer and proving the merge.
- **The harness's own question to its user.** The approval would live in
  one agent's terminal, where the daemon cannot read it and cannot refuse a
  close without it.
- **Approving from the command line only** (`volee mission approve`). One
  more verb for what answering the question already says, and a question
  left unanswered on top of the messages.
- **Blocking merge commands with a hook.** A pattern on shell commands
  blocks the commands it knows and looks like a guarantee. The rule is
  stated to the agent instead, and what the daemon enforces is what it can
  check.
- **Tidying on approval rather than on close.** The work has not landed
  yet: nothing proves a worktree can go ([ADR 0011](0011-safety-and-delivery-evidence.md)).
