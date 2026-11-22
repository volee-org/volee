# 0038. A permission or a question is held by its hook until the person decides

- Status: Accepted
- Date: 2026-11-22

## Context

Steps 4 and 5 of the v2 plan let the person decide, from the browser, what an
agent asks in a terminal: a permission to run a tool, and a question
(`AskUserQuestion`). Claude Code gives no way to type into a session, and v2
never does. The two checks of the
[framing note](https://github.com/volee-org/volee/blob/v2/docs/v2/framing.md)
(section 7, T1 and T2) showed that a blocking `http` hook can carry the
answer: a `PermissionRequest` hook returns allow or deny, and a `PreToolUse`
hook on `AskUserQuestion` returns the answers.

What the checks left open is what the terminal does while the daemon holds the
request, and who wins when both the browser and the terminal can answer.

## Decision

- **The daemon holds the request.** The handler of the hook waits on a channel
  until the person decides in the browser. The card appears in the inbox
  (above the plain waiting cards, which it replaces for that agent) and the
  hook is answered only then.
- **A permission is a concurrent decider.** Claude Code shows its own dialog
  at the same time. Whoever answers first wins, and the card goes in every
  case:
  - the person decides in the browser: allow, allow with one of the rules
    Claude Code suggested (`updatedPermissions`), or deny with a message the
    model reads;
  - the terminal answers first: the next hook events of the same session say
    so (`PostToolUse`, `PostToolUseFailure` or `PermissionDenied` for the same
    tool and input, another `PreToolUse`, `Stop`, `UserPromptSubmit`,
    `SessionEnd`), and the held request is answered with nothing;
  - Claude Code closes the connection: the handler is dropped and takes its
    card with it;
  - the hold runs out: nothing is answered, and the terminal keeps its dialog.
  The request carries no id, so a request is matched by session, tool name and
  tool input. A wrong match only drops a card early; the terminal dialog is
  never touched.
- **A question is held for a short delay, then handed back.** While the hook
  runs, the terminal shows only the spinner (`running PreToolUse hooks...`),
  no dialog. So the hold is short (`[hooks] question_delay`, 120 s by
  default); when it runs out the daemon answers nothing and Claude Code shows
  its normal dialog. The person can also hand a question or a permission back
  at once ("Ask in terminal").
- **Two hook entries for questions.** `volee doctor` keeps the plain
  `PreToolUse` entry that stores every event, and adds one with the matcher
  `AskUserQuestion` that posts to `/decide/AskUserQuestion`. That entry holds
  and stores nothing, so an event is never stored twice. Its timeout is the
  delay plus a margin, so the daemon always hands back before Claude Code
  gives up. The `PermissionRequest` entry is the same entry as before with a
  long, explicit timeout (`[hooks] permission_timeout`, 3600 s by default);
  the daemon holds for the same time.
- **Answers.** A permission is answered with
  `hookSpecificOutput.decision {behavior, message?, updatedPermissions?}`. A
  question is answered with `permissionDecision: allow` and an `updatedInput`
  that echoes the original `questions` and adds `answers`, a map from the
  question text to the chosen label or the free text. A multi-select answer is
  the chosen labels joined by a comma and a space, exactly what Claude Code
  records when the same question is answered in the terminal (measured).
  `AskUserQuestion` and `ExitPlanMode` never become permission cards: they
  need an `updatedInput` a permission card cannot build, and the terminal
  keeps them.
- **The daemon stopping is safe.** A held request is lost with the process:
  Claude Code treats the closed connection as a failed hook and asks in the
  terminal. Nothing is decided and nothing is lost (measured).
- **Held cards do not wait for the poller.** The inbox reads the last
  published reading of the agents instead of waiting for a reading in
  progress, which can take seconds; a card appears and goes within a hook
  round trip.
- **Several tabs.** The inbox event stream wakes on any change of the held
  requests, so a decision in one tab updates all of them. A late decision
  (another tab, the terminal, an expiry) gets `404`, which the page words.

## Consequences

- Both decisions work for any session whose project has the hooks, with no
  SDK and no typing into a terminal.
- A person at the terminal sees nothing while a question is held. That is the
  cost of the delay; the default is a compromise to tune with use, and the
  Lua rules can set it per repository later.
- A held permission with the default 3600 s timeout leaves a Claude Code
  session blocked on a hook for up to an hour if the person never looks; the
  dialog in the terminal remains answerable meanwhile.
