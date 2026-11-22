<script lang="ts">
  import { onMount } from "svelte";

  interface Props {
    source: string;
  }
  let { source }: Props = $props();

  let svg = $state("");
  let error = $state("");
  let loading = $state(true);

  /** Whether the page is drawn dark, by choice or by the system. */
  function isDark(): boolean {
    const forced = document.documentElement.getAttribute("data-theme");
    if (forced) return forced === "dark";
    return window.matchMedia("(prefers-color-scheme: dark)").matches;
  }

  onMount(async () => {
    try {
      // Mermaid is large: it is fetched only when this layer opens.
      const { default: mermaid } = await import("mermaid");
      mermaid.initialize({
        startOnLoad: false,
        securityLevel: "strict",
        // The default layout engine; elk would add 1.5 MB for nothing here.
        layout: "dagre",
        theme: isDark() ? "dark" : "neutral",
        flowchart: { htmlLabels: false, curve: "basis" },
      });
      const result = await mermaid.render(`diagram-${Math.random().toString(36).slice(2)}`, source);
      svg = result.svg;
    } catch (cause) {
      error = String(cause);
    } finally {
      loading = false;
    }
  });
</script>

<div class="diagram" data-testid="diagram">
  {#if loading}
    <p class="note">Drawing the diagram…</p>
  {:else if error}
    <p class="note" role="alert">The diagram could not be drawn: {error}</p>
    <pre class="mono source">{source}</pre>
  {:else}
    <!-- Mermaid's strict security level sanitises what it draws. -->
    <!-- eslint-disable-next-line svelte/no-at-html-tags -->
    {@html svg}
  {/if}
</div>

<style>
  .diagram {
    overflow-x: auto;
  }
  .diagram :global(svg) {
    max-width: 100%;
    height: auto;
  }
  .note {
    color: var(--muted);
    margin: 0;
  }
  .source {
    overflow-x: auto;
    background: var(--bg);
    padding: 10px;
    border-radius: 6px;
  }
</style>
