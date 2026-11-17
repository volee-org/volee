"""A message for an agent that rests at its prompt with no hook waiting for it.

The trip: an agent volee launched answers and rests. Its hook waits for messages
for `[agents] wake_timeout` seconds only (five here, an hour by default); past that,
nothing hands it a message. volee holds its terminal, so once the agent has rested
twenty seconds with no hook waiting and nobody typing into it, the daemon types the
message at its prompt and marks it delivered. The agent answers it.

Found on 2026-11-17: a question to an agent that had rested since the 12th was
never delivered.

    python3 journeys/deliver.py --volee target/release/volee --sandbox ~/code/sandbox
"""
import os
import re
import time

from journey import Journey, answered_with, arguments

SPELL = "Reply only with the word made of these letters, in uppercase: "


def main():
    j = Journey("deliver", arguments(__doc__))
    name = "journey-deliver-" + j.stamp
    try:
        config = j.vars["VOLEE_CONFIG_DIR"]
        os.makedirs(config, exist_ok=True)
        with open(os.path.join(config, "config.toml"), "w", encoding="utf-8") as file:
            file.write("[agents]\nwake_timeout = 5\n")
        if not j.check("the journey's own daemon answers", j.start_daemon()):
            return
        launched = j.launch(name, SPELL + "r e a d y")
        if not j.check("an agent is launched in a volee host", launched.returncode == 0,
                       (launched.stdout + launched.stderr).strip()[:160]):
            return
        ready = j.wait(lambda: answered_with(j.volee_run("logs", name).stdout, "ready"), 120, 2)
        if not j.check("it answered READY and rests at its prompt", ready):
            return
        # Its hook waits five seconds for messages; volee types once the agent has
        # rested twenty. Thirty seconds on, only volee can hand it a message.
        time.sleep(30)
        sent = j.volee_run("msg", "send", name, SPELL + "t y p e d")
        if not j.check("a message is sent to it", sent.returncode == 0,
                       (sent.stdout + sent.stderr).strip()[:160]):
            return
        typed = re.compile(r"\bTYPED\b")
        answered = j.wait(lambda: typed.search(j.volee_run("logs", name).stdout), 120, 2)
        screen = j.volee_run("logs", name).stdout
        j.check("volee typed the message at its prompt, and the agent answered it",
                bool(answered), screen[-400:])
        j.say("----- the agent's screen")
        j.say(screen)
    finally:
        j.finish()


main()
