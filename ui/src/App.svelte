<script lang="ts">
  import { onMount } from "svelte";

  import Card from "./Card.svelte";
  import { follow, openTerminal, readToken, type Status } from "./live";
  import { type Card as CardData, emptyInbox, type Inbox, split } from "./model";

  type Theme = "auto" | "light" | "dark";

  let inbox = $state<Inbox>(emptyInbox);
  let status = $state<Status>("connecting");
  let now = $state(Date.now());
  let token = $state<string | null>(null);
  let theme = $state<Theme>("auto");
  let notes = $state<Record<string, string>>({});

  let groups = $derived(split(inbox.cards));

  function applyTheme(value: Theme) {
    theme = value;
    if (value === "auto") document.documentElement.removeAttribute("data-theme");
    else document.documentElement.setAttribute("data-theme", value);
    try {
      localStorage.setItem("volee-theme", value);
    } catch {
      // Storage can be blocked; the theme then lasts for this page only.
    }
  }

  function cycleTheme() {
    applyTheme(theme === "auto" ? "light" : theme === "light" ? "dark" : "auto");
  }

  async function open(card: CardData) {
    if (!token) return;
    notes[card.id] = "opening…";
    const result = await openTerminal(token, card.id);
    notes[card.id] = result.message || (result.ok ? "opened" : "could not open");
    setTimeout(() => delete notes[card.id], 6000);
  }

  onMount(() => {
    try {
      const saved = localStorage.getItem("volee-theme");
      if (saved === "light" || saved === "dark") applyTheme(saved);
    } catch {
      // Blocked storage: follow the system theme.
    }
    const found = readToken(location.hash, sessionStorage);
    token = found.token;
    if (found.fromHash) history.replaceState(null, "", location.pathname);
    if (!token) {
      status = "unauthorized";
      return;
    }
    const connection = follow(
      token,
      (next) => (inbox = next),
      (next) => (status = next),
    );
    const tick = setInterval(() => (now = Date.now()), 15_000);
    return () => {
      connection.close();
      clearInterval(tick);
    };
  });
</script>

<main>
  <header class="top">
    <div class="brand">
      <span class="dot {status}" title={status}></span>
      <h1>volee</h1>
      <span class="status">{status === "live" ? "live" : status}</span>
    </div>
    <button class="theme" type="button" onclick={cycleTheme} title="Theme: {theme}">
      {theme}
    </button>
  </header>

  {#if status === "unauthorized"}
    <section class="empty">
      <h2>This page needs its link</h2>
      <p>Run <span class="mono">volee ui</span> to open the inbox with the right token.</p>
    </section>
  {:else}
    {#if inbox.error}
      <p class="error" role="alert">Claude Code's sessions cannot be listed: {inbox.error}</p>
    {/if}

    {#if inbox.cards.length === 0}
      <section class="empty" data-testid="empty">
        <h2>Nothing needs you</h2>
        <p>
          {#if inbox.others > 0}
            {inbox.others} other {inbox.others === 1 ? "agent is" : "agents are"} running.
          {:else}
            No decision is waiting. This page updates by itself.
          {/if}
        </p>
      </section>
    {:else}
      {#if groups.waiting.length > 0}
        <section data-section="waiting">
          <h2>Waiting for you <span class="count">{groups.waiting.length}</span></h2>
          {#each groups.waiting as card (card.id)}
            <Card {card} {now} terminal={inbox.terminal} onopen={open} note={notes[card.id] ?? ""} />
          {/each}
        </section>
      {/if}
      {#if groups.finished.length > 0}
        <section data-section="finished">
          <h2>Finished work <span class="count">{groups.finished.length}</span></h2>
          {#each groups.finished as card (card.id)}
            <Card {card} {now} terminal={inbox.terminal} onopen={open} note={notes[card.id] ?? ""} />
          {/each}
        </section>
      {/if}
      {#if inbox.others > 0}
        <p class="others">{inbox.others} other {inbox.others === 1 ? "agent" : "agents"} running, nothing to decide.</p>
      {/if}
    {/if}
  {/if}
</main>

<style>
  main {
    max-width: 720px;
    margin: 0 auto;
    padding: 24px 16px 64px;
  }
  .top {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 28px;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  h1 {
    margin: 0;
    font-size: 1.25rem;
    font-weight: 650;
    letter-spacing: 0.01em;
  }
  .status {
    color: var(--muted);
    font-size: 0.85rem;
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--muted);
  }
  .dot.live {
    background: var(--finished);
  }
  .dot.reconnecting,
  .dot.unauthorized {
    background: var(--danger);
  }
  .theme {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--muted);
    border-radius: 7px;
    padding: 2px 10px;
    cursor: pointer;
    text-transform: capitalize;
  }
  section {
    display: grid;
    gap: 12px;
    margin-bottom: 32px;
  }
  h2 {
    margin: 0;
    font-size: 0.8rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--muted);
  }
  .count {
    margin-left: 6px;
    background: var(--border);
    color: var(--text);
    border-radius: 999px;
    padding: 0 8px;
    letter-spacing: 0;
  }
  .empty {
    text-align: center;
    padding: 72px 0;
    color: var(--muted);
  }
  .empty h2 {
    font-size: 1.2rem;
    text-transform: none;
    letter-spacing: 0;
    color: var(--text);
  }
  .error {
    background: var(--waiting-bg);
    color: var(--text);
    border: 1px solid var(--waiting);
    border-radius: 8px;
    padding: 10px 14px;
  }
  .others {
    color: var(--muted);
    text-align: center;
  }
</style>
