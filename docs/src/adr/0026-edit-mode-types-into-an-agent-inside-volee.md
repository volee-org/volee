# 0026. Edit mode types into an agent inside volee; the host encodes keys

- Status: Proposed
- Date: 2026-11-10

## Context

[ADR 0021](0021-take-over-watch-and-the-wall-of-screens.md) made `a` suspend
the interface and hand the whole terminal to the agent's host until Ctrl+Q.
On 2026-11-10 the product owner, testing 0.9.1 in tuios, reported that:

- nothing said he was still in volee: the agent filled the terminal, and
  the full-screen view he came from (`z`) was gone;
- the full-screen view itself was a picture refreshed every two seconds or
  on `r`, which "did nothing" he could see;
- typing into Claude Code after taking it over looked broken, and Ctrl+Z
  froze it.

He asked to stay on the full-screen view and "just enter edit mode", like
vim's insert mode, and chose `i` or `a` to enter and Ctrl+Q to leave, with
`Esc` going to the agent.

Measured on Claude Code 2.1.32x in a host:

- Claude Code turns on the kitty keyboard protocol when the terminal
  answers its query, and the host answers it while no person's terminal is
  connected. Keys must then be encoded for that protocol, which the
  person's terminal (tuios here) does not know about.
- Ctrl+Z makes Claude Code restore the terminal and stop itself. A host's
  program leads its own session with no shell, so its process group is
  orphaned: the kernel discards the stop signal and nothing ever sends
  `SIGCONT`. Claude Code waits, its terminal left in cooked mode, and
  echoes keys raw.

## Decision

**Edit mode, inside the full-screen view.** For an agent in a volee host,
the full-screen view is live: the interface connects to the host as a
watcher and keeps the screen with a terminal emulator, updated as the
program writes. `i` or `a` (the `take-over` action) enters edit mode: the
interface reconnects as the input owner, the agent's terminal takes the
size of the panel, and the screen is drawn cell for cell with the
program's cursor. The frame takes the input colour, and the title and the
bottom line say `EDIT` and whose keys these are. Every key goes to the
agent but the leave key (`leave` of the `attached` context, Ctrl+Q by
default); pastes go too, the mouse does nothing. The leave key reconnects
as a watcher: the screen stays, and the size stays as the owner left it.
On the main screen, `i` and `a` open the full-screen view in edit mode at
once. The messages move from `i` to `I` and `Space i`.

**The host encodes keys.** The interface sends keys by name with their
modifiers (`Key` frames: `{"key": "c", "ctrl": true}`) and pasted text as
text (`Paste` frames). The host encodes them with libghostty's key encoder
set from its own terminal state, so that the kitty keyboard protocol,
cursor key mode and bracketed paste follow what the program asked, and
strips control characters from pastes. A client that draws the screen
itself says so in its hello (`embedded`): the host then answers the
program's terminal queries even while that client owns the input, since no
person's terminal sees them. The welcome says whether a host encodes keys
(`keys`); hosts started by older versions, which outlive an upgrade, get
the bytes a terminal in its usual mode sends instead, encoded by the
interface. The protocol version stays 1: the new fields are optional, and
the new frames go only to hosts that announce them.

**Ctrl+Z resumes.** After forwarding a Ctrl+Z, in any encoding and from
any client, the host sends `SIGCONT` to the program's process group 300 ms
later. A program that suspends itself comes back at once; for one that
does not, the signal changes nothing.

**`volee attach` stays.** From a shell, `volee attach` and `volee host
attach` still take an agent over in the whole terminal as ADR 0021 says.
Taking over in one place ends edit mode in the other, which says so and
watches again.

## Consequences

- The person never leaves volee to type into an agent, and always sees
  where the keys go. The full-screen view of a hosted agent is live
  without polling.
- The interface opens one connection per full-screen view, and none for
  the preview beside the list, which still reads the screen through the
  daemon.
- Keys a keyboard protocol tells apart (Shift+Enter) reach agents in new
  hosts as such, whatever the person's terminal supports, because the
  person's terminal only has to report the key, not encode it. Agents in
  hosts from before 0.10 get plain encodings until they get a new host.
- An agent's size follows the panel in edit mode, smaller than the
  terminal by the frame: two columns and three rows.
- The interface sends structured keys as crossterm reports them: a key
  the person's terminal does not report (some modifier combinations, key
  releases) cannot reach the agent.

## Alternatives considered

- **Keep handing over the whole terminal, with a volee bar drawn by the
  host's client.** Tried on a branch: the bar said "volee", but the person
  still left the interface and its view, and the bar took a row from the
  agent.
- **Forward the bytes of the person's terminal.** The interface reads keys
  through crossterm, which decodes them; re-encoding them in the interface
  would mean tracking the program's keyboard modes there, which only the
  host's emulator knows.
- **Not forwarding Ctrl+Z.** Simpler, but a key that silently does nothing
  surprises more than one that does what the agent says, and `volee
  attach` forwards it anyway.
