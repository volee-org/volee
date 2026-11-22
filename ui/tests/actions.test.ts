import { describe, expect, it } from "vitest";

import {
  addComment,
  askFix,
  autoMergeText,
  composeFix,
  deleteComment,
  fixItems,
  type JournalEntry,
  journalLine,
  journalOf,
  mergeWork,
  openEditor,
} from "../src/actions";
import type { Finding } from "../src/review";

interface Call {
  url: string;
  method: string;
  body: unknown;
  auth: string | null;
}

/** A fetch that records what it was asked and answers `status` with `body`. */
function fake(status: number, body: unknown): { fetcher: typeof fetch; calls: Call[] } {
  const calls: Call[] = [];
  const fetcher = (async (url: string, init: RequestInit) => {
    const headers = init.headers as Record<string, string>;
    calls.push({
      url,
      method: init.method ?? "GET",
      body: init.body ? JSON.parse(init.body as string) : undefined,
      auth: headers.Authorization ?? null,
    });
    return new Response(JSON.stringify(body), { status });
  }) as unknown as typeof fetch;
  return { fetcher, calls };
}

const finding = (over: Partial<Finding>): Finding => ({
  file: "src/a.rs",
  line: 10,
  severity: "important",
  summary: "reads out of bounds",
  failure_scenario: "last([]) panics",
  ...over,
});

describe("fix request", () => {
  it("starts with the important findings and every comment selected", () => {
    const items = fixItems(
      [finding({}), finding({ severity: "nit", summary: "naming" })],
      [{ id: 3, file: "docs/guide.md", line: 2, text: "Why two lines?", created_at: 1 }],
    );
    expect(items.map((item) => [item.kind, item.checked])).toEqual([
      ["comment", true],
      ["finding", true],
      ["finding", false],
    ]);
  });

  it("composes a numbered request from the selection only", () => {
    const items = fixItems(
      [finding({}), finding({ severity: "nit", summary: "naming" })],
      [{ id: 3, file: "docs/guide.md", line: 2, text: "Why two\nlines?", created_at: 1 }],
    );
    const text = composeFix(items);
    expect(text).toContain("1. docs/guide.md:2: Why two lines?");
    expect(text).toContain("2. src/a.rs:10: (review, important) reads out of bounds Failure scenario: last([]) panics");
    expect(text).not.toContain("naming");
    expect(composeFix(items.map((item) => ({ ...item, checked: false })))).toBe("");
  });

  it("names a finding with no line by its file", () => {
    const text = composeFix(fixItems([finding({ line: null })], []));
    expect(text).toContain("1. src/a.rs: (review");
  });
});

describe("calls", () => {
  it("merges with the token and no body", async () => {
    const { fetcher, calls } = fake(200, { ok: true, message: "Merged feat into main locally." });
    const result = await mergeWork("tok", "s 1", fetcher);
    expect(result).toEqual({ ok: true, message: "Merged feat into main locally.", delivered: null });
    expect(calls[0]).toMatchObject({ url: "api/agents/s%201/merge", method: "POST", auth: "Bearer tok" });
    expect(calls[0]!.body).toBeUndefined();
  });

  it("reports a refused merge with the daemon's reason", async () => {
    const { fetcher } = fake(422, { ok: false, message: "Not merged, nothing changed: dirty checkout" });
    const result = await mergeWork("tok", "s", fetcher);
    expect(result.ok).toBe(false);
    expect(result.message).toContain("nothing changed");
  });

  it("says whether a fix request was delivered, so the fallback can show", async () => {
    const delivered = fake(200, { ok: true, message: "ok", delivered: true });
    expect((await askFix("t", "s", "do it", delivered.fetcher)).delivered).toBe(true);
    expect(delivered.calls[0]!.body).toEqual({ text: "do it" });
    const lost = fake(422, { ok: false, message: "NOT delivered", delivered: false });
    const result = await askFix("t", "s", "do it", lost.fetcher);
    expect(result).toEqual({ ok: false, message: "NOT delivered", delivered: false });
  });

  it("survives a daemon that cannot be reached", async () => {
    const down = (async () => {
      throw new Error("offline");
    }) as unknown as typeof fetch;
    const result = await askFix("t", "s", "x", down);
    expect(result.ok).toBe(false);
    expect(result.message).toContain("offline");
  });

  it("adds and removes a comment", async () => {
    const added = fake(200, { id: 5, file: "a.rs", line: 3, text: "hm", created_at: 9 });
    const result = await addComment("t", "s", { file: "a.rs", line: 3, text: "hm" }, added.fetcher);
    expect(result.comment?.id).toBe(5);
    expect(added.calls[0]!.url).toBe("api/agents/s/comments");
    const refused = fake(422, { ok: false, message: "a comment needs a file, a line and a text" });
    expect((await addComment("t", "s", { file: "", line: 0, text: "" }, refused.fetcher)).comment).toBeNull();
    const removed = fake(200, { ok: true });
    expect(await deleteComment("t", "s", 5, removed.fetcher)).toBe(true);
    expect(removed.calls[0]).toMatchObject({ url: "api/agents/s/comments/5", method: "DELETE" });
  });

  it("asks to open a file in an editor", async () => {
    const { fetcher, calls } = fake(200, { ok: true, message: "Opened with nvim: a.rs:3" });
    await openEditor("t", "s", { editor: "nvim", file: "a.rs", line: 3 }, fetcher);
    expect(calls[0]!.body).toEqual({ editor: "nvim", file: "a.rs", line: 3 });
  });
});

describe("journal", () => {
  const entry = (over: Partial<JournalEntry>): JournalEntry => ({
    id: 1,
    at: 1,
    repo: "/r",
    branch: "feat",
    tip: "abc",
    kind: "merge",
    ok: true,
    automatic: false,
    message: "Merged feat into main.",
    ...over,
  });

  it("says who did it: a rule, or you", () => {
    expect(journalLine(entry({}))).toBe("Merge by you: Merged feat into main.");
    expect(journalLine(entry({ automatic: true }))).toBe("Merge by a rule: Merged feat into main.");
    expect(journalLine(entry({ kind: "fix", message: "delivered" }))).toBe("Fix request by you: delivered");
  });

  it("keeps the entries of one branch", () => {
    const all = [entry({ id: 1 }), entry({ id: 2, branch: "other" })];
    expect(journalOf(all, "feat").map((e) => e.id)).toEqual([1]);
    expect(journalOf(all, null)).toHaveLength(2);
  });

  it("words the automatic merge decision", () => {
    expect(autoMergeText({ allowed: true, reason: "docs only" })).toContain("by itself when the agent finishes: docs only");
    expect(autoMergeText({ allowed: false, reason: "high risk" })).toBe("Not merged by itself: high risk");
    expect(autoMergeText(undefined)).toBe("");
  });
});
