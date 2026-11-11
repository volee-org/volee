"""An agent of another AI, started by hand, seen then opened where it runs.

The trip: OpenCode started by hand in a pane of tuios shows in the list as an agent
of another harness, with the pane it runs in; its preview is the pane's screen, row
for row; Enter opens that pane inside volee; what is typed reaches it; the leave key
comes back and it goes on as before, the same process; its tile on the wall shows
its screen. Quit in its pane, it stops, then leaves the list.

Nothing is asked of a model: the text is typed in the program's input box, left
there, then erased. The program runs in a directory of its own, whose name is the
agent's: no key can go to another agent.

    python3 journeys/other_ai.py --volee target/release/volee --sandbox ~/code/sandbox

With `--stand-in codex`, the same trip is made for a harness volee has no listing
for (ADR 0030): tuios recognizes the program in the pane by its name, and volee
finds the agent through tuios. The program is then a stand-in, a copy of `cat`
under that name: it shows what tuios and volee do with a program of that name, and
nothing of what the real program draws.

    python3 journeys/other_ai.py --stand-in codex --volee target/release/volee --sandbox ~/code/sandbox
"""
import os
import shlex
import shutil

from journey import Journey, arguments, cells, same_tail

PANE = "agent"

# Typed in the program's input box and left there: the screen changes, and nothing
# is asked of a model.
LEFT = "left-here-42"


def more(parser):
    parser.add_argument("--program", default="opencode",
                        help="the other AI's program (default: opencode, the one volee "
                             "finds in the process table)")
    parser.add_argument("--stand-in", metavar="NAME",
                        help="instead of a real program, run a copy of `cat` named NAME "
                             "(codex, gemini, aider...), which tuios takes for that "
                             "harness by its name")


def main():
    args = arguments(__doc__, more)
    j = Journey("other-ai", args)
    name = "journey-ai-" + j.stamp
    j.by_hand.append(name)
    if args.stand_in:
        # A program that carries the harness's name, and nothing of the harness.
        args.program = os.path.join(j.workdir, "bin", args.stand_in)
        os.makedirs(os.path.dirname(args.program))
        shutil.copyfile("/bin/cat", args.program)
        os.chmod(args.program, 0o755)
    program = os.path.basename(args.program)

    def pane_screen():
        return j.tuios("capture-pane", "-s", j.session, "-w", PANE).stdout

    def pane_rows():
        return [cells(line) for line in pane_screen().splitlines()]

    def in_volee():
        """The keys of a person who has the agent open inside volee."""
        return ["-s", j.session, "-w", j.window]

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
        # the pane's whole screen.
        j.check("the window takes the size of a tall, narrow terminal", j.resize(96, 60),
                "{} lines".format(len(j.screen().splitlines())))

        # A person starts the other AI in a pane, outside volee, in a directory of its
        # own.
        home = os.path.join(j.workdir, name)
        os.makedirs(home)
        command = "cd {} && exec {}".format(shlex.quote(home), shlex.quote(args.program))
        if args.stand_in:
            # A stand-in draws nothing by itself: one line first, for a screen to
            # compare.
            command = "cd {} && echo 'a stand-in for {}' && exec {}".format(
                shlex.quote(home), program, shlex.quote(args.program))
        started = j.tuios("new-window", "-s", j.session, PANE, "--", "sh", "-c", command)
        j.check("{} starts in a tuios pane".format(program), started.returncode == 0,
                (started.stdout + started.stderr).strip()[:160])

        # ---- the list -----------------------------------------------------------
        def in_its_pane():
            agent = j.agent(name) or {}
            return agent if agent.get("location", {}).get("kind") == "pane" else None
        row = j.wait(in_its_pane, 60, 1)
        if not j.check("volee lists it, in its tuios pane", row,
                       str((j.agent(name) or {}).get("location"))):
            return
        j.check("as an agent of another harness, which volee only watches",
                row.get("harness") != "claude-code" and row.get("role") == "independent",
                "{} · {}".format(row.get("harness"), row.get("role")))
        if args.stand_in:
            harness = row.get("harness")
            named = harness.get("other", "") if isinstance(harness, dict) else str(harness)
            j.check("of the harness tuios recognizes the program as",
                    named.startswith(args.stand_in), "{} for a program named {}".format(
                        named, args.stand_in))
        session = j.session_of(row)
        j.check("in the pane the person started it in",
                j.session in row["location"].get("target", ""), str(row["location"]))

        # ---- its screen, without opening it ------------------------------------------
        if not j.select(name):
            return
        screen = j.still(pane_rows)
        if not j.check("its screen comes to rest", screen):
            return

        def shows(title, margin):
            def caught_up():
                rows, width = j.frame(title, margin)
                return rows is not None and same_tail(rows, screen, width)[0]
            j.wait(caught_up, 20, 1)
            rows, width = j.frame(title, margin)
            if rows is None:
                return False, "no such frame on screen"
            same, compared, difference = same_tail(rows, screen, width)
            return same, difference or "{} cells compared, in {} columns".format(compared, width)

        same, seen = shows("Preview · " + name, 1)
        j.check("its preview is the pane's screen, row for row", same, seen)
        j.keys("5", 2.0)
        same, seen = shows(name, 0)
        j.check("so is its tile on the wall", same, seen)
        j.keys("q", 1.0)

        # ---- open, type, come back ---------------------------------------------------
        j.keys("Enter", 2.5)
        j.check("Enter opens the pane inside volee, in edit mode",
                j.sees("your keys go to " + name, timeout=20))
        j.type(LEFT)
        j.check("typed text reaches the program, in its pane",
                j.wait(lambda: LEFT in pane_screen(), 15, 1))
        j.show("another AI's pane opened where it runs")
        j.tuios("send-keys", *in_volee(), "Backspace", "--repeat", str(len(LEFT)))
        j.check("and so do the keys that erase it",
                j.wait(lambda: LEFT not in pane_screen(), 15, 1))
        j.keys("ctrl+q", 1.5)
        j.check("the leave key comes back to the list and says it keeps running",
                j.sees("Left " + name + ": it keeps running", "Agents", timeout=10))
        now = j.agent(name) or {}
        j.check("it is the same process, in the same pane",
                j.session_of(now) == session and now.get("location") == row["location"]
                and now.get("id") == row.get("id"),
                "{} / {}".format(j.session_of(now), session))
        j.check("the view ends by itself once nobody uses it",
                j.wait(lambda: not j.views(), 40, 2), str(j.views()))

        # ---- quit, it leaves the list ---------------------------------------------
        j.tuios("send-keys", "-s", j.session, "-w", PANE, "ctrl+c")
        stopped = j.wait(lambda: (j.agent(name) or {"state": {"state": "gone"}})
                         .get("state", {}).get("state") not in ("idle", "working"), 60, 1)
        j.check("quit in its pane, it stops", stopped,
                str((j.agent(name) or {}).get("state")))
        j.check("then leaves volee's list",
                j.wait(lambda: j.agent(name) is None, 120, 2), str(j.agent(name))[:120])
        j.keys("q", 1.0)
    finally:
        j.finish()


main()
