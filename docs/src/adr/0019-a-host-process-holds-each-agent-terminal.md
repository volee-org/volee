# 0019. A volee-host process holds each agent's terminal

- Status: Proposed (supersedes [0002](0002-harness-owns-processes.md) once accepted)
- Date: 2026-11-08

## Context

[ADR 0002](0002-harness-owns-processes.md) left agent processes and their
terminals to the harness: every Claude Code agent volee launches is a
background session (`claude --bg`), watched through `claude logs` and taken
over through `claude attach`. Two days of use showed its limits:

- `claude logs` replays only recent output. What was drawn before the replay
  and never redrawn (the prompt frame, the status line) is missing, so a long
  session previews with holes. Version 0.7.0 patched this with mirrors
  ([ADR 0018](0018-panes-that-show-background-sessions.md)), which tie volee to
  a multiplexer and do nothing for a session no pane shows. The product owner
  rejected them.
- `claude attach` imposes the size of the last client on every client, and
  the size sticks after it leaves (measured on 2.1.319). A preview cannot read
  the screen without changing it.
- Only Claude Code has a supervisor. Codex CLI, Gemini CLI and the others run
  in a terminal and nowhere else, so volee could not host them at all.
- Known Claude Code bugs hit background sessions and multiplexers: background
  sessions never get `SIGWINCH` (claude-code issue 86775), and redraws corrupt
  under tmux (52309) and tmux with Ghostty (77689).

Tools that keep a terminal alive for a program fall in two families. dtach and
abduco pass bytes and keep no screen, so a new client sees nothing until the
program redraws. shpool (one client, vt100) and zmx (one daemon per session,
libghostty-vt, restores the screen on reattach) keep the screen in the process
that holds the terminal. Mitchell Hashimoto's server-side multiplexer work on
libghostty takes the same path and calls tmux, Zellij and screen too slow for
this job.

## Decision

**One host per agent.** For each agent volee launches or adopts, volee starts a
small process, `volee host run`, from its own binary. The host:

- starts the agent's command line in a new pseudo-terminal (portable-pty), in
  the agent's directory, with the agent's environment;
- feeds everything the program writes into a terminal emulator
  ([ADR 0020](0020-libghostty-vt-emulates-terminals.md)), so it always knows
  the exact screen;
- detaches from the terminal that started it (its own session, no controlling
  terminal), so it survives that terminal, the interface and the daemon;
- serves a Unix socket, and keeps a small record of itself next to it.

Claude Code runs interactive inside its host. volee no longer starts agents
with `claude --bg`, and does not need them to appear in `claude agents`. Hooks
still come from `--settings` ([ADR 0004](0004-sources-and-reconciliation.md)),
and worktrees are still created as [ADR 0013](0013-worktrees-live-in-their-repository.md)
says.

**Files.** Hosts live in `<state>/hosts/` (mode 0700, created by volee):
`<agent id>.sock` and `<agent id>.json`. The record holds the host's process
id, the program's process id, the command line, the directory, the harness
profile ([ADR 0022](0022-harness-profiles.md)), the start time and, once the
program ends, its exit status. It is written atomically and survives a reboot
([ADR 0024](0024-agents-come-back-after-a-reboot.md)).

**Protocol.** Frames of a 4-byte length, a 1-byte kind and a payload. Control
messages are JSON; screen bytes are raw. The first frame from each side carries
a protocol version: a host can outlive an upgrade of volee, so a newer volee
must keep speaking the versions of the hosts still running.

**Clients.** A client says what it wants when it connects:

- a **watcher** receives a snapshot of the screen, then the stream of output.
  It never writes and never changes the size. Previews, the full-screen view,
  the wall of screens and the daemon are watchers.
- the **input owner** also sends keys and its terminal size. There is one at a
  time. Its size is the terminal's size; when it leaves, the size stays. A new
  input owner takes the input from the previous one, which is told why and
  returns to its caller.

**Snapshot.** On connection the host renders its screen as VT sequences (text,
colors, cursor, modes, keyboard protocol, scrolling region, tab stops) and
sends it before any further output. The host reads the program's output in
chunks cut between complete sequences and characters, so a snapshot never
falls in the middle of one. While the program holds synchronized output (DEC
mode 2026), snapshots wait for the end of the update, one second at most.

**Terminal queries.** Programs ask their terminal questions (device
attributes, cursor position, colors, version, keyboard protocol, size). While
an input owner is connected, its own terminal sees the questions and answers
them. While none is, the host answers them from its emulator. A snapshot
contains state, not the stream, so replaying it never asks a question again.

**Slow watchers.** Each client has a bounded queue. A watcher that falls
behind stops receiving output and gets a fresh snapshot once it catches up;
the program is never slowed down by a watcher.

**End.** When the program ends, the host records its exit status, tells its
clients, keeps the last screen for ten minutes so that the daemon and the
interface can show it, then removes its socket. `volee stop` asks the host to
end the program like a closed terminal would (`SIGHUP` to its process group),
then `SIGKILL` after a grace period.

**Daemon.** The daemon finds hosts by listing `<state>/hosts/`, connects to
each as a watcher, and learns activity, title, bell and exit from it. A host
whose socket refuses connections and whose program is gone is stale: the
daemon records how it ended and removes its socket.

## Consequences

- Every agent volee runs, of any harness, has an exact screen that any number
  of watchers read without changing it.
- volee now owns PTY code: sizes, signals, process groups, terminal queries.
  The host is small and has one job, and its rules are tested without a
  terminal.
- An agent survives the interface and the daemon, but not its host. A crashed
  host ends its agent; [ADR 0024](0024-agents-come-back-after-a-reboot.md)
  resumes it.
- Claude Code's own supervisor (reaping, `claude attach`) no longer applies to
  volee's agents. Claude Code's background-session bugs no longer apply either.
- The protocol is a compatibility surface between versions of volee.
- Each host costs a process and its emulator's memory (measured by the
  prototype, see the pull request).

## Alternatives considered

- **Keep `claude --bg` and improve the replay.** The replay cannot show what
  was drawn before it started, `claude attach` cannot watch without resizing,
  and other harnesses have no supervisor.
- **Run agents in tmux (or another multiplexer).** Brings back pane identity
  ([ADR 0001](0001-agent-identity.md)) and the redraw bugs, and requires a
  multiplexer ([ADR 0008](0008-multiplexers-are-optional.md)).
- **One server for every agent.** One crash would end every agent, and an
  upgrade would have to restart them all. A host per agent fails alone.
- **The daemon holds the terminals.** Restarting or upgrading the daemon would
  end every agent.
- **dtach or abduco.** No screen state: a new client sees nothing until the
  program redraws.
