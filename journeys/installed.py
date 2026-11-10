"""The volee that is installed, with the person's own daemon and list.

The last check before and after a release: the installed binary, the real daemon and
its real list of agents. The journey compares that list with what Claude Code lists,
launches one throwaway agent in the sandbox and types into it, starts one background
session by hand and opens it by Enter, then removes both. It finds them by their
unique names and touches no other agent.

    python3 journeys/installed.py --sandbox ~/code/sandbox [--expect 0.11.0]
"""
from journey import Journey, arguments


def more(parser):
    parser.add_argument("--expect", help="the version the installed volee must report")


def launched_agent(j, name):
    """An agent volee launches: edit mode in its own host."""
    launched = j.launch(name, "Reply only READY, then wait for my next message.")
    j.check("the person's daemon launches a throwaway agent", launched.returncode == 0,
            (launched.stdout + launched.stderr).strip()[:160])
    ready = j.wait(lambda: "READY" in j.volee_run("logs", name).stdout, 120, 2)
    if not j.check("the agent answered READY", ready):
        return
    agent = j.agent(name)
    j.check("it runs in a volee host", bool(agent) and agent["location"]["kind"] == "host",
            str(agent and agent["location"]))
    if not j.select(name):
        return
    j.keys("z", 2.0)
    j.check("z shows its screen live", j.sees("Screen · " + name + " · live", "READY"))
    j.keys("Enter", 2.0)
    j.check("Enter opens it in edit mode", j.sees("your keys go to " + name, timeout=10))
    j.type("Reply with the word fruit in uppercase, nothing else")
    j.keys("Enter")
    j.check("typed text reaches the agent and it answers", j.sees("FRUIT", timeout=90, every=1))
    j.show("edit mode on the installed volee")
    j.keys("ctrl+z", 2.0)
    j.type("Reply with the word sky in uppercase, nothing else")
    j.keys("Enter")
    j.check("after Ctrl+Z the agent still answers", j.sees("SKY", timeout=90, every=1))
    j.keys("ctrl+q", 1.0)
    j.check("the leave key comes back, the screen stays",
            j.sees("Screen · " + name + " · live", timeout=10))
    j.keys("z", 1.0)


def session_by_hand(j, name):
    """A background session started outside volee: opened where it runs."""
    session = j.background_session(name, "Remember the number 7341. Reply only OK.")
    if not j.check("a background session started by hand ran its first turn", session,
                   str(j.claude_session(name))[:200]):
        return
    handle, session_id, pid = session["id"], session["sessionId"], session.get("pid")
    row = j.wait(lambda: j.agent(name), 60, 2)
    j.check("your list shows it, as a background session", row
            and row["location"]["kind"] == "background", str(row and row["location"]))
    if not j.select(name):
        return
    j.keys("Enter", 2.0)
    j.check("Enter opens it inside volee, with no question about adopting it",
            j.sees("your keys go to " + name, "7341", timeout=20)
            and "Move " + name not in j.screen())
    j.type("Reply only with that number plus one")
    j.keys("Enter")
    j.check("typed text reaches the session and it answers", j.sees("7342", timeout=90, every=1))
    j.show("a background session opened on the installed volee")
    now = j.claude_session(name) or {}
    j.check("it is the same session in the same process",
            now.get("sessionId") == session_id and now.get("pid") == pid,
            "{} pid {}".format(now.get("sessionId"), now.get("pid")))
    j.keys("ctrl+q", 1.5)
    j.check("the leave key comes back to the list and says it keeps running",
            j.sees("Left " + name + ": it keeps running", timeout=10))
    j.claude("stop", handle)
    j.wait(lambda: not (j.claude_session(name) or {"pid": 1}).get("pid"), 30, 1)
    j.claude("rm", handle)
    j.check("removed in Claude Code, it leaves your list",
            j.wait(lambda: j.agent(name) is None, 100, 2), str(j.agent(name))[:120])


def main():
    args = arguments(__doc__, more)
    j = Journey("installed", args, isolated=False)
    try:
        version = j.volee_run("--version").stdout.strip()
        j.check("volee is installed" + (" in version " + args.expect if args.expect else ""),
                bool(version) and (not args.expect or args.expect in version), version)
        status = j.volee_run("daemon", "status").stdout
        j.check("your daemon is that version too",
                not args.expect or ("volee daemon " + args.expect) in status,
                (status.splitlines() or ["no daemon"])[0])
        missing, orphans, twice = j.compare()
        j.check("every session Claude Code lists has one row in your list",
                not missing and not twice,
                "no row for {}; several rows for {}".format(missing, twice))
        j.check("no row of a session started elsewhere that Claude Code no longer lists",
                not orphans, str(orphans))
        j.open_interface()
        if not j.check("the interface opens", j.sees("Agents", timeout=30)):
            return
        launched_agent(j, "journey-installed-" + j.stamp)
        session_by_hand(j, "journey-installed-bg-" + j.stamp)
        j.keys("q", 1.0)
    finally:
        j.finish()


main()
