<script lang="ts">
  import * as api from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import type { BookCastCharacter } from '#lib/types.js';

  interface Props {
    bookId: string;
    unattached: { speaker: string; lines: number }[];
    characters: BookCastCharacter[];
    onChanged: () => void;
  }

  let { bookId, unattached, characters, onChanged }: Props = $props();

  let targets = $state<Record<string, string>>({});
  let busy = $state(false);
  let speakers = $derived(characters.filter((c) => !c.narrator));

  async function attach(speaker: string) {
    const characterId = targets[speaker];
    if (!characterId) return;
    busy = true;
    try {
      const res = await api.attachCastSpeaker(bookId, speaker, characterId);
      showMessage(`Attached ${res.attached} line(s)`);
      onChanged();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed');
    } finally {
      busy = false;
    }
  }
</script>

<ul class="unattached">
  {#each unattached as u (u.speaker)}
    <li>
      <span class="speaker">“{u.speaker}”</span>
      <span class="count">{u.lines} line(s)</span>
      <select aria-label="Attach {u.speaker} to" bind:value={targets[u.speaker]}>
        <option value="">Attach to…</option>
        {#each speakers as c (c.id)}
          <option value={c.id}>{c.name}</option>
        {/each}
      </select>
      <button
        type="button"
        class="btn alt small"
        disabled={busy || !targets[u.speaker]}
        onclick={() => attach(u.speaker)}
      >
        Attach
      </button>
    </li>
  {/each}
</ul>

<style>
  .unattached {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 6px;
  }

  li {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .speaker {
    font-weight: 600;
  }

  .count {
    font-size: 12px;
    color: var(--muted);
  }

  select {
    font: inherit;
    font-size: 13px;
  }
</style>
