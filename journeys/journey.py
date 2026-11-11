"""Shared pieces of the journeys: see README.md in this directory.

A journey drives the real `volee` interface by keyboard, the way a person does, with a
real harness behind it, and checks what shows on the screen. The interface runs in a
window of a headless tuios session: keys go in through `tuios send-keys`, the screen is
read with `tuios capture-pane`.

Nothing here is imported by volee, and nothing here runs in CI.
"""
import argparse
import fcntl
import json
import os
import pty
import re
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time
import unicodedata

# What a Claude Code session leaves in the environment of its children. A journey
# started from inside one must not pass them on: a nested session would not save its
# conversation.
SESSION_VARIABLES = """CLAUDECODE CLAUDE_CODE_AGENT CLAUDE_CODE_BRIDGE_SESSION_ID
CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_ENTRYPOINT CLAUDE_CODE_EXECPATH
CLAUDE_CODE_HOST_SHORT_TEMP_DIRS CLAUDE_CODE_MESSAGING_SOCKET
CLAUDE_CODE_MESSAGING_TOKEN CLAUDE_CODE_SESSION_ATTENDED CLAUDE_CODE_SESSION_ID
CLAUDE_EFFORT CLAUDE_JOB_DIR CLAUDE_PID CLAUDE_ENV_FILE""".split()


def arguments(description, more=None):
    """The command line every journey shares."""
    parser = argparse.ArgumentParser(description=description,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--volee", default="volee",
                        help="the volee binary under test (default: the one on PATH)")
    parser.add_argument("--sandbox", required=True,
                        help="a git repository Claude Code already trusts; the journey makes "
                             "throwaway branches and worktrees in it and removes them")
    parser.add_argument("--model", default="haiku", help="the model of the throwaway agents")
    parser.add_argument("--workdir",
                        help="where the journey keeps its state (default: a new directory "
                             "under /tmp; keep it short, Unix socket paths are limited)")
    parser.add_argument("--keep", action="store_true",
                        help="leave everything running at the end, to look at it")
    if more:
        more(parser)
    return parser.parse_args()


def cells(line):
    """A line of a screen as display cells: a wide character takes two of them, and
    the second one is None. What takes no cell (a combining mark, a variation
    selector) goes with the character before it."""
    out = []
    for character in line:
        if unicodedata.combining(character) or character in "\ufe0f\u200d":
            last = next((i for i in range(len(out) - 1, -1, -1) if out[i] is not None), None)
            if last is not None:
                out[last] += character
            continue
        out.append(character)
        if unicodedata.east_asian_width(character) in ("W", "F"):
            out.append(None)
    return out


def cut(row, width):
    """The first `width` cells of a row, as text without its trailing blanks. A wide
    character the edge would cut in two is left out, as the interface leaves it out."""
    row = list(row)
    if len(row) > width and row[width] is None and width > 0:
        row[width - 1] = " "
    return "".join(cell for cell in row[:width] if cell is not None).rstrip()


def same_tail(view, reference, width):
    """Whether a view that crops a screen shows its last rows, each cut at `width`:
    cell for cell, as far as text tells. `view` and `reference` are rows of cells.
    Returns whether it does, how many cells were compared, and the first difference."""
    shown = [cut(row, width) for row in view]
    while shown and not shown[-1]:
        shown.pop()
    wanted = [cut(row, width) for row in reference]
    while wanted and not wanted[-1]:
        wanted.pop()
    if not shown or len(shown) > len(wanted):
        return False, 0, "the view shows {} rows, the screen holds {}".format(
            len(shown), len(wanted))
    wanted = wanted[-len(shown):]
    for index, (seen, held) in enumerate(zip(shown, wanted)):
        if seen != held:
            return False, 0, "row {} of {}: the view shows {!r}, the screen holds {!r}".format(
                index + 1, len(shown), seen[:70], held[:70])
    return True, sum(len(row) for row in shown), ""


