"""A question that offers its answers: asked by an agent, answered with a key.

The trip: an agent of another AI, joined through volee's MCP server, asks the person a
question with the answers to choose from. The messages window shows it on top, its
answers numbered under its text; the number of an answer asks before anything is sent,
`n` takes it back, `y` sends it, and the agent reads exactly the answer it offered, in
reply to its question, which stops waiting. Two more questions are answered from a
shell with `volee msg answer`: one by the number of an answer, one in the person's own
words, which volee says is none of the offered answers.

No model is asked anything: the journey speaks MCP to `volee mcp --join`, as the
harness of that agent would, so the questions and what the agent reads are exact.

    python3 journeys/question.py --volee target/release/volee --sandbox ~/code/sandbox
"""
import json
import select
import subprocess

from journey import Journey, arguments


class Joined:
    """An agent of another harness: a process that speaks MCP to `volee mcp --join`."""

    def __init__(self, journey, name):
        self.process = subprocess.Popen(
            [journey.volee, "mcp", "--join", name, "--harness", "journey"],
            env=journey.env, cwd=journey.workdir, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1)
        self.last = 0
        self.ready = self.request("initialize", {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "journey", "version": "1"}}) is not None
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, message):
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def request(self, method, params, timeout=20):
        """One request and its result, or None when no answer came."""
        self.last += 1
        self.send({"jsonrpc": "2.0", "id": self.last, "method": method, "params": params})
        while select.select([self.process.stdout], [], [], timeout)[0]:
            line = self.process.stdout.readline()
            if not line:
                return None
            try:
                answer = json.loads(line)
            except json.JSONDecodeError:
                continue
            if answer.get("id") == self.last:
                return answer.get("result")
        return None

    def tool(self, name, arguments):
        """Calls a tool. Returns (whether it succeeded, the text it returned)."""
        result = self.request("tools/call", {"name": name, "arguments": arguments}) or {}
        text = "".join(block.get("text", "") for block in result.get("content", []))
        return bool(result) and not result.get("isError"), text

    def tools(self):
        result = self.request("tools/list", {}) or {}
        return sorted(tool["name"] for tool in result.get("tools", []))

    def read(self):
        """The messages the agent has not read yet, as volee hands them."""
        _, text = self.tool("read_messages", {})
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            return []

    def close(self):
        try:
            self.process.stdin.close()
            self.process.wait(timeout=10)
        except (OSError, subprocess.TimeoutExpired):
            self.process.kill()


def asked_id(text):
    """The id of the question, from what `ask_human` returned."""
    return next((word.strip("().,") for word in text.split() if word.startswith("msg_")), "")


