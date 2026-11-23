import { describe, expect, it } from "vitest";

import type { Card } from "../src/model";
import { cardStamp, sameChange, type Work } from "../src/work";

type RiskView = NonNullable<Work["risk"]>;

const risk = (over: Partial<RiskView> = {}): RiskView => ({
  tip: "abc",
  level: "high",
  reasons: ["touches src/auth"],
  computed: "high",
  overridden: false,
  layers: ["summary"],
  auto_merge: { allowed: false, reason: "high risk" },
  notes: [],
  errors: [],
  ...over,
});

const work = (over: Partial<Work> = {}): Work => ({
  id: "s1",
  name: null,
  transcript: { available: true, reason: null, lines: 1, skipped: 0, prompts: [] },
  summary: { line: "Done", text: null, source: "transcript" },
  plan: null,
  open_points: [],
  commits: [],
  files: [],
  other_touched: [],
  commands: [],
  tests: { ran: 0, last: "unknown" },
  diff: { base: "main", branch: "feat", error: null, truncated: false, files: 0, insertions: 0, deletions: 0 },
  diagram: "flowchart LR\n",
  ...over,
});

describe("what the rules and the agent change without touching the diff", () => {
  it("is not the same change when the risk the rules give moves", () => {
    const a = work({ risk: risk() });
    expect(sameChange(a, work({ risk: risk() }))).toBe(true);
    expect(sameChange(a, work({ risk: risk({ level: "low", computed: "low", reasons: [] }) }))).toBe(false);
    expect(sameChange(a, work({ risk: risk({ overridden: true }) }))).toBe(false);
    expect(sameChange(a, work({ risk: risk({ layers: ["summary", "diff"] }) }))).toBe(false);
  });

  it("is not the same change when the agent finishes, which enables the Merge button", () => {
    const busy = work({ agent: { alive: true, pid: 1, finished_work: false } });
    const done = work({ agent: { alive: true, pid: 1, finished_work: true } });
    expect(sameChange(busy, done)).toBe(false);
    expect(sameChange(done, work({ agent: { alive: true, pid: 1, finished_work: true } }))).toBe(true);
    expect(sameChange(done, work({ agent: { alive: false, pid: null, finished_work: true } }))).toBe(false);
  });
});

const card = (over: Partial<Card> = {}): Card => ({
  id: "s1",
  kind: "finished",
  name: null,
  state: "idle",
  waiting_for: null,
  since: 1,
  repo: "alpha",
  branch: "feat",
  base: "main",
  base_source: "reflog",
  cwd: "/x",
  diff: { files: 1, insertions: 2, deletions: 0 },
  ahead: 1,
  dirty: false,
  risk: risk(),
  ...over,
});

describe("the stamp of an open work screen, pushed with the inbox", () => {
  it("is empty without a card, and changes when the card's risk, kind or state changes", () => {
    expect(cardStamp(null)).toBe("");
    const base = cardStamp(card());
    expect(cardStamp(card())).toBe(base);
    expect(cardStamp(card({ risk: risk({ level: "low", computed: "low" }) }))).not.toBe(base);
    expect(cardStamp(card({ risk: risk({ overridden: true }) }))).not.toBe(base);
    expect(cardStamp(card({ kind: "waiting" }))).not.toBe(base);
    expect(cardStamp(card({ state: "working" }))).not.toBe(base);
    expect(cardStamp(card({ ahead: 2 }))).not.toBe(base);
    expect(cardStamp(card({ risk: undefined }))).not.toBe(base);
  });
});
