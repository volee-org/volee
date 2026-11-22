<script lang="ts">
  import { ago } from "./format";
  import { type JournalEntry, journalLine } from "./actions";

  interface Props {
    entries: JournalEntry[];
    now: number;
    /** How many to show; the rest is behind "Show more". */
    limit?: number;
    title?: string;
  }
  let { entries, now, limit = 8, title = "Journal" }: Props = $props();

  let all = $state(false);
  const shown = $derived(all ? entries : entries.slice(0, limit));
</script>

{#if entries.length > 0}
  <section class="journal" data-testid="journal">
    <h2>{title} <span class="count">{entries.length}</span></h2>
    <ul>
      {#each shown as entry (entry.id)}
        <li class:failed={!entry.ok} data-kind={entry.kind} data-automatic={entry.automatic}>
          <span class="mark" aria-label={entry.ok ? "worked" : "failed"}>{entry.ok ? "✓" : "✗"}</span>
          <span class="line">{journalLine(entry)}</span>
          <span class="meta mono">{entry.branch} · {ago(entry.at, now)}</span>
        </li>
      {/each}
    </ul>
    {#if entries.length > limit}
      <button type="button" onclick={() => (all = !all)}>{all ? "Show less" : `Show all ${entries.length}`}</button>
    {/if}
  </section>
{/if}

<style>
  .journal {
    margin-top: 28px;
  }
  h2 {
    margin: 0 0 8px;
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--muted);
  }
  .count {
    font-weight: 400;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 4px;
  }
  li {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 10px;
    align-items: baseline;
    font-size: 0.88rem;
  }
  .mark {
    color: var(--finished);
    width: 1.2em;
  }
  li.failed .mark {
    color: var(--danger);
  }
  .line {
    flex: 1;
    min-width: 16ch;
    overflow-wrap: anywhere;
  }
  .meta {
    color: var(--muted);
    font-size: 0.8rem;
  }
  button {
    margin-top: 6px;
    background: transparent;
    border: 0;
    color: var(--accent);
    cursor: pointer;
    padding: 0;
  }
</style>
