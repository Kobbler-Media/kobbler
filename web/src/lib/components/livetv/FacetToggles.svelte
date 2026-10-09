<script lang="ts">
  import type { FacetToggle } from './types';

  interface Props {
    options: FacetToggle[];
    onToggle: (value: string) => void;
  }

  let { options, onToggle }: Props = $props();
</script>

<div class="toggles">
  {#each options as option (option.value)}
    <button
      type="button"
      class="toggle"
      class:active={option.active}
      aria-pressed={option.active}
      onclick={() => onToggle(option.value)}
    >
      {#if option.active}
        <span class="material-icons-outlined check">check</span>
      {:else}
        <span class="box"></span>
      {/if}
      {option.label}
    </button>
  {/each}
</div>

<style>
  .toggles {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 6px 12px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
  }

  .toggle:hover {
    color: var(--ink);
    border-color: var(--muted);
  }

  .toggle.active {
    border-color: var(--accent);
    color: var(--ink);
  }

  .box {
    width: 11px;
    height: 11px;
    border-radius: 3px;
    border: 1px solid var(--muted);
  }

  .check {
    font-size: 13px;
    color: var(--accent);
  }
</style>
