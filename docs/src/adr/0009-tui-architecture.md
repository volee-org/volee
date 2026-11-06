# 0009. The TUI follows the Elm architecture with one keymap table

- Status: Accepted
- Date: 2026-11-06

## Context

The prototype's interface was a single class of more than 2,500 lines whose
drawing code also changed state, and whose keys were described in three
hand-written lists (dispatch, help, key bar) that had already drifted apart.
Opening an agent created windows in five different places.

## Decision

- **Elm architecture.** A `Model`, a `Msg` enum, a pure `update(model, msg)
  -> effects` and a pure `view(model, frame)`. Drawing never mutates the
  model; hit areas for the mouse are computed by the layout and stored in the
  model by `update`.
- **One keymap table.** Every binding is declared once with its context, its
  action and its description. Dispatch, the help screen and the key hint bar
  are generated from it. Users override bindings in `config.toml`.
- **One screen.** Previews, diffs and messages open inside volee. The only
  ways out are explicit: taking over an agent (suspend and attach) and opening
  an external tool the user configured. Both return to the same row.
- **Semantic themes.** Widgets ask for roles (`state.working`,
  `edge.pulse`, `diff.added`), never for colors. Themes map roles to colors,
  with built-in Catppuccin flavors and a theme that uses the terminal's own
  ANSI palette.
- **Semantic zoom.** Each graph view has three levels of detail (cards,
  compact, dots) chosen by size and changeable by the user.
- **Render only when needed.** Animation ticks run only while something
  moves; otherwise the TUI redraws on input and on daemon events.

## Consequences

- Key handling and layout are unit-tested without a terminal; rendering is
  snapshot-tested with ratatui's test backend.
- Adding a binding is a one-line change that also updates the help.

## Alternatives considered

- **Component objects with their own state** (the prototype). Hard to test,
  and state leaks into drawing.
