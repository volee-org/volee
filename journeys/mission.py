"""A mission from the person's words to its end, in two repositories without a remote
(ADR 0032, ADR 0033).

The trip: the person starts a lead and, in the interface, tells it what they want
across two repositories. The lead starts a mission and gives the hand back. The
mission's coordinator plans it and launches one worker per repository; each worker
commits its work on a branch of its own, which is all a repository without a remote
can ask of it. The coordinator merges nothing: it presents its report, and the mission
waits for the person. In the interface the question sits on top of the messages with
its two answers; `1` then `y` says "ok merge". Only then is each worker's branch
merged into the main branch of its repository, with a merge commit, by volee itself:
no agent merges. volee then closes the mission, tidies the workers' worktrees, keeps
their branches, stops the coordinator, and says so in one message, which the lead
hears too. Last, a task in one repository: the lead gives it to one agent itself,
which commits and reports to it; nobody merges it.

Real agents do the work, on a small model: the goal is a file of one line in each
repository.

At its end the journey puts both repositories back where they were: the main branch
at the commit it started from, its branches and worktrees removed.

    python3 journeys/mission.py --volee target/release/volee --sandbox ~/code/alpha \
        --other-sandbox ~/code/beta
"""
import json
import os
import time

from journey import Journey, arguments

# How long real agents get for each leg of the trip (seconds).
PLANNING = 300
WORKING = 600
# volee merges: no model is waited for.
MERGING = 60
# The coordinator ends the turn the person's answer woke, then is stopped.
TIDYING = 180


def more(parser):
    parser.add_argument("--other-sandbox",
                        help="a second git repository Claude Code trusts (default: the "
                             "directory `beta` next to the sandbox)")


class Repo:
    """One of the mission's repositories, and what it was before the journey."""

    def __init__(self, journey, path):
        self.j = journey
        self.path = os.path.realpath(os.path.expanduser(path))
        self.name = os.path.basename(self.path)
        self.base = self.git("symbolic-ref", "--short", "HEAD")
        self.start = self.git("rev-parse", "HEAD")

    def git(self, *args):
        return self.j.run(["git", "-C", self.path, *args], env=self.j.clean).stdout.strip()

    def ok(self, *args):
        return self.j.run(["git", "-C", self.path, *args], env=self.j.clean).returncode == 0

    def clean(self):
        return self.git("status", "--porcelain") == ""

    def head(self):
        return self.git("rev-parse", self.base)

    def worktrees(self, branch):
        """The worktrees that have `branch` checked out."""
        paths, path = [], None
        for line in self.git("worktree", "list", "--porcelain").splitlines():
            if line.startswith("worktree "):
                path = line[len("worktree "):]
            elif line == "branch refs/heads/" + branch and path:
                paths.append(path)
        return paths

    def restore(self, branch):
        """Removes the journey's worktree and branch, and puts the base branch back at
        the commit it started from when everything above it is the journey's own:
        the commits of its branch, and merges of them. Returns why it could not."""
        tip = self.git("rev-parse", "--verify", "--quiet", "refs/heads/" + branch)
        for path in self.worktrees(branch):
            self.j.run(["git", "-C", self.path, "worktree", "unlock", path], env=self.j.clean)
            self.j.run(["git", "-C", self.path, "worktree", "remove", "--force", path],
                       env=self.j.clean)
        problem = ""
        if self.head() != self.start:
            own = set(self.git("rev-list", self.start + ".." + tip).split()) if tip else set()
            above = self.git("rev-list", "--topo-order", "--reverse",
                             self.start + ".." + self.base).split()
            for commit in above:
                parents = self.git("rev-list", "--parents", "-n", "1", commit).split()[1:]
                merge = len(parents) == 2 and all(
                    parent == self.start or parent in own for parent in parents)
                if commit in own or merge:
                    own.add(commit)
                else:
                    problem = "{} holds a commit the journey did not make: {}".format(
                        self.base, commit[:10])
                    break
            if not problem and self.git("symbolic-ref", "--short", "HEAD") != self.base:
                problem = "the main checkout is no longer on " + self.base
            if not problem and not self.clean():
                problem = "the main checkout has changes"
            if not problem:
                self.git("reset", "--quiet", "--hard", self.start)
        if tip:
            self.git("branch", "-D", branch)
        return problem


