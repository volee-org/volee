"""Edit mode on an agent whose host was started by an older volee.

Hosts outlive upgrades, so a new interface must still type into an agent that an older
release launched. Here the daemon and the agent's host run the older binary, and the
interface is the one under test: typing works through plain terminal bytes, Ctrl+Z is
kept back with a word why (nothing in such a host would resume the agent), and the
leave key comes back.

    python3 journeys/old_host.py --volee target/release/volee \\
        --old-volee ~/bin/volee-0.9.1 --sandbox ~/code/sandbox
"""
from journey import Journey, answered_with, arguments, resolve


def more(parser):
    parser.add_argument("--old-volee", required=True,
                        help="the binary of an older release (0.8 or 0.9), which starts the "
                             "daemon and the agent's host")


def main():
    args = arguments(__doc__, more)
    j = Journey("old", args)
    old = resolve(args.old_volee)
    name = "journey-old-" + j.stamp
    try:
        j.check("the older daemon answers", j.start_daemon(binary=old))
        # The whole trip rests on this: a daemon of the version under test would
        # launch the agent in a host of that version, which encodes keys itself.
        version = (j.run([old, "--version"]).stdout.split() or [""])[-1]
        if not j.check("it is the older version, not the one under test",
                       bool(version) and j.daemon_version() == version
                       and version not in j.run([j.volee, "--version"]).stdout,
                       "daemon {} / older binary {}".format(j.daemon_version(), version)):
            return
        launched = j.launch(name, "Reply only READY, then wait for my next message.", binary=old)
        j.check("the older volee launches an agent", launched.returncode == 0,
                (launched.stdout + launched.stderr).strip()[:160])
        ready = j.wait(lambda: answered_with(j.run([old, "logs", name]).stdout, "ready"), 120, 2)
        if not j.check("the agent answered READY", ready):
            return
        j.open_interface()
        j.check("the new interface lists it", j.sees(name, timeout=30))
        if not j.select(name):
            return
        j.keys("i", 2.0)
        j.check("edit mode opens on the older host",
                j.sees(" EDIT ", timeout=10) and j.said("ready", timeout=10))
        j.type("Reply with the word fruit in uppercase, nothing else")
        j.keys("Enter")
        j.check("typed text reaches the agent through the older host", j.said("fruit"),
                "it answered: " + j.last_answer())
        j.keys("ctrl+z", 0.5)
        j.check("Ctrl+Z is kept back, with a word why",
                j.sees("Ctrl+Z is not sent", timeout=5, every=0.3))
        j.show("Ctrl+Z on an older host")
        # The notice goes away on its own; the agent must not be frozen meanwhile.
        j.wait(lambda: False, 4)
        j.type("Reply with the word sky in uppercase, nothing else")
        j.keys("Enter")
        j.check("the agent still answers", j.said("sky"), "it answered: " + j.last_answer())
        j.keys("ctrl+q", 1.0)
        j.check("the leave key comes back", j.sees("Screen · " + name + " · live", timeout=10))
        j.keys("z", 0.5)
        j.keys("q", 1.0)
    finally:
        j.finish()


main()
