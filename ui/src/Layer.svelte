<script lang="ts">
  import type { Snippet } from "svelte";

  interface Props {
    id: string;
    title: string;
    /** A short line shown while the layer is closed. */
    hint?: string;
    open: boolean;
    focused: boolean;
    ontoggle: () => void;
    children: Snippet;
  }
  let { id, title, hint = "", open, focused, ontoggle, children }: Props = $props();
</script>

<section class="layer" class:focused data-layer={id} data-open={open}>
  <h2>
    <button
      type="button"
      class="head"
      aria-expanded={open}
      aria-controls="layer-{id}"
      onclick={ontoggle}
    >
      <span class="caret" aria-hidden="true">{open ? "▾" : "▸"}</span>
      <span class="title">{title}</span>
      {#if hint && !open}<span class="hint">{hint}</span>{/if}
    </button>
  </h2>
  {#if open}
    <div class="body" id="layer-{id}">
      {@render children()}
    </div>
  {/if}
</section>

<style>
  .layer {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: var(--shadow);
  }
  .layer.focused {
    border-color: var(--accent);
  }
  h2 {
    margin: 0;
    font-size: 1rem;
  }
  .head {
    all: unset;
    box-sizing: border-box;
    display: flex;
    align-items: baseline;
    gap: 10px;
    width: 100%;
    padding: 12px 16px;
    cursor: pointer;
    font-weight: 600;
  }
  .head:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
    border-radius: 10px;
  }
  .caret {
    color: var(--muted);
    width: 1em;
  }
  .hint {
    color: var(--muted);
    font-weight: 400;
    font-size: 0.88rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .body {
    padding: 0 16px 16px;
  }
</style>
