<script lang="ts">
  import { diffSummary, sinceText, waitingText } from "./format";
  import Place from "./Place.svelte";
  import { type Card, title } from "./model";
  import { badge } from "./review";
  import { riskText, riskTitle } from "./risk";

  interface Props {
    card: Card;
    now: number;
    terminal: string | null;
    onopen: (card: Card) => void;
    /** Opens the screen of a finished card; waiting cards have none. */
    onread?: (card: Card) => void;
    note?: string;
  }
  let { card, now, terminal, onopen, onread, note = "" }: Props = $props();

  const reviewBadge = $derived(badge(card.review));
  const readable = $derived(card.kind === "finished" && onread !== undefined);

  /** A click anywhere on a finished card reads it, except on its own buttons. */
  function onclick(event: MouseEvent) {
    if (!readable || (event.target as HTMLElement).closest("button")) return;
    onread?.(card);
  }
</script>

<!-- The "Read the work" button is the keyboard way in; the click is a shortcut. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<article class="card {card.kind}" class:readable data-kind={card.kind} data-id={card.id} {onclick}>
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
      {#if card.risk}
        <span
          class="badge risk {card.risk.level}"
          class:overridden={card.risk.overridden}
          data-testid="risk-badge"
          title={riskTitle(card.risk)}>{riskText(card.risk)}</span
        >
      {/if}
      {#if reviewBadge}
        <span class="badge {reviewBadge.tone}" data-testid="review-badge" title="The AI review's opinion, not a gate">
          {#if reviewBadge.tone === "running"}<span class="dot" aria-hidden="true"></span>{/if}{reviewBadge.text}
        </span>
      {/if}
    </p>
  {/if}

  <p class="where">
    <Place
      repo={card.repo}
      branch={card.branch}
      base={card.base}
      baseSource={card.base_source}
      cwd={card.cwd}
    />
    {#if card.dirty}
      <span class="chip warn">uncommitted changes</span>
    {/if}
  </p>

  {#if terminal || readable}
    <footer>
      {#if readable}
        <button type="button" class="read" onclick={() => onread?.(card)}>Read the work</button>
      {/if}
      {#if terminal}
        <button type="button" onclick={() => onopen(card)}>Open terminal</button>
      {/if}
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
  .card.readable {
    cursor: pointer;
  }
  .card.readable:hover {
    border-color: var(--accent);
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
  .chip {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 0 9px;
    font-size: 0.82rem;
    color: var(--text);
  }
  .badge {
    margin-left: 8px;
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 0 9px;
    font-size: 0.8rem;
    color: var(--muted);
    white-space: nowrap;
  }
  .badge.findings {
    color: var(--waiting);
    border-color: var(--waiting);
  }
  .badge.clean {
    color: var(--finished);
    border-color: var(--finished);
  }
  .badge.risk.low {
    color: var(--finished);
    border-color: var(--finished);
  }
  .badge.risk.medium {
    color: var(--waiting);
    border-color: var(--waiting);
  }
  .badge.risk.high {
    color: var(--danger);
    border-color: var(--danger);
  }
  .badge.risk.overridden {
    border-style: dashed;
  }
  .badge.failed {
    color: var(--danger);
    border-color: var(--danger);
  }
  .badge .dot {
    display: inline-block;
    width: 0.55em;
    height: 0.55em;
    margin-right: 5px;
    border-radius: 50%;
    background: var(--accent);
    animation: pulse 1.2s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.25;
    }
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
