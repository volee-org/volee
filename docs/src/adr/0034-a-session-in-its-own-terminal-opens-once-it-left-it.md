# 0034. A session in a terminal of its own opens once it left it, on the same row

- Status: Accepted (amends [0027](0027-open-without-owning.md))
- Date: 2026-11-11

## Context

[ADR 0027](0027-open-without-owning.md) opens every agent through the door
its session already has, and names one case without a door: a session in a
terminal volee cannot reach. Enter on such a row failed, with a sentence
that said to switch to that terminal, and that the left arrow there moves a
Claude Code session to the background.

The person volee is built for met it at once. Two Claude Code sessions, in
two tabs of the same terminal application, "opened the same way": one
opened in volee, the other did not. What differs is not the terminal, it is
who holds the session's terminal:

| The session | Who holds its terminal | Who else can show it and type in it |
| --- | --- | --- |
| A Claude Code background session | a process of Claude Code | any terminal, with `claude attach` |
| A session in a tuios or tmux pane | the multiplexer | whoever the multiplexer lets in |
| A session typed in a terminal tab | the terminal application | nobody |

The tab that showed the first session ran Claude Code's own list of
agents, a window on a background session. The second ran `claude` itself.

Two things were asked of a fix. It must be the same in every terminal: a
tool that needs one terminal application is not compatible with the
others. And it must not stop, fork or adopt the session
([ADR 0027](0027-open-without-owning.md)).

What was measured, on Claude Code 2.1.322 and macOS, with throwaway
sessions in a pseudo-terminal:

- A program cannot press a key in a terminal it does not hold. The system
  refuses it to a process of the same user (`TIOCSTI`: permission denied;
  taking the terminal as its own first: operation not permitted). Claude
  Code has no command that takes a session typed in a terminal from
  outside: `attach`, `logs`, `stop`, `rm` and `respawn` take background
  sessions.
- The left arrow on the session's empty prompt moves the conversation to a
  background session: another session id, another process. The terminal's
  own process stays and shows the list of agents; Escape twice shows the
  conversation there again, as a window on the background session. It is
  one way: the session stays in the background.
- It works at rest and in the middle of a turn. The turn goes on: asked to
  run a command of forty seconds and then a second one, a session moved
  during the first ran the second, 42 seconds after the key.
- A command the session runs in its background keeps running, still a
  child of the terminal's process.
- The name is kept when it was given with `--name`. A session Claude Code
  named itself takes a title made from the conversation (`alpha-c7` became
  "Background sleep command"), and an empty conversation takes its short
  id.
- Nothing in the listing links the two sessions.
- The listing during the move, sampled every 0.15 s or so, three times: at
  the key press it shows both sessions, the new one without a process; for
  0.7 s to 1.4 s it shows the new one alone, still without a process, in
  the state `blocked`; then the new one has its process. It never showed
  neither.
- Until this decision the conversation showed up as a new row, and the row
  of the session said `stopped` for a minute.

## Decision

**The row follows the conversation.** When a session that ran in a
terminal, the session of an agent volee only watches, stops being listed
while its process still runs, it did not end: a session that ends takes
its process with it. The daemon reads the process table (one `ps`) to tell
the two apart. Such a session is continued by the background session of
the same directory that nobody owns, that did not start before it, and
that runs or started less than five seconds ago: a conversation that
arrives in the background is listed about a second before it has a
process. When several are there, it is the one that carries its name; when
that leaves a doubt (two sessions left that directory, or two arrived
under other names), none is taken and the rows are what they were before
this decision. The agent releases the old session and takes the new one:
the same row, with the name and the place Claude Code now gives the
conversation. Agents volee launched learn it from their hooks, as before
([ADR 0026](0026-edit-mode-types-into-an-agent-inside-volee.md)).

**A listing taken during the move makes no second row.** A background
session that started less than five seconds ago in a directory where a
session runs in a terminal waits before it becomes an agent, with or
without a process: it may be that session's conversation. If the session in
the terminal is still listed once the five seconds have passed, they are
two conversations and it becomes a row.

**Enter waits.** Enter, `i`, `a` or a double click on a live Claude Code
session in a terminal volee cannot reach no longer fails. The bottom line
says which key to press, and where: `<name> runs in its own terminal: press
← on its empty prompt there, and volee opens it here.` While it waits, the
interface has the daemon list the sessions every two seconds. As soon as
the agent runs somewhere volee opens, it is opened, in edit mode, without
being asked again. The wait ends with Escape, with the session if it ends
in its terminal, or when another agent is opened. From then on the session
is a background session like any other: Enter opens it at once.

**No terminal application is asked for anything.** The key is pressed by
the person, in the session's terminal, whatever that terminal is. volee
only reads what Claude Code lists and what `ps` prints.

## Consequences

- A Claude Code session typed in any terminal is one key away from being
  opened in volee, with nothing stopped, forked or resumed by volee.
- The row may change its name on the way: it carries the name Claude Code
  gives the session ([ADR 0027](0027-open-without-owning.md)), and Claude
  Code gives a moved conversation a title when it had named the session
  itself.
- A conversation that rests after its move is listed `blocked` by Claude
  Code: its row says that it needs the person.
- A command that the session ran in its background still belongs to the
  terminal's process: closing that terminal ends it, as it did before the
  move.
- A session in a terminal still has no preview while it is there: nothing
  but its terminal has its screen.
- A background session started beside a session in a terminal shows up to
  five seconds later than it used to.
- When the process table cannot be read, nothing says that a session did
  not end: the conversation shows on a new row, as before.
- One `ps` runs when a session in a terminal leaves the listing. While the
  interface waits, the daemon lists every two seconds.
- A session of another harness in a plain terminal is unchanged: volee
  says that it cannot reach it. So is `volee attach` on a Claude Code
  session in a terminal: it refuses, and says which key moves it.
- The order of the listing during a move is Claude Code's, measured, not
  promised. A harness that stopped listing the old session before it
  listed the new one would give a new row, as before this decision.

## Alternatives considered

- **Press the key for the person, through the terminal application.**
  Some have a door for it: Ghostty 1.4 on macOS lists its terminals with
  their process and takes keys for one of them through AppleScript, and
  the tab of a session was found that way, read-only. It is one piece of
  code per terminal application and per system, and none for the terminals
  that have no such door. The product owner refused it as the way in. It
  can come later as a shortcut where a terminal allows it; the wait above
  stays what works everywhere.
- **Type into the terminal from outside.** The system refuses it, and a
  tool that did it where it can would be doing what malware does.
- **Adopt the session** (stop it, resume it in a volee host). It changes
  the session it claims to open ([ADR 0027](0027-open-without-owning.md)).
- **Start every session inside volee** (an alias of `claude` to
  `volee run -- claude`, [ADR 0029](0029-volee-run.md)). It changes how the
  person starts a session, and says nothing of the ones already running.
- **Remember for a while the sessions that left, and join two rows after
  the fact.** Not needed with the order Claude Code lists a move in, and
  more state to keep right.
