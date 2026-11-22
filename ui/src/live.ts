import type { Inbox } from "./model";

const KEY = "volee-token";

/** Minimal storage surface, so tests need no browser. */
export interface TokenStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/**
 * The token the page was opened with. `volee ui` prints a URL whose fragment
 * is `#token=...`; a fragment never leaves the browser, so it is not logged
 * anywhere. It is kept for the tab's lifetime (reloads) and the second value
 * says whether the address bar should be cleaned.
 */
export function readToken(
  hash: string,
  storage: TokenStorage,
): { token: string | null; fromHash: boolean } {
  const fromHash = new URLSearchParams(hash.replace(/^#/, "")).get("token");
  if (fromHash) {
    try {
      storage.setItem(KEY, fromHash);
    } catch {
      // Private mode: the token then lives only in this page load.
    }
    return { token: fromHash, fromHash: true };
  }
  try {
    return { token: storage.getItem(KEY), fromHash: false };
  } catch {
    return { token: null, fromHash: false };
  }
}

export type Status = "connecting" | "live" | "reconnecting" | "unauthorized";

export interface Connection {
  close(): void;
}

interface Source {
  addEventListener(type: string, listener: (event: MessageEvent<string>) => void): void;
  onopen: ((event: Event) => void) | null;
  onerror: ((event: Event) => void) | null;
  close(): void;
}

/**
 * Follows the daemon's server-sent events. The browser reconnects by itself
 * after a drop, and the daemon sends the whole inbox first on every
 * connection, so nothing is lost between two connections.
 */
export function follow(
  token: string,
  onInbox: (inbox: Inbox) => void,
  onStatus: (status: Status) => void,
  open: (url: string) => Source = (url) => new EventSource(url),
): Connection {
  const source = open(`api/events?token=${encodeURIComponent(token)}`);
  onStatus("connecting");
  source.onopen = () => onStatus("live");
  source.onerror = () => onStatus("reconnecting");
  source.addEventListener("inbox", (event) => {
    try {
      onInbox(JSON.parse(event.data) as Inbox);
      onStatus("live");
    } catch {
      // A frame that is not an inbox is skipped; the next one replaces it.
    }
  });
  return { close: () => source.close() };
}

/** Asks the daemon to focus the agent's terminal. */
export async function openTerminal(
  token: string,
  id: string,
  fetcher: typeof fetch = fetch,
): Promise<{ ok: boolean; message: string }> {
  try {
    const response = await fetcher(`api/agents/${encodeURIComponent(id)}/terminal`, {
      method: "POST",
      headers: { Authorization: `Bearer ${token}` },
    });
    const body = (await response.json()) as { ok?: boolean; message?: string };
    return { ok: response.ok && body.ok === true, message: body.message ?? "" };
  } catch (error) {
    return { ok: false, message: String(error) };
  }
}

/** What the person chose on a held card; the daemon turns it into Claude Code's answer. */
export type DecisionBody =
  | { action: "allow"; suggestion?: number }
  | { action: "deny"; message?: string }
  | { action: "answer"; answers: Record<string, string> }
  | { action: "terminal" };

/** Sends a decision. `gone` means the terminal answered first, or the hold ran out. */
export async function decide(
  token: string,
  id: string,
  body: DecisionBody,
  fetcher: typeof fetch = fetch,
): Promise<{ ok: boolean; gone: boolean; message: string }> {
  try {
    const response = await fetcher(`api/decisions/${encodeURIComponent(id)}`, {
      method: "POST",
      headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    const reply = (await response.json().catch(() => ({}))) as { message?: string };
    return {
      ok: response.ok,
      gone: response.status === 404,
      message: reply.message ?? (response.ok ? "" : `refused (${response.status})`),
    };
  } catch (error) {
    return { ok: false, gone: false, message: String(error) };
  }
}
