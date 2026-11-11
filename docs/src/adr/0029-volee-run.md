# 0029. `volee run` makes any program an agent, in the terminal it is typed in

- Status: Accepted (amends [0022](0022-harness-profiles.md) and [0023](0023-every-agent-on-the-machine.md))
- Date: 2026-11-11

## Context

volee shows the agents it can find: the sessions Claude Code lists, the
OpenCode processes of the process table, and the agents that join through
MCP ([ADR 0023](0023-every-agent-on-the-machine.md),
[ADR 0014](0014-agents-of-other-harnesses-join-through-mcp.md)). It opens
them where they run: in a volee host, through `claude attach`, or in a tuios
pane ([ADR 0027](0027-open-without-owning.md)).

Two kinds of agents stay out of reach.

- **An AI volee cannot find.** Codex CLI, Gemini CLI and the next ones have
  no listing volee reads and no profile yet
  ([ADR 0022](0022-harness-profiles.md)): started in a terminal, they are
  not in the list at all.
- **An agent in a plain terminal.** Found or not, a program that runs in a
  terminal outside a multiplexer has no door: nothing can show its screen
  or type into it from elsewhere. volee says so and sends the person to
  that terminal.

A host runs any command line since 0.8.0 (the generic profile of
[ADR 0022](0022-harness-profiles.md)), and `volee host spawn -- <command>`
starts one. But such a host is not an agent: it is in no list, no preview
and no wall of the interface.

One fact decides who must start the host. A program gets the environment of
whoever starts it. The daemon runs in the background, started once, often at
login: its `PATH`, its keys and its directory are not those of the terminal
a person types in. A program the daemon started for a person would not be
the program that person runs.

## Decision

**`volee run [--name <name>] -- <command>`** runs any program as an agent,
from a terminal:

1. the command starts a volee host itself, with the command line as typed,
   in the current directory, with the terminal's environment and size;
2. it asks the daemon to make the program of that host an agent
   (`agents.run`, with the host's id and the name);
3. it takes the program over in this terminal, as `volee attach` does. The
   leave key (`Ctrl+Q`) gives the terminal back; the program keeps running,
   and the interface opens it like any agent in a host.

Without a terminal on standard input and output (a script), step 3 is
skipped and the command prints how to open the agent.

**What the agent is.** The human started it through volee, it has no place
in the organization of the work (the role `independent`), its harness is the
program, by its name, and it runs in a host. It has no session: no harness
volee drives knows it. That combination is what tells it from every other
agent (`Agent::generic`):

| | launched by | role | location | session |
| --- | --- | --- | --- | --- |
| launched by volee | human or agent | lead, worker, coordinator | host | yes, once started |
| found by volee | outside | independent | terminal, pane, background | yes |
| adopted | outside | independent | host | yes |
| joined through MCP | outside | independent | unknown | no |
| **run with `volee run`** | **human** | **independent** | **host** | **no** |

**What volee does with it** is what the generic profile allows, no more:

- its screen is the host's: exact in the preview, on the wall, full screen
  and in `volee logs`; Enter opens it and the person types into it;
- its state comes from its screen, the last resort of
  [ADR 0022](0022-harness-profiles.md): working while the screen changes,
  idle once it has stayed the same for a few seconds, stopped when the
  program ended. volee cannot tell when it waits for the person;
- `volee stop` ends its program, as closing its terminal would;
- its repository and branch are read like any agent's, and its repository
  is not fetched on its account (`[git] fetch`): the person started that
  work, in their own checkout;
- nothing hands it a message, and nothing resumes it, after it ended or
  after a reboot. The composer, `volee msg send` and the refusal to open an
  ended one say so, instead of advising a message that would resume it;
- once its program ended, its row stays a minute, so that its end can be
  seen, then leaves the list, like a session volee only watched
  ([ADR 0027](0027-open-without-owning.md)): there is nothing left to do
  with it.

**One program, one row.** A harness may list what runs inside the host:
`volee run -- claude` starts a session that Claude Code lists, and
`volee run -- opencode` a process of the process table. The host's own
record comes first, as [ADR 0023](0023-every-agent-on-the-machine.md)
orders the joins: a session or a process whose process is the one the host
started, or any process below it (a command may go through `npx`, a shell
or a version manager), is the agent volee already has, and makes no second
one. The process table gives the processes below: one `ps` with each
listing, only while such an agent runs.

**The daemon only records.** `agents.run` refuses an id that does not name
a file of its hosts directory, a host it does not find, a program that
already ended, and a host that already is an agent. When it refuses, the
command ends the program it started: nothing runs unseen.

## Consequences

- Any AI can be put under volee from a terminal, today, without a profile
  and without a multiplexer: it is listed, shown and opened like the others.
  That is the door a plain terminal does not have.
- States are the coarsest volee has. A program that redraws while it waits
  looks working, and one that waits for an answer looks idle.
- `volee run -- claude` gives less than Claude Code elsewhere: one row, of
  the generic kind, where a session volee finds by itself has exact states,
  a name and a summary, and one volee launches has hooks and messages too.
  It is worth it for the door, in a plain terminal; in a tuios pane the
  session is found and opened without it.
- Typing into the program makes its screen change, so the agent shows as
  working while a person types.
- The program keeps the whole environment of the terminal it was typed in,
  the variables of a multiplexer included: whatever reads them still sees
  the pane the command was typed in, after the person left it.
- Each run leaves a host record and a log in the hosts directory, like any
  host. Nothing removes them yet.
- The kind rests on a combination of recorded facts, not on a field of its
  own. Tests pin the combination against every other kind of agent, and
  `agents.spawn` refuses the one launch that would produce it (an
  independent agent on a harness other than Claude Code).

## Alternatives considered

- **The daemon starts the host**, as it does for `volee spawn`. The program
  would get the daemon's environment and directory, not the terminal's:
  another `PATH`, missing keys, another version of the program. Rejected:
  the person's command must run as typed, where typed.
- **`volee run -- claude` as a Claude Code agent of volee's own**, with
  hooks, messages and resume. volee launches Claude Code with its own
  settings, its own MCP configuration and its permission mode: that is not
  the session the person asked for. Linking the listed session to the agent
  instead would give exact states without touching the session; it needs
  the listing and the screen to agree on one agent, and is left for when
  someone needs it.
- **Putting a program that already runs under volee.** A terminal cannot be
  taken from a running process: it would have to be stopped and started
  again, which is an adoption ([ADR 0023](0023-every-agent-on-the-machine.md)),
  and needs a way to resume.
- **Keeping an ended run in the list until someone forgets it**, like an
  agent volee launched. Such an agent can be resumed by a message; this one
  cannot, so its row would only be removed by hand.
- **A field of its own in the registration event.** Clearer to read, but
  every reader and writer of the registry would carry it for one kind of
  agent. The combination says the same with facts the registry already
  records, each of them true of such an agent, and costs a predicate and
  its tests.
