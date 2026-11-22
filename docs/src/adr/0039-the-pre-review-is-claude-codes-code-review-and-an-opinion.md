# 0039. The pre-review is Claude Code's `/code-review`, run read-only, and an opinion

- Status: Accepted
- Date: 2026-11-22

## Context

Step 7 of the v2 plan puts an AI pre-review on the card of finished work. The
[framing note](https://github.com/volee-org/volee/blob/v2/docs/v2/framing.md)
decided on Claude Code's native `/code-review` (Haiku by default, two
reviewers when the change is risky, the risk computed by the Lua rules) and
that the review is an opinion, never a gate by itself.

What had to be measured (Claude Code 2.1.330, a two-bug branch, Haiku):

- `claude -p "/code-review low main...feat/stats"` works non-interactively,
  in the foreground, in 8 to 10 s and for about 0.015 USD. `medium` took 24 s
  for the same cost.
- The findings come back as **text** in `result`. There is no typed output in
  `-p`: no `ReportFindings` event, and `--json-schema` does not apply to a
  slash command. The layout depends on the level, the model and the user's
  language (`file:line - severity - summary` at `low`; a numbered list with
  `Failure scenario:` at `medium`). Line numbers are the reviewer's own and can
  be a few lines off.
- Hooks run in `-p`, including the user's. `--settings
  '{"disableAllHooks":true}'` turned all five `SessionStart` hooks off (0
  `hook_started` events); `--bare` would too, but needs an API key.

## Decision

- The daemon runs `claude -p "/code-review <level> <base>...<tip>"
  --model <model> --permission-mode plan --disallowedTools
  Edit,Write,NotebookEdit --name volee-review-<tip>-<level> --settings
  '{"disableAllHooks":true}' --output-format json` in the agent's worktree,
  with `VOLEE_REVIEW=1`. Never `--fix`.
- The model and the number of reviewers come from the Lua rules
  ([0037](0037-lua-config-and-rules.md): `review.model`, `review.reviewers`,
  more reviewers at high risk). One reviewer is a `low` pass; the second is a
  `medium` pass, run after the first. Findings are merged, one per file and
  line.
- The text is parsed by a tolerant parser into findings (file, line,
  severity `important` / `nit` / `pre_existing`, summary, failure scenario);
  the raw text is always stored too, so a layout the parser does not know
  loses nothing.
- One review per `(repo, branch, tip)`, stored in SQLite: a tip is never
  reviewed twice, a failed one is redone only on an explicit retry. Reviews a
  stopped daemon left unfinished are failed at the next start.
- At most `[review] concurrency` reviews run at once (default 1), each with a
  `[review] timeout`. `[review] enabled` and `auto` switch it off or make it
  on-demand.
- A review session is not an agent: its name starts with `volee-review`, the
  agents view drops such sessions, and its hooks are off, so it posts no event
  and cannot start a loop.
- `GET /api/agents/{id}/review` serves the status (`none`, `pending`,
  `running`, `done`, `failed`, `disabled`) and the findings;
  `POST` asks for a review now (`?retry=1` redoes a failed one).
- The web interface is told as it happens: the inbox frame of the events
  stream carries a `review` brief on each finished card and is sent again each
  time a review changes. The finished-work screen reads the whole review again
  when its card's brief changes. The review is labelled as an opinion, and the
  Run / Retry button shows only when nobody asked yet or the last try failed.
- A finding is shown at its file and line in the guided diff; one whose line
  the diff does not show, or that has no line, goes on the file's header. Its
  text is model output and is only ever inserted as text.

## Consequences

- The opinion costs a few cents of Haiku per finished branch, and nothing when
  switched off.
- The parser can miss a finding when `/code-review` changes its layout. The
  raw text is still shown, and the risk is a missing finding, never a wrong
  merge: the review gates nothing.
- Line numbers are not verified against the diff. The card shows them as the
  reviewer said them.

## Alternatives considered

- **Ask `/code-review` for JSON** (`--json-schema`, `ReportFindings`): tried;
  neither applies to a slash command in `-p`.
- **A second model call to structure the text**: more cost for a result the
  parser already gives in the layouts seen.
- **Open Code Review** in delegation mode: no net gain at equal model, and a
  second tool to install (see the framing note).
- **`--bare`**: skips hooks, but needs `ANTHROPIC_API_KEY`, which the
  subscription does not give.
