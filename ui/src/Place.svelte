<script lang="ts">
  import { landing, shortPath } from "./format";

  interface Props {
    repo: string | null;
    branch: string | null;
    base: string | null;
    baseSource: string | null;
    cwd: string | null;
  }
  let { repo, branch, base, baseSource, cwd }: Props = $props();

  let place = $derived(landing(branch, base, baseSource));
</script>

{#if repo}
  <span class="chip">{repo}</span>
{/if}
{#if branch}
  {#if place.onBase}
    <span class="arrow">on</span> <span class="mono">{branch}</span>
  {:else}
    <span class="mono">{branch}</span>
    {#if base}
      <span class="arrow">into</span> <span class="mono">{base}</span>
    {/if}
  {/if}
  {#if place.guessed}
    <span class="guess" title={place.why} data-base-source={baseSource}>~</span>
  {/if}
{/if}
{#if !repo && cwd}
  <span class="mono">{shortPath(cwd)}</span>
{/if}

<style>
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
  .guess {
    color: var(--muted);
    opacity: 0.7;
    cursor: help;
    font-size: 0.85rem;
  }
</style>
