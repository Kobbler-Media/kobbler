<script lang="ts">
  import type { LivetvSeriesRule } from '$lib/types';

  interface Props {
    rules: LivetvSeriesRule[];
    selectedId: string | null;
    creating: boolean;
    onSelect: (rule: LivetvSeriesRule) => void;
    onCreate: () => void;
  }

  let { rules, selectedId, creating, onSelect, onCreate }: Props = $props();

  function state(rule: LivetvSeriesRule): string {
    if (!rule.enabled) return 'paused';
    if (rule.queued_count > 0) {
      return `${rule.queued_count} queued`;
    }
    return 'no match in window';
  }
</script>

<aside class="list">
  {#each rules as rule (rule.id)}
    <button
      type="button"
      class="entry"
      class:selected={rule.id === selectedId && !creating}
      onclick={() => onSelect(rule)}
    >
      <span class="name">{rule.title}</span>
      <span class="state">{state(rule)}</span>
    </button>
  {/each}

  {#if rules.length === 0}
    <p class="none">No rules yet.</p>
  {/if}

  <button type="button" class="new" class:selected={creating} onclick={onCreate}>
    <span class="material-icons-outlined">add</span>
    New rule
  </button>
</aside>

<style>
  .list {
    width: 210px;
    flex-shrink: 0;
    box-sizing: border-box;
    border: 1px solid var(--ring);
    border-radius: 12px;
    background: var(--card);
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    overflow-y: auto;
  }

  .entry {
    display: flex;
    flex-direction: column;
    gap: 1px;
    padding: 8px 11px;
    border-radius: 9px;
    box-sizing: border-box;
    border: 1px solid transparent;
    background: transparent;
    text-align: left;
    cursor: pointer;
  }

  .entry.selected {
    background: var(--accent-dim);
    border-color: var(--accent);
  }

  .entry:hover:not(.selected) {
    border-color: var(--ring);
  }

  .name {
    font-size: 12.5px;
    font-weight: 600;
    color: #c2cad2;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .entry.selected .name {
    color: var(--ink);
  }

  .state {
    font-size: 10.5px;
    color: #5f6874;
  }

  .none {
    margin: 4px 11px;
    font-size: 11.5px;
    color: var(--muted);
  }

  .new {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 38px;
    padding: 0 11px;
    border-radius: 9px;
    border: 1px dashed var(--ring);
    background: transparent;
    color: var(--muted);
    font-size: 12.5px;
    font-weight: 600;
    margin-top: 4px;
    cursor: pointer;
  }

  .new.selected,
  .new:hover {
    color: var(--ink);
    border-color: var(--accent);
  }

  .new .material-icons-outlined {
    font-size: 16px;
  }
</style>
