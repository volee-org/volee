# 0007. Missions are run by a supervisor program and a coordinator agent

- Status: Accepted (amended by [0032](0032-a-mission-is-merged-only-after-the-person-approved.md):
  the coordinator presents its report and merges only after the person
  approved; it no longer closes on its own word, and closing tidies the
  mission)
- Date: 2026-11-06

## Context

In the prototype, a mission was led by an LLM agent that waited on its
workers in a loop, with broad permissions. If it died or ran out of context,
the mission stayed "active" forever. "Done" meant "finished a turn", not
"delivered".

## Decision

A mission has a goal, one or more repositories, a kind (`change` delivers
branches and pull requests; `analysis` delivers reports) and two halves:

- The **supervisor**, a program inside the daemon. It launches the
  coordinator, tracks workers, waits for them without tokens, verifies
  delivery evidence (ADR 0011) when a worker stops, retries or escalates, and
  closes the mission only when every worker has delivered or been abandoned
  by the human.
- The **coordinator agent**, an LLM session with the MCP tools
  `spawn_worker`, `send_message`, `report` and `close_mission`. It splits the
  goal, writes worker tasks, answers their questions or escalates them to the
  human, and writes the final report. It is woken by messages from the
  supervisor and its workers; it never polls.

A mission can be aborted at any time; aborting never deletes work.

## Consequences

- Waiting costs nothing; the LLM is used for judgement only.
- Delivery is checked by code, so "done" means pushed, reviewed or reported.
- The coordinator needs no multiplexer or administrator rights.

## Alternatives considered

- **LLM-only coordinator** (the prototype). Fragile, expensive while waiting,
  and unverifiable.
- **Program-only coordinator.** Cannot split a fuzzy goal or answer a
  worker's design question.
