# Journeys

A journey is what a person does with volee, done for real: the real `volee`
interface, driven by keyboard, with a real coding agent behind it. Each script
types keys, reads the screen, and says which checks passed.

The test suite checks the parts. The journeys check the trip: a release whose
parts all pass can still fail a person at the first key. No version of volee
is released unless the journeys pass on a real machine
([RELEASING.md](../RELEASING.md)).

They never run in CI: they need a logged-in Claude Code and a few short
prompts on a small model.

## State after the v2 restart

Every 0.x journey script drove the TUI through tuios and was removed with the
terminal-owning crates ([docs/v2/framing.md](../docs/v2/framing.md), step 0).
Only `journey.py` stays, as the base to rewrite: its sandbox convention, its
checks and its cleanup. It still starts a tuios window to run the interface,
which no longer exists, so it is to be reworked for browser journeys (real
Claude Code through hooks, the real UI driven by Playwright) at stage 1 of the
plan.
