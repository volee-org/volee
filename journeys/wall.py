"""The wall of screens, and the live preview (ADR 0028).

The trip: two agents run, one that volee launched, in a volee host, and a Claude Code
background session started by hand, which volee hosts nothing of. The preview of the
first is its live screen. `5` shows both screens side by side; moving on the wall moves
between them; Enter opens the selected one where it runs, what is typed reaches it,
and the leave key comes back to the wall; `z` shows a screen full size and comes back
to the wall; `q` leaves the wall on the agent it was on.

The wall only looks: it opens no terminal on a session and resizes none.

    python3 journeys/wall.py --volee target/release/volee --sandbox ~/code/sandbox
"""
from journey import Journey, arguments

SPELL = "Reply only with the word made of these letters, in uppercase: "


def selected_tile(j):
    """The title of the wall's selected tile: the one with the thick border."""
    for line in j.screen().splitlines():
        start = line.find("┏")
        if start >= 0:
            end = line.find("┓", start)
            return line[start + 1:end if end >= 0 else len(line)].strip(" ━")
    return ""


def move_to(j, name):
    """Moves the wall's selection to the tile of `name`: two tiles, one key."""
    if name not in selected_tile(j):
        j.keys("l", 0.8)
    if name not in selected_tile(j):
        j.keys("h", 0.8)
    return name in selected_tile(j)


def main():
    args = arguments(__doc__)
    j = Journey("wall", args)
    prefix = "jw-" + j.stamp
    hosted, outside = prefix + "-h", prefix + "-b"
    try:
        if not j.check("the journey's own daemon answers", j.start_daemon()):
            return
        launched = j.launch(hosted, SPELL + "h o s t e d")
        j.check("an agent is launched in a volee host", launched.returncode == 0,
                (launched.stdout + launched.stderr).strip()[:160])
        session = j.background_session(outside, SPELL + "o u t s i d e")
        if not j.check("a background session started by hand ran its first turn", session,
                       str(j.claude_session(outside))[:200]):
            return
        session_id, pid = session["sessionId"], session.get("pid")
        answered = j.wait(lambda: "HOSTED" in j.volee_run("logs", hosted).stdout, 120, 2)
        if not j.check("the launched agent answered", answered):
            return
        j.check("volee lists both", j.wait(lambda: j.agent(hosted) and j.agent(outside), 60, 1))

        opened = j.open_interface()
        j.check("the interface runs in a tuios window", opened.returncode == 0,
                (opened.stdout + opened.stderr).strip()[:160])
        j.sees("Agents", timeout=30)
        # Room for two screens side by side, as on a desk: a tile shows the bottom of
        # a screen, and needs rows above an agent's prompt and status lines.
        j.check("the window takes the size of a large terminal", j.resize(170, 50),
                "{} lines".format(len(j.screen().splitlines())))
        # ---- the live preview ------------------------------------------------------
        if not j.select(hosted):
            return
        j.check("the preview of an agent in a volee host is its live screen",
                j.sees("Preview · " + hosted + " · live", "HOSTED", timeout=15))
        # Only the journey's two agents: the wall shows the agents of the list, and
        # the filter keeps the list to them. No key goes to anyone else's agent.
        j.keys("Escape", 0.5)
        j.keys("/")
        j.type(prefix)
        j.keys("Enter", 1.0)
        rows = [line for line in j.screen().splitlines() if prefix + "-" in line and "│" in line]
        listed = {name for name in (hosted, outside) if any(name in row for row in rows)}
        if not j.check("the filter keeps the list to the journey's two agents",
                       listed == {hosted, outside}, str(sorted(listed))):
            return
        rows = [line for line in j.screen().splitlines() if "▌" in line]
        if not j.check("the selection stays on the agent in a volee host",
                       bool(rows) and hosted in rows[0], (rows or ["no selected row"])[0][:100]):
            return
        size = j.host_size(hosted)

        # ---- the wall ---------------------------------------------------------------
        j.keys("5", 2.0)
        j.check("5 shows the wall, on the two agents of the list",
                j.sees("Wall", "2 agents run", hosted, outside, timeout=15))
        j.check("the agent in a volee host is followed live",
                j.wait(lambda: any(hosted in line and "live" in line
                                   for line in j.screen().splitlines()), 15))
        j.check("each tile shows its agent's screen",
                j.sees("HOSTED", "OUTSIDE", timeout=30, every=1))
        j.show("the wall")
        j.check("the wall opened no terminal on the session it only shows", not j.views(),
                str(j.views()))
        j.check("the wall resized nothing", j.host_size(hosted) == size,
                "{} / {}".format(j.host_size(hosted), size))
        j.check("the selected tile is the agent the list was on", hosted in selected_tile(j),
                selected_tile(j))

        # ---- a session volee does not host, opened from the wall --------------------
        if not j.check("a key moves the selection to the other tile", move_to(j, outside),
                       selected_tile(j)):
            return
        j.keys("Enter", 2.0)
        j.check("Enter opens the selected tile's session inside volee",
                j.sees("your keys go to " + outside, timeout=20))
        j.type(SPELL + "b a c k")
        j.keys("Enter")
        j.check("typed text reaches it and it answers", j.sees("BACK", timeout=90, every=1))
        now = j.claude_session(outside) or {}
        j.check("it is the same session in the same process: nothing stopped or forked",
                now.get("sessionId") == session_id and now.get("pid") == pid,
                "{} pid {} / {} pid {}".format(now.get("sessionId"), now.get("pid"),
                                               session_id, pid))
        j.keys("ctrl+q", 1.5)
        j.check("the leave key comes back to the wall",
                j.sees("Wall", "2 agents run", timeout=10))
        j.check("on the tile that was opened", outside in selected_tile(j), selected_tile(j))
        j.check("the tile shows what was just said", j.sees("BACK", timeout=30, every=1))

        # ---- an agent in a volee host, opened from the wall --------------------------
        if not j.check("a key moves the selection back", move_to(j, hosted), selected_tile(j)):
            return
        j.keys("i", 2.0)
        j.check("i opens the agent in edit mode",
                j.sees(" EDIT ", "your keys go to " + hosted, timeout=10))
        j.type(SPELL + "t i l e")
        j.keys("Enter")
        j.check("typed text reaches it and it answers", j.sees("TILE", timeout=90, every=1))
        j.keys("ctrl+q", 1.5)
        j.check("the leave key gives the keys back, the screen stays",
                j.sees("Screen · " + hosted + " · live", timeout=10))
        j.keys("Escape", 1.5)
        j.check("back comes back to the wall",
                j.sees("Wall", "2 agents run", timeout=10))
        j.check("whose tile shows what was just said", j.sees("TILE", timeout=15, every=1))

        # ---- full size, and out -------------------------------------------------------
        j.keys("z", 1.5)
        j.check("z shows the selected screen full size",
                j.sees("Screen · " + hosted, timeout=10) and "Wall" not in j.screen())
        j.keys("Escape", 1.5)
        j.check("and comes back to the wall", j.sees("Wall", "2 agents run", timeout=10))
        j.keys("q", 1.5)
        left = j.wait(lambda: "Agents" in j.screen() and "Wall ·" not in j.screen(), 10)
        j.check("q leaves the wall", left)
        rows = [line for line in j.screen().splitlines() if "▌" in line]
        j.check("on the agent the wall was on", bool(rows) and hosted in rows[0],
                (rows or ["no selected row"])[0][:100])
        j.keys("Escape", 0.5)
        j.keys("q", 1.0)
    finally:
        j.finish()


main()
