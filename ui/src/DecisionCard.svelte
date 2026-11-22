<script lang="ts">
  import { tick } from "svelte";

  import { answerText, remaining } from "./format";
  import type { DecisionBody } from "./live";
  import { type Decision, type Question, title } from "./model";
  import Place from "./Place.svelte";

  interface Props {
    decision: Decision;
    now: number;
    /** The card the keyboard shortcuts act on. */
    active: boolean;
    terminal: string | null;
    ondecide: (id: string, body: DecisionBody) => Promise<string>;
    onopen: (sessionId: string) => void;
  }
  let { decision, now, active, terminal, ondecide, onopen }: Props = $props();

  let questions = $derived<Question[]>(decision.questions ?? []);
  let selected = $state<string[][]>([]);
  let other = $state<string[]>([]);
  let denying = $state(false);
  let denyMessage = $state("");
  let busy = $state(false);
  let problem = $state("");
  let denyInput = $state<HTMLInputElement | null>(null);
  let otherInputs = $state<(HTMLInputElement | null)[]>([]);

  let left = $derived(remaining(decision.expires_at, now));
  let ending = $derived(decision.expires_at - now < 10_000);

  function answers(): Record<string, string> {
    const out: Record<string, string> = {};
    questions.forEach((q, i) => {
      out[q.question] = answerText(selected[i] ?? [], other[i] ?? "", q.multiSelect === true);
    });
    return out;
  }

  let complete = $derived(
    questions.length > 0 && questions.every((q, i) => answerText(selected[i] ?? [], other[i] ?? "", q.multiSelect === true) !== ""),
  );

  async function send(body: DecisionBody) {
    if (busy) return;
    busy = true;
    problem = await ondecide(decision.id, body);
    busy = false;
  }

  function toggle(index: number, label: string) {
    const q = questions[index];
    if (!q) return;
    const chosen = selected[index] ?? [];
    if (q.multiSelect) {
      selected[index] = chosen.includes(label) ? chosen.filter((l) => l !== label) : [...chosen, label];
    } else {
      selected[index] = [label];
      other[index] = "";
    }
  }

  /** The first question that still has no answer: where a digit key lands. */
  function current(): number {
    const i = questions.findIndex((q, k) => answerText(selected[k] ?? [], other[k] ?? "", q.multiSelect === true) === "");
    return i === -1 ? questions.length - 1 : i;
  }

  async function startDeny() {
    denying = true;
    await tick();
    denyInput?.focus();
  }

  /** Called by the inbox for a key pressed while this card is the active one. */
  export function shortcut(key: string): boolean {
    if (busy) return false;
    if (key === "d") {
      void startDeny();
      return true;
    }
    if (key === "t") {
      void send({ action: "terminal" });
      return true;
    }
    if (decision.kind === "permission") {
      if (key === "a") {
        void send({ action: "allow" });
        return true;
      }
      const first = decision.suggestions[0];
      if (key === "r" && first) {
        void send({ action: "allow", suggestion: first.index });
        return true;
      }
      return false;
    }
    if (key === "Enter" && complete) {
      void send({ action: "answer", answers: answers() });
      return true;
    }
    if (key === "o") {
      const at = current();
      void tick().then(() => otherInputs[at]?.focus());
      return true;
    }
    if (/^[1-9]$/.test(key)) {
      const at = current();
      const option = questions[at]?.options[Number(key) - 1];
      if (option) {
        toggle(at, option.label);
        return true;
      }
    }
    return false;
  }
</script>

<article
  class="card decision {decision.kind}"
  class:active
  data-kind="decision"
  data-decision={decision.kind}
  data-id={decision.id}
