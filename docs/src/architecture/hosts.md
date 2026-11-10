# Hosts

Every agent volee launches or adopts runs in a **volee host**: one small
process per agent that holds the agent's pseudo-terminal and its exact
screen, and serves both on a private Unix socket. The decision is
[ADR 0019](../adr/0019-a-host-process-holds-each-agent-terminal.md); the
code is the `volee-host` crate, with the screen kept by `volee-term` over
libghostty-vt ([ADR 0020](../adr/0020-libghostty-vt-emulates-terminals.md)).
This page explains how a host works.

## The process

The daemon starts a host with `volee host spawn`, which starts
`volee host run` from the same `volee` binary, with nothing on its standard
streams, and waits until the host answers on its socket (five seconds at
most). The daemon names each host after its agent, with a suffix of its
own, so that a resumed agent gets a new host. The host then:

- leaves the session and the terminal that started it, so that closing that
  terminal, quitting the interface or stopping the daemon does not end it;
- binds its socket first, readable and writable by you only, so that a
  socket that cannot be bound never leaves a program running;
- starts the agent's command line in a new pseudo-terminal, in the agent's
  directory, with `TERM=xterm-256color`, `COLORTERM=truecolor` and
  `VOLEE_HOST` set to its id;
- feeds everything the program writes into a terminal emulator, so that it
  always knows the exact screen, with 1,000 lines of history.

One terminal thread owns the screen and every decision; the threads that
read the program's output, write its input, wait for its end and serve
clients only talk to it. Output, snapshots and input are therefore handled
in one order.

A host's size, before anyone takes the agent over, is 160 columns by 45
rows for the agents the daemon launches, and the starting terminal's size
for `volee host spawn` (or `--cols` and `--rows`).

## Files

Hosts live in `<state>/hosts/` (`~/.local/state/volee/hosts/` by default),
a directory volee creates with mode 0700. Each host has three files, named
by its id:

| File | Content |
| --- | --- |
| `<id>.sock` | The host's socket, mode 0600. Removed when the host ends. |
| `<id>.json` | The host's record: its process id, the program's process id, the command line, the directory, the harness profile, the name, the start time, the current size and, once the program ended, its exit code or signal. Written atomically (a temporary file, then a rename), so it survives a reboot. |
| `<id>.log` | What the host itself logged, appended to, never the program's output. |

A record with no exit and no host answering on the socket is what a reboot
leaves: the daemon resumes such an agent in a new host
([ADR 0024](../adr/0024-agents-come-back-after-a-reboot.md)), and
`volee host ls` shows the host as `interrupted`.

## The protocol

A frame is a 4-byte big-endian length, a 1-byte kind and a payload of that
length, at most 64 MiB. Control payloads are JSON; screen bytes and keys
are raw.

| Kind | Direction | Payload |
| --- | --- | --- |
| 1 `Hello` | client to host | JSON: protocol version, role (`watch` or `control`), terminal size, client name |
| 2 `Welcome` | host to client | JSON: protocol version, id, command line, directory, program's process id, size, lines of history, title, exit if any, whether this client got the input |
| 3 `Snapshot` | host to client | VT bytes: reset your screen to this |
| 4 `Output` | host to client | the bytes the program wrote |
| 5 `Input` | input owner to host | keys |
| 6 `Resize` | input owner to host | JSON: the owner's terminal size |
| 7 `Event` | host to client | JSON: `resized`, `title`, `bell`, `exited` or `input_taken` |
| 8 `Stop` | client to host | empty: end the program |

A client says hello within five seconds, or the host drops it. Each side
names its protocol version first: a host can outlive an upgrade of volee,
so a newer volee must keep speaking the versions of the hosts still
running. The protocol is at version 1.

## Watchers and the input owner

A client says in its hello what it wants:

- A **watcher** receives a snapshot of the screen, then the program's
  output as it comes. It never writes and never changes the size. The
  preview, the full-screen view, the wall of screens, `volee host peek` and
  the daemon are watchers; any number of them can read an agent at once.
- The **input owner** also sends keys and its terminal size. There is one
  at a time. Its size becomes the program's size (at most 1000 columns or
  rows), and stays when it leaves. A new input owner takes the input from
  the previous one, which receives `input_taken`, is disconnected and
  returns to its caller.

