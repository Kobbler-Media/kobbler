<script lang="ts">
  import type { FacetPill } from './types';

  interface Props {
    options: FacetPill[];
    value: string | null;
    allLabel?: string;
    onChange: (next: string | null) => void;
  }

  let { options, value, allLabel = 'All', onChange }: Props = $props();
</script>

<div class="pills">
  <button type="button" class="pill" class:active={value === null} onclick={() => onChange(null)}>
    {allLabel}
  </button>
  {#each options as option (option.value)}
    <button
      type="button"
      class="pill"
      class:active={value === option.value}
      onclick={() => onChange(value === option.value ? null : option.value)}
    >
      {#if option.tint}
        <span class="dot" style:background={option.tint}></span>
      {/if}
      {option.label}
      {#if option.count != null}
        <span class="count">{option.count}</span>
      {/if}
    </button>
  {/each}
</div>

<style>
  .pills {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 14px;
    border-radius: 20px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }

  .pill:hover {
    color: var(--ink);
    border-color: var(--muted);
  }

  .pill.active {
    border-color: var(--accent);
    background: var(--accent-dim);
    color: var(--ink);
  }

  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .count {
    font-size: 11px;
    font-weight: 500;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }
</style>
