"""Opening a session that runs in a tuios pane, where it runs (ADR 0027).

The trip: Claude Code started by hand in a pane of tuios shows in the list once it
has lived a few seconds, with the pane it runs in; Enter opens that pane inside
volee; what is typed reaches the session and it answers; the leave key comes back
and the pane goes on as before. `/clear` in the pane starts a new session in the
same process: the row stays the same row. Quit, the session leaves the list.

    python3 journeys/pane.py --volee target/release/volee --sandbox ~/code/sandbox
"""
import shlex

from journey import Journey, arguments

PANE = "agent"


def main():
    args = arguments(__doc__)
    j = Journey("pane", args)
    name = "journey-pane-" + j.stamp
    j.by_hand.append(name)

    def pane_screen():
        return j.tuios("capture-pane", "-s", j.session, "-w", PANE).stdout

    def in_pane(text):
        j.tuios("send-text", "-s", j.session, "-w", PANE, text)
        j.tuios("send-keys", "-s", j.session, "-w", PANE, "Enter")

    try:
        if not j.check("the daemon starts", j.start_daemon()):
            return
        j.open_interface()
        j.sees("Agents", timeout=30)
        # A person starts Claude Code in a pane, outside volee.
        command = "cd {} && exec claude --model {} --name {}".format(
            shlex.quote(j.sandbox), shlex.quote(j.model), shlex.quote(name))
        started = j.tuios("new-window", "-s", j.session, PANE, "--", "sh", "-c", command)
        j.check("Claude Code starts in a tuios pane", started.returncode == 0,
                (started.stdout + started.stderr).strip()[:160])
        before = j.wait(lambda: j.claude_session(name), 40, 1)
        if not j.check("Claude Code lists the session", before, str(before)[:160]):
            return

        # ---- the list ---------------------------------------------------------
        j.check("a session a few seconds old is not in the list yet", j.agent(name) is None,
                str(j.agent(name))[:120])
        row = j.wait(lambda: (j.agent(name) or {}).get("location", {}).get("kind") == "pane"
                     and j.agent(name), 60, 1)
        j.check("once it has lived ten seconds, volee lists it in its tuios pane", row,
                str((j.agent(name) or {}).get("location")))
        agent_id = (row or {}).get("id")

        # ---- open, type, come back -------------------------------------------
        if not j.select(name):
            return
        j.keys("Enter", 2.5)
        j.check("Enter opens the pane inside volee, in edit mode",
                j.sees("your keys go to " + name, timeout=20))
        j.type("Reply only with the word fruit in uppercase")
        j.keys("Enter")
        j.check("typed text reaches the session and it answers",
                j.sees("FRUIT", timeout=90, every=1))
        j.show("a tuios pane opened where it runs")
        now = j.claude_session(name) or {}
        j.check("it is the same session in the same process",
                now.get("sessionId") == before.get("sessionId") and now.get("pid") == before.get("pid"),
                "{} pid {}".format(now.get("sessionId"), now.get("pid")))
        j.keys("ctrl+q", 1.5)
        j.check("the leave key comes back to the list and says it keeps running",
                j.sees("Left " + name + ": it keeps running", "Agents", timeout=10))
        j.check("the pane goes on as before, with what was said in it",
                j.wait(lambda: "FRUIT" in pane_screen(), 15, 1), pane_screen()[-200:])
        j.check("the view ends by itself once nobody uses it",
                j.wait(lambda: not j.views(), 40, 2), str(j.views()))

        # ---- /clear: a new session in the same process is the same row -----------
        in_pane("/clear")
        changed = j.wait(lambda: (j.claude_session(name) or before).get("sessionId")
                         != before.get("sessionId"), 30, 1)
        j.check("/clear gave the pane's process a new session id", changed)
        new_session = (j.claude_session(name) or {}).get("sessionId")
        followed = j.wait(lambda: j.session_of(j.agent(name)) == new_session, 40, 1)
        rows = [agent for agent in j.agents_listed() if agent["name"] == name]
        j.check("volee keeps the same row for it",
                followed and len(rows) == 1 and rows[0].get("id") == agent_id,
                str([(agent.get("id"), agent["state"]) for agent in rows]))

        # ---- quit, it leaves the list ---------------------------------------------
        in_pane("/exit")
        j.check("quit in its pane, it leaves volee's list",
                j.wait(lambda: j.agent(name) is None, 120, 2), str(j.agent(name))[:120])
        j.keys("q", 1.0)
    finally:
        j.finish()


main()