`volee attach`, `a` in the interface and `volee host attach` connect as the
input owner ([ADR 0021](../adr/0021-take-over-watch-and-the-wall-of-screens.md)).
The client puts the person's terminal in raw mode, switches it to the
alternate screen, clears it and writes the snapshot. It then forwards the
person's keys without Ctrl+Q, which it recognizes in every encoding: the
plain byte `0x11`, the kitty keyboard protocol (`CSI 113 ; 5 u`, with any
event type) and xterm's `modifyOtherKeys` (`CSI 27 ; 5 ; 113 ~`). Ctrl+Q
ends the take over and never reaches the agent. On the way out, the client
turns off what the agent may have left on (mouse reporting, focus events,
bracketed paste, synchronized output, application cursor keys and keypad,
the scrolling region, the cursor style, `modifyOtherKeys`, kitty keyboard
flags, an open hyperlink, color scheme reports, a hidden cursor) and
restores the person's screen.

## Snapshots

When a client connects, the host renders its screen as VT sequences (text,
colors, cursor, modes, keyboard protocol, scrolling region, tab stops) and
sends that snapshot before any further output. A watcher builds its own
copy of the screen from it, then follows the stream. The host reads the
program's output in chunks cut between complete sequences and characters,
so a snapshot never falls in the middle of one.

**Synchronized output.** Agents' interfaces wrap their redraws in DEC mode
2026 so that a terminal shows each frame whole. While the program is in
such an update, snapshots wait for its end, one second at most, so that no
client starts from half a frame.

**Resizes.** When the input owner changes the size, every watcher receives
`resized` and a fresh snapshot at the new size.

## Terminal queries

Programs ask their terminal questions: device attributes, cursor position,
colors, version, keyboard protocol, size. While an input owner is
connected and up to date, its own terminal sees the questions in the
output and answers them. While none is, the host answers them from its
emulator, so an agent nobody watches never waits for an answer. A snapshot
holds state, not the stream, so replaying one never asks a question again.

## Slow watchers

Each client has a queue of 256 frames. A watcher that falls behind stops
receiving output, and gets a fresh snapshot once it has drained its queue:
the program is never slowed down by a watcher. A client that stops reading
for 30 seconds while it waits for a snapshot is dropped.

## The end of a program

`volee stop`, `volee host stop` and a `Stop` frame end the program like a
closed terminal would: `SIGHUP` to its process group, then `SIGKILL` five
seconds later if it is still there. When the program ends, by itself or
not, the host waits half a second for its last output, records its exit
code or signal, sends `exited` to every client, and keeps the last screen
for ten minutes (`--linger`, in seconds, for `volee host spawn`), so that
the daemon and the interface can still show it. Then it removes its socket
and exits; the record stays.

## How the daemon uses hosts

The daemon knows each agent's host from its launch: the agent's location
is `volee host`, with the host's id. It takes a snapshot to answer a
preview, gives the client the `volee host attach` command for a take over,
and asks the host to stop the program for `volee stop`. For the hosted
agents whose harness has no hooks (OpenCode), it takes a snapshot of each
after every look for sessions: a screen that changed is working, one
unchanged for about six seconds is idle, a program that ended is stopped.
When the daemon starts, it resumes the agents whose host is gone without
recording an end ([ADR 0024](../adr/0024-agents-come-back-after-a-reboot.md)).

## Views

A session volee did not start has no host: it runs in Claude Code's
background, or in a pane of a multiplexer. To show it inside the interface
and type into it, the daemon runs the session's own door in a host, a
**view** ([ADR 0027](../adr/0027-open-without-owning.md)): `claude attach
<id>` for a Claude Code background session, `tuios pane-view` for a tuios
pane. `agents.open` answers with the view's socket, and the interface
connects to it as to any host, in edit mode.

A view differs from an agent's host on three points. It runs in a thread
of the daemon, under `<state>/views`, and ends with it. It ends its
program once nobody has held its input for 15 seconds (`idle_exit`, also
`volee host spawn --idle-exit <seconds>`): the program is a door, so
ending it ends nothing else. And it leaves no file: its socket and its
record go when it ends, and a daemon that starts clears what a killed one
left.

## Limits

- An agent survives the interface and the daemon, but not its host: a host
  that crashes ends its agent, which the daemon resumes the next time it
  starts, as after a reboot.
- Each host costs a process and the memory of its emulator.
- Claude Code's own supervisor (reaping idle background sessions,
  `claude attach`) does not apply to the agents in hosts.
- The protocol is a compatibility surface between versions of volee.