>
  <header>
    <h3>{title({ ...decision, id: decision.session_id })}</h3>
    <span class="timer" class:ending title="Then Claude Code asks in the terminal">
      terminal in <span class="mono" data-testid="remaining">{left}</span>
    </span>
  </header>

  {#if decision.kind === "permission"}
    <p class="what"><strong>{decision.tool_name}</strong> wants permission</p>
    <pre class="summary mono" data-testid="summary">{decision.summary}</pre>
    <details>
      <summary>Full request</summary>
      <pre class="mono">{JSON.stringify(decision.tool_input, null, 2)}</pre>
    </details>
  {:else}
    {#each questions as q, i (q.question)}
      <fieldset class="question" data-question={i}>
        <legend>
          {#if q.header}<span class="chip">{q.header}</span>{/if}
          <span data-testid="question-text">{q.question}</span>
          {#if q.multiSelect}<span class="hint">choose any</span>{/if}
        </legend>
        {#each q.options as option, k (option.label)}
          <button
            type="button"
            class="option"
            class:chosen={(selected[i] ?? []).includes(option.label)}
            aria-pressed={(selected[i] ?? []).includes(option.label)}
            data-option={option.label}
            onclick={() => toggle(i, option.label)}
          >
            {#if i === current() || questions.length === 1}<kbd>{k + 1}</kbd>{/if}
            <span class="label">{option.label}</span>
            {#if option.description}<span class="desc">{option.description}</span>{/if}
          </button>
        {/each}
        <input
          class="other"
          type="text"
          placeholder="Other: write your own answer"
          aria-label="Other answer for {q.question}"
          bind:value={other[i]}
          bind:this={otherInputs[i]}
          oninput={() => {
            if (!q.multiSelect && (other[i] ?? "").trim() !== "") selected[i] = [];
          }}
          onkeydown={(event) => {
            event.stopPropagation();
            if (event.key === "Enter" && complete) void send({ action: "answer", answers: answers() });
            if (event.key === "Escape") (event.currentTarget as HTMLInputElement).blur();
          }}
        />
      </fieldset>
    {/each}
  {/if}

  <p class="where">
    <Place
      repo={decision.repo}
      branch={decision.branch}
      base={decision.base}
      baseSource={decision.base_source}
      cwd={decision.cwd}
    />
  </p>

  {#if denying}
    <form
      class="deny"
      onsubmit={(event) => {
        event.preventDefault();
        void send({ action: "deny", message: denyMessage });
      }}
    >
      <input
        type="text"
        placeholder={decision.kind === "permission" ? "Why? The agent reads it (optional)" : "Why not? The agent reads it (optional)"}
        aria-label="Reason for denying"
        bind:value={denyMessage}
        bind:this={denyInput}
        onkeydown={(event) => {
          event.stopPropagation();
          if (event.key === "Escape") denying = false;
        }}
      />
      <button type="submit" class="danger" data-action="deny-send" disabled={busy}>Deny</button>
      <button type="button" class="quiet" onclick={() => (denying = false)}>Cancel</button>
    </form>
  {:else}
    <footer>
      {#if decision.kind === "permission"}
        <button type="button" class="primary" data-action="allow" disabled={busy} onclick={() => send({ action: "allow" })}>
          Allow <kbd>a</kbd>
        </button>
        {#each decision.suggestions as suggestion, i (suggestion.index)}
          <button
            type="button"
            data-action="allow-rule"
            disabled={busy}
            onclick={() => send({ action: "allow", suggestion: suggestion.index })}
          >
            {suggestion.label}
            {#if i === 0}<kbd>r</kbd>{/if}
          </button>
        {/each}
      {:else}
        <button
          type="button"
          class="primary"
          data-action="answer"
          disabled={busy || !complete}
          onclick={() => send({ action: "answer", answers: answers() })}
        >
          Send answer <kbd>↵</kbd>
        </button>
      {/if}
      <button type="button" class="danger" data-action="deny" disabled={busy} onclick={startDeny}>
        Deny <kbd>d</kbd>
      </button>
      <button type="button" class="quiet" data-action="terminal" disabled={busy} onclick={() => send({ action: "terminal" })}>
        Ask in terminal <kbd>t</kbd>
      </button>
      {#if terminal}
        <button type="button" class="quiet" onclick={() => onopen(decision.session_id)}>Open terminal</button>
      {/if}
    </footer>
  {/if}
  {#if problem}<p class="problem" role="alert">{problem}</p>{/if}
</article>

<style>
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-left: 4px solid var(--waiting);
    border-radius: 10px;
    padding: 14px 18px;
    box-shadow: var(--shadow);
  }
  .card.active {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
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
  .timer {
    color: var(--muted);
    font-size: 0.85rem;
    white-space: nowrap;
  }
  .timer.ending {
    color: var(--danger);
  }
  .what {
    margin: 6px 0 4px;
  }
  .summary {
    margin: 4px 0 6px;
    padding: 8px 10px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 7px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  details {
    color: var(--muted);
    margin-bottom: 6px;
  }
  details pre {
    margin: 6px 0 0;
    max-height: 240px;
    overflow: auto;
    white-space: pre-wrap;
  }
  .where {
    margin: 6px 0 0;
    color: var(--muted);
    display: flex;
    flex-wrap: wrap;
    gap: 4px 8px;
    align-items: center;
  }
  fieldset {
    border: 0;
    padding: 0;
    margin: 8px 0 0;
    display: grid;
    gap: 6px;
  }
  legend {
    padding: 0;
    margin-bottom: 6px;
    font-weight: 600;
  }
  .hint {
    font-weight: 400;
    color: var(--muted);
    font-size: 0.82rem;
    margin-left: 6px;
  }
  .chip {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 0 9px;
    font-size: 0.82rem;
    margin-right: 6px;
    font-weight: 400;
  }
  .option {
    display: flex;
    align-items: baseline;
    gap: 8px;
    text-align: left;
    width: 100%;
  }
  .option.chosen {
    background: var(--accent);
    color: var(--accent-text);
  }
  .desc {
    color: var(--muted);
    font-size: 0.85rem;
  }
  .option.chosen .desc {
    color: inherit;
    opacity: 0.85;
  }
  .label {
    font-weight: 550;
  }
  input[type="text"] {
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 5px 10px;
    font: inherit;
    min-width: 0;
  }
  input[type="text"]:focus-visible {
    outline: 2px solid var(--accent);
    border-color: transparent;
  }
  .other {
    width: 100%;
    box-sizing: border-box;
  }
  footer,
  .deny {
    margin-top: 12px;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  .deny input {
    flex: 1;
    min-width: 200px;
  }
  button {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 7px;
    padding: 4px 12px;
    cursor: pointer;
    font: inherit;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  button.primary {
    border-color: var(--accent);
    color: var(--accent);
  }
  button.danger {
    border-color: var(--danger);
    color: var(--danger);
  }
  button.quiet {
    color: var(--muted);
  }
  button:not(:disabled):hover,
  button:not(:disabled):focus-visible {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-text);
    outline: none;
  }
  button.danger:not(:disabled):hover,
  button.danger:not(:disabled):focus-visible {
    background: var(--danger);
    border-color: var(--danger);
  }
  kbd {
    font-family: ui-monospace, monospace;
    font-size: 0.75rem;
    border: 1px solid currentColor;
    border-radius: 4px;
    padding: 0 4px;
    margin-left: 4px;
    opacity: 0.7;
  }
  .problem {
    margin: 8px 0 0;
    color: var(--danger);
    font-size: 0.9rem;
  }
</style>
