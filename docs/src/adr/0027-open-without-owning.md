# 0027. volee opens every agent where it runs, and its list shows what runs

- Status: Accepted (amends [0023](0023-every-agent-on-the-machine.md), [0026](0026-edit-mode-types-into-an-agent-inside-volee.md), [0021](0021-take-over-watch-and-the-wall-of-screens.md) and [0004](0004-sources-and-reconciliation.md))
- Date: 2026-11-10

## Context

On 2026-11-10 the product owner said again what volee is for: a cockpit
over every agent that runs on the machine, whatever started it. Opening one
(Enter, a double click) puts a terminal on its session inside the tool,
"a bit like a multiplexer would", and he types there.

volee 0.10.0 did something else, on two points.

**Typing needed an adoption.**
[ADR 0023](0023-every-agent-on-the-machine.md) made every agent volee did
not start read only, and adoption "the one way to take full control". An
adoption stops the session (`claude stop`) and resumes the conversation in a
volee host with volee's own `--settings`, `--mcp-config` and
`--permission-mode`: the person's session loses its own MCP servers and
settings, in another process. Pressing `a` on a session in a terminal
answered that it "runs in a terminal of its own".

**The list was a registry, not what runs.** Every session ever seen stayed
until someone forgot it, a finished session never seen alive was skipped,
and an agent that held several session ids showed the state of the last one
linked. Measured on his machine that day: 21 rows for the 10 sessions
Claude Code listed. Thirteen rows were sessions that no longer existed, the
four finished background sessions of Claude Code's own agents view were
missing, and the conversation he was working in showed `stopped`.

Measured the same day, with Claude Code 2.1.322 and tuios 0.12.1:

- `claude attach <id>` in a volee host shows a background session exactly,
  within 0.3 s. Keys reach it. It stays the same session in the same
  process, ending the host leaves it running, and on a stopped session
  `attach` starts it again under the same id. The attach client is not
  listed as a session. It takes about 139 MB, and the session takes the
  size of its last client.
- `tuios pane-view` shows one pane alone and types into it. The pane takes
  the viewer's size while no client of tuios shows its session, and goes
  back to its own when the viewer leaves.
- `/clear` in an interactive session keeps the process, its start time and
  its name, and gives it a new session id; the old id leaves the listing at
  once.
- Nine of the unknown rows were named `claude-XX`. A menu-bar program runs
  `claude --print` from `/` every five minutes, 2.3 s each time, and Claude
  Code lists such a run as an interactive session, with nothing in the
  listing that tells it from one a person started.

## Decision

**Open without owning.** Opening an agent puts its terminal inside volee,
in edit mode ([ADR 0026](0026-edit-mode-types-into-an-agent-inside-volee.md)),
through the door its session already has. Nothing is stopped, forked,
resumed or reconfigured for it.

| Where the agent runs | Its door |
| --- | --- |
| A volee host (volee launched or adopted it) | the host itself, as before |
| A Claude Code background session, running or finished | `claude attach <id>` |
| A tuios pane | `tuios pane-view` on that pane |
| A tmux pane | none inside volee: the pane is brought forward, as before |
| A terminal volee cannot reach | none: volee says so, and that the left arrow on its empty prompt moves a Claude Code session to the background, where volee opens it |

The daemon answers `agents.open` with the socket of a volee host: the
agent's own, or a **view**, a host that runs the door at the size of the
panel. A view lives in a thread of the daemon, in `<state>/views`. The
interface connects to it like to any host. Leaving edit mode closes a view
and comes back to the list: the door's program is not the agent, so ending
it ends nothing, and the interface says that the agent keeps running. A
view ends its program once nobody has held its input for 15 seconds, and
leaves no file. Opening the agent again within that time gives the same
view.

`volee attach` opens a background session with the same command, in the
person's own terminal.

**Keys.** Enter, `i`, `a` and a double click open the selected agent. The
subtree of the hierarchy moves from Enter to `s`.

**Adoption is a choice, not a way in.** `A` still moves a session into a
volee host, for someone who wants volee to drive it (messages handed over,
resume after a reboot). It asks first, and says that the session is stopped
and resumed with volee's settings.

**The list shows what runs.** An agent volee only watches (someone else
started it, it runs in none of volee's hosts, and volee knows it by a
session its harness lists) is one session:

- every session the harness lists is a row, a finished background session
  included: it is one the person can still open;
- it carries the name and the place (background, terminal) the harness
  gives its session;
- an agent that holds several sessions keeps the one listed alive and
  releases the others, which become rows of their own (`session-released`);
- when its session is no longer listed, it shows `stopped` or `lost` for a
  minute, then leaves the registry (`agent-expired`). Unlike a forgotten
  agent's, its session is not ignored: listed again, it comes back;
- a session that changes its id inside the same process (same process id
  and start time) keeps its row;
- a session started elsewhere, outside the background, becomes a row once
  it has lived ten seconds;
- a watched agent that failed sorts with the stopped ones: a failure calls
  for the person who launched the work through volee.

Agents volee launched, and adopted ones, keep the rules of
[ADR 0001](0001-agent-identity.md): they stay until forgotten, with the
sessions of their conversation.

**Nothing is kept of an agent that left.** The daemon removes an agent's
directory (hook settings, MCP configuration, setup log) when the agent
leaves the registry, sweeps the directories left by earlier versions when it
starts, and writes those files only for agents it launches, resumes or
adopts.

## Consequences

- Any Claude Code background session, and any session in a tuios pane, is
  one key away from being typed into, with its own tools and settings.
- The list matches Claude Code's own: on the machine above, the same
  registry replayed with these rules gives 10 rows for 10 sessions.
- A view makes the session it shows take the size of volee's panel while
  it is open: Claude Code gives a background session the size of its last
  client, and tuios gives the pane back its size when the viewer leaves.
- A view on a background session costs one `claude attach` client while it
  is open and for 15 seconds after.
- A session started by hand shows ten seconds after it starts. A run of a
  few seconds never shows, whoever started it.
- The full-screen view (`z`) of a background session is still the replay
  of `claude logs`: an exact screen needs a client, which resizes the
  session.
- The mouse is not forwarded to an opened agent yet
  ([ADR 0028](0028-one-emulator-and-the-wall-inside-the-interface.md)
  forwards it).
- The log has three new kinds of events. An interface from before 0.11
  connected to a newer daemon stops updating and says that it cannot read
  an event: restart it.
- The `left arrow` hint is Claude Code's own behaviour, which this decision
  names but does not control.

## Alternatives considered

- **Keep adoption as the way in.** It changes the session it claims to
  open, and it is what the product owner rejected.
- **A view per session, kept open.** Exact previews at all times, at the
  price of every background session held at volee's size and of one client
  each.
- **Views as separate `volee host` processes.** One more process per view
  and a dependency on the path of the binary, for something that should not
  outlive the daemon anyway.
- **Hide old rows in the interface** (as tuios-cockpit hides ended items
  after an hour). The registry would keep growing, and every client
  (`volee ls`, the MCP server) would have to hide the same rows the same
  way.
- **Tell one-off runs by their command line** (`--print`). It depends on
  one harness's flags and needs a look at every new process; the age rule
  needs neither.
- **Type through a multiplexer's `send-keys`.** No screen comes back with
  it, and it exists for panes only.