def in_order(view, reference, width):
    """Whether a view that wraps long rows shows, in order, every row of the screen
    that fits in `width` cells. Returns the same three things as `same_tail`."""
    shown = [cut(row, width) for row in view]
    position, compared = 0, 0
    for row in reference:
        wanted = cut(row, len(row))
        if not wanted or len(row) > width:
            continue
        try:
            position = shown.index(wanted, position) + 1
        except ValueError:
            return False, compared, "the view lacks the row {!r}".format(wanted[:70])
        compared += len(wanted)
    return compared > 0, compared, "" if compared else "no row to compare"


def answered_with(text, word):
    """Whether, on a screen given as text, the agent wrote `word` in a line of its
    own, which it marks with ⏺: an answer, not the prompt that asked for it. Whatever
    the case: asked for a word in uppercase, a small model now and then writes it in
    lowercase, which is the model's business and not volee's."""
    wanted = re.compile(r"(?<![0-9A-Za-z])" + re.escape(word) + r"(?![0-9A-Za-z])",
                        re.IGNORECASE)
    return any("⏺" in line and wanted.search(line) for line in text.splitlines())


def resolve(program):
    """The absolute path of a program given as a path or as a name on PATH."""
    if os.sep in program:
        return os.path.abspath(os.path.expanduser(program))
    return shutil.which(program) or program


