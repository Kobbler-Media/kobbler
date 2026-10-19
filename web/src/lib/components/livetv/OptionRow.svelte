<script lang="ts">
  import MetaLine from '$lib/components/shared/MetaLine.svelte';

  interface Props {
    label: string;
    meta?: string | string[];
    selected: boolean;
    isDefault?: boolean;
    disabled?: boolean;
    onSelect: () => void;
  }

  let { label, meta, selected, isDefault = false, disabled = false, onSelect }: Props = $props();
</script>

<button
  type="button"
  class="option"
  class:selected
  role="radio"
  aria-checked={selected}
  {disabled}
  onclick={onSelect}
>
  <span class="radio" class:on={selected}></span>
  <span class="label">{label}</span>
  {#if isDefault}
    <span class="default-chip">DEFAULT</span>
  {/if}
  <span class="spacer"></span>
  {#if meta}
    <span class="meta"><MetaLine items={[meta].flat()} /></span>
  {/if}
</button>

<style>
  .option {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 30px;
    width: 100%;
    padding: 0;
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
  }

  .option:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .radio {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    border: 1px solid var(--muted);
    background: var(--bg);
    box-sizing: border-box;
    flex-shrink: 0;
  }

  .radio.on {
    border: 4px solid var(--accent);
  }

  .label {
    font-size: 12.5px;
    font-weight: 500;
    color: var(--muted);
  }

  .option.selected .label {
    font-weight: 600;
    color: var(--ink);
  }

  .default-chip {
    font-size: 9.5px;
    font-weight: 700;
    letter-spacing: 0.05em;
    color: var(--accent);
    border: 1px solid var(--accent);
    border-radius: 4px;
    padding: 2px 7px;
    flex-shrink: 0;
  }

  .spacer {
    flex-grow: 1;
  }

  .meta {
    font-size: 11px;
    color: var(--muted);
    opacity: 0.8;
    text-align: right;
    --meta-line-justify: flex-end;
  }
</style>
