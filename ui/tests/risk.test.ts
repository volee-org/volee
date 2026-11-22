import { describe, expect, it } from "vitest";

import { LEVELS, type RiskView, riskText, riskTitle, rulesBannerText, setRisk } from "../src/risk";
import { initialOpen, layersOpenedBy, type Work } from "../src/work";

const risk = (over: Partial<RiskView> = {}): RiskView => ({
  tip: "abc",
  level: "low",
  reasons: ["small change, nothing sensitive"],
  computed: "low",
  overridden: false,
  layers: ["summary", "diff"],
  auto_merge: { allowed: false, reason: "automatic merge is off" },
  notes: [],
  errors: [],
  ...over,
});

const work = (over: Partial<Work> = {}): Work => ({
  id: "s1",
  name: null,
  transcript: { available: true, reason: null, lines: 1, skipped: 0, prompts: [] },
  summary: { line: "Done", text: null, source: "transcript" },
  plan: { source: "tasks", steps: [], text: null },
  open_points: [{ kind: "detected", text: "x" }],
  commits: [],
  files: [],
  other_touched: [],
  commands: [],
  tests: { ran: 0, last: "unknown" },
  diff: { base: "main", branch: "feat", error: null, truncated: false, files: 0, insertions: 0, deletions: 0 },
  diagram: "flowchart LR\n",
  ...over,
});

describe("risk", () => {
  it("words the level, and that the person set it", () => {
    expect(riskText(risk())).toBe("Low risk");
    expect(riskText(risk({ level: "high", computed: "low", overridden: true }))).toBe("High risk (set by you)");
    expect(LEVELS).toEqual(["low", "medium", "high"]);
  });

  it("explains itself in the tooltip", () => {
    const text = riskTitle(risk({ level: "high", computed: "low", overridden: true, reasons: ["set by you"] }));
    expect(text).toContain("The rules said low.");
    expect(text).toContain("Not merged by itself: automatic merge is off");
    expect(riskTitle(risk({ auto_merge: { allowed: true, reason: "low risk" } }))).toContain("May merge by itself.");
  });

  it("says which file fails and how many more", () => {
    expect(rulesBannerText({ errors: [], warnings: [] })).toBe("");
    const first = { file: "/r/.volee/rules.lua", line: 2, function: null, message: "boom", text: "/r/.volee/rules.lua:2: boom" };
    expect(rulesBannerText({ errors: [first], warnings: [] })).toContain("/r/.volee/rules.lua:2: boom");
    expect(rulesBannerText({ errors: [first, first], warnings: [] })).toContain("and 1 more");
  });
});

describe("layers follow the rules", () => {
  it("opens exactly what the rules name, the plan with its open points and the diff with its files", () => {
    expect(layersOpenedBy(["summary", "diff"])).toEqual({
      summary: true,
      review: false,
      plan: false,
      points: false,
      files: true,
      diagram: false,
      diff: true,
    });
    const all = layersOpenedBy(["summary", "plan", "review", "diagram", "diff"]);
    expect(Object.values(all).every(Boolean)).toBe(true);
  });

  it("a low risk change opens the summary only when the rules say so, even if there is a plan", () => {
    const open = initialOpen(work({ risk: risk({ layers: ["summary"] }) }));
    expect(open).toEqual({ summary: true, review: false, plan: false, points: false, files: false, diagram: false, diff: false });
  });

  it("a high risk change opens everything", () => {
    const open = initialOpen(work({ risk: risk({ level: "high", layers: ["summary", "plan", "review", "diagram", "diff"] }) }));
    expect(Object.values(open).every(Boolean)).toBe(true);
  });

  it("a review that is running opens its layer even when the rules do not name it", () => {
    const open = initialOpen(work({ risk: risk({ layers: ["summary"] }) }), { status: "running", findings: [] });
    expect(open.review).toBe(true);
    expect(open.diff).toBe(false);
  });

  it("without rules the old default holds", () => {
    expect(initialOpen(work({ risk: null })).plan).toBe(true);
  });
});

describe("setRisk", () => {
  const answer = (status: number, body: unknown): typeof fetch =>
    (async () => new Response(JSON.stringify(body), { status })) as unknown as typeof fetch;

  it("posts the level and returns the new risk", async () => {
    let seen: { url: string; init: RequestInit | undefined } | null = null;
    const fetcher = (async (url: string, init?: RequestInit) => {
      seen = { url, init };
      return new Response(JSON.stringify(risk({ level: "high", overridden: true })), { status: 200 });
    }) as unknown as typeof fetch;
    const result = await setRisk("tok", "a/b", "high", fetcher);
    expect(result.risk?.level).toBe("high");
    expect(seen!.url).toBe("api/agents/a%2Fb/risk");
    expect(seen!.init?.body).toBe('{"level":"high"}');
    expect((seen!.init?.headers as Record<string, string>).Authorization).toBe("Bearer tok");
  });

  it("sends null to take the choice back", async () => {
    let body = "";
    const fetcher = (async (_url: string, init?: RequestInit) => {
      body = String(init?.body);
      return new Response(JSON.stringify(risk()), { status: 200 });
    }) as unknown as typeof fetch;
    await setRisk("tok", "s1", null, fetcher);
    expect(body).toBe('{"level":null}');
  });

  it("returns the daemon's reason when it refuses, and a network failure as text", async () => {
    expect(await setRisk("t", "s1", "low", answer(422, { message: "nope" }))).toEqual({ risk: null, message: "nope" });
    const down = (async () => {
      throw new Error("offline");
    }) as unknown as typeof fetch;
    expect((await setRisk("t", "s1", "low", down)).message).toContain("offline");
  });
});
