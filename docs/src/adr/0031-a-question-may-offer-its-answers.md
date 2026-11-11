# 0031. A question to the person may offer its answers

- Status: Accepted (amends [0006](0006-messaging.md))
- Date: 2026-11-11

## Context

An agent that needs a decision writes a question to the person, which stays
on top of their messages until an answer refers to it
([ADR 0006](0006-messaging.md)). The answer is free text. That is right for
"with or without tax?", and wrong for a decision the agent will act on at
once: "yes, but only the first one" starts with a yes. The agent that reads
it decides what the person meant.

Missions need exactly such a decision: their coordinator must not merge
before the person said so. A go-ahead that volee cannot tell from a sentence
is not one it can record or check.

Claude Code has a tool that asks its user to pick among answers. It is asked
in the agent's own terminal: volee does not see the question or the answer,
agents of other harnesses have no such tool, and the person has to open
the agent to find out that it waits.

## Decision

A question to the person may carry the answers it offers.

- **The offer.** A message has `options`: two to six answers, each a few
  words on one line (40 characters at most), all different whatever their
  case. Only a question whose recipients include the person may have some.
  The daemon refuses anything else with the reason, as a sentence the asking
  model can act on.
- **The choice.** An answer chooses an option only when it *is* that option:
  the same words, whatever their case and the blanks around them. A longer
  sentence, even one that contains an option, is the person's own words and
  chooses nothing. The rule is one function of the domain
  (`Message::chosen`), used by everything that reads an answer.
- **Answering stays a message.** The person's answer is an `answer` message
  in reply to the question, in its thread, like any other
  ([ADR 0006](0006-messaging.md)): it reaches the agent at the end of its
  turn or wakes it, it is in the log, and the question stops waiting. When
  the person picked an option, its text is the option's own spelling.
- **Asking.** The MCP tool `ask_human(question, options)` asks one, for any
  agent of any harness. It returns at once: the agent ends its turn and the
  answer wakes it. A question without options is still a `question` sent
  with `send_message`.
- **Picking, in the interface.** In the messages window the offered answers
  show under the question, numbered. The digit of an answer asks on the
  bottom line whether to send it; `y` sends, `n` takes it back. A digit is
  what the screen shows, like the letter of a jump: it is not a binding of
  the keymap. `r` still writes an answer in the person's own words.
- **Picking, on the command line.** `volee msg questions` lists the
  questions waiting with their answers; `volee msg answer` answers one by
  the number of an answer, by its words, or in the person's own words, and
  says when what was typed is none of the offered answers.

## Consequences

- An agent can ask for a go-ahead and read a yes that is a yes. A program
  can too: the daemon can tell, from the log alone, whether the person chose
  a given answer.
- The person answers a question with two keys, from the interface, without
  opening the agent that asked.
- A slip of one key sends nothing: the bottom line asks first.
- The answers are few and short by rule. A question with ten possible
  answers, or answers that need a sentence each, is asked without options.
- Messages recorded before this decision have no options and read as
  before. A volee older than this decision ignores the field: it shows the
  question as a free one.
- Anyone who can run `volee msg answer` as the person can answer for them,
  an agent with a shell included. volee does not defend against that here;
  what an answer authorizes is the business of whatever asked it.

## Alternatives considered

- **Reading a go-ahead in free text** ("ok", "yes", "go"). Every agent would
  read it its own way, and no program could check it.
- **The harness's own tool for asking its user.** The question would live in
  one agent's terminal, outside volee's messages and log, and only for
  harnesses that have such a tool.
- **Sending on the digit, without asking.** One key less, and one wrong key
  away from an answer the person did not mean. The answers a question
  offers can be decisions.
- **A kind of message of its own** (a "choice"). The pinned question, its
  thread and its answer already are what is needed; a new kind would repeat
  them.
