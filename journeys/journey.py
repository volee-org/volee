"""Shared pieces of the journeys: see README.md in this directory.

A journey drives the real `volee` interface by keyboard, the way a person does, with a
real harness behind it, and checks what shows on the screen. The interface runs in a
window of a headless tuios session: keys go in through `tuios send-keys`, the screen is
read with `tuios capture-pane`.

Nothing here is imported by volee, and nothing here runs in CI.
"""
import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time

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

    def volee_run(self, *args):
        return self.run([self.volee, *args])

    def start_daemon(self, binary=None):
        """Starts the journey's own daemon and waits until it answers."""
        os.makedirs(self.vars["VOLEE_CONFIG_DIR"], exist_ok=True)
        log = open(os.path.join(self.workdir, "daemon.log"), "w", encoding="utf-8")
        self.daemon = subprocess.Popen([binary or self.volee, "daemon", "run"], env=self.env,
                                       stdout=log, stderr=log, start_new_session=True)
        return bool(self.wait(lambda: self.volee_run("ls").returncode == 0, 20))

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

    def sees(self, *needles, timeout=15, every=0.5):
        """Whether every needle shows on the interface's screen within `timeout`."""
        return bool(self.wait(lambda: all(needle in self.screen() for needle in needles),
                              timeout, every))

    def select(self, name):
        """Selects the agent called `name` with the filter, and checks that the
        selected row is that agent's: no key must ever go to another agent."""
        self.keys("/")
        self.type(name)
        self.keys("Enter", 1.0)
        rows = [line for line in self.screen().splitlines() if "▌" in line]
        return self.check("the filter selects " + name + " and nothing else",
                          bool(rows) and name in rows[0], (rows or ["no selected row"])[0][:100])

    # ---- the end ----------------------------------------------------------------

    def cleanup(self):
        """Stops and removes everything the journey started."""
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
        for name in self.agents:
            self.run([own, "forget", name], env=self.clean)

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