def select_report(j, title):
    """In the messages window, moves the selection to the report of the mission
    `title`, among the questions that wait for the person (they are listed oldest
    first, as `volee msg questions` prints them), and says whether it is selected."""
    asked = [block for block in j.volee_run("msg", "questions").stdout.split("\nmsg_")]
    ready = 'Mission "{}" is ready to merge.'.format(title)
    index = next((n for n, block in enumerate(asked) if ready in block), None)
    if index is None:
        return False
    for _ in range(index):
        j.keys("j", 0.3)
    lines = j.screen().splitlines()
    chosen = next((n for n, text in enumerate(lines) if "▌?" in text), None)
    return (chosen is not None and chosen + 1 < len(lines)
            and "is ready to merge" in lines[chosen + 1])


def thread_shown(j, title):
    """The text of the thread pane of the messages window, on the thread of the
    mission `title`, its lines joined: a long message wraps there."""
    marker = "◆ " + title
    lines = j.screen().splitlines()
    at = next((line.rindex(marker) for line in lines if marker in line), None)
    if at is None:
        return ""
    return " ".join(" ".join(line[at:].strip(" │─╭╮╰╯▲▼").split())
                    for line in lines if len(line) > at)


def closing(text, title):
    """The sentence that closes the mission `title`, in the text of its thread."""
    start = text.find('Mission "{}" is closed.'.format(title))
    end = text.find("branches kept).", start)
    if start < 0:
        return ""
    return text[start:end + len("branches kept).")] if end >= 0 else text[start:]


def spoke_since(j, name, since_ms):
    """Whether the agent called `name` took a turn after `since_ms` (milliseconds since
    the epoch) and rests again. What it says is its own, in the language it answers in:
    the journey shows its screen and does not look for words in it."""
    agent = j.agent(name) or {}
    return ((agent.get("state") or {}).get("state") in ("idle", "done")
            and (agent.get("state_since") or 0) > since_ms)


def missions(j):
    try:
        return json.loads(j.volee_run("mission", "ls", "--all", "--json").stdout or "[]")
    except json.JSONDecodeError:
        return []


