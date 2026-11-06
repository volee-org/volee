# 0000. Record architecture decisions

- Status: Accepted
- Date: 2026-11-06

## Context

volee is an open-source project meant to welcome contributors, many of whom
will work with coding agents. Both humans and agents need to know why the code
is shaped the way it is, without archaeology in commit history.

## Decision

Every significant decision is written as an ADR in `docs/src/adr/`, numbered,
in the MADR style, and published with the documentation site. A pull request
that changes a decision updates or supersedes the matching ADR in the same
pull request.

## Consequences

- Reviewers can point to an ADR instead of re-arguing a choice.
- Agents working on the repository read the ADRs through `AGENTS.md`.
- Writing an ADR costs a few minutes; we accept that cost.

## Alternatives considered

- **Design notes in issues or discussions.** Easy to write, hard to find and
  not versioned with the code.
- **A single design document.** Grows stale and loses the history of why a
  choice changed.
