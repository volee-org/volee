<script lang="ts">
  import { type ActionResult, composeFix, type FixItem } from "./actions";

  interface Props {
    /** The points that can go in the request: the person's comments and the review's findings. */
    items: FixItem[];
    /** Sends the text to the agent. */
    onsend: (text: string) => Promise<ActionResult>;
    /** Opens the agent's terminal, the fallback when the message did not arrive. */
    onterminal?: (() => void) | null;
    onclose: () => void;
  }
  let { items, onsend, onterminal = null, onclose }: Props = $props();

  // svelte-ignore state_referenced_locally
  let checks = $state<Record<string, boolean>>(Object.fromEntries(items.map((item) => [item.key, item.checked])));
  let text = $state("");
  /** The person typed in the box: choosing points no longer rewrites it. */
  let edited = $state(false);
  let sending = $state(false);
  let result = $state<ActionResult | null>(null);
  let copied = $state(false);
  let area = $state<HTMLTextAreaElement | null>(null);

  const selection = $derived(items.map((item) => ({ ...item, checked: checks[item.key] ?? false })));

  $effect(() => {
    const composed = composeFix(selection);
    if (!edited) text = composed;
  });

  $effect(() => {
    area?.focus();
  });

  function rebuild() {
    edited = false;
    text = composeFix(selection);
  }

  async function send() {
    if (!text.trim() || sending) return;
    sending = true;
    result = null;
    result = await onsend(text);
    sending = false;
  }

  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      copied = true;
    } catch {
      area?.select();
    }
  }

  function onkeydown(event: KeyboardEvent) {
    event.stopPropagation();
    if (event.key === "Escape") onclose();
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) void send();
  }
</script>

<section class="fix" data-testid="fix-panel" aria-label="Ask for a fix">
  <h3>Ask for a fix</h3>
  {#if items.length > 0}
    <ul class="items">
      {#each selection as item (item.key)}
        <li>
          <label>
            <input type="checkbox" bind:checked={checks[item.key]} />
            <span class="chip {item.kind}">{item.kind === "comment" ? "your comment" : "review"}</span>
            <span class="mono where">{item.line === null ? item.file : `${item.file}:${item.line}`}</span>
            <span class="what">{item.text}</span>
          </label>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="note">No review finding and no line comment yet: write what you want changed. Click a line number in the diff to comment on it.</p>
  {/if}
  <textarea
    bind:this={area}
    bind:value={text}
    {onkeydown}
    oninput={() => (edited = true)}
    rows="7"
    aria-label="The request, as the agent will read it"
    placeholder="What should the agent change?"
  ></textarea>
  <p class="note">
    The agent reads it as a message from you, sent through volee. <kbd>Ctrl</kbd>+<kbd>Enter</kbd> sends.
  </p>
  <div class="row">
    <button type="button" class="send" disabled={sending || !text.trim()} onclick={send}>
      {sending ? "Sending…" : "Send to the agent"}
    </button>
    {#if edited}<button type="button" onclick={rebuild}>Rebuild from the selection</button>{/if}
    <button type="button" onclick={onclose}>Close</button>
  </div>
  {#if result}
    <p class="result {result.ok ? 'ok' : 'failed'}" role="status" data-testid="fix-result">{result.message}</p>
    {#if result.delivered === false}
      <div class="fallback" data-testid="fix-fallback">
        <p>
          The message did not reach the agent. Copy it and paste it in the agent's terminal, or open that terminal.
        </p>
        <div class="row">
          <button type="button" onclick={copy}>{copied ? "Copied" : "Copy the text"}</button>
          {#if onterminal}<button type="button" onclick={onterminal}>Open terminal</button>{/if}
        </div>
      </div>
    {/if}
  {/if}
</section>

<style>
  .fix {
    display: grid;
    gap: 8px;
    border: 1px solid var(--accent);
    border-radius: 8px;
    padding: 10px 14px;
    background: var(--surface);
  }
  h3 {
    margin: 0;
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--muted);
  }
  .items {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 4px;
  }
  label {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 8px;
    align-items: baseline;
    cursor: pointer;
  }
  .what {
    flex-basis: 100%;
    color: var(--muted);
    font-size: 0.88rem;
    padding-left: 1.7em;
    overflow-wrap: anywhere;
  }
  .chip {
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 0 8px;
    font-size: 0.78rem;
    color: var(--muted);
  }
  .chip.comment {
    color: var(--accent);
    border-color: var(--accent);
  }
  textarea {
    width: 100%;
    box-sizing: border-box;
    font: inherit;
    font-size: 0.9rem;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
    color: var(--text);
    resize: vertical;
  }
  .note {
    margin: 0;
    color: var(--muted);
    font-size: 0.85rem;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  button {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 7px;
    padding: 4px 12px;
    cursor: pointer;
  }
  button:hover:not(:disabled),
  button:focus-visible {
    border-color: var(--accent);
    outline: none;
  }
  button.send {
    border-color: var(--accent);
    color: var(--accent);
  }
  button:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .result {
    margin: 0;
  }
  .result.ok {
    color: var(--finished);
  }
  .result.failed {
    color: var(--danger);
  }
  .fallback {
    border: 1px dashed var(--waiting);
    border-radius: 8px;
    padding: 8px 12px;
  }
  .fallback p {
    margin: 0 0 6px;
  }
  kbd {
    font-family: ui-monospace, Menlo, monospace;
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 5px;
    background: var(--bg);
  }
</style>