def main():
    args = arguments(__doc__)
    j = Journey("question", args)
    name = "journey-asker-" + j.stamp
    asker = None
    try:
        if not j.check("the daemon starts", j.start_daemon()):
            return
        asker = Joined(j, name)
        j.agents.append(name)
        row = j.wait(lambda: j.agent(name), 15, 0.5)
        if not j.check("an agent of another harness joins through volee's MCP server",
                       asker.ready and row, str(row)[:160]):
            return
        j.check("volee offers it a tool to ask the person a question with its answers",
                "ask_human" in asker.tools(), str(asker.tools()))

        # ---- the agent asks -------------------------------------------------------
        question = "Journey {}: both pull requests are green. Merge them?".format(j.stamp)
        asked, said = asker.tool("ask_human", {"question": question,
                                               "options": ["ok merge", "no"]})
        first = asked_id(said)
        if not j.check("it asks, and is told which message the answer will refer to",
                       asked and first, said[:200]):
            return
        refused, why = asker.tool("ask_human", {"question": question, "options": ["ok merge"]})
        j.check("a choice of one answer is refused, with the reason",
                not refused and "between 2 and 6 answers" in why, why[:200])
        listed = j.volee_run("msg", "questions").stdout
        j.check("`volee msg questions` lists the one question, with its answers",
                listed.count(" asks, ") == 1 and name in listed
                and "Answers: 1 ok merge · 2 no" in listed, listed[:400])

        # ---- the person answers with a key ---------------------------------------
        j.open_interface()
        j.sees("Agents", timeout=30)
        # Wide enough for the agent's name to show whole in the list of messages.
        j.check("the window is given the size of a large terminal", j.resize(160, 45))
        j.keys("I", 1.0)
        j.check("the messages open on the question, on top",
                j.sees("1 question for you", name + " asks you", timeout=10))
        j.check("its answers show under its text, numbered",
                j.sees(question[:40], "1 ok merge", "2 no",
                       "press its number to answer, or reply in your own words", timeout=10))
        j.show("a question that offers its answers")
        j.keys("3", 1.0)
        j.check("a number beyond its answers asks nothing", "Answer \"" not in j.screen())
        j.keys("1", 1.0)
        prompt = "Answer \"ok merge\" to " + name + "?"
        j.check("the number of an answer asks before anything is sent", j.sees(prompt, timeout=5))
        j.show("asked before sending")
        j.keys("n", 1.0)
        j.check("n takes it back: nothing was sent, the question still waits",
                prompt not in j.screen() and "1 question for you" in j.screen()
                and not asker.read())
        j.keys("1", 1.0)
        j.keys("y", 1.5)
        j.check("y sends it: the interface says so",
                j.sees("Message sent to " + name, timeout=10))
        j.check("the question no longer waits",
                j.wait(lambda: "question for you" not in j.screen(), 10))
        j.show("answered")
        mail = asker.read()
        answer = mail[0] if len(mail) == 1 else {}
        j.check("the agent reads one answer: exactly the words it offered",
                answer.get("body") == "ok merge" and answer.get("kind") == "answer",
                str(mail)[:300])
        j.check("in reply to its question, from the person",
                answer.get("in_reply_to") == first and answer.get("from") == "the human",
                "{} / {}".format(answer.get("in_reply_to"), first))
        j.check("`volee msg questions` has nothing left",
                j.volee_run("msg", "questions").stdout.strip() == "No question waits for you.")
        j.keys("Escape", 0.5)
        j.keys("q", 1.0)

        # ---- the person answers from a shell -------------------------------------
        asked, said = asker.tool("ask_human", {
            "question": "Journey {}: when do I deploy?".format(j.stamp),
            "options": ["now", "tonight", "never"]})
        second = asked_id(said)
        listed = j.volee_run("msg", "questions").stdout
        j.check("a second question, with three answers, is listed with their numbers",
                asked and "Answers: 1 now · 2 tonight · 3 never" in listed, listed[:400])
        beyond = j.volee_run("msg", "answer", name, "4")
        j.check("a number it does not offer is refused, with its answers",
                beyond.returncode != 0 and "1 now, 2 tonight, 3 never" in beyond.stderr,
                (beyond.stdout + beyond.stderr)[:200])
        by_number = j.volee_run("msg", "answer", name, "2")
        j.check("`volee msg answer <agent> 2` answers with its second answer",
                by_number.returncode == 0 and "Answered \"tonight\" to " + name in by_number.stdout,
                (by_number.stdout + by_number.stderr)[:200])
        mail = asker.read()
        answer = mail[0] if len(mail) == 1 else {}
        j.check("the agent reads that answer, in reply to its second question",
                answer.get("body") == "tonight" and answer.get("in_reply_to") == second,
                str(mail)[:300])

        asked, said = asker.tool("ask_human", {
            "question": "Journey {}: shall I delete the old branch?".format(j.stamp),
            "options": ["yes", "no"]})
        third = asked_id(said)
        own = "yes, but only once the release is out"
        words = j.volee_run("msg", "answer", third, own)
        j.check("an answer in the person's own words is sent, and said to be none of its answers",
                words.returncode == 0 and "not one of the answers it offers" in words.stdout,
                (words.stdout + words.stderr)[:300])
        mail = asker.read()
        answer = mail[0] if len(mail) == 1 else {}
        j.check("the agent reads the person's words as they are: not its answer `yes`",
                answer.get("body") == own and answer.get("in_reply_to") == third,
                str(mail)[:300])
        j.check("no question is left waiting",
                j.volee_run("msg", "questions").stdout.strip() == "No question waits for you.")
    finally:
        if asker is not None:
            asker.close()
        j.finish()


main()
