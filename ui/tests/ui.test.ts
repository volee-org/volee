import { describe, expect, it } from "vitest";

import { ago, diffSummary, shortPath, sinceText, waitingText } from "../src/format";
import { follow, openTerminal, readToken, type TokenStorage } from "../src/live";
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
