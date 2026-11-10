<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { BookCastCharacter } from '#lib/types.js';
  import { bibleText, partLabel } from './castCharacter.js';

  interface Props {
    character: BookCastCharacter;
    samples?: number;
    actions?: Snippet;
  }

  let { character, samples = 3, actions }: Props = $props();

  let traits = $derived(
    (['gender', 'age', 'accent'] as const)
      .map((f) => bibleText(character, f))
      .filter((t): t is string => t !== null)
  );
  let personality = $derived(bibleText(character, 'personality'));
  let otherNames = $derived(character.names.slice(1).filter((n) => n.lines > 0));
  let label = $derived(partLabel(character));
</script>

<div class="summary">
  <header>
    <h2>{character.name}</h2>
    {#if label}<span class="badge">{label}</span>{/if}
    {#if !character.narrator}
      <span class="count"
        >{character.line_count} {character.line_count === 1 ? 'line' : 'lines'}</span
      >
    {/if}
    <span class="spacer"></span>
    {@render actions?.()}
  </header>

  {#if traits.length > 0}
    <ul class="traits">
      {#each traits as t (t)}<li>{t}</li>{/each}
    </ul>
  {/if}
  {#if personality}<p class="personality">{personality}</p>{/if}
  {#if otherNames.length > 0}
    <p class="names">
      Also called
      {#each otherNames as n, i (n.name)}<strong>{n.name}</strong>
        <span class="n">({n.lines})</span>{i < otherNames.length - 1 ? ', ' : ''}{/each}
    </p>
  {/if}
  {#if samples > 0 && character.samples.length > 0}
    <ul class="samples">
      {#each character.samples.slice(0, samples) as s, i (i)}
        <li>“{s}”</li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .summary {
    display: grid;
    gap: 8px;
  }

  header {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }

  h2 {
    margin: 0;
    font-size: 20px;
  }

  .badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--ring);
    color: var(--muted);
  }

  .count {
    font-size: 12px;
    color: var(--muted);
  }

  .spacer {
    flex: 1;
  }

  .traits {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .traits li {
    font-size: 12px;
    padding: 2px 10px;
    border-radius: 999px;
    background: var(--bg-2);
    text-transform: capitalize;
  }

  .personality,
  .names {
    margin: 0;
    font-size: 13px;
  }

  .names {
    color: var(--muted);
  }

  .names strong {
    color: var(--ink);
    font-weight: 500;
  }

  .n {
    font-size: 11px;
  }

  .samples {
    display: grid;
    gap: 6px;
    margin: 0;
    padding: 0 0 0 12px;
    list-style: none;
    border-left: 2px solid var(--ring);
    font-family: Georgia, 'Iowan Old Style', 'Times New Roman', serif;
    font-size: 14px;
    line-height: 1.5;
  }
</style>