def main():
    args = arguments(__doc__, more)
    j = Journey("mission", args)
    title = "Journey " + j.stamp
    branch = "journey-" + j.stamp
    file = "journey-" + j.stamp + ".txt"
    line = "journey " + j.stamp
    other = args.other_sandbox or os.path.join(os.path.dirname(j.sandbox), "beta")
    repos = [Repo(j, j.sandbox), Repo(j, other)]
    j.branches.append(branch)
    # The branch of the one agent the lead launches itself, at the end.
    solo = "journey-solo-" + j.stamp
    j.branches.append(solo)
    try:
        if not j.check("both repositories are git checkouts with nothing uncommitted",
                       all(repo.start and repo.clean() for repo in repos),
                       str([(repo.name, repo.base, repo.start[:10]) for repo in repos])):
            return
        j.check("neither has a remote", all(repo.git("remote") == "" for repo in repos))
        os.makedirs(j.vars["VOLEE_CONFIG_DIR"], exist_ok=True)
        with open(os.path.join(j.vars["VOLEE_CONFIG_DIR"], "config.toml"), "w",
                  encoding="utf-8") as config:
            config.write('[agents]\nmodel = "{}"\npermission_mode = "auto"\n'.format(j.model))
            for repo in repos:
                config.write('[repos.{}]\npath = "{}"\n'.format(repo.name, repo.path))
        if not j.check("the daemon starts", j.start_daemon()):
            return

        # ---- the person starts a lead, and tells it what they want ---------------
        lead = "journey-lead-" + j.stamp
        j.agents.append(lead)
        started = j.run([j.volee, "lead", "--name", lead], cwd=repos[0].path)
        if not j.check("`volee lead` starts a lead and says how to open it",
                       started.returncode == 0 and "is your lead here" in started.stdout,
                       (started.stdout + started.stderr).strip()[:300]):
            return
        j.open_interface()
        j.resize(160, 45)
        row = j.wait(lambda: j.agent(lead), 30, 1) or {}
        j.check("it is listed as a lead, without a worktree",
                row.get("role") == "lead" and not row.get("workspace", {}).get("worktree"),
                str((row.get("role"), row.get("workspace")))[:200])
        ready = j.wait(lambda: ((j.agent(lead) or {}).get("state") or {}).get("state")
                       in ("idle", "done"), 120, 1)
        j.check("it says that it is ready, and waits", ready,
                str((j.agent(lead) or {}).get("state")))
        if not j.select(lead):
            return
        j.keys("Enter", 2.0)
        if not j.check("Enter opens the lead inside volee",
                       j.sees("your keys go to " + lead, timeout=30)):
            return
        request = ("Start a mission titled `{title}` in the repositories {a} and {b}. "
                   "Its goal: in each of the two repositories, add a file named `{file}` "
                   "at the root of the repository, containing exactly the line `{line}`, "
                   "and commit it with the message `test: add {file}`. One worker per "
                   "repository, each on a branch named `{branch}`. Nothing else: no test, "
                   "no other file, no other change.").format(
                       title=title, a=repos[0].name, b=repos[1].name, file=file,
                       line=line, branch=branch)
        j.type(request)
        j.keys("Enter")
        mission = j.wait(lambda: next((m for m in missions(j) if m["title"] == title), None),
                         PLANNING, 2)
        if not j.check("the lead starts the mission it was asked for", mission,
                       "it answered: " + j.last_answer()):
            j.show("the lead did not start the mission")
            return
        j.check("in its own name: the mission says which lead asked for it",
                mission.get("requested_by") == row.get("id"),
                "{} / {}".format(mission.get("requested_by"), row.get("id")))
        j.check("it tells the person what it launched, and hands back",
                j.wait(lambda: ((j.agent(lead) or {}).get("state") or {}).get("state")
                       in ("idle", "done"), 120, 1),
                str((j.agent(lead) or {}).get("state")))
        j.show("the lead, asked for a mission")
        j.keys("ctrl+q", 1.5)
        j.check("the leave key gives the keyboard back to volee, on the lead's screen",
                j.wait(lambda: "your keys go to " + lead not in j.screen()
                       and "Screen · " + lead in j.screen(), 10))
        j.keys("Escape", 1.0)
        j.check("Escape comes back to the list, and the lead keeps running",
                j.sees("Agents", lead, timeout=10)
                and ((j.agent(lead) or {}).get("state") or {}).get("state")
                in ("idle", "done", "working"))
        # The filter that selected the lead still narrows the list.
        j.keys("Escape", 1.0)

        def members(role):
            return [agent for agent in j.agents_listed()
                    if agent.get("mission") == mission["id"] and agent.get("role") == role]

        def status():
            return next((m["status"] for m in missions(j) if m["id"] == mission["id"]), "")

        def remember():
            for agent in members("coordinator") + members("worker"):
                if agent["name"] not in j.agents:
                    j.agents.append(agent["name"])

        coordinator = j.wait(lambda: members("coordinator"), 30, 1)
        remember()
        if not j.check("its coordinator shows in the list",
                       coordinator and j.sees(coordinator[0]["name"], timeout=30),
                       str(coordinator)[:160]):
            j.show("no coordinator in the list")
            return
        j.check("the coordinator answers to the lead",
                coordinator[0].get("parent") == row.get("id"),
                "{} / {}".format(coordinator[0].get("parent"), row.get("id")))
        coordinator, coordinator_id = coordinator[0]["name"], coordinator[0]["id"]

        # ---- the coordinator launches one worker per repository ----------------
        workers = j.wait(lambda: len(members("worker")) >= 2 and members("worker"),
                         PLANNING, 3)
        remember()
        if not j.check("the coordinator launched a worker in each repository",
                       workers and sorted(w["workspace"].get("repo") for w in workers)
                       == sorted(repo.name for repo in repos),
                       str([(w["name"], w["workspace"].get("repo"),
                             w["workspace"].get("branch")) for w in workers or []])):
            j.show("no workers")
            return
        j.show("the mission at work")

        # ---- every worker delivered: the coordinator asks, and merges nothing ---
        asked = j.wait(lambda: status() == "awaiting-approval", WORKING, 3)
        remember()
        if not j.check("the mission waits for the person once the coordinator presented "
                       "its report", asked, "status: " + status()):
            j.show("the mission never asked")
            j.say(j.volee_run("mission", "show", title).stdout)
            return
        shown = j.volee_run("mission", "show", title).stdout
        j.say(shown)
        j.check("`volee mission show` says it waits for you", "waiting for you" in shown)
        # A session that just started rests at its prompt before its task is
        # submitted: that is not a worker that stopped.
        j.check("no worker was said to have stopped with no commits",
                "stopped without delivering: no commits" not in shown)
        record = next(m for m in missions(j) if m["id"] == mission["id"])
        verdicts = [a["verdict"] + ": " + a["evidence"]
                    for a in record.get("assessments", {}).values()]
        j.check("both workers delivered, in repositories without a remote",
                len(verdicts) == 2 and all(v.startswith("delivered: ") and "no remote" in v
                                           for v in verdicts), str(verdicts))
        j.check("each worker's branch holds its commit",
                all(repo.git("rev-list", "--count", repo.start + ".." + branch) == "1"
                    for repo in repos),
                str([repo.git("log", "--oneline", repo.start + ".." + branch)
                     for repo in repos]))
        j.check("nothing is merged: both main branches are where they started",
                all(repo.head() == repo.start for repo in repos))
        listed = j.volee_run("msg", "questions").stdout
        j.check("the report is a question to the person, with ok merge and no",
                'Mission "{}" is ready to merge.'.format(title) in listed
                and "Done: " in listed and "Proof: " in listed and "Left: " in listed
                and "Answers: 1 ok merge · 2 no" in listed, listed[:500])

        # ---- the person says ok merge, with two keys -----------------------------
        j.keys("I", 1.5)
        j.check("the messages open with the report, its answers numbered",
                j.sees("for you", "is ready to merge", "1 ok merge", "2 no", timeout=15))
        j.check("the report is the only question the mission asks the person",
                j.sees("1 question for you", timeout=2),
                next((text.strip(" │╭╮─") for text in j.screen().splitlines()
                      if "Messages ·" in text), ""))
        # Whatever else waits, the answer goes to the report: the journey selects it,
        # and reads the confirmation before it sends anything.
        j.check("the report is selected", select_report(j, title))
        j.show("the report, waiting for the person")
        j.keys("1", 1.0)
        prompt = 'Answer "ok merge" to ' + coordinator + "?"
        asks = j.sees(prompt, timeout=5)
        j.check("1 asks before anything is sent", asks)
        j.check("still nothing is merged",
                all(repo.head() == repo.start for repo in repos))
        if not asks:
            # What is offered is not the approval: nothing is sent.
            j.keys("n", 0.5)
            j.show("not the approval")
            return
        approved_at = time.time()
        j.keys("y", 0.3)
        j.check("y sends the approval", j.sees("Message sent to " + coordinator, timeout=10))

        # ---- volee merges, then closes: no agent does -----------------------------
        closed = j.wait(lambda: status() == "closed", MERGING, 0.25)
        took = time.time() - approved_at
        if not j.check("volee merged and closed the mission", closed,
                       "status: {} after {:.1f} s".format(status(), took)):
            j.say(j.volee_run("mission", "show", title).stdout)
        j.say("closed within {:.1f} s of the approval".format(took))
        for repo in repos:
            j.check("{}: the worker's branch is merged into {}".format(repo.name, repo.base),
                    repo.head() != repo.start
                    and repo.ok("merge-base", "--is-ancestor", branch, repo.base),
                    repo.git("log", "--oneline", "-3", repo.base))
            parents = repo.git("rev-list", "--parents", "-n", "1", repo.base).split()[1:]
            j.check("{}: with a merge commit on top of where {} was, that keeps the "
                    "worker's commit".format(repo.name, repo.base),
                    parents == [repo.start, repo.git("rev-parse", branch)]
                    and repo.git("log", "-1", "--format=%s", repo.base)
                    == "Merge branch '{}'".format(branch),
                    repo.git("log", "-1", "--format=%s %p", repo.base))
            j.check("{}: the file is in the main checkout, with its line".format(repo.name),
                    repo.git("show", repo.base + ":" + file) == line
                    and os.path.isfile(os.path.join(repo.path, file)),
                    repr(repo.git("show", repo.base + ":" + file)))
            j.check("{}: the main checkout is on {} and has nothing uncommitted".format(
                        repo.name, repo.base),
                    repo.git("symbolic-ref", "--short", "HEAD") == repo.base
                    and repo.clean(), repo.git("status", "--short", "--branch"))
        shown = j.volee_run("mission", "show", title).stdout
        j.check("the coordinator was told that the merge is volee's",
                "volee merges every worker's work itself" in " ".join(shown.split()),
                shown[-600:])
        j.check("no merge failed", "could not merge" not in shown)

        # ---- closing tidied everything, in one gesture ------------------------------
        gone = j.wait(lambda: not members("worker") and not members("coordinator"),
                      TIDYING, 2)
        j.check("the workers and the coordinator left the list", gone,
                str([a["name"] for a in members("worker") + members("coordinator")]))
        for repo in repos:
            j.check("{}: the worker's worktree is gone, its branch is kept".format(repo.name),
                    not repo.worktrees(branch)
                    and repo.ok("rev-parse", "--verify", "--quiet", "refs/heads/" + branch),
                    str(repo.worktrees(branch)))
        j.keys("Escape", 0.5)
        j.keys("I", 1.5)
        said = j.wait(lambda: closing(thread_shown(j, title), title), 20, 0.5) or ""
        j.check("one message says what is merged and what was tidied",
                "Merged: " in said and "({} into {})".format(branch, repos[0].base) in said
                and "Tidied: " in said and said.endswith("(worktrees removed, branches kept)."),
                said or thread_shown(j, title)[-400:])
        j.check("no question is left waiting", "question for you" not in j.screen())
        j.check("the thread still names the coordinator, which left the list",
                coordinator + " → you" in j.screen()
                and "ag_" + coordinator_id[-6:] not in j.screen(),
                "ag_" + coordinator_id[-6:])
        j.show("the mission, closed and tidied")
        j.keys("Escape", 0.5)

        # ---- the lead heard it, and tells the person -------------------------------
        ended = next(m for m in missions(j) if m["id"] == mission["id"]).get("ended_at") or 0
        told = j.wait(lambda: spoke_since(j, lead, ended), 120, 2)
        if j.select(lead):
            j.keys("z", 1.5)
            j.check("the lead takes a turn once the mission is closed, to tell the person",
                    told, "it said: " + j.last_answer())
            j.show("the lead, at the end")
            j.keys("Escape", 0.5)
        j.check("the lead is still there, for the next request",
                ((j.agent(lead) or {}).get("state") or {}).get("state")
                in ("idle", "done", "working"), str((j.agent(lead) or {}).get("state")))

        # ---- one task in one repository: the lead gives it to one agent ------------
        merged = repos[0].head()
        if j.select(lead):
            j.keys("Enter", 2.0)
            j.check("the lead opens again for another request",
                    j.sees("your keys go to " + lead, timeout=30))
            j.type(("Now launch one agent in the repository {a}, on a branch named "
                    "`{solo}`, to add a file named `{solo}.txt` at the root of the "
                    "repository, containing exactly the line `solo {stamp}`, and to "
                    "commit it with the message `test: add {solo}.txt`. Nothing "
                    "else.").format(a=repos[0].name, solo=solo, stamp=j.stamp))
            j.keys("Enter")

            def launched():
                return next((agent for agent in j.agents_listed()
                             if agent.get("workspace", {}).get("branch") == solo), None)
            agent = j.wait(launched, PLANNING, 2) or {}
            if agent.get("name"):
                j.agents.append(agent["name"])
            j.check("the lead launches one agent on that branch, outside any mission",
                    agent.get("role") == "worker" and not agent.get("mission")
                    and agent.get("workspace", {}).get("repo") == repos[0].name,
                    str((agent.get("name"), agent.get("role"), agent.get("mission")))[:200])
            j.check("the agent answers to the lead", agent.get("parent") == row.get("id"),
                    "{} / {}".format(agent.get("parent"), row.get("id")))
            j.check("the agent commits its work on its branch",
                    j.wait(lambda: repos[0].git("rev-list", "--count", merged + ".." + solo)
                           == "1", WORKING, 3),
                    repos[0].git("log", "--oneline", merged + ".." + solo))
            j.check("nobody merges it: the main branch has not moved",
                    repos[0].head() == merged)
            # The agent reports to its lead once its work is committed.
            committed = int(repos[0].git("log", "-1", "--format=%ct", solo) or 0) * 1000
            j.check("the lead takes a turn once the agent reported, to tell the person",
                    j.wait(lambda: spoke_since(j, lead, committed), 180, 2),
                    "it said: " + j.last_answer())
            j.show("the lead, after one agent's work")
            j.keys("ctrl+q", 1.5)
            j.keys("Escape", 1.0)
            j.keys("Escape", 1.0)
        j.keys("q", 1.0)
    finally:
        if j.keep:
            j.finish()
        # What a run that stopped half-way left: the mission's agents.
        if j.daemon is not None:
            j.volee_run("mission", "abort", title)
        j.close_viewer()
        for name in j.agents:
            j.volee_run("stop", name)
        time.sleep(3 if j.agents else 0)
        for repo in repos:
            problem = repo.restore(branch)
            j.check("{} is back where it started".format(repo.name),
                    not problem and repo.head() == repo.start and repo.clean()
                    and not repo.worktrees(branch), problem)
        # The lone agent's worktree and branch, which nobody merged.
        repos[0].restore(solo)
        j.check("the lone agent's branch and worktree are gone",
                not repos[0].worktrees(solo)
                and not repos[0].ok("rev-parse", "--verify", "--quiet", "refs/heads/" + solo))
        j.finish()


main()
