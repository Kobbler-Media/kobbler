<script lang="ts">
  import * as api from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import type { BookCastCharacter, BookCastPresetChoice } from '#lib/types.js';
  import CastVoiceStep from './CastVoiceStep.svelte';

  interface Props {
    bookId: string;
    character: BookCastCharacter;
    others: BookCastCharacter[];
    presets: BookCastPresetChoice[];
    onChanged: () => void;
  }

  let { bookId, character, others, presets, onChanged }: Props = $props();

  const BIBLE_FIELDS = ['gender', 'age', 'accent', 'personality'] as const;

  let editing = $state(false);
  let busy = $state(false);
  let form = $state({ name: '', aliases: '', bible: {} as Record<string, string> });
  let absorb = $state('');
  let changeFrom = $state(2);
  let changeWhat = $state('');
  let changeVoice = $state('');

  let ownVoice = $derived(character.narrator || (!character.pooled && !character.generic));
  let mergeable = $derived(others.filter((o) => !o.narrator && o.id !== character.id));

  async function act(work: () => Promise<unknown>, done?: string) {
    busy = true;
    try {
      await work();
      if (done) showMessage(done);
      onChanged();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed');
    } finally {
      busy = false;
    }
  }

  function toggleEdit() {
    editing = !editing;
    if (!editing) return;
    form = {
      name: character.name,
      aliases: character.aliases.join(', '),
      bible: Object.fromEntries(
        BIBLE_FIELDS.map((f) => [
          f,
          typeof character.bible[f] === 'string' ? (character.bible[f] as string) : ''
        ])
      )
    };
  }

  function save() {
    act(async () => {
      await api.updateCastCharacter(bookId, character.id, {
        name: form.name.trim(),
        aliases: form.aliases
          .split(',')
          .map((a) => a.trim())
          .filter(Boolean),
        bible: Object.fromEntries(BIBLE_FIELDS.map((f) => [f, form.bible[f]?.trim() || null]))
      });
      editing = false;
    }, 'Saved');
  }
</script>

<article class="character" class:narrator={character.narrator}>
  <header>
    <h3>{character.name}</h3>
    {#if character.narrator}
      <span class="badge narrator-badge">Narrator</span>
    {:else if character.generic}
      <span class="badge">Generic: read by the narrator</span>
    {:else if character.pooled}
      <span class="badge">Pooled</span>
    {/if}
    {#if !character.narrator}
      <span class="count">{character.line_count} lines</span>
    {/if}
    <span class="spacer"></span>
    <button type="button" class="btn alt small" onclick={toggleEdit}>
      {editing ? 'Close' : 'Edit'}
    </button>
  </header>

  {#if character.aliases.length > 0}
    <p class="aliases">Also: {character.aliases.join(', ')}</p>
  {/if}
  {#if character.samples.length > 0}
    <ul class="samples">
      {#each character.samples as s, i (i)}
        <li>“{s}”</li>
      {/each}
    </ul>
  {/if}

  {#if editing}
    <div class="edit">
      <label class="field"><span>Name</span><input bind:value={form.name} /></label>
      <label class="field"
        ><span>Aliases (comma-separated)</span><input bind:value={form.aliases} /></label
      >
      {#each BIBLE_FIELDS as f (f)}
        <label class="field"><span>{f}</span><input bind:value={form.bible[f]} /></label>
      {/each}
      <div class="row">
        <button type="button" class="btn small" disabled={busy} onclick={save}>Save</button>
        {#if !character.narrator}
          <button
            type="button"
            class="btn alt small"
            disabled={busy}
            onclick={() =>
              act(() =>
                api.updateCastCharacter(bookId, character.id, { generic: !character.generic })
              )}
          >
            {character.generic ? 'Give own voice' : 'Make generic'}
          </button>
        {/if}
      </div>
      {#if !character.narrator && mergeable.length > 0}
        <div class="row">
          <label class="field grow">
            <span>Merge into {character.name}</span>
            <select bind:value={absorb}>
              <option value="">Choose a character…</option>
              {#each mergeable as o (o.id)}
                <option value={o.id}>{o.name} ({o.line_count})</option>
              {/each}
            </select>
          </label>
          <button
            type="button"
            class="btn alt small"
            disabled={busy || !absorb}
            onclick={() =>
              act(async () => {
                await api.mergeCastCharacters(bookId, character.id, absorb);
                absorb = '';
              }, 'Merged')}
          >
            Merge
          </button>
        </div>
      {/if}
      {#if ownVoice}
        <div class="row">
          <label class="field narrow">
            <span>Voice changes from chapter</span>
            <input type="number" min="2" bind:value={changeFrom} />
          </label>
          <label class="field grow"><span>Change</span><input bind:value={changeWhat} /></label>
        </div>
        <label class="field"
          ><span>Voice from then on</span><textarea rows="2" bind:value={changeVoice}
          ></textarea></label
        >
        <button
          type="button"
          class="btn alt small"
          disabled={busy || changeFrom < 2}
          onclick={() =>
            act(async () => {
              await api.addCastVoiceChange(bookId, character.id, {
                from_chapter: changeFrom - 1,
                change: changeWhat,
                description: changeVoice
              });
              changeWhat = '';
              changeVoice = '';
            }, 'Voice change added')}
        >
          Add voice change
        </button>
      {/if}
    </div>
  {/if}

  {#if character.voices.length > 0}
    <div class="voices">
      {#each character.voices as voice (voice.from_chapter)}
        <CastVoiceStep {bookId} {character} {voice} {presets} {onChanged} />
      {/each}
    </div>
  {/if}
</article>

<style>
  .character {
    display: grid;
    gap: 8px;
    padding: 14px 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 14px;
  }

  .character.narrator {
    border-width: 2px;
  }

  header {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  h3 {
    margin: 0;
    font-size: 15px;
  }

  .badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--ring);
    color: var(--muted);
  }

  .narrator-badge {
    color: var(--ink);
    font-weight: 600;
  }

  .count {
    font-size: 12px;
    color: var(--muted);
  }

  .spacer {
    flex: 1;
  }

  .aliases {
    margin: 0;
    font-size: 12px;
    color: var(--muted);
  }

  .samples {
    margin: 0;
    padding-left: 16px;
    font-size: 13px;
    font-style: italic;
  }

  .edit,
  .voices {
    display: grid;
    gap: 8px;
  }

  .row {
    display: flex;
    gap: 8px;
    align-items: flex-end;
    flex-wrap: wrap;
  }

  .field {
    display: grid;
    gap: 4px;
    font-size: 12px;
    color: var(--muted);
    text-transform: none;
  }

  .field input,
  .field select,
  .field textarea {
    font: inherit;
    font-size: 13px;
    color: var(--ink);
  }

  .grow {
    flex: 1;
  }

  .narrow {
    width: 180px;
  }
</style>
