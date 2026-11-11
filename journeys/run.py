"""A program run under volee from a terminal, then used from the interface (ADR 0029).

The trip, with another AI: a person types `volee run -- opencode` in a terminal (a
pane of tuios). The terminal is on the program at once. volee lists it as an agent
in a volee host, once, though it also finds that program in the process table; its
preview is the screen the person sees, row for row. The leave key gives the terminal
back and the program keeps running. From the interface, Enter opens it; what is
typed reaches it; the leave key gives the keys back; its tile on the wall shows its
screen. `volee attach` opens it in a terminal again. Quit there, it stops, says that
nothing resumes it, then leaves the list.

Then the same with Claude Code, which lists its own sessions: `volee run -- claude`
is one row, the program the person ran, and never a second one for the session
Claude Code lists inside it.

Nothing is asked of a model: the text is typed in the program's input box, left
there, then erased.

    python3 journeys/run.py --volee target/release/volee --sandbox ~/code/sandbox
"""
import os
import shlex
import shutil
import time

from journey import Journey, arguments, cells, same_tail

# Typed in the program's input box and left there: the screen changes, and nothing
# is asked of a model.
LEFT = "left-here-42"

# What a pane prints once the volee command typed in it has ended: the terminal is
# the person's again.
BACK = "terminal-is-back"


def more(parser):
    parser.add_argument("--program", default="opencode",
                        help="the other AI's program (default: opencode, which volee also "
                             "finds in the process table)")


