<script lang="ts">
  import type { BookCastCharacter, BookCastPresetChoice } from '#lib/types.js';
  import CastCharacterEdit from './CastCharacterEdit.svelte';
  import CastCharacterSummary from './CastCharacterSummary.svelte';
  import CastVoiceStep from './CastVoiceStep.svelte';
  import { bibleText, partLabel } from './castCharacter.js';

  interface Props {
    bookId: string;
    character: BookCastCharacter;
    others: BookCastCharacter[];
    presets: BookCastPresetChoice[];
    onChanged: () => void;
    onRead: () => void;
  }

  let { bookId, character, others, presets, onChanged, onRead }: Props = $props();

  let open = $state(false);
  let editing = $state(false);

  let approved = $derived(
    character.voices.length > 0 && character.voices.every((v) => v.approved_at !== null)
  );
  let traits = $derived(
    (['gender', 'age'] as const)
      .map((f) => bibleText(character, f))
      .filter(Boolean)
      .join(', ')
  );
  let label = $derived(partLabel(character));
</script>

<article class="character" class:open>
  <button type="button" class="row" aria-expanded={open} onclick={() => (open = !open)}>
    {#if !character.generic}
      <span
        class="material-icons-outlined status"
        class:approved
        title={approved ? 'Voice approved' : 'Voice not approved yet'}
      >
        {approved ? 'check_circle' : 'radio_button_unchecked'}
      </span>
    {:else}
      <span class="material-icons-outlined status" title="Read by the narrator">menu_book</span>
    {/if}
    <span class="name">{character.name}</span>
    {#if traits}<span class="traits">{traits}</span>{/if}
    {#if label && !character.narrator}<span class="badge">{label}</span>{/if}
    <span class="spacer"></span>
    {#if !character.narrator}<span class="count">{character.line_count}</span>{/if}
    <span class="material-icons-outlined chev">expand_more</span>
  </button>

  {#if open}
    <div class="body">
      <CastCharacterSummary {character}>
        {#snippet actions()}
          {#if !character.narrator && character.line_count > 0}
            <button type="button" class="btn alt small" onclick={onRead}>Read their lines</button>
          {/if}
          <button type="button" class="btn alt small" onclick={() => (editing = !editing)}>
            {editing ? 'Close' : 'Edit'}
          </button>
        {/snippet}
      </CastCharacterSummary>
      {#if editing}
        <CastCharacterEdit
          {bookId}
          {character}
          {others}
          {onChanged}
          onClose={() => (editing = false)}
        />
      {/if}
      {#each character.voices as voice (voice.from_chapter)}
        <CastVoiceStep {bookId} {character} {voice} {presets} {onChanged} />
      {/each}
    </div>
  {/if}
</article>

<style>
  .character {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    overflow: hidden;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    font: inherit;
    font-size: 14px;
    text-align: left;
    padding: 10px 14px;
    border: none;
    background: none;
    color: var(--ink);
    cursor: pointer;
  }

  .row:hover {
    background: var(--bg-2);
  }

  .status {
    font-size: 18px;
    color: var(--muted);
  }

  .status.approved {
    color: var(--success);
  }

  .name {
    font-weight: 600;
  }

  .traits {
    font-size: 12px;
    color: var(--muted);
    text-transform: capitalize;
  }

  .badge {
    font-size: 11px;
    padding: 1px 8px;
    border-radius: 999px;
    border: 1px solid var(--ring);
    color: var(--muted);
  }

  .spacer {
    flex: 1;
  }

  .count {
    font-size: 12px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .chev {
    color: var(--muted);
    transition: transform 0.15s;
  }

  .open .chev {
    transform: rotate(180deg);
  }

  .body {
    display: grid;
    gap: 12px;
    padding: 4px 16px 16px;
  }
</style>
