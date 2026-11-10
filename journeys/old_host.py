"""Edit mode on an agent whose host was started by an older volee.

Hosts outlive upgrades, so a new interface must still type into an agent that an older
release launched. Here the daemon and the agent's host run the older binary, and the
interface is the one under test: typing works through plain terminal bytes, Ctrl+Z is
kept back with a word why (nothing in such a host would resume the agent), and the
leave key comes back.

    python3 journeys/old_host.py --volee target/release/volee \\
        --old-volee ~/bin/volee-0.9.1 --sandbox ~/code/sandbox
"""
from journey import Journey, arguments, resolve


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
        launched = j.launch(name, "Reply only READY, then wait for my next message.", binary=old)
        j.check("the older volee launches an agent", launched.returncode == 0,
                (launched.stdout + launched.stderr).strip()[:160])
        ready = j.wait(lambda: "READY" in j.run([old, "logs", name]).stdout, 120, 2)
        if not j.check("the agent answered READY", ready):
            return
        j.open_interface()
        j.check("the new interface lists it", j.sees(name, timeout=30))
        if not j.select(name):
            return
        j.keys("i", 2.0)
        j.check("edit mode opens on the older host", j.sees(" EDIT ", "READY", timeout=10))
        j.type("Reply with the word fruit in uppercase, nothing else")
        j.keys("Enter")
        j.check("typed text reaches the agent through the older host",
                j.sees("FRUIT", timeout=90, every=1))
        j.keys("ctrl+z", 1.0)
        j.check("Ctrl+Z is kept back, with a word why", "Ctrl+Z is not sent" in j.screen())
        j.show("Ctrl+Z on an older host")
        # The notice goes away on its own; the agent must not be frozen meanwhile.
        j.wait(lambda: False, 4)
        j.type("Reply with the word sky in uppercase, nothing else")
        j.keys("Enter")
        j.check("the agent still answers", j.sees("SKY", timeout=90, every=1))
        j.keys("ctrl+q", 1.0)
        j.check("the leave key comes back", j.sees("Screen · " + name + " · live", timeout=10))
        j.keys("z", 0.5)
        j.keys("q", 1.0)
    finally:
        j.finish()


main()
