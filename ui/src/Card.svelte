<script lang="ts">
  import { diffSummary, shortPath, sinceText, waitingText } from "./format";
  import { type Card, title } from "./model";

  interface Props {
    card: Card;
    now: number;
    terminal: string | null;
    onopen: (card: Card) => void;
    note?: string;
  }
  let { card, now, terminal, onopen, note = "" }: Props = $props();
</script>

<article class="card {card.kind}" data-kind={card.kind} data-id={card.id}>
  <header>
    <h3>{title(card)}</h3>
    <span class="since" title="Since when">
      {sinceText(card.kind === "waiting" ? "waiting" : "finished", card.since, now)}
    </span>
  </header>

  {#if card.kind === "waiting"}
    <p class="what"><strong>{waitingText(card.waiting_for)}</strong></p>
  {:else}
    <p class="what">
      Work is ready to read
      {#if card.diff}
        <span class="diff mono">{diffSummary(card.diff.files, card.diff.insertions, card.diff.deletions)}</span>
      {/if}
    </p>
  {/if}

  <p class="where">
    {#if card.repo}
      <span class="chip">{card.repo}</span>
    {/if}
    {#if card.branch}
      <span class="mono">{card.branch}</span>
      {#if card.base}
        <span class="arrow">into</span> <span class="mono">{card.base}</span>
      {/if}
    {/if}
    {#if !card.repo}
      <span class="mono">{shortPath(card.cwd)}</span>
    {/if}
    {#if card.dirty}
      <span class="chip warn">uncommitted changes</span>
    {/if}
  </p>

  {#if terminal}
    <footer>
      <button type="button" onclick={() => onopen(card)}>Open terminal</button>
      {#if note}<span class="note">{note}</span>{/if}
    </footer>
  {/if}
</article>

<style>
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-left: 4px solid var(--border);
    border-radius: 10px;
    padding: 14px 18px;
    box-shadow: var(--shadow);
  }
  .card.waiting {
    border-left-color: var(--waiting);
  }
  .card.finished {
    border-left-color: var(--finished);
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 12px;
  }
  h3 {
    margin: 0;
    font-size: 1.05rem;
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .since {
    color: var(--muted);
    font-size: 0.85rem;
    white-space: nowrap;
  }
  .what {
    margin: 6px 0 4px;
  }
  .diff {
    margin-left: 8px;
    color: var(--muted);
  }
  .where {
    margin: 0;
    color: var(--muted);
    display: flex;
    flex-wrap: wrap;
    gap: 4px 8px;
    align-items: center;
  }
  .arrow {
    font-size: 0.85rem;
  }
  .chip {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 0 9px;
    font-size: 0.82rem;
    color: var(--text);
  }
  .chip.warn {
    color: var(--waiting);
    border-color: var(--waiting);
  }
  footer {
    margin-top: 12px;
    display: flex;
    align-items: center;
    gap: 12px;
  }
  button {
    background: transparent;
    border: 1px solid var(--accent);
    color: var(--accent);
    border-radius: 7px;
    padding: 4px 12px;
    cursor: pointer;
  }
  button:hover,
  button:focus-visible {
    background: var(--accent);
    color: var(--accent-text);
    outline: none;
  }
  .note {
    color: var(--muted);
    font-size: 0.85rem;
  }
</style>
