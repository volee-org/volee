# 0026. Edit mode types into an agent inside volee; the host encodes keys

- Status: Proposed. Amended by [0027](0027-open-without-owning.md): Enter and a double click enter edit mode too, and an agent that runs in no volee host is edited through a view.
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
default); pastes go too, the mouse does nothing. What is typed or pasted
before the host answers the new connection waits for it and goes, in
order, once it does: the screen stays meanwhile, so nothing tells the person
to wait (until 0.16 those keys were dropped, and a first letter typed right
away on a busy machine was lost). The leave key reconnects as a watcher:
the screen stays, the size stays as the owner left it, and what was typed
ahead and not sent yet is dropped.
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
interface, which keeps Ctrl+Z from them since they would not resume the
program. The protocol version stays 1: the new fields are optional, and
the new frames go only to hosts that announce them.

**Ctrl+Z resumes.** After forwarding a Ctrl+Z, in any encoding and from
any client, the host sends `SIGCONT` to the program's process group 300 ms
later. A program that suspends itself comes back at once; for one that
does not, the signal changes nothing.

**A hosted agent stays in its host.** Claude Code's own agents view,
opened inside a hosted agent (`←` on an empty prompt), moves the
conversation to a background process: measured on 2.1.322, the view says
"Your conversation moved to the background", `Esc` twice comes back, and
from then on the host's program shows that process, which runs the
conversation and its hooks. Those hooks, inherited through `--settings`,
name the hosted agent. Until 0.9, the daemon then moved the agent to the
background, and `i` offered to adopt it, which made a second copy of the
conversation. Now `volee hook` sends the process that ran it (its
parent), and for an agent in a running host whose program's pid is known,
the daemon tells the hooks of that program and its descendants (one `ps`
when the pid is not the program's own) from the others. Another process's
hook counts when it starts a session, which becomes the agent's current
one (the conversation going on elsewhere), or when it belongs to the
agent's current session; any other is dropped, such as those of an older
session the view revived. A wait for messages follows the same rule, with
the session id the hook's payload carries. And discovery never moves an
agent out of a host that still runs: once the host ends, the agent follows
its conversation to the background as before. Replayed hooks, whose
process is gone, are not checked.

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
