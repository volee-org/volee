import { describe, expect, it } from "vitest";

import { initialOpen } from "../src/work";
import {
  badge,
  briefKey,
  type Finding,
  fetchReview,
  findingsOf,
  placeFindings,
  requestReview,
  type Review,
  reviewFacts,
  reviewHint,
  runLabel,
  sameFile,
  sortFindings,
  visibleLines,
  whereText,
} from "../src/review";

const finding = (over: Partial<Finding> = {}): Finding => ({
  file: "src/stats.py",
  line: 2,
  severity: "important",
  summary: "last() reads out of range",
  failure_scenario: null,
  ...over,
});

const review = (over: Partial<Review> = {}): Review => ({ status: "done", findings: [], ...over });

const PATCH = [
  "--- a/src/stats.py",
  "+++ b/src/stats.py",
  "@@ -1,3 +1,5 @@",
  " keep",
  "-old",
  "+new one",
  "+new two",
  " tail",
  "@@ -20,2 +22,3 @@",
  " far",
  "+far added",
  " end",
  "",
].join("\n");

describe("findings", () => {
  it("sorts important first, then nits, then what was already there, then by file and line", () => {
    const sorted = sortFindings([
      finding({ severity: "pre_existing", file: "a.py", line: 1 }),
      finding({ severity: "nit", file: "b.py", line: 9 }),
      finding({ severity: "important", file: "z.py", line: 5 }),
      finding({ severity: "important", file: "a.py", line: 7 }),
      finding({ severity: "important", file: "a.py", line: 3 }),
      finding({ severity: "nit", file: "a.py", line: 2 }),
    ]);
    expect(sorted.map((f) => `${f.severity}:${f.file}:${f.line}`)).toEqual([
      "important:a.py:3",
      "important:a.py:7",
      "important:z.py:5",
      "nit:a.py:2",
      "nit:b.py:9",
      "pre_existing:a.py:1",
    ]);
  });

  it("says where a finding is, with or without a line", () => {
    expect(whereText(finding())).toBe("src/stats.py:2");
    expect(whereText(finding({ line: null }))).toBe("src/stats.py");
  });

  it("matches a file tolerantly: absolute paths, ./ and a left-out folder", () => {
    expect(sameFile("src/stats.py", "src/stats.py")).toBe(true);
    expect(sameFile("/work/repo/src/stats.py", "src/stats.py")).toBe(true);
    expect(sameFile("./src/stats.py", "src/stats.py")).toBe(true);
    expect(sameFile("stats.py", "src/stats.py")).toBe(true);
    expect(sameFile("src/other.py", "src/stats.py")).toBe(false);
    expect(sameFile("mystats.py", "stats.py")).toBe(false);
    expect(findingsOf([finding(), finding({ file: "x.py" })], "src/stats.py")).toHaveLength(1);
  });
});

describe("findings in the diff", () => {
  it("lists the new-side lines a patch shows", () => {
    expect([...visibleLines(PATCH)]).toEqual([1, 2, 3, 4, 22, 23, 24]);
  });

  it("puts a finding at its line when the diff shows it, else on the file header", () => {
    const placed = placeFindings(PATCH, [
      finding({ line: 2 }),
      finding({ line: 2, severity: "nit", summary: "second on the same line" }),
      finding({ line: 999, summary: "not in the diff" }),
      finding({ line: null, summary: "no line at all" }),
    ]);
    expect([...placed.inline.keys()]).toEqual([2]);
    expect(placed.inline.get(2)).toHaveLength(2);
    expect(placed.inline.get(2)![0]!.severity).toBe("important");
    expect(placed.header.map((f) => f.summary)).toEqual(["no line at all", "not in the diff"]);
  });

  it("puts everything on the header for a patch without hunks", () => {
    const placed = placeFindings("", [finding()]);
    expect(placed.inline.size).toBe(0);
    expect(placed.header).toHaveLength(1);
  });
});

