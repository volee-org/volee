"""Edit mode on an agent volee launched.

An isolated daemon launches one Claude Code agent in the sandbox. In the interface:
the full-screen view shows it live, `i` types into it, Esc and Ctrl+Z reach the agent
without freezing it, a paste arrives in one piece, the agent follows the size of the
panel, and the leave key comes back to volee with the screen still there.

    python3 journeys/edit_mode.py --volee target/release/volee --sandbox ~/code/sandbox
"""
import fcntl
import os
import pty
import select
import signal
import struct
import termios
import time

from journey import Journey, arguments


def by_keyboard(j, name):
    opened = j.open_interface()
    j.check("the interface runs in a tuios window", opened.returncode == 0,
            (opened.stdout + opened.stderr).strip()[:160])
    j.check("the interface lists the agent", j.sees(name, timeout=30))
    if not j.select(name):
        return

    j.keys("z")
    j.check("z shows the agent's screen live", j.sees("Screen · " + name + " · live", "READY"))
    j.check("the full-screen view offers edit mode", "i edit" in j.screen())

    j.keys("i")
    editing = "your keys go to " + name
    j.check("i enters edit mode inside volee's frame", j.sees(" EDIT ", editing, timeout=10))
    j.show("edit mode")

    j.type("Reply with the word fruit in uppercase, nothing else")
    j.keys("Enter")
    j.check("typed text reaches the agent and it answers", j.sees("FRUIT", timeout=90, every=1))

    # With Claude Code's vim mode, Esc leaves its insert mode: `i` goes back to it.
    vim = "-- INSERT --" in j.screen()
    j.keys("Escape", 1.0)
    j.check("Esc goes to the agent: still in edit mode", editing in j.screen())
    if vim:
        j.keys("i", 0.5)
        j.check("the agent is back in its insert mode", "-- INSERT --" in j.screen())

    j.keys("ctrl+z", 2.0)
    j.type("Reply with the word sky in uppercase, nothing else")
    j.keys("Enter")
    j.check("after Ctrl+Z the agent still answers", j.sees("SKY", timeout=90, every=1))

    j.paste("Reply with the word sea in uppercase,\nnothing else")
    j.show("after a two-line paste")
    j.keys("Enter")
    j.check("a paste reaches the agent in one piece", j.sees("SEA", timeout=90, every=1))

    j.keys("ctrl+q", 1.0)
    back = j.wait(lambda: "Screen · " + name + " · live" in j.screen()
                  and editing not in j.screen(), 10)
    j.check("the leave key comes back to volee, the screen stays", back)

    j.keys("z", 1.0)
    j.keys("I", 1.0)
    j.check("I opens the messages", "Messages" in j.screen() or "messages" in j.screen())
    j.keys("Escape", 0.5)
    j.keys("q", 1.0)


def resized(j, name):
    """The agent follows the panel's size. A window of a headless tuios session cannot
    be resized, so the interface runs here in a pseudo-terminal of its own."""
    pid, fd = pty.fork()
    if pid == 0:
        os.execvpe(j.volee, [j.volee], j.env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))

    def drain(seconds):
        end = time.time() + seconds
        while time.time() < end:
            readable, _, _ = select.select([fd], [], [], 0.1)
            if readable:
                try:
                    os.read(fd, 65536)
                except OSError:
                    return

    def send(data, wait=0.6):
        os.write(fd, data)
        drain(wait)

    drain(3)
    send(b"/")
    send(name.encode())
    send(b"\r", 1)
    send(b"i", 2)
    first = j.wait(lambda: j.host_size(name), 5)
    j.check("edit mode gives the agent the panel's size (97x27 in 100x30)", first == (97, 27), str(first))
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 26, 90, 0, 0))
    os.kill(pid, signal.SIGWINCH)
    drain(2)
    second = j.wait(lambda: j.host_size(name) if j.host_size(name) != first else None, 5)
    j.check("the agent follows a smaller terminal (87x23 in 90x26)", second == (87, 23), str(second))
    send(b"\x11", 1)
    send(b"q", 1)
    send(b"q", 1)
    # The interface writes its last output while it exits: keep reading meanwhile.
    end = time.time() + 10
    exited = False
    while time.time() < end and not exited:
        try:
            done, _ = os.waitpid(pid, os.WNOHANG)
        except ChildProcessError:
            done = pid
        exited = bool(done)
        if not exited:
            drain(0.2)
    if not exited:
        os.kill(pid, signal.SIGKILL)
    j.check("q quits the interface", exited)
    os.close(fd)


def main():
    j = Journey("edit", arguments(__doc__))
    name = "journey-edit-" + j.stamp
    try:
        j.check("the journey's own daemon answers", j.start_daemon())
        launched = j.launch(name, "Reply only READY, then wait for my next message.")
        j.check("an agent is launched in a volee host", launched.returncode == 0,
                (launched.stdout + launched.stderr).strip()[:160])
        ready = j.wait(lambda: "READY" in j.volee_run("logs", name).stdout, 120, 2)
        if j.check("the agent answered READY", ready):
            by_keyboard(j, name)
            resized(j, name)
    finally:
        j.finish()


main()
