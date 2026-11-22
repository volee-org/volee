import { describe, expect, it } from "vitest";

import {
  fetchWork,
  initialOpen,
  initialOpenFiles,
  type LayerId,
  moveLayer,
  nextClosed,
  patchOf,
  planSourceText,
  stepMark,
  testsText,
  type Work,
  type WorkFile,
  workIdFromHash,
} from "../src/work";

const file = (over: Partial<WorkFile>): WorkFile => ({
  path: "src/a.rs",
  old_path: null,
  status: "modified",
  insertions: 1,
  deletions: 0,
  binary: false,
  category: "source",
  collapsed: false,
  patch: "diff --git a/src/a.rs b/src/a.rs\n",
  truncated: false,
  touched_by: [],
  why: null,
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

describe("layers", () => {
  it("opens the summary first, and the plan and open points only when they exist", () => {
    const bare = initialOpen(work());
    expect(bare).toEqual({ summary: true, review: false, plan: false, points: false, files: false, diagram: false, diff: false });
    const full = initialOpen(
      work({
        plan: { source: "tasks", steps: [], text: null },
        open_points: [{ kind: "detected", text: "x" }],
      }),
    );
    expect(full.plan).toBe(true);
    expect(full.points).toBe(true);
    // The heavy layers always wait for the reader.
    expect(full.diagram).toBe(false);
    expect(full.diff).toBe(false);
  });

  it("finds the next closed layer, after the focused one, wrapping around", () => {
    const open: Record<LayerId, boolean> = {
      summary: true,
      review: true,
      plan: true,
      points: false,
      files: true,
      diagram: false,
      diff: false,
    };
    expect(nextClosed(open, null)).toBe("points");
    expect(nextClosed(open, "points")).toBe("diagram");
    expect(nextClosed(open, "diagram")).toBe("diff");
    expect(nextClosed(open, "diff")).toBe("points");
    expect(nextClosed({ ...open, points: true, diagram: true, diff: true }, "summary")).toBeNull();
  });

  it("moves between layers and stops at the ends", () => {
    expect(moveLayer(null, 1)).toBe("summary");
    expect(moveLayer("summary", 1)).toBe("review");
    expect(moveLayer(null, -1)).toBe("diff");
    expect(moveLayer("summary", -1)).toBe("summary");
    expect(moveLayer("review", 1)).toBe("plan");
    expect(moveLayer("plan", 1)).toBe("points");
    expect(moveLayer("diff", 1)).toBe("diff");
  });
});

describe("guided diff", () => {
  it("opens a small change whole, never a lock or generated file", () => {
    const files = [
      file({ path: "src/a.rs" }),
      file({ path: "tests/a.rs", category: "test" }),
      file({ path: "Cargo.lock", category: "lock", collapsed: true }),
      file({ path: "dist/app.js", category: "generated", collapsed: true }),
      file({ path: "img.png", binary: true }),
    ];
    expect([...initialOpenFiles(files)]).toEqual(["src/a.rs", "tests/a.rs"]);
  });

  it("opens only the three most important files of a big change", () => {
    const files = ["a", "b", "c", "d", "e"].map((name) => file({ path: `src/${name}.rs`, insertions: 200 }));
    expect([...initialOpenFiles(files)]).toEqual(["src/a.rs", "src/b.rs", "src/c.rs"]);
  });

  it("gives diff2html a patch that ends with a newline", () => {
    expect(patchOf(file({ patch: "x" }))).toBe("x\n");
    expect(patchOf(file({ patch: "x\n" }))).toBe("x\n");
  });
});

describe("words", () => {
  it("marks plan steps and says where a plan comes from", () => {
    expect(stepMark("done")).toBe("✓");
    expect(stepMark("pending")).toBe("○");
    expect(planSourceText("prompt")).toContain("not a record");
    expect(planSourceText("tasks")).toContain("task list");
  });

  it("says what the tests did", () => {
    expect(testsText({ ran: 0, last: "unknown" })).toBe("No test run recorded");
    expect(testsText({ ran: 1, last: "passed" })).toBe("1 test run, last one passed");
    expect(testsText({ ran: 3, last: "failed" })).toBe("3 test runs, last one failed");
    expect(testsText({ ran: 2, last: "unknown" })).toBe("2 test runs, last one unclear");
  });

  it("reads an agent id from the address", () => {
    expect(workIdFromHash("#work/abc-1")).toBe("abc-1");
    expect(workIdFromHash("#work/a%20b")).toBe("a b");
    expect(workIdFromHash("#token=x")).toBeNull();
    expect(workIdFromHash("")).toBeNull();
  });
});

describe("fetchWork", () => {
  const reply = (status: number, body: unknown) =>
    (async () => new Response(JSON.stringify(body), { status })) as unknown as typeof fetch;

  it("sends the token and returns the work", async () => {
    let seen: { url: string; auth: string | null } | null = null;
    const fetcher = (async (url: string, init: RequestInit) => {
      seen = { url, auth: new Headers(init.headers).get("Authorization") };
      return new Response(JSON.stringify(work()), { status: 200 });
    }) as unknown as typeof fetch;
    const result = await fetchWork("tok", "a/b", fetcher);
    expect(result.work?.id).toBe("s1");
    expect(seen).toEqual({ url: "api/agents/a%2Fb/work", auth: "Bearer tok" });
  });

  it("says why when the daemon refuses or cannot be reached", async () => {
    expect(await fetchWork("t", "x", reply(404, { ok: false, message: "this agent is gone" }))).toEqual({
      work: null,
      message: "this agent is gone",
    });
    expect((await fetchWork("t", "x", reply(500, "nope"))).message).toContain("500");
    const broken = (async () => {
      throw new Error("offline");
    }) as unknown as typeof fetch;
    expect((await fetchWork("t", "x", broken)).message).toContain("offline");
  });
});
