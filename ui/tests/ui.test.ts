import { describe, expect, it } from "vitest";

import { ago, answerText, diffSummary, landing, remaining, shortPath, sinceText, waitingText } from "../src/format";
import { decide, follow, openTerminal, readToken, type TokenStorage } from "../src/live";
import { type Card, split, title } from "../src/model";

function memory(initial: Record<string, string> = {}): TokenStorage {
  const data = { ...initial };
  return {
    getItem: (key) => data[key] ?? null,
    setItem: (key, value) => {
      data[key] = value;
    },
  };
}

const card = (over: Partial<Card>): Card => ({
  id: "s1",
  kind: "finished",
  name: null,
  state: "idle",
  waiting_for: null,
  since: null,
  repo: null,
  branch: null,
  base: null,
  base_source: null,
  cwd: "/Users/me/code/app",
  diff: null,
  ahead: 0,
  dirty: false,
  ...over,
});

describe("format", () => {
  const now = 10_000_000;
  it("says how long ago, coarsely", () => {
    expect(ago(null, now)).toBe("unknown");
    expect(ago(now - 10_000, now)).toBe("just now");
    expect(ago(now - 4 * 60_000, now)).toBe("4 min");
    expect(ago(now - 3 * 3_600_000, now)).toBe("3 h");
    expect(ago(now - 50 * 3_600_000, now)).toBe("2 d");
    expect(ago(now + 5_000, now)).toBe("just now");
  });
  it("words how long ago something happened", () => {
    expect(sinceText("waiting", null, now)).toBe("waiting");
    expect(sinceText("finished", now - 1000, now)).toBe("finished just now");
    expect(sinceText("waiting", now - 4 * 60_000, now)).toBe("waiting 4 min ago");
  });
  it("words what a waiting agent waits for", () => {
    expect(waitingText("permission for Bash")).toBe("Waiting for permission for Bash");
    expect(waitingText("blocked")).toBe("Needs an answer from you");
    expect(waitingText(null)).toBe("Needs an answer from you");
  });
  it("summarises a diff", () => {
    expect(diffSummary(1, 3, 0)).toBe("1 file, +3 −0");
    expect(diffSummary(4, 120, 8)).toBe("4 files, +120 −8");
  });
  it("shortens the home directory", () => {
    expect(shortPath("/Users/me/code/app")).toBe("~/code/app");
    expect(shortPath("/srv/app")).toBe("/srv/app");
  });
});

describe("model", () => {
  it("titles a card by name, then repository and branch, then folder", () => {
    expect(title(card({ name: "fix-login" }))).toBe("fix-login");
    expect(title(card({ repo: "app", branch: "feat/x" }))).toBe("app · feat/x");
    expect(title(card({}))).toBe("app");
  });
  it("splits waiting from finished, keeping the order", () => {
    const cards = [
      card({ id: "a", kind: "waiting" }),
      card({ id: "b" }),
      card({ id: "c", kind: "waiting" }),
    ];
    const { waiting, finished } = split(cards);
    expect(waiting.map((c) => c.id)).toEqual(["a", "c"]);
    expect(finished.map((c) => c.id)).toEqual(["b"]);
  });
});

describe("readToken", () => {
  it("reads the fragment and remembers it", () => {
    const storage = memory();
    expect(readToken("#token=abc", storage)).toEqual({ token: "abc", fromHash: true });
    expect(readToken("", storage)).toEqual({ token: "abc", fromHash: false });
  });
  it("has no token without a fragment or a memory", () => {
    expect(readToken("", memory())).toEqual({ token: null, fromHash: false });
  });
  it("survives a storage that throws", () => {
    const broken: TokenStorage = {
      getItem: () => {
        throw new Error("no");
      },
      setItem: () => {
        throw new Error("no");
      },
    };
    expect(readToken("#token=t", broken).token).toBe("t");
    expect(readToken("", broken).token).toBeNull();
  });
});

describe("follow", () => {
  it("passes inboxes on, reports status and skips bad frames", () => {
    const listeners: Record<string, (event: MessageEvent<string>) => void> = {};
    const source = {
      onopen: null as ((event: Event) => void) | null,
      onerror: null as ((event: Event) => void) | null,
      addEventListener: (type: string, listener: (event: MessageEvent<string>) => void) => {
        listeners[type] = listener;
      },
      close: () => {},
    };
    let url = "";
    const seen: string[] = [];
    const statuses: string[] = [];
    follow(
      "t k",
      (inbox) => seen.push(`${inbox.cards.length}`),
      (status) => statuses.push(status),
      (u) => {
        url = u;
        return source;
      },
    );
    expect(url).toBe("api/events?token=t%20k");
    source.onopen?.(new Event("open"));
    listeners["inbox"]?.({ data: "not json" } as MessageEvent<string>);
    listeners["inbox"]?.({
      data: JSON.stringify({ cards: [card({})], others: 0, terminal: null }),
    } as MessageEvent<string>);
    source.onerror?.(new Event("error"));
    expect(seen).toEqual(["1"]);
    expect(statuses).toEqual(["connecting", "live", "live", "reconnecting"]);
  });
});

