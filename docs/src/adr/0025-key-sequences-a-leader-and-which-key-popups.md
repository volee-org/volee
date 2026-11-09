# 0025. Key sequences, a leader key and which-key popups; every key configurable

- Status: Proposed
- Date: 2026-11-09

## Context

[ADR 0009](0009-tui-architecture.md) put every binding in one table, from
which dispatch, the help and the hint bar are generated, and let users move
an action to one other key. By 0.8.1 the product owner hit four limits:

- **The help scrolled.** It was one column of about 160 lines: at 120x40,
  only "Everywhere" and part of the agents list were in sight.
- **Keys were not the ones people expect.** volee is used by people who
  live in vim, LazyVim and terminal tools such as lazygit or k9s. `n` and
  `N` launched an agent and a mission where those people expect the next
  and previous match; `g` went to the top in one key where they type `gg`;
  there was no `Ctrl+d` / `Ctrl+u`, no `:`.
- **The theme could not be found.** Themes existed (`[tui] theme`, five
  built-in, user files), but nothing in the interface said so or let anyone
  change it.
- **Not every key could be changed.** Overrides were one key per action, in
  every context at once, never none. The wall, the text fields, the launch
  dialog's choices and the leave key of a take over (Ctrl+Q,
  [ADR 0021](0021-take-over-watch-and-the-wall-of-screens.md): "configurable
  later") were written in code.

He chose, among proposals: a which-key popup at the bottom right, a leader
on the space bar as in LazyVim, vim-aligned keys, every key configurable,
and a theme picker with a live preview that keeps its choice, plus a
light or dark guess when no theme is set.

## Decision

**Sequences.** A binding is a sequence of one to three keys: `q`, `g g`,
`space n`. The interface keeps the keys typed so far: a key that starts
longer sequences waits for the next one, a whole sequence acts, anything
else (`Esc` included) drops it. There is no timeout: a key is either an
action or a prefix where it applies, and the keymap refuses a key that
would be both (`g` alone would hide `gg`), so nothing ever waits to guess.

**The leader.** The space bar starts the less frequent actions: `Space n`
new agent, `Space N` new mission, `Space t` theme, `Space w` wall, `Space k`
every key, `Space o` / `a` / `f` open with, adopt, forget. Frequent actions
keep one key (`j`, `k`, `a`, `m`, `d`, `/`, `?`). This is not the prefix key
ADR 0021 refused: that one would have cost a key press on every shortcut,
the leader costs one on rare ones only.

**Which-key popups.** While a sequence waits, a popup at the bottom right
lists what can follow, a key and a label each, groups marked. `?` opens the
same kind of popup with the keys of the current screen (the focused panel,
then everywhere), pairs folded into one entry (`h/j/k/l move`, `gg/G
first/last`, `1-4 views`) and laid out in columns so that nothing scrolls;
any other key closes it and acts. `? ?` (or `Space k`) lists every key of
every context in columns. Every binding gets a short label for these
popups, next to its description for the guide.

**Vim-aligned defaults.** `gg` / `G`, `Ctrl+d` / `Ctrl+u` (and Page Down /
Up) wherever something scrolls, `n` / `N` through the agents that match the
filter, `H` / `L` previous and next view, `:` a command palette that runs
any action of the screen, or a theme, by name. New agent and new mission
move behind the leader.

**Every key configurable.** `[tui.keys]` takes one key, a list, or `[]` to
unbind, for an action in every context, and `[tui.keys.<context>]` tables
for one context. The wall (`wall`), the text fields (`editing`), the
dialog's choices (`choice`), the popups and the palette have contexts like
the panels. The leave key is the `leave` action of the `attached` context:
Control and a letter, the only keys that reach volee the same way in raw
mode, under the kitty keyboard protocol and with `modifyOtherKeys`; not
`h`, `i`, `j` or `m`, whose Control bytes are also Backspace, Tab, Line Feed
and Enter. `volee attach`, `volee host attach` and the wall read it from
the configuration. Typing keys (characters, Backspace, the arrows in a
text) are not shortcuts and stay fixed.

**Themes.** `Space t` or `:theme` opens a picker at the bottom right;
moving shows each theme at once, `Enter` writes `[tui] theme` into the
configuration file without touching anything else in it (comments, order,
other keys), `Esc` restores the theme there was. Without a configured
theme, volee asks the terminal for its background colour (OSC 11, bounded
wait, skipped when not a terminal, then `COLORFGBG`) and picks
`catppuccin-latte` on a light background, `catppuccin-mocha` otherwise.
`volee --theme <name>` overrides both for one run.

## Consequences

- The popups, the hints, the palette and the guide's key tables all come
  from the one table: a binding is still one line, now with a label.
- People used to 0.8 lose `n` / `N` for launching (now `Space n` / `Space
  N`) and `g` alone for the top (now `gg`); the hint bar announces `␣
  more`, and `?` shows the new keys.
- A configuration with a typo fails to start the interface with the line
  and the action named; the wall and a take over from a shell fall back to
  the default keys with a warning rather than refusing to work.
- The light or dark guess queries the terminal once at start; a terminal
  that does not answer costs the bounded wait (700 ms) and keeps the
  default.

## Alternatives considered

- **A timeout for ambiguous keys, as vim's `timeoutlen`.** Allows `g` and
  `gg` together, but makes `g` slow and the interface guess. Refusing the
  ambiguity is simpler and keeps every key instant.
- **A larger help page.** The product owner asked for less scrolling; a
  page at least as big as the screen cannot give that once the keys grow.
- **Keep `n` / `N` for launching.** Launching is rare and costs money; the
  next match is a reflex. The leader keeps launching two keys away.
- **Configure the leave key as any key.** Only Control and a letter
  survives every keyboard protocol an agent may switch on.
