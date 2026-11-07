# Themes

volee draws with semantic roles, such as `state.needs_input` or
`diff.added`, never with fixed colors. A theme maps those roles to colors.

## Pick a theme

volee ships with five themes:

| Theme | Look |
| --- | --- |
| `catppuccin-mocha` | Dark, the default. |
| `catppuccin-macchiato` | Dark, a little softer. |
| `catppuccin-frappe` | Dark, the softest. |
| `catppuccin-latte` | Light. |
| `terminal` | Your terminal's own 16 colors and background. |

Choose one in the [configuration](configuration.md):

```toml
[tui]
theme = "catppuccin-latte"
```

or try one with the demo, which then ignores the configured theme:

```sh
volee demo --theme catppuccin-latte
```

volee looks for a theme in `~/.config/volee/themes/<name>.toml` first, then
among the built-in themes.

volee uses 24-bit colors when the terminal announces them in `COLORTERM`
(`truecolor` or `24bit`), and otherwise replaces each color by the nearest
one of the 256-color palette. The `terminal` theme uses only the 16 colors
your terminal lets you configure.

## Write a theme

A theme is a TOML file with an optional parent, a palette of named colors,
and the roles:

```toml
# ~/.config/volee/themes/dusk.toml
inherits = "catppuccin-mocha"

[palette]
ember = "#ff9e64"

[roles]
"state.needs_input" = { fg = "ember", modifiers = ["bold"] }
"role.worker" = "sky"
edge = "#3b4261"
```

- `inherits` starts from another theme (yours or a built-in one). A theme
  may inherit from the built-in theme of the same name to change only a few
  roles of it.
- `[palette]` names colors. A palette entry of the child replaces the
  parent's, and every role that uses it follows: that is how the Catppuccin
  flavors share one set of roles.
- `[roles]` gives each role a color (a plain string sets the foreground) or
  a style: `{ fg = "...", bg = "...", modifiers = [...] }`. Modifiers are
  `bold`, `dim`, `italic`, `underlined`, `reversed` and `crossed-out`.
- A color is a palette name, an ANSI name (`red`, `light-blue`,
  `dark-gray`...), `reset` for the terminal's default, `#rrggbb`, or a
  256-color index.
- A role a theme does not set falls back to its parent role
  (`state.working` falls back to `state`), then to the terminal's default
  style. Unknown roles are ignored, so a theme keeps working with older and
  newer versions of volee.

## Roles

<!-- roles:start -->
| Role | Used for |
| --- | --- |
| `background` | The background and default text color of the whole screen. |
| `text` | Regular text. |
| `text.dim` | Secondary text: labels, separators, elapsed times, paths. |
| `title` | Panel titles. |
| `border` | Borders of panels. |
| `border.focused` | The border and the title of the focused panel. |
| `selection` | The selected row of a list. |
| `card.selected` | The border of the selected card in the hierarchy. |
| `topbar` | The top bar. |
| `topbar.brand` | The volee badge at the left of the top bar. |
| `tab` | Tabs that are not shown. |
| `tab.active` | The tab on screen. |
| `tab.number` | The number in front of each tab. |
| `counter` | Counters in the top bar. |
| `counter.urgent` | The count of agents that need you, when it is not zero. |
| `letter` | The letter that jumps to an agent. |
| `state.launching` | An agent volee launched but has not seen running yet. |
| `state.working` | An agent running a turn. |
| `state.needs_input` | An agent that waits for a person: permission, question or dialog. |
| `state.idle` | An agent waiting for its next instruction. |
| `state.done` | An agent that finished its last task. |
| `state.failed` | An agent that ended with an error. |
| `state.stopped` | An agent whose process stopped; its conversation can be resumed. |
| `state.lost` | An agent volee cannot find any more. |
| `role.lead` | The lead agent you talk to. |
| `role.coordinator` | The coordinator of a mission. |
| `role.worker` | A worker in one repository. |
| `role.subagent` | A helper started by another agent inside its session. |
| `role.independent` | A session volee did not launch. |
| `edge` | Lines between cards, and who launched whom in the network view. |
| `edge.pulse` | A message moving along an edge of the network view. |
| `edge.message` | Agents that exchanged messages, in the network view. |
| `edge.question` | A question that waits for its answer, in the network view. |
| `diff.added` | Added lines. |
| `diff.removed` | Removed lines. |
| `badge.pr` | Pull request numbers. |
| `checks.passing` | Checks that passed. |
| `checks.failing` | Checks that failed. |
| `checks.pending` | Checks that still run. |
| `sparkline` | Activity sparklines on cards. |
| `hint.key` | Keys in the hint bar and in the help. |
| `hint.text` | Descriptions in the hint bar and in the help. |
| `notice` | Short notices at the bottom of the screen. |
| `notice.error` | Notices about something that failed. |
| `input` | Text being typed, such as a filter. |
| `placeholder` | Words that stand in for missing content, such as an empty view. |
<!-- roles:end -->
