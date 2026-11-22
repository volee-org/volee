<script lang="ts">
  import { onMount } from "svelte";

  import Card from "./Card.svelte";
  import DecisionCard from "./DecisionCard.svelte";
  import { mergeWork } from "./actions";
  import Journal from "./Journal.svelte";
  import { workIdFromHash } from "./work";
  import WorkScreen from "./Work.svelte";
  import { decide, type DecisionBody, follow, openTerminal, readToken, type Status } from "./live";
  import { type Card as CardData, emptyInbox, type Inbox, split, title } from "./model";
  import { rulesBannerText } from "./risk";

  type Theme = "auto" | "light" | "dark";

  let inbox = $state<Inbox>(emptyInbox);
  let status = $state<Status>("connecting");
  let now = $state(Date.now());
  let token = $state<string | null>(null);
  let theme = $state<Theme>("auto");
  let notes = $state<Record<string, string>>({});

  let groups = $derived(split(inbox.cards));
  let waitingCount = $derived(inbox.decisions.length + groups.waiting.length);
  let empty = $derived(inbox.cards.length === 0 && inbox.decisions.length === 0);

  /** The held card the keyboard acts on; `n` moves to the next one. */
  let activeIndex = $state(0);
  let activeId = $derived(inbox.decisions[Math.min(activeIndex, inbox.decisions.length - 1)]?.id ?? null);
  let refs = $state<Record<string, { shortcut: (key: string) => boolean } | undefined>>({});
  let message = $state("");

  /** The finished work being read, when its screen is open. */
  let workId = $state<string | null>(null);
  /** Whether this page pushed the screen's address, so Back can undo it. */
  let pushed = false;
  let workCard = $derived(inbox.cards.find((card) => card.id === workId) ?? null);

  function readWork(card: CardData) {
    pushed = true;
    location.hash = `work/${encodeURIComponent(card.id)}`;
  }

  function leaveWork() {
    if (pushed) {
      pushed = false;
      history.back();
    } else {
      history.replaceState(null, "", location.pathname);
      workId = null;
    }
  }

  async function decideCard(id: string, body: DecisionBody): Promise<string> {
    if (!token) return "no token";
    const result = await decide(token, id, body);
    if (result.gone) {
      message = "That request is gone: it was answered in the terminal or it expired.";
      setTimeout(() => (message = ""), 6000);
      return "";
    }
    return result.ok ? "" : result.message;
  }

  function onkeydown(event: KeyboardEvent) {
    if (workId) return; // the work screen has its own keys
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const target = event.target as HTMLElement | null;
    if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA")) return;
    if (event.key === "r" && groups.finished[0]) {
      readWork(groups.finished[0]);
      event.preventDefault();
      return;
    }
    if (event.key === "n" && inbox.decisions.length > 1) {
      activeIndex = (activeIndex + 1) % inbox.decisions.length;
      event.preventDefault();
      return;
    }
    if (activeId && refs[activeId]?.shortcut(event.key)) event.preventDefault();
  }

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

  async function openById(id: string) {
    if (!token) return;
    notes[id] = "opening…";
    const result = await openTerminal(token, id);
    notes[id] = result.message || (result.ok ? "opened" : "could not open");
    setTimeout(() => delete notes[id], 6000);
  }

  function open(card: CardData) {
    return openById(card.id);
  }

  /** Merges a low risk change from its card: the daemon merges, the journal and the card say how it went. */
  async function mergeCard(card: CardData) {
    if (!token) return;
    notes[card.id] = "merging…";
    const result = await mergeWork(token, card.id);
    notes[card.id] = result.message || (result.ok ? "merged" : "not merged");
    setTimeout(() => delete notes[card.id], 12000);
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
    workId = workIdFromHash(location.hash);
    if (found.fromHash) history.replaceState(null, "", location.pathname);
    const onhash = () => (workId = workIdFromHash(location.hash));
    window.addEventListener("hashchange", onhash);
    if (!token) {
      status = "unauthorized";
      return;
    }
    const connection = follow(
      token,
      (next) => (inbox = next),
      (next) => (status = next),
    );
    const tick = setInterval(() => (now = Date.now()), 1_000);
    return () => {
      window.removeEventListener("hashchange", onhash);
      connection.close();
      clearInterval(tick);
    };
  });
</script>

<svelte:window {onkeydown} />

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
  {:else if workId && token}
    <WorkScreen
      {token}
      id={workId}
      title={workCard ? title(workCard) : workId.slice(0, 8)}
      terminal={inbox.terminal}
      brief={workCard?.review ?? null}
      stamp={workCard ? `${workCard.diff?.files}:${workCard.diff?.insertions}:${workCard.diff?.deletions}:${workCard.ahead}:${workCard.dirty}` : ""}
      journal={inbox.journal}
      {now}
      onback={leaveWork}
      onterminal={openById}
    />
  {:else}
    {#if inbox.rules.errors.length > 0}
      <p class="rules-banner" role="status" data-testid="rules-banner" title="Fix the file and save it: it is read again by itself.">
        {rulesBannerText(inbox.rules)}
      </p>
    {:else if inbox.rules.warnings.length > 0}
      <p class="rules-note" role="status" data-testid="rules-warnings">{inbox.rules.warnings.join(" · ")}</p>
    {/if}

    {#if inbox.error}
      <p class="error" role="alert">Claude Code's sessions cannot be listed: {inbox.error}</p>
    {/if}

    {#if message}
      <p class="error" role="status">{message}</p>
    {/if}

    {#if empty}
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
      {#if waitingCount > 0}
        <section data-section="waiting">
          <h2>Waiting for you <span class="count">{waitingCount}</span></h2>
          {#each inbox.decisions as decision (decision.id)}
            <DecisionCard
              bind:this={refs[decision.id]}
              {decision}
              {now}
              active={decision.id === activeId}
              terminal={inbox.terminal}
              ondecide={decideCard}
              onopen={openById}
            />
          {/each}
          {#each groups.waiting as card (card.id)}
            <Card {card} {now} terminal={inbox.terminal} onopen={open} note={notes[card.id] ?? ""} />
          {/each}
        </section>
      {/if}
      {#if groups.finished.length > 0}
        <section data-section="finished">
          <h2>Finished work <span class="count">{groups.finished.length}</span></h2>
          {#each groups.finished as card (card.id)}
            <Card {card} {now} terminal={inbox.terminal} onopen={open} onread={readWork} onmerge={mergeCard} note={notes[card.id] ?? ""} />
          {/each}
        </section>
      {/if}
      {#if inbox.others > 0}
        <p class="others">{inbox.others} other {inbox.others === 1 ? "agent" : "agents"} running, nothing to decide.</p>
      {/if}
    {/if}
    <Journal entries={inbox.journal} {now} />
  {/if}
</main>

<style>
  .rules-banner,
  .rules-note {
    margin: 0 0 12px;
    padding: 6px 12px;
    border-radius: 8px;
    font-size: 0.85rem;
    overflow-wrap: anywhere;
  }
  .rules-banner {
    border: 1px solid var(--danger);
    color: var(--danger);
  }
  .rules-note {
    border: 1px dashed var(--border);
    color: var(--muted);
  }
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
