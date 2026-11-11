"""A Claude Code session in a terminal of its own, opened from volee (ADR 0034).

The trip: someone types `claude` in a terminal tab, with no multiplexer around it.
volee lists the session, in a terminal it cannot reach: only that terminal shows it.
Enter on its row does not fail: the bottom line says which key to press in that
terminal, and waits. The left arrow, pressed there on the empty prompt, has Claude
Code move the conversation to the background, under another session id; volee then
opens it by itself, on the same row, and what is typed reaches the same
conversation. From then on Enter opens it at once, like any background session.

The terminal is a pseudo-terminal the journey holds, which is what a terminal tab is
to the program typed in it. It works the same in any terminal application: nothing
here asks one for anything, and nothing shows on the person's screen.

    python3 journeys/terminal.py --volee target/release/volee --sandbox ~/code/sandbox
"""
import time

from journey import Journey, Terminal, arguments

# A session started elsewhere shows once it has lived this long (seconds).
SETTLE = 10


def main():
    args = arguments(__doc__)
    j = Journey("terminal", args)
    name = "journey-terminal-" + j.stamp
    try:
        if not j.check("the daemon starts", j.start_daemon()):
            return
        known = {session.get("sessionId") for session in j.claude_sessions()}
        terminal, first = j.claude_in_a_terminal(
            name, "Remember the number 7341. For this message only, reply with the word READY.")
        if not j.check("a session typed in a plain terminal ran its first turn there", first,
                       terminal.wrote(count=200)):
            return
        session_id, pid = first["sessionId"], first["pid"]

        # ---- the list ---------------------------------------------------------
        row = j.wait(lambda: j.agent(name), SETTLE + 30, 1)
        if not j.check("volee lists it under its name", row, str(row)[:160]):
            return
        j.check("in a terminal volee cannot reach, started outside volee",
                row["location"]["kind"] == "terminal"
                and (row.get("launched_by") or {}).get("by") == "outside",
                str((row["location"], row.get("launched_by"))))
        agent = row["id"]

        # ---- Enter waits, and says what to do there ------------------------------
        j.open_interface()
        j.sees("Agents", timeout=30)
        if not j.check("the interface has a window wide enough for its bottom line",
                       j.resize(160, 45)):
            return
        if not j.select(name):
            return
        waits = name + " runs in its own terminal: press ← on its empty prompt there"
        j.keys("Enter", 0.5)

        def answered_enter():
            # A notice of failure leaves the screen after a few seconds: the screen
            # is kept as it was when volee answered the key.
            screen = j.screen()
            return screen if waits in screen or "Cannot open" in screen else None
        said = j.wait(answered_enter, 10, 0.3) or ""
        j.check("Enter says which key to press in the session's own terminal", waits in said)
        j.show("volee waits for the session to leave its terminal")
        j.check("and says nothing of a failure", bool(said) and "Cannot open" not in said)
        j.keys("Escape", 1.0)
        j.check("Escape gives the wait up", j.wait(lambda: waits not in j.screen(), 5))
        still = j.claude_session(name) or {}
        j.check("and nothing happened to the session",
                still.get("sessionId") == session_id and still.get("pid") == pid
                and still.get("kind") == "interactive",
                "{} pid {}".format(still.get("sessionId"), still.get("pid")))
        j.keys("Enter", 1.5)
        j.check("Enter waits again", j.sees(waits, timeout=10))

        # ---- the left arrow, pressed in the session's own terminal -----------------
        pressed = time.time()
        terminal.press(Terminal.LEFT)
        moved = j.moved_to_background(known)
        if not j.check("the left arrow there has Claude Code move the conversation to the "
                       "background", moved, terminal.wrote(count=200)):
            return
        title = moved.get("name") or moved["id"]
        j.check("under another session id, in another process",
                moved["sessionId"] != session_id and moved["pid"] != pid,
                "{} pid {} / {} pid {}".format(moved["sessionId"], moved["pid"],
                                               session_id, pid))

        # ---- volee opens it by itself, on the same row -----------------------------
        opened = j.check("volee opens it by itself, in edit mode, on the conversation",
                         j.sees("your keys go to " + title, "7341", timeout=40),
                         "{:.0f} s after the key".format(time.time() - pressed))
        j.show("the conversation opened once it left its terminal")
        if not opened:
            # Nothing is typed into a screen that is not the conversation's.
            return
        rows = j.agents_listed()
        row = next((listed for listed in rows if listed["id"] == agent), None)
        j.check("its row followed it to the background",
                bool(row) and row["location"]["kind"] == "background"
                and j.session_of(row) == moved["sessionId"] and len(row.get("sessions", [])) == 1,
                str(row and (row["name"], row["location"], row.get("sessions")))[:200])
        of_it = [listed["name"] for listed in rows
                 if j.session_of(listed) in (session_id, moved["sessionId"])]
        j.check("and it is the only row of that conversation", len(of_it) == 1, str(of_it))
        asked = "Add one to the number you remembered. Reply with the result in digits only."
        j.type(asked)
        j.keys("Enter")
        j.check("typed text reaches the conversation and it answers", j.answered(asked),
                "it answered: " + j.last_answer())
        j.check("it is the conversation that was in the terminal: it remembers the number",
                j.sees("7342", timeout=30, every=1), "it answered: " + j.last_answer())
        now = next((session for session in j.claude_sessions()
                    if session.get("sessionId") == moved["sessionId"]), {})
        j.check("opening it stopped and copied nothing: the same session in the same process",
                now.get("pid") == moved["pid"] and now.get("kind") == "background",
                "pid {} / {}".format(now.get("pid"), moved["pid"]))
        copies = [session.get("kind") for session in j.claude_sessions()
                  if session.get("sessionId") in (session_id, moved["sessionId"])
                  or (session.get("name") or "").startswith(name)]
        j.check("Claude Code lists the conversation once", copies == ["background"], str(copies))
        j.keys("ctrl+q", 1.5)
        j.check("the leave key comes back to the list and says it keeps running",
                j.sees("Left " + title + ": it keeps running", "Agents", timeout=10))
        # What that terminal shows is the person's own list of agents: not printed.
        j.check("its own terminal still runs: Claude Code shows its agents there",
                terminal.runs())

        # ---- from then on, a background session like any other ------------------------
        if j.select(title):
            j.keys("Enter", 2.0)
            j.check("Enter now opens it at once",
                    j.sees("your keys go to " + title, timeout=20) and waits not in j.screen())
            j.keys("ctrl+q", 1.5)
            j.sees("Left " + title, timeout=10)
        time.sleep(SETTLE + 6)
        missing, orphans, twice = j.compare(SETTLE)
        j.check("every session Claude Code lists has one row", not missing and not twice,
                "no row for {}; several rows for {}".format(missing, twice))
        j.check("no row without a session Claude Code lists", not orphans, str(orphans))
        j.keys("q", 1.0)
    finally:
        j.finish()


main()
