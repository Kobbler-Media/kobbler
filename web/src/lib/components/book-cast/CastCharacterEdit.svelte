<script lang="ts">
  import * as api from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import type { BookCastCharacter } from '#lib/types.js';
  import { BIBLE_FIELDS, bibleText } from './castCharacter.js';

  interface Props {
    bookId: string;
    character: BookCastCharacter;
    others: BookCastCharacter[];
    onChanged: () => void;
    onClose: () => void;
  }

  let { bookId, character, others, onChanged, onClose }: Props = $props();

  let busy = $state(false);
  let form = $state(initialForm());
  let absorb = $state('');
  let changeFrom = $state(2);
  let changeWhat = $state('');
  let changeVoice = $state('');
  let moveName = $state('');
  let moveTo = $state('');
  let moveNewName = $state('');

  let ownVoice = $derived(character.narrator || (!character.pooled && !character.generic));
  let mergeable = $derived(others.filter((o) => !o.narrator && o.id !== character.id));
  let aliasNames = $derived(character.names.slice(1));

  function initialForm() {
    return {
      name: character.name,
      aliases: character.aliases.join(', '),
      bible: Object.fromEntries(BIBLE_FIELDS.map((f) => [f, bibleText(character, f) ?? '']))
    };
  }

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
      onClose();
    }, 'Saved');
  }

  function moveAway() {
    act(async () => {
      await api.moveCastName(bookId, character.id, {
        name: moveName,
        ...(moveTo ? { character_id: moveTo } : { new_name: moveNewName.trim() || moveName })
      });
      moveName = '';
      moveNewName = '';
      moveTo = '';
    }, 'Name moved');
  }
</script>

<div class="edit">
  <fieldset>
    <legend>Who they are</legend>
    <div class="grid">
      <label class="field"><span>Name</span><input bind:value={form.name} /></label>
      <label class="field"
        ><span>Also called (comma-separated)</span><input bind:value={form.aliases} /></label
      >
      {#each BIBLE_FIELDS as f (f)}
        <label class="field"><span>{f}</span><input bind:value={form.bible[f]} /></label>
      {/each}
    </div>
    <div class="row">
      <button type="button" class="btn small" disabled={busy} onclick={save}>Save</button>
      <button type="button" class="btn alt small" onclick={onClose}>Cancel</button>
      <span class="spacer"></span>
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
          {character.generic ? 'Give them their own voice' : 'Let the narrator read them'}
        </button>
      {/if}
    </div>
  </fieldset>

  {#if !character.narrator && aliasNames.length > 0}
    <fieldset>
      <legend>One of their names is someone else</legend>
      <div class="row">
        <label class="field">
          <span>Name</span>
          <select bind:value={moveName}>
            <option value="">Choose a name…</option>
            {#each aliasNames as n (n.name)}
              <option value={n.name}>{n.name} ({n.lines} lines)</option>
            {/each}
          </select>
        </label>
        <button
          type="button"
          class="btn alt small"
          disabled={busy || !moveName}
          onclick={() =>
            act(async () => {
              await api.updateCastCharacter(bookId, character.id, { name: moveName });
              moveName = '';
            }, 'Renamed')}
        >
          Use as main name
        </button>
      </div>
      <div class="row">
        <label class="field">
          <span>Move it and its lines to</span>
          <select bind:value={moveTo}>
            <option value="">A new character</option>
            {#each mergeable as o (o.id)}
              <option value={o.id}>{o.name}</option>
            {/each}
          </select>
        </label>
        {#if !moveTo}
          <label class="field grow">
            <span>New character's name</span>
            <input bind:value={moveNewName} placeholder={moveName || 'The name itself'} />
          </label>
        {/if}
        <button type="button" class="btn alt small" disabled={busy || !moveName} onclick={moveAway}>
          Move
        </button>
      </div>
    </fieldset>
  {/if}

  {#if !character.narrator && mergeable.length > 0}
    <fieldset>
      <legend>Same person as someone else</legend>
      <div class="row">
        <label class="field grow">
          <span>Fold into {character.name}</span>
          <select bind:value={absorb}>
            <option value="">Choose a character…</option>
            {#each mergeable as o (o.id)}
              <option value={o.id}>{o.name} ({o.line_count} lines)</option>
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
    </fieldset>
  {/if}

  {#if ownVoice}
    <fieldset>
      <legend>Their voice changes later in the book</legend>
      <div class="row">
        <label class="field narrow">
          <span>From chapter</span>
          <input type="number" min="2" bind:value={changeFrom} />
        </label>
        <label class="field grow">
          <span>What changes</span><input bind:value={changeWhat} placeholder="e.g. hoarse" />
        </label>
      </div>
      <label class="field"
        ><span>How they sound from then on</span><textarea rows="2" bind:value={changeVoice}
        ></textarea></label
      >
      <div class="row">
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
      </div>
    </fieldset>
  {/if}
</div>

<style>
  .edit {
    display: grid;
    gap: 12px;
  }

  fieldset {
    display: grid;
    gap: 8px;
    margin: 0;
    padding: 12px 14px;
    border: 1px solid var(--ring);
    border-radius: 10px;
  }

  legend {
    font-size: 12px;
    font-weight: 600;
    padding: 0 4px;
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
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
    text-transform: capitalize;
  }

  .field input,
  .field select,
  .field textarea {
    font: inherit;
    font-size: 13px;
    color: var(--ink);
    text-transform: none;
  }

  .grow {
    flex: 1;
  }

  .narrow {
    width: 120px;
  }

  .spacer {
    flex: 1;
  }
</style>
