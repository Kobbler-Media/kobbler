<script lang="ts">
  import type { ChipDef } from '$lib/utils/chip-mappings';

  interface Props {
    icon?: string;
    label?: string;
    color?: string;
    chip?: ChipDef | null;
    size?: 'sm' | 'md';
    class?: string;
  }

  let { icon, label, color, chip, size = 'sm', class: className = '' }: Props = $props();

  const resolvedIcon = $derived(chip?.icon ?? icon ?? '');
  const resolvedLabel = $derived(chip?.label ?? label ?? '');
  const resolvedColor = $derived(chip?.color ?? color ?? 'var(--muted)');
</script>

{#if resolvedIcon}
  <span
    class="icon-chip {size} {className}"
    style="--chip-color: {resolvedColor}"
    title={resolvedLabel}
  >
    <span class="material-icons-outlined">{resolvedIcon}</span>
  </span>
{/if}

<style>
  .icon-chip {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 6px;
    background: color-mix(in srgb, var(--chip-color) 15%, transparent);
    color: var(--chip-color);
    line-height: 1;
  }

  .icon-chip.sm {
    padding: 3px;
  }

  .icon-chip.sm .material-icons-outlined {
    font-size: 16px;
  }

  .icon-chip.md {
    padding: 4px;
  }

  .icon-chip.md .material-icons-outlined {
    font-size: 20px;
  }
</style>
