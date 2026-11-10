"""Opening a session volee did not start, where it runs (ADR 0027).

The trip: a Claude Code background session started by hand shows in the list under
its own name; Enter opens it inside volee, with no adoption; what is typed reaches it
and it answers; the leave key comes back to the list and the session keeps running,
the same session in the same process. Stopped, it stays listed and opens again.
Removed, it leaves the list.

The list is compared with what Claude Code lists on the machine: every session one
row, no row without a session.

    python3 journeys/open.py --volee target/release/volee --sandbox ~/code/sandbox
"""
import time

from journey import Journey, arguments

# A session started elsewhere shows once it has lived this long (seconds).
SETTLE = 10


def main():
    args = arguments(__doc__)
    j = Journey("open", args)
    name = "journey-open-" + j.stamp
    try:
        if not j.check("the daemon starts", j.start_daemon()):
            return
        session = j.background_session(name, "Remember the number 7341. Reply only OK.")
        if not j.check("a background session started by hand ran its first turn", session,
                       str(j.claude_session(name))[:200]):
            return
        handle, session_id, pid = session["id"], session["sessionId"], session.get("pid")

        # ---- the list ---------------------------------------------------------
        row = j.wait(lambda: j.agent(name), 40, 1)
        j.check("volee lists it under the name Claude Code gives it", row, str(row)[:160])
        j.check("as a background session volee did not launch",
                bool(row) and row["location"]["kind"] == "background"
                and (row.get("launched_by") or {}).get("by") == "outside",
                str(row and (row["location"], row.get("launched_by"))))
        j.check("its row is that session", j.session_of(row) == session_id,
                "{} / {}".format(j.session_of(row), session_id))
        time.sleep(SETTLE + 6)
        missing, orphans, twice = j.compare(SETTLE)
        j.check("every session Claude Code lists has one row", not missing and not twice,
                "no row for {}; several rows for {}".format(missing, twice))
        j.check("no row without a session Claude Code lists", not orphans, str(orphans))

        # ---- open, type, come back -------------------------------------------
        j.open_interface()
        j.sees("Agents", timeout=30)
        if not j.select(name):
            return
        j.keys("Enter", 2.0)
        j.check("Enter opens it inside volee, in edit mode",
                j.sees("your keys go to " + name, "7341", timeout=20))
        j.show("a background session opened where it runs")
        j.check("no prompt asked to adopt it", "Move " + name not in j.screen())
        j.type("Reply only with that number plus one")
        j.keys("Enter")
        j.check("typed text reaches the session and it answers",
                j.sees("7342", timeout=90, every=1))
        now = j.claude_session(name) or {}
        j.check("it is the same session in the same process: nothing stopped or forked",
                now.get("sessionId") == session_id and now.get("pid") == pid
                and now.get("kind") == "background",
                "{} pid {} / {} pid {}".format(now.get("sessionId"), now.get("pid"),
                                               session_id, pid))
        copies = [s for s in j.claude_sessions() if (s.get("name") or "").startswith(name)]
        j.check("Claude Code lists no second session for it", len(copies) == 1,
                str([s.get("name") for s in copies]))
        row = j.agent(name) or {}
        j.check("volee still shows it in the background, not in a host of its own",
                row.get("location", {}).get("kind") == "background" and len(row.get("sessions", [])) == 1,
                str(row.get("location")))
        j.keys("ctrl+q", 1.5)
        j.check("the leave key comes back to the list and says it keeps running",
                j.sees("Left " + name + ": it keeps running", "Agents", timeout=10))
        now = j.claude_session(name) or {}
        j.check("the session still runs", now.get("pid") == pid, str(now.get("pid")))
        j.check("the view ends by itself once nobody uses it",
                j.wait(lambda: not j.views(), 40, 2), str(j.views()))

        # ---- a finished session is listed and opens too -------------------------
        j.claude("stop", handle)
        stopped = j.wait(lambda: not (j.claude_session(name) or {"pid": 1}).get("pid"), 30, 1)
        j.check("Claude Code stopped the session", stopped)
        state = j.wait(lambda: ((j.agent(name) or {}).get("state") or {}).get("state")
                       in ("stopped", "done"), 40, 1)
        j.check("stopped, it stays in the list", state, str((j.agent(name) or {}).get("state")))
        if j.select(name):
            j.keys("Enter", 2.0)
            j.check("Enter opens a stopped session too",
                    j.sees("your keys go to " + name, timeout=30))
            j.type("Reply only with that number plus two")
            j.keys("Enter")
            j.check("it answers again", j.sees("7343", timeout=120, every=1))
            now = j.claude_session(name) or {}
            j.check("under the same session id", now.get("sessionId") == session_id,
                    str(now.get("sessionId")))
            j.show("a stopped session opened again")
            j.keys("ctrl+q", 1.5)
            j.sees("Left " + name, timeout=10)

        # ---- removed, it leaves the list ------------------------------------------
        now = j.claude_session(name) or {}
        if now.get("pid"):
            j.claude("stop", handle)
            j.wait(lambda: not (j.claude_session(name) or {"pid": 1}).get("pid"), 30, 1)
        j.claude("rm", handle)
        j.check("removed in Claude Code, it leaves volee's list",
                j.wait(lambda: j.agent(name) is None, 100, 2), str(j.agent(name))[:120])
        j.keys("q", 1.0)
    finally:
        j.finish()


main()