describe("badge, hint and button", () => {
  it("badges running, failed, clean and found reviews, and nothing without one", () => {
    expect(badge(null)).toBeNull();
    expect(badge({ status: "running", findings: 0, tip: "a" })).toEqual({ text: "Review running", tone: "running" });
    expect(badge({ status: "pending", findings: 0, tip: "a" })?.tone).toBe("running");
    expect(badge({ status: "failed", findings: 0, tip: "a" })?.tone).toBe("failed");
    expect(badge({ status: "done", findings: 0, tip: "a" })).toEqual({ text: "Review: no findings", tone: "clean" });
    expect(badge({ status: "done", findings: 1, tip: "a" })?.text).toBe("Review: 1 finding");
    expect(badge({ status: "done", findings: 3, tip: "a" })?.text).toBe("Review: 3 findings");
  });

  it("reads the review again only when the brief changed", () => {
    const a = { status: "running", findings: 0, tip: "t1" };
    expect(briefKey(a)).toBe(briefKey({ ...a }));
    expect(briefKey(a)).not.toBe(briefKey({ ...a, status: "done", findings: 2 }));
    expect(briefKey(a)).not.toBe(briefKey({ ...a, tip: "t2" }));
    expect(briefKey(null)).toBe("none");
  });

  it("offers Run when nobody asked and Retry after a failure, never while it runs or when done", () => {
    expect(runLabel(review({ status: "none" }))).toBe("Run review");
    expect(runLabel(review({ status: "failed" }))).toBe("Retry");
    expect(runLabel(review({ status: "running" }))).toBeNull();
    expect(runLabel(review({ status: "done" }))).toBeNull();
    expect(runLabel(review({ status: "disabled" }))).toBeNull();
    expect(runLabel(null)).toBeNull();
  });

  it("words the closed layer and the facts", () => {
    expect(reviewHint(review({ status: "done", findings: [finding(), finding()] }))).toBe("2 findings");
    expect(reviewHint(review({ status: "done" }))).toBe("no findings");
    expect(reviewHint(review({ status: "none" }))).toBe("not run yet");
    expect(reviewHint(null)).toBe("reading…");
    expect(
      reviewFacts(review({ reviewers: 2, model: "haiku", started_at: 1000, finished_at: 14_000, cost_usd: 0.0156 })),
    ).toBe("2 reviewers · haiku · 13 s · $0.016");
    expect(reviewFacts(review({}))).toBe("1 reviewer");
  });

  it("opens the review layer when there is something to show", () => {
    const work = { plan: null, open_points: [] } as unknown as Parameters<typeof initialOpen>[0];
    expect(initialOpen(work).review).toBe(false);
    expect(initialOpen(work, review({ status: "none" })).review).toBe(false);
    expect(initialOpen(work, review({ status: "running" })).review).toBe(true);
    expect(initialOpen(work, review({ status: "done" })).review).toBe(true);
  });
});

describe("talking to the daemon", () => {
  const reply = (status: number, body: unknown): typeof fetch =>
    (async () => new Response(JSON.stringify(body), { status })) as unknown as typeof fetch;

  it("fetches the review with the token", async () => {
    let seen: { url: string; auth: string | null } | null = null;
    const fetcher = (async (url: string, init?: RequestInit) => {
      seen = { url, auth: new Headers(init?.headers).get("Authorization") };
      return new Response(JSON.stringify({ status: "done", findings: [] }), { status: 200 });
    }) as unknown as typeof fetch;
    const result = await fetchReview("tok", "a/b", fetcher);
    expect(result.review?.status).toBe("done");
    expect(seen).toEqual({ url: "api/agents/a%2Fb/review", auth: "Bearer tok" });
  });

  it("says why a review cannot be read", async () => {
    expect((await fetchReview("t", "x", reply(404, { message: "this agent is gone" }))).message).toBe("this agent is gone");
    expect((await fetchReview("t", "x", reply(500, {}))).message).toBe("refused (500)");
  });

  it("asks for a review, with retry after a failure, and reports a refusal", async () => {
    const calls: string[] = [];
    const fetcher = (async (url: string, init?: RequestInit) => {
      calls.push(`${init?.method} ${url}`);
      return new Response(JSON.stringify({ message: "queued" }), { status: 202 });
    }) as unknown as typeof fetch;
    expect((await requestReview("t", "id", false, fetcher)).ok).toBe(true);
    await requestReview("t", "id", true, fetcher);
    expect(calls).toEqual(["POST api/agents/id/review", "POST api/agents/id/review?retry=1"]);
    const refused = await requestReview("t", "id", false, reply(409, { message: "reviews are switched off" }));
    expect(refused).toEqual({ ok: false, message: "reviews are switched off" });
  });
});
