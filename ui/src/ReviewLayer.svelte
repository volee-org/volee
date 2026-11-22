<script lang="ts">
  import { inline } from "./markdown";
  import {
    busy,
    type Review,
    reviewFacts,
    runLabel,
    SEVERITY_TEXT,
    sortFindings,
    whereText,
  } from "./review";

  interface Props {
    review: Review | null;
    /** What the last ask said, when it did not work. */
    problem: string;
    asking: boolean;
    onrun: (retry: boolean) => void;
  }
  let { review, problem, asking, onrun }: Props = $props();

  const findings = $derived(review ? sortFindings(review.findings) : []);
  const label = $derived(runLabel(review));
</script>

<div data-testid="review">
  <p class="opinion">
    An AI reviewer's opinion. It can be wrong and it never blocks anything: you decide.
  </p>

  {#if !review}
    <p class="note">Reading the review…</p>
  {:else if review.status === "disabled"}
    <p class="note">Reviews are switched off (<span class="mono">[review] enabled</span>).</p>
  {:else}
    <p class="state" data-status={review.status}>
      {#if busy(review)}
        <span class="spin" aria-hidden="true"></span>
        {review.status === "pending" ? "Waiting for a free reviewer…" : "Reviewing the change…"}
      {:else if review.status === "failed"}
        <strong class="bad">The review failed:</strong> {review.error ?? "no reason given"}
      {:else if review.status === "done"}
        <strong>{findings.length === 0 ? "No findings" : `${findings.length} ${findings.length === 1 ? "finding" : "findings"}`}</strong>
        <span class="facts">{reviewFacts(review)}</span>
      {:else}
        No review of this change yet.
      {/if}
      {#if label}
        <button type="button" class="run" disabled={asking} onclick={() => onrun(review.status === "failed")}>
          {asking ? "Asking…" : label}
        </button>
      {/if}
    </p>
    {#if problem}<p class="bad" role="alert">{problem}</p>{/if}

    {#if findings.length > 0}
      <ul class="findings">
        {#each findings as finding, i (i)}
          <li class="finding {finding.severity}">
            <span class="head">
              <span class="sev">{SEVERITY_TEXT[finding.severity] ?? finding.severity}</span>
              <span class="mono where">{whereText(finding)}</span>
            </span>
            <!-- eslint-disable-next-line svelte/no-at-html-tags -->
            <span class="summary">{@html inline(finding.summary)}</span>
            {#if finding.failure_scenario}
              <!-- eslint-disable-next-line svelte/no-at-html-tags -->
              <span class="scenario"><strong>Failure scenario:</strong> {@html inline(finding.failure_scenario)}</span>
            {/if}
          </li>
        {/each}
      </ul>
    {:else if review.status === "done" && review.raw}
      <details class="raw">
        <summary>Nothing was read as a finding: what the reviewer said</summary>
        <pre>{review.raw}</pre>
      </details>
    {/if}
  {/if}
</div>

<style>
  .opinion {
    margin: 0 0 8px;
    color: var(--muted);
    font-size: 0.88rem;
    font-style: italic;
  }
  .note {
    margin: 6px 0 0;
    color: var(--muted);
  }
  .state {
    margin: 0 0 8px;
    display: flex;
    flex-wrap: wrap;
    gap: 6px 12px;
    align-items: center;
  }
  .facts {
    color: var(--muted);
    font-size: 0.88rem;
  }
  .bad {
    color: var(--danger);
  }
  button.run {
    background: transparent;
    border: 1px solid var(--accent);
    color: var(--accent);
    border-radius: 7px;
    padding: 3px 12px;
    cursor: pointer;
  }
  button.run:hover:not(:disabled),
  button.run:focus-visible {
    background: var(--accent);
    color: var(--accent-text);
    outline: none;
  }
  button.run:disabled {
    opacity: 0.6;
    cursor: progress;
  }
  .spin {
    width: 0.8em;
    height: 0.8em;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    display: inline-block;
    animation: turn 0.9s linear infinite;
  }
  @keyframes turn {
    to {
      transform: rotate(360deg);
    }
  }
  .findings {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 8px;
  }
  .finding {
    display: grid;
    gap: 3px;
    border: 1px solid var(--border);
    border-left: 4px solid var(--waiting);
    border-radius: 8px;
    padding: 8px 12px;
    background: var(--bg);
  }
  .finding.important {
    border-left-color: var(--danger);
  }
  .finding.nit,
  .finding.pre_existing {
    border-left-color: var(--muted);
  }
  .head {
    display: flex;
    gap: 10px;
    align-items: baseline;
  }
  .sev {
    font-size: 0.74rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--muted);
    font-weight: 700;
  }
  .where {
    overflow-wrap: anywhere;
  }
  .summary,
  .scenario {
    overflow-wrap: anywhere;
  }
  .scenario {
    color: var(--muted);
    font-size: 0.9rem;
  }
  .raw {
    margin-top: 6px;
  }
  .raw summary {
    cursor: pointer;
    color: var(--muted);
  }
  .raw pre {
    white-space: pre-wrap;
    background: var(--bg);
    padding: 8px 10px;
    border-radius: 6px;
    overflow-x: auto;
  }
  .mono {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 0.9em;
  }
</style>
