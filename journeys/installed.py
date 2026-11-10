"""Edit mode on the volee that is installed, with the person's own daemon.

The last check before and after a release: the installed binary, the real daemon and
its real list of agents. The journey launches one throwaway agent in the sandbox,
finds it by its unique name, types into it, then stops and forgets it. It touches no
other agent.

    python3 journeys/installed.py --sandbox ~/code/sandbox [--expect 0.10.0]
"""
from journey import Journey, arguments


def more(parser):
    parser.add_argument("--expect", help="the version the installed volee must report")


def main():
    args = arguments(__doc__, more)
    j = Journey("installed", args, isolated=False)
    name = "journey-installed-" + j.stamp
    try:
        version = j.volee_run("--version").stdout.strip()
        j.check("volee is installed" + (" in version " + args.expect if args.expect else ""),
                bool(version) and (not args.expect or args.expect in version), version)
        launched = j.launch(name, "Reply only READY, then wait for my next message.")
        j.check("the person's daemon launches a throwaway agent", launched.returncode == 0,
                (launched.stdout + launched.stderr).strip()[:160])
        ready = j.wait(lambda: "READY" in j.volee_run("logs", name).stdout, 120, 2)
        if not j.check("the agent answered READY", ready):
            return
        agent = j.agent(name)
        j.check("it runs in a volee host", bool(agent) and agent["location"]["kind"] == "host",
                str(agent and agent["location"]))
        j.open_interface()
        j.sees("Agents", timeout=30)
        if not j.select(name):
            return
        j.keys("z", 2.0)
        j.check("z shows its screen live", j.sees("Screen · " + name + " · live", "READY"))
        j.keys("i", 2.0)
        j.check("i enters edit mode", j.sees("your keys go to " + name, timeout=10))
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
        j.keys("q", 1.0)
    finally:
        j.finish()


main()