def main():
    args = arguments(__doc__, more)
    j = Journey("run", args)
    name = "journey-run-" + j.stamp
    claude_name = "journey-run-claude-" + j.stamp
    # The person's own volee may list what runs here as sessions started elsewhere.
    j.by_hand += [name, claude_name]
    program = os.path.basename(args.program)
    volee_vars = " ".join(key + "=" + shlex.quote(value) for key, value in j.vars.items())

    def typed_in(pane, cwd, command):
        """What a person types in a terminal: a volee command, in a directory. The
        pane says when the command has ended, then stays."""
        line = "cd {} && env {} {} {}; echo; echo {}; exec sleep 900".format(
            shlex.quote(cwd), volee_vars, shlex.quote(j.volee), command, BACK)
        return j.tuios("new-window", "-s", j.session, pane, "--", "sh", "-c", line)

    def screen_of(pane):
        return j.tuios("capture-pane", "-s", j.session, "-w", pane).stdout

    def rows_of(pane):
        return [cells(line) for line in screen_of(pane).splitlines()]

    def keys_in(pane, *keys):
        j.tuios("send-keys", "-s", j.session, "-w", pane, *keys)

    def logs(agent):
        return j.volee_run("logs", agent).stdout

    def state(agent):
        return ((j.agent(agent) or {}).get("state") or {}).get("state")

    def named(agent):
        return [row for row in j.agents_listed() if row["name"] == agent]

    def in_host(agent):
        row = j.agent(agent) or {}
        return row if row.get("location", {}).get("kind") == "host" else None

    def shows(title, margin, reference):
        """Whether the frame called `title` shows `reference`, row for row, once it
        has had the time of a few readings."""
        def caught_up():
            rows, width = j.frame(title, margin)
            return rows is not None and same_tail(rows, reference, width)[0]
        j.wait(caught_up, 20, 1)
        rows, width = j.frame(title, margin)
        if rows is None:
            return False, "no such frame on screen"
        same, compared, difference = same_tail(rows, reference, width)
        return same, difference or "{} cells compared, in {} columns".format(compared, width)

    try:
        # What the trip rests on.
        if not j.check("{} is installed".format(program), shutil.which(args.program),
                       "this trip needs it on PATH"):
            return
        if not j.check("the daemon starts", j.start_daemon()):
            return
        j.open_interface()
        j.sees("Agents", timeout=30)
        # A tall, narrow terminal: the preview then takes most of the height and holds
        # the program's whole screen.
        j.check("the window takes the size of a tall, narrow terminal", j.resize(96, 60),
                "{} lines".format(len(j.screen().splitlines())))

        # ---- typed in a terminal ---------------------------------------------------
        home = os.path.join(j.workdir, name)
        os.makedirs(home)
        started = typed_in("shell", home, "run -- " + shlex.quote(args.program))
        j.check("`volee run -- {}` is typed in a terminal".format(program),
                started.returncode == 0, (started.stdout + started.stderr).strip()[:160])
        row = j.wait(lambda: in_host(name), 30, 1)
        if not j.check("volee lists the program, in a volee host", row,
                       str((j.agent(name) or {}).get("location")) + " · " + screen_of("shell")[-200:]):
            return
        j.check("as a program the person runs on their own, known by its name",
                row.get("harness") == {"other": program} and row.get("role") == "independent"
                and row.get("launched_by", {}).get("by") == "human" and not row.get("sessions"),
                "{} · {} · {} · {} sessions".format(row.get("harness"), row.get("role"),
                                                    row.get("launched_by"),
                                                    len(row.get("sessions") or [])))
        j.check("in the directory the command was typed in",
                os.path.realpath(row["workspace"]["cwd"]) == os.path.realpath(home),
                row["workspace"]["cwd"])
        screen = j.still(lambda: rows_of("shell"))
        if not j.check("the terminal is on the program, whose screen comes to rest", screen):
            return
        # volee also finds this program by itself, in the process table: several
        # rounds later it is still one row.
        time.sleep(12)
        here = [agent for agent in j.agents_listed()
                if os.path.realpath(agent["workspace"]["cwd"]) == os.path.realpath(home)]
        j.check("it is one row, though volee also finds the program in the process table",
                len(here) == 1 and here[0]["id"] == row["id"],
                str([(agent["name"], agent["harness"], agent["location"].get("kind"))
                     for agent in here]))

        # ---- its screen, without opening it ---------------------------------------
        if not j.select(name):
            return
        same, seen = shows("Preview · " + name, 1, screen)
        j.check("its preview is the screen the person sees in the terminal, row for row",
                same, seen)

        # ---- the person leaves; the program keeps running ----------------------------
        keys_in("shell", "ctrl+q")
        j.check("the leave key gives the terminal back and says the program keeps running",
                j.wait(lambda: "it keeps running" in screen_of("shell")
                       and BACK in screen_of("shell"), 15, 1), screen_of("shell")[-200:])
        j.check("it keeps running, in the same host",
                state(name) in ("working", "idle")
                and (j.agent(name) or {}).get("location") == row["location"],
                "{} · {}".format(state(name), (j.agent(name) or {}).get("location")))
        j.check("left alone, it comes to rest: idle",
                j.wait(lambda: state(name) == "idle", 40, 1), str(state(name)))

        # ---- from the interface: open, type, come back -----------------------------
        j.keys("Enter", 2.5)
        j.check("Enter opens it inside volee, in edit mode",
                j.sees("your keys go to " + name, timeout=20))
        j.type(LEFT)
        j.check("typed text reaches the program",
                j.wait(lambda: LEFT in logs(name), 15, 1), logs(name)[-200:])
        j.show("a program run under volee, opened from the interface")
        j.keys("Backspace", 0.1)
        j.tuios("send-keys", "-s", j.session, "-w", j.window, "Backspace", "--repeat",
                str(len(LEFT) - 1))
        j.check("and so do the keys that erase it",
                j.wait(lambda: LEFT not in logs(name), 15, 1), logs(name)[-200:])
        j.keys("ctrl+q", 1.5)
        j.check("the leave key gives the keys back; its screen stays, live",
                j.wait(lambda: "your keys go to" not in j.screen()
                       and "Screen · " + name in j.screen(), 10, 1))
        j.keys("z", 1.0)
        j.check("and `z` comes back to the list", j.sees("Agents", timeout=10))

        # ---- its tile on the wall ---------------------------------------------------
        printed = j.still(lambda: [cells(line) for line in logs(name).splitlines()])
        j.keys("5", 2.0)
        same, seen = shows(name, 0, printed or [])
        j.check("its tile on the wall shows its screen, row for row", bool(printed) and same, seen)
        j.keys("q", 1.0)

        # ---- `volee attach` opens it in a terminal again; quit there -----------------
        typed_in("again", home, "attach " + shlex.quote(name))
        j.check("`volee attach` opens it in a terminal again, with its screen",
                j.wait(lambda: printed and same_tail(rows_of("again"), printed, 80)[0], 20, 1),
                screen_of("again")[-200:])
        keys_in("again", "ctrl+c")
        j.check("quit there, it stops: its program ended",
                j.wait(lambda: state(name) == "stopped", 60, 1),
                str((j.agent(name) or {}).get("state")) + " · "
                + str((j.agent(name) or {}).get("detail")))
        j.check("and that terminal is the person's again",
                j.wait(lambda: BACK in screen_of("again"), 15, 1), screen_of("again")[-200:])
        again = j.volee_run("attach", name)
        j.check("opened again, volee says that nothing resumes it",
                again.returncode != 0
                and "nothing resumes a program started with `volee run`" in again.stderr,
                again.stderr.strip()[:200])
        j.check("a minute later it has left the list",
                j.wait(lambda: j.agent(name) is None, 120, 2), str(j.agent(name))[:120])

        # ---- Claude Code, which lists its own sessions ---------------------------------
        command = "run --name {} -- claude --model {} --name {}".format(
            shlex.quote(claude_name), shlex.quote(j.model), shlex.quote(claude_name))
        typed_in("claude", j.sandbox, command)
        hosted = j.wait(lambda: in_host(claude_name), 30, 1)
        if not j.check("`volee run -- claude`: volee lists the program, in a volee host", hosted,
                       str(j.agent(claude_name))[:160] + " · " + screen_of("claude")[-200:]):
            return
        session = j.wait(lambda: j.claude_session(claude_name), 40, 1)
        if not j.check("Claude Code lists a session of its own for it", session,
                       str(session)[:160]):
            return
        # A session started elsewhere is listed once it has lived ten seconds: well
        # after that, the program is still one row, the one the person ran.
        time.sleep(25)
        rows = named(claude_name)
        j.check("it is one row, the program the person ran, though Claude Code lists its "
                "session",
                len(rows) == 1 and rows[0]["id"] == hosted["id"]
                and rows[0]["location"].get("kind") == "host" and not rows[0].get("sessions"),
                str([(agent["harness"], agent["location"].get("kind"),
                      len(agent.get("sessions") or [])) for agent in rows])
                + " · session pid {}".format(session.get("pid")))
        j.tuios("send-text", "-s", j.session, "-w", "claude", "/exit")
        keys_in("claude", "Enter")
        j.check("quit in its terminal, it stops",
                j.wait(lambda: state(claude_name) == "stopped", 60, 1),
                str((j.agent(claude_name) or {}).get("state")))
        j.check("the terminal is the person's again",
                j.wait(lambda: BACK in screen_of("claude"), 15, 1), screen_of("claude")[-200:])
        j.check("a minute later it has left the list, and nothing took its place",
                j.wait(lambda: not named(claude_name), 120, 2), str(named(claude_name))[:160])
        j.keys("q", 1.0)
    finally:
        j.finish()


main()