describe("openTerminal", () => {
  it("posts with the token and reads the answer", async () => {
    let seen: { url: string; auth: string } | null = null;
    const fetcher = (async (url: string, init: RequestInit) => {
      seen = { url, auth: (init.headers as Record<string, string>)["Authorization"] ?? "" };
      return new Response(JSON.stringify({ ok: true, message: "focused" }));
    }) as unknown as typeof fetch;
    expect(await openTerminal("tok", "a/b", fetcher)).toEqual({ ok: true, message: "focused" });
    expect(seen).toEqual({ url: "api/agents/a%2Fb/terminal", auth: "Bearer tok" });
  });
  it("reports a failure as a message", async () => {
    const fetcher = (async () => {
      throw new Error("down");
    }) as unknown as typeof fetch;
    const result = await openTerminal("tok", "x", fetcher);
    expect(result.ok).toBe(false);
    expect(result.message).toContain("down");
  });
});

describe("where the agent works", () => {
  it("says on the branch when the agent works on the base itself", () => {
    expect(landing("master", "master", "default").onBase).toBe(true);
    expect(landing("feat/x", "main", "reflog").onBase).toBe(false);
    expect(landing(null, "main", null).onBase).toBe(false);
  });
  it("marks a guessed base discreetly, and only a guess", () => {
    expect(landing("feat/x", "main", "nearest").guessed).toBe(true);
    expect(landing("feat/x", "main", "default").guessed).toBe(true);
    expect(landing("feat/x", "main", "upstream").guessed).toBe(false);
    expect(landing("feat/x", "main", "reflog").guessed).toBe(false);
    expect(landing("feat/x", null, "default").guessed).toBe(false);
    expect(landing("feat/x", "main", "nearest").why).toContain("nearest");
  });
});

describe("held requests", () => {
  it("shows the time left before the terminal takes over", () => {
    const now = 1_000_000;
    expect(remaining(now + 105_000, now)).toBe("1:45");
    expect(remaining(now + 9_000, now)).toBe("0:09");
    expect(remaining(now - 5_000, now)).toBe("0:00");
    expect(remaining(now + 30 * 60_000, now)).toBe("30 min");
    expect(remaining(now + 3_600_000, now)).toBe("1 h");
    expect(remaining(now + 3_900_000, now)).toBe("1 h 5 min");
  });
  it("builds the answer to a question", () => {
    expect(answerText(["Red"], "", false)).toBe("Red");
    expect(answerText(["Red"], "Green", false)).toBe("Green");
    expect(answerText([], "", false)).toBe("");
    expect(answerText(["A", "B"], "", true)).toBe("A, B");
    expect(answerText(["A"], "and more", true)).toBe("A, and more");
    expect(answerText([], "  ", true)).toBe("");
  });
  it("posts a decision with the token and tells a card that is gone", async () => {
    const seen: { url: string; init: RequestInit }[] = [];
    const reply = (status: number, body: unknown): typeof fetch =>
      (async (url: string, init: RequestInit) => {
        seen.push({ url, init });
        return new Response(JSON.stringify(body), { status });
      }) as unknown as typeof fetch;
    const ok = await decide("tok", "01 A", { action: "allow", suggestion: 0 }, reply(200, { ok: true }));
    expect(ok).toEqual({ ok: true, gone: false, message: "" });
    expect(seen[0]!.url).toBe("api/decisions/01%20A");
    expect(new Headers(seen[0]!.init.headers).get("Authorization")).toBe("Bearer tok");
    expect(JSON.parse(seen[0]!.init.body as string)).toEqual({ action: "allow", suggestion: 0 });
    const gone = await decide("tok", "x", { action: "terminal" }, reply(404, { ok: false, message: "gone" }));
    expect(gone).toEqual({ ok: false, gone: true, message: "gone" });
    const refused = await decide("tok", "x", { action: "deny" }, reply(422, { message: "bad" }));
    expect(refused.ok).toBe(false);
    expect(refused.message).toBe("bad");
    const down = await decide("tok", "x", { action: "deny" }, (async () => {
      throw new Error("offline");
    }) as unknown as typeof fetch);
    expect(down.ok).toBe(false);
  });
});