class Journey:
    """One run: its environment, its tuios window, its checks and its cleanup."""

    def __init__(self, name, args, isolated=True):
        self.name = name
        self.volee = resolve(args.volee)
        self.sandbox = os.path.abspath(os.path.expanduser(args.sandbox))
        self.model = args.model
        self.keep = args.keep
        self.stamp = str(int(time.time()))[-6:]
        if args.workdir:
            self.workdir = os.path.abspath(os.path.expanduser(args.workdir))
            os.makedirs(self.workdir, exist_ok=True)
        else:
            self.workdir = tempfile.mkdtemp(prefix="vj-", dir="/tmp")
        self.clean = {key: value for key, value in os.environ.items()
                      if key not in SESSION_VARIABLES and not key.startswith("VOLEE_")}
        self.clean["TERM"] = "xterm-256color"
        # An isolated journey has its own daemon, registry and hosts; the others use
        # the person's own volee.
        self.isolated = isolated
        self.vars = {}
        if isolated:
            self.vars = {
                "VOLEE_STATE_DIR": os.path.join(self.workdir, "state"),
                "VOLEE_CONFIG_DIR": os.path.join(self.workdir, "config"),
                "VOLEE_DATA_DIR": os.path.join(self.workdir, "data"),
                "VOLEE_SOCKET": os.path.join(self.workdir, "state", "volee.sock"),
            }
        self.env = dict(self.clean, **self.vars)
        self.session = "volee-journey-" + self.stamp
        self.window = "volee"
        self.results = []
        self.daemon = None
        self.agents = []
        self.branches = []
        self.background = []
        # Names of the sessions the journey started by hand, in a pane.
        self.by_hand = []
        # The client that gives the interface's window its size, if any.
        self.viewer = None

    # ---- output -----------------------------------------------------------------

    def say(self, text):
        print(text, flush=True)

    def check(self, what, passed, seen=""):
        """Records one check and says how it went. Returns whether it passed."""
        self.results.append((what, bool(passed)))
        self.say(("PASS " if passed else "FAIL ") + what + (": " + seen if seen else ""))
        return bool(passed)

    def show(self, title):
        """Prints the interface's screen, for the record of a run."""
        self.say("----- " + title)
        self.say("\n".join(line.rstrip() for line in self.screen().splitlines()))

    # ---- processes --------------------------------------------------------------

    def run(self, command, env=None, cwd=None):
        return subprocess.run(command, env=self.env if env is None else env, cwd=cwd,
                              capture_output=True, text=True, check=False)

    def wait(self, ready, timeout, every=0.5):
        """Polls `ready` until it returns something true, or `timeout` seconds pass."""
        end = time.time() + timeout
        while time.time() < end:
            value = ready()
            if value:
                return value
            time.sleep(every)
        return None

    def still(self, read, quiet=4, timeout=90):
        """Polls `read` until it has returned the same thing for `quiet` seconds: a
        screen at rest, once an agent has finished writing on it. Returns what it
        read, or None when it never came to rest within `timeout` seconds."""
        last, since, end = read(), time.time(), time.time() + timeout
        while time.time() < end:
            time.sleep(1)
            now = read()
            if now != last:
                last, since = now, time.time()
            elif time.time() - since >= quiet:
                return last
        return None

    def volee_run(self, *args):
        return self.run([self.volee, *args])

    def start_daemon(self, binary=None):
        """Starts the journey's own daemon and waits until it answers.

        No volee command runs before the daemon listens. Any command starts a daemon
        when none answers yet, and that second daemon would be the binary under test:
        it could take the lock first, and a journey that means to run an older daemon
        would run the new one without knowing. So the wait only knocks on the socket,
        which starts nothing, and the daemon that answers must be the process started
        here."""
        os.makedirs(self.vars["VOLEE_CONFIG_DIR"], exist_ok=True)
        log = open(os.path.join(self.workdir, "daemon.log"), "w", encoding="utf-8")
        self.daemon = subprocess.Popen([binary or self.volee, "daemon", "run"], env=self.env,
                                       stdout=log, stderr=log, start_new_session=True)

        def listening():
            knock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                knock.connect(self.vars["VOLEE_SOCKET"])
                return True
            except OSError:
                return False
            finally:
                knock.close()
        if not self.wait(listening, 20, 0.1):
            return False
        return self.daemon.poll() is None and self.volee_run("ls").returncode == 0

    def daemon_version(self):
        """The version of the daemon that answers, as `volee daemon status` says it:
        `volee daemon 0.9.1 (pid ...)`."""
        words = self.volee_run("daemon", "status").stdout.split()
        return words[2] if words[:2] == ["volee", "daemon"] and len(words) > 2 else ""

    def launch(self, name, task, binary=None):
        """Launches a throwaway agent in the sandbox, on a branch of its own."""
        branch = name if name.startswith("journey-") else "journey-" + name
        self.agents.append(name)
        self.branches.append(branch)
        return self.run([binary or self.volee, "spawn", self.sandbox, branch, task,
                         "--model", self.model, "--name", name])

    def agents_listed(self):
        """The agents volee lists, as `volee ls --json` gives them."""
        try:
            data = json.loads(self.volee_run("ls", "--json").stdout or "[]")
        except json.JSONDecodeError:
            return []
        return data if isinstance(data, list) else data.get("agents", [])

    def agent(self, name):
        return next((agent for agent in self.agents_listed() if agent["name"] == name), None)

    def hosts(self):
        """The volee hosts of this journey's daemon (or of the person's own volee)."""
        command = [self.volee, "host"]
        if self.isolated:
            command += ["--dir", os.path.join(self.vars["VOLEE_STATE_DIR"], "hosts")]
        try:
            return json.loads(self.run(command + ["ls", "--json"]).stdout or "[]")
        except json.JSONDecodeError:
            return []

    def host_size(self, name):
        """Columns and rows of the running host of the agent called `name`."""
        for row in self.hosts():
            record = row["record"]
            if record.get("name") == name and row["live"]:
                return (record["size"]["cols"], record["size"]["rows"])
        return None

    def claude(self, *args, cwd=None):
        return self.run(["claude", *args], env=self.clean, cwd=cwd)

    def claude_sessions(self, everything=True):
        """What Claude Code lists: `claude agents --json`, with finished ones too."""
        command = ["agents", "--json"] + (["--all"] if everything else [])
        try:
            return json.loads(self.claude(*command).stdout or "[]")
        except json.JSONDecodeError:
            return []

    def claude_session(self, name):
        """The session Claude Code lists under exactly `name`, if any."""
        return next((session for session in self.claude_sessions()
                     if session.get("name") == name), None)

    def background_session(self, name, prompt):
        """Starts a Claude Code background session by hand, in the sandbox, as a
        person would outside volee, and waits until its first turn is over. Returns
        what Claude Code lists for it."""
        self.background.append(name)
        started = self.claude("--bg", "--model", self.model, "--name", name, "--", prompt,
                              cwd=self.sandbox)
        if started.returncode != 0:
            self.say((started.stdout + started.stderr).strip()[:300])
            return None

        def settled():
            session = self.claude_session(name)
            return session if session and session.get("state") in ("idle", "done") else None
        return self.wait(settled, 120, 2)

    def views(self):
        """The views the journey's daemon holds open now (ADR 0027)."""
        state = self.vars.get("VOLEE_STATE_DIR") or self.clean.get("XDG_STATE_HOME")
        if state and "VOLEE_STATE_DIR" not in self.vars:
            state = os.path.join(state, "volee")
        state = state or os.path.expanduser("~/.local/state/volee")
        directory = os.path.join(state, "views")
        try:
            return sorted(os.listdir(directory))
        except OSError:
            return []

    def compare(self, settle=10):
        """What Claude Code lists against what volee lists, as three lists of names:
        sessions without a row, rows without a session, sessions with several rows.
        A session started elsewhere shows once it has lived `settle` seconds; one
        that is younger is not expected yet."""
        now = time.time() * 1000
        sessions = self.claude_sessions()
        listed = {}
        for session in sessions:
            background = session.get("kind") == "background"
            settled = now - (session.get("startedAt") or 0) >= (settle + 5) * 1000
            if background or (session.get("pid") and settled):
                listed[session.get("sessionId")] = session.get("name") or session.get("id")
        rows = {}
        watched = {}
        for agent in self.agents_listed():
            session = self.session_of(agent)
            rows.setdefault(session, []).append(agent["name"])
            outside = (agent.get("launched_by") or {}).get("by") == "outside"
            # A session that just ended shows stopped or lost for a minute.
            state = (agent.get("state") or {}).get("state")
            leaving = state in ("stopped", "lost") and now - agent.get("state_since", 0) < 75_000
            if outside and agent["location"]["kind"] != "host" and session and not leaving:
                watched[session] = agent["name"]
        current = {session.get("sessionId") for session in sessions}
        missing = sorted(name for session, name in listed.items() if session not in rows)
        orphans = sorted(name for session, name in watched.items() if session not in current)
        twice = sorted(str(names) for session, names in rows.items()
                       if session and len(names) > 1)
        return missing, orphans, twice

    def session_of(self, agent):
        """The id of the harness session a row of `volee ls --json` stands for."""
        sessions = (agent or {}).get("sessions") or []
        if not sessions:
            return None
        key = sessions[-1].get("key", sessions[-1])
        return key.get("id") if isinstance(key, dict) else None

    # ---- the interface, in a tuios window -----------------------------------------

    def tuios(self, *args):
        return self.run(["tuios", *args], env=self.clean)

    def open_interface(self, binary=None, more_env=None):
        """Runs the interface in a window of a new headless tuios session."""
        self.tuios("kill-session", self.session)
        created = self.tuios("new", self.session, "--detach", "--cwd", self.workdir)
        if created.returncode != 0:
            return created
        pairs = [key + "=" + value for key, value in dict(self.vars, **(more_env or {})).items()]
        return self.tuios("new-window", "-s", self.session, self.window, "--",
                          "env", *pairs, binary or self.volee)

    def resize(self, cols, rows):
        """Gives the interface's window `cols` columns and `rows` rows. A window of a
        headless tuios session has 80 columns and 24 rows until a client shows it:
        `tuios pane-view` shows one pane with nothing around it, here in a
        pseudo-terminal of the wanted size that the journey holds until its end."""
        self.close_viewer()
        pid, fd = pty.fork()
        if pid == 0:
            os.execvpe("tuios", ["tuios", "pane-view", "-s", self.session, "-w", self.window],
                       self.clean)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        try:
            os.kill(pid, signal.SIGWINCH)
        except OSError:
            pass

        def drain():
            try:
                while os.read(fd, 65536):
                    pass
            except OSError:
                pass
        threading.Thread(target=drain, daemon=True).start()
        self.viewer = (pid, fd)
        return bool(self.wait(lambda: len(self.screen().splitlines()) >= rows - 2, 15))

    def close_viewer(self):
        if self.viewer is None:
            return
        pid, fd = self.viewer
        self.viewer = None
        try:
            os.kill(pid, signal.SIGTERM)
        except OSError:
            pass
        try:
            os.close(fd)
        except OSError:
            pass
        try:
            os.waitpid(pid, 0)
        except OSError:
            pass

    def keys(self, keys, pause=0.4):
        """Presses keys, named as `tuios send-keys` names them: `Enter`, `ctrl+q`, `z`."""
        self.tuios("send-keys", "-s", self.session, "-w", self.window, keys)
        time.sleep(pause)

    def type(self, text, pause=0.4):
        """Types text as one burst, as a fast typist or a paste tool does."""
        self.tuios("send-text", "-s", self.session, "-w", self.window, text)
        time.sleep(pause)

    def paste(self, text, pause=1.0):
        """Pastes text, in bracketed paste when the interface turned it on."""
        buffer = "journey-" + self.stamp
        self.tuios("set-buffer", "-b", buffer, text)
        self.tuios("paste-buffer", "-s", self.session, "-w", self.window, "-b", buffer)
        self.tuios("delete-buffer", "-b", buffer)
        time.sleep(pause)

    def screen(self):
        return self.tuios("capture-pane", "-s", self.session, "-w", self.window).stdout

    def frame(self, needle, margin=0):
        """The rows inside the frame whose top border carries `needle`, as rows of
        cells, starting `margin` columns in from its left border, and their width.
        (None, 0) when no such frame shows."""
        lines = [cells(line) for line in self.screen().splitlines()]
        target = cells(needle)
        for top, row in enumerate(lines):
            at = next((i for i in range(len(row) - len(target) + 1)
                       if row[i:i + len(target)] == target), None)
            if at is None:
                continue
            left = next((i for i in range(at - 1, -1, -1) if row[i] in ("╭", "┏")), None)
            right = next((i for i in range(at + len(target), len(row))
                          if row[i] in ("╮", "┓")), None)
            if left is None or right is None:
                continue
            rows = []
            for below in lines[top + 1:]:
                if len(below) > left and below[left] in ("╰", "┗"):
                    return rows, right - left - 1 - margin
                rows.append(below[left + 1 + margin:right])
        return None, 0

    def answered(self, text, timeout=90):
        """Whether the agent on screen took `text` as a turn and answered it: the text
        shows in the conversation, after a prompt mark, and the agent wrote below it.
        Text that only sits in the input box has no answer under it."""
        start = text[:40]

        def taken():
            lines = self.screen().splitlines()
            asked = next((index for index in range(len(lines) - 1, -1, -1)
                          if start in lines[index] and "❯" in lines[index]), None)
            return asked is not None and any("⏺" in line for line in lines[asked + 1:])
        return bool(self.wait(taken, timeout, 1))

    def last_answer(self):
        """The last line the agent wrote on the screen, to say what it answered when it
        is not what a check expected."""
        lines = [line for line in self.screen().splitlines() if "⏺" in line]
        return lines[-1].split("⏺", 1)[1].strip(" │┃") if lines else "nothing"

    def said(self, word, timeout=90, every=1):
        """Whether the agent on the interface's screen answers with `word` within
        `timeout`, in a line of its own and whatever the case (`answered_with`)."""
        return bool(self.wait(lambda: answered_with(self.screen(), word), timeout, every))

    def sees(self, *needles, timeout=15, every=0.5):
        """Whether every needle shows on the interface's screen within `timeout`."""
        return bool(self.wait(lambda: all(needle in self.screen() for needle in needles),
                              timeout, every))

    def select(self, name):
        """Selects the agent called `name` with the filter, and checks that the
        selected row is that agent's: no key must ever go to another agent."""
        # A filter left by an earlier call is cleared first, or the name would be
        # typed after it.
        self.keys("Escape", 0.3)
        self.keys("/")
        self.type(name)
        self.keys("Enter", 1.0)
        rows = [line for line in self.screen().splitlines() if "▌" in line]
        return self.check("the filter selects " + name + " and nothing else",
                          bool(rows) and name in rows[0], (rows or ["no selected row"])[0][:100])

    # ---- the end ----------------------------------------------------------------

    def cleanup(self):
        """Stops and removes everything the journey started."""
        self.close_viewer()
        for name in self.agents:
            self.volee_run("stop", name)
        if self.agents:
            time.sleep(3)
        for name in self.agents:
            self.volee_run("forget", name)
        for name in self.background:
            for session in self.claude_sessions():
                if (session.get("name") or "").startswith(name) and session.get("kind") == "background":
                    if session.get("pid"):
                        self.claude("stop", session["id"])
                    self.claude("rm", session["id"])
        if self.isolated:
            # A host lingers a few minutes after its program ends, to show its last
            # screen. The journey's own hosts have nobody left to show it to.
            for row in self.hosts():
                pid = row["record"].get("host_pid")
                if row["live"] and pid:
                    try:
                        os.kill(pid, signal.SIGTERM)
                    except OSError:
                        pass
        if self.daemon is not None:
            self.volee_run("daemon", "stop")
            try:
                self.daemon.wait(timeout=20)
            except subprocess.TimeoutExpired:
                self.daemon.kill()
        self.tuios("kill-session", self.session)
        self.remove_branches()
        if self.isolated:
            self.forget_in_own_volee()
        shutil.rmtree(self.workdir, ignore_errors=True)

    def forget_in_own_volee(self):
        """The person's own daemon, when one runs, sees the journey's agents as
        sessions started elsewhere and lists them: unlist them there too."""
        own = shutil.which("volee", path=self.clean.get("PATH")) or self.volee
        # `volee daemon status` starts nothing; any other command would start a daemon.
        status = self.run([own, "daemon", "status"], env=self.clean).stdout
        if not status.startswith("volee daemon "):
            return
        # A volee from before 0.11 keeps the rows of sessions that are gone, one per
        # session id: by id, since several rows may carry the same name.
        names = set(self.agents + self.background + self.by_hand)
        try:
            data = json.loads(self.run([own, "ls", "--json"], env=self.clean).stdout or "[]")
        except json.JSONDecodeError:
            return
        for agent in data if isinstance(data, list) else data.get("agents", []):
            if agent.get("name") in names:
                self.run([own, "forget", agent["id"]], env=self.clean)

    def remove_branches(self):
        """Removes the worktrees and branches the journey made in the sandbox."""
        listing = self.run(["git", "-C", self.sandbox, "worktree", "list", "--porcelain"],
                           env=self.clean).stdout
        path = None
        for line in listing.splitlines():
            if line.startswith("worktree "):
                path = line[len("worktree "):]
            elif line.startswith("branch refs/heads/") and path:
                if line[len("branch refs/heads/"):] in self.branches:
                    self.run(["git", "-C", self.sandbox, "worktree", "unlock", path], env=self.clean)
                    self.run(["git", "-C", self.sandbox, "worktree", "remove", "--force", path],
                             env=self.clean)
        for branch in self.branches:
            self.run(["git", "-C", self.sandbox, "branch", "-D", branch], env=self.clean)

    def finish(self):
        """Cleans up, prints the verdict and exits: 0 when every check passed."""
        if self.keep:
            self.say("kept: tuios session " + self.session + ", state in " + self.workdir)
        else:
            self.cleanup()
        failed = [what for what, passed in self.results if not passed]
        self.say("{}/{} checks passed".format(len(self.results) - len(failed), len(self.results)))
        for what in failed:
            self.say("failed: " + what)
        sys.exit(1 if failed or not self.results else 0)
