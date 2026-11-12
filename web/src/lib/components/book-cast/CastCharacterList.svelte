<script lang="ts">
  import type { BookCast, BookCastCharacter } from '#lib/types.js';
  import CastCharacterCard from './CastCharacterCard.svelte';
  import CastCharacterLines from './CastCharacterLines.svelte';

  interface Props {
    bookId: string;
    cast: BookCast;
    onChanged: () => void;
  }

  let { bookId, cast, onChanged }: Props = $props();

  let query = $state('');
  let reading = $state<string | null>(null);
  let readingCharacter = $derived(cast.characters.find((c) => c.id === reading));

  let groups = $derived.by(() => {
    const q = query.trim().toLowerCase();
    const all = cast.characters
      .filter((c) => !q || [c.name, ...c.aliases].some((n) => n.toLowerCase().includes(q)))
      .toSorted((a, b) => b.line_count - a.line_count);
    const by = (f: (c: BookCastCharacter) => boolean) => all.filter(f);
    return [
      { title: 'Narrator', note: null, characters: by((c) => c.narrator) },
      {
        title: 'Main cast',
        note: 'Each has a voice of their own',
        characters: by((c) => !c.narrator && !c.pooled && !c.generic)
      },
      {
        title: 'Minor parts',
        note: 'A few lines each, sharing voices by gender and age',
        characters: by((c) => c.pooled)
      },
      {
        title: 'Read by the narrator',
        note: 'Crowds, groups and passers-by',
        characters: by((c) => c.generic)
      }
    ].filter((g) => g.characters.length > 0);
  });
</script>

{#if readingCharacter}
  <CastCharacterLines
    {bookId}
    character={readingCharacter}
    characters={cast.characters}
    {onChanged}
    onBack={() => (reading = null)}
  />
{:else}
  <div class="list">
    <input class="search" type="search" placeholder="Find a character…" bind:value={query} />
    {#each groups as g (g.title)}
      <section class="group">
        <header>
          <h2>{g.title} <span class="n">{g.characters.length}</span></h2>
          {#if g.note}<span class="note">{g.note}</span>{/if}
          <span class="spacer"></span>
          {#if g.title !== 'Narrator'}<span class="note">lines</span>{/if}
        </header>
        {#each g.characters as character (character.id)}
          <CastCharacterCard
            {bookId}
            {character}
            others={cast.characters}
            presets={cast.presets}
            {onChanged}
            onRead={() => (reading = character.id)}
          />
        {/each}
      </section>
    {:else}
      <p class="note">Nobody matches “{query}”.</p>
    {/each}
  </div>
{/if}

<style>
  .list {
    display: grid;
    gap: 18px;
  }

  .search {
    font: inherit;
    font-size: 14px;
    max-width: 320px;
  }

  .group {
    display: grid;
    gap: 6px;
  }

  header {
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 0 14px;
  }

  h2 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
  }

  .n,
  .note {
    font-size: 12px;
    font-weight: 400;
    color: var(--muted);
  }

  .spacer {
    flex: 1;
  }
</style>
