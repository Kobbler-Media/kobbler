<script lang="ts">
  import { SearchSelect } from '@koblibs/ui';
  import type { BookCastCharacter } from '#lib/types.js';

  interface Props {
    characters: BookCastCharacter[];
    suggestions: BookCastCharacter[];
    disabled: boolean;
    onPick: (characterId: string) => void;
  }

  let { characters, suggestions, disabled, onPick }: Props = $props();

  let other = $state('');
  let items = $derived(
    characters
      .filter((c) => !c.narrator)
      .map((c) => ({
        value: c.id,
        label: c.name,
        description: c.aliases.length > 0 ? c.aliases.join(', ') : undefined
      }))
  );
</script>

<div class="picker">
  <div class="chips">
    {#each suggestions as c, i (c.id)}
      <button type="button" class="chip" {disabled} onclick={() => onPick(c.id)}>
        {#if i < 9}<kbd>{i + 1}</kbd>{/if}
        <span>{c.name}</span>
      </button>
    {/each}
  </div>
  <div class="other">
    <SearchSelect
      {items}
      bind:value={other}
      placeholder="Someone else…"
      {disabled}
      onchange={(v) => {
        if (v) onPick(v);
        other = '';
      }}
    />
  </div>
</div>

<style>
  .picker {
    display: grid;
    gap: 10px;
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font: inherit;
    font-size: 13px;
    padding: 6px 12px;
    border-radius: 999px;
    border: 1px solid var(--ring);
    background: var(--bg-2);
    color: var(--ink);
    cursor: pointer;
  }

  .chip:not(:disabled):hover {
    border-color: var(--accent);
  }

  .chip:disabled {
    opacity: 0.5;
    cursor: default;
  }

  kbd {
    font-family: ui-monospace, monospace;
    font-size: 10px;
    color: var(--muted);
    border: 1px solid var(--ring);
    border-radius: 4px;
    padding: 0 4px;
  }

  .other {
    max-width: 320px;
  }
</style>
