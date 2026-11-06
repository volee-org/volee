# Governance

volee is a small open-source project. This document describes how decisions are
made. It will grow if the project does.

## Maintainers

| Name          | GitHub                                     | Role            |
| ------------- | ------------------------------------------ | --------------- |
| Jeff Le Floch | [@katsenkatorz](https://github.com/katsenkatorz) | Lead maintainer |

Maintainers review and merge pull requests, triage issues, cut releases and
enforce the [Code of Conduct](CODE_OF_CONDUCT.md).

## Decision making

- **Pull requests** use lazy consensus. If a maintainer approves and nobody
  objects within a reasonable time (a few days for non-trivial changes), the
  change can be merged.
- **Significant changes** (architecture, public interfaces, dependencies with a
  large footprint, security model) are proposed as an ADR in `docs/src/adr`, using
  the template there. The ADR is discussed in its pull request and merged once
  the discussion settles.
- **Disagreements** are discussed openly in the issue or pull request. If there
  is no consensus, the lead maintainer decides.

## Becoming a maintainer

Maintainers are invited based on sustained, good-quality contributions: code,
reviews, documentation, triage or help for other users. If you are interested,
keep contributing and tell a maintainer. A current maintainer proposes the
invitation and the lead maintainer confirms it. A maintainer who becomes
inactive for a long time may be moved to emeritus status, with thanks.

## Changes to this document

Changes to governance are made by pull request and need the approval of the
lead maintainer.
