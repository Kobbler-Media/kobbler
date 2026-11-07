<script lang="ts">
  import { page } from '$app/state';
  import * as api from '#lib/api/client.js';
  import { ApiError } from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { getAuthState } from '#lib/stores/auth.svelte.js';
  import type { BookCast, BookCastCharacter } from '#lib/types.js';
  import { formatDuration } from '#lib/utils/format.js';
  import CastCharacterCard from '#lib/components/book-cast/CastCharacterCard.svelte';
  import CastChapterLines from '#lib/components/book-cast/CastChapterLines.svelte';
  import CastUnattachedSpeakers from '#lib/components/book-cast/CastUnattachedSpeakers.svelte';
  import CastUnattributedLines from '#lib/components/book-cast/CastUnattributedLines.svelte';
  import CastRenderPanel from '#lib/components/book-cast/CastRenderPanel.svelte';

  const auth = getAuthState();
  let bookId = $derived(page.params.id ?? '');
  let cast = $state<BookCast | null>(null);
  let missing = $state(false);
  let loading = $state(true);
  let tab = $state<'cast' | 'lines'>('cast');
  let starting = $state(false);

  const ANALYSING = ['chapters', 'bible', 'voices'];
  const POLL_MS = 15_000;
  const REDO_WARNING =
    'Redo everything? This throws away every approved voice and its clone, every edit to the ' +
    'cast, its voices and its lines, and every rendered chapter, then analyses the book again ' +
    'from nothing. The audiobook already imported stays in the library until a new render ' +
    'replaces it.';
  const RERUN_WARNING = {
    chapters:
      'Run the whole analysis again? It takes as long as the first time. Approved voices, ' +
      'edited descriptions, voice changes, merges, aliases, bible edits and hand-corrected ' +
      'lines are kept for every character it finds again.',
    bible:
      'Run the cast pass again on the attribution as it stands? Approved voices, edited ' +
      'descriptions, voice changes, merges, aliases, bible edits and hand-corrected lines are ' +
      'kept for every character it finds again.'
  };
  let analysing = $derived(cast !== null && ANALYSING.includes(cast.state));

  let groups = $derived.by(() => {
    const all = cast?.characters ?? [];
    const by = (f: (c: BookCastCharacter) => boolean) => all.filter(f);
    return [
      { title: 'Narrator', characters: by((c) => c.narrator) },
      { title: 'Cast', characters: by((c) => !c.narrator && !c.pooled && !c.generic) },
      {
        title: 'Pooled: a few lines each, sharing voices by gender and age',
        characters: by((c) => c.pooled)
      },
      { title: 'Generic: read by the narrator', characters: by((c) => c.generic) }
    ].filter((g) => g.characters.length > 0);
  });
  let approved = $derived(
    (cast?.characters ?? []).flatMap((c) => c.voices).filter((v) => v.approved_at).length
  );
  let voiceCount = $derived((cast?.characters ?? []).flatMap((c) => c.voices).length);

  async function load() {
    try {
      cast = await api.getBookCast(bookId);
      missing = false;
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) {
        missing = true;
        cast = null;
      } else {
        showMessage(e instanceof Error ? e.message : 'Failed to load the cast');
      }
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    if (bookId && auth.isAdmin) load();
  });

  $effect(() => {
    if (!analysing) return;
    const timer = setInterval(load, POLL_MS);
    return () => clearInterval(timer);
  });

  async function start(options: { fresh?: boolean; rerun?: 'chapters' | 'bible' } = {}) {
    if (options.fresh && !confirm(REDO_WARNING)) return;
    if (options.rerun && !confirm(RERUN_WARNING[options.rerun])) return;
    starting = true;
    try {
      await api.startBookCast(bookId, options);
      showMessage('Cast analysis started');
      await load();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to start');
    } finally {
      starting = false;
    }
  }
</script>

<svelte:head>
  <title>Full cast — Kobbler</title>
</svelte:head>

<div class="cast-page">
  <header class="top">
    <a href="/library/books/{bookId}" class="back">
      <span class="material-icons-outlined">arrow_back</span>
      Book
    </a>
    <h1>Full cast</h1>
    <span class="spacer"></span>
    {#if cast}
      <span class="state">{cast.state}</span>
    {/if}
  </header>

  {#if !auth.isAdmin}
    <p class="muted">Only an admin can cast a book.</p>
  {:else if loading}
    <p class="muted">Loading…</p>
  {:else if missing}
    <div class="panel">
      <p>This book has not been cast yet.</p>
      <button type="button" class="btn" disabled={starting} onclick={() => start()}>
        Analyse the cast
      </button>
    </div>
  {:else if cast}
    <div class="panel summary">
      <span>Chapters {cast.chapters.done}/{cast.chapters.total}</span>
      <span>{cast.lines} lines</span>
      <span>{cast.unresolved} unresolved</span>
      {#if voiceCount > 0}<span>{approved}/{voiceCount} voices approved</span>{/if}
      {#if analysing && cast.eta.analysis_seconds !== null}
        <span>About {formatDuration(cast.eta.analysis_seconds)} of analysis left</span>
      {/if}
      {#if cast.error}<span class="error">{cast.error}</span>{/if}
      <span class="spacer"></span>
      {#if analysing}
        <button type="button" class="btn alt small" disabled={starting} onclick={() => start()}>
          Resume analysis
        </button>
      {:else if cast.state !== 'rendering'}
        <button
          type="button"
          class="btn alt small"
          disabled={starting}
          onclick={() => start({ rerun: 'bible' })}
        >
          Rerun cast pass
        </button>
        <button
          type="button"
          class="btn alt small"
          disabled={starting}
          onclick={() => start({ rerun: 'chapters' })}
        >
          Rerun analysis
        </button>
      {/if}
      {#if cast.state !== 'rendering'}
        <button
          type="button"
          class="btn alt small danger"
          disabled={starting}
          onclick={() => start({ fresh: true })}
        >
          Redo everything
        </button>
      {/if}
    </div>

    {#if !analysing}
      <CastRenderPanel {bookId} {cast} onChanged={load} />
    {/if}

    {#if analysing}
      <p class="muted">Voices and corrections open once analysis reaches casting.</p>
    {:else}
      <div class="tabs" role="tablist">
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'cast'}
          class:active={tab === 'cast'}
          onclick={() => (tab = 'cast')}>Cast</button
        >
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'lines'}
          class:active={tab === 'lines'}
          onclick={() => (tab = 'lines')}>Lines</button
        >
      </div>

      {#if tab === 'cast'}
        {#if cast.unattached.length > 0}
          <section class="panel">
            <h2>Speakers no character claims</h2>
            <CastUnattachedSpeakers
              {bookId}
              unattached={cast.unattached}
              characters={cast.characters}
              onChanged={load}
            />
          </section>
        {/if}
        {#if cast.unresolved > 0}
          <section class="panel">
            <h2>Lines that need a speaker ({cast.unresolved})</h2>
            <CastUnattributedLines
              {bookId}
              unresolved={cast.unresolved}
              characters={cast.characters}
              onChanged={load}
            />
          </section>
        {/if}
        {#each groups as g (g.title)}
          <section class="group">
            <h2>{g.title}</h2>
            {#each g.characters as character (character.id)}
              <CastCharacterCard
                {bookId}
                {character}
                others={cast.characters}
                presets={cast.presets}
                onChanged={load}
              />
            {/each}
          </section>
        {/each}
      {:else}
        <CastChapterLines
          {bookId}
          chapters={cast.chapters.total}
          characters={cast.characters}
          onChanged={load}
        />
      {/if}
    {/if}
  {/if}
</div>

<style>
  .cast-page {
    display: grid;
    gap: 16px;
    padding: 24px clamp(16px, 5vw, 64px);
    max-width: 1200px;
    margin: 0 auto;
  }

  .top {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .back {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--muted);
    text-decoration: none;
    font-size: 13px;
  }

  h1 {
    margin: 0;
    font-size: 20px;
  }

  h2 {
    margin: 0;
    font-size: 14px;
    color: var(--muted);
    font-weight: 600;
  }

  .spacer {
    flex: 1;
  }

  .state {
    font-size: 12px;
    padding: 2px 10px;
    border-radius: 999px;
    border: 1px solid var(--ring);
    text-transform: capitalize;
  }

  .panel {
    display: grid;
    gap: 10px;
    padding: 14px 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 14px;
  }

  .summary {
    display: flex;
    align-items: center;
    gap: 16px;
    flex-wrap: wrap;
    font-size: 13px;
  }

  .group {
    display: grid;
    gap: 10px;
  }

  .tabs {
    display: flex;
    gap: 4px;
  }

  .tabs button {
    font: inherit;
    font-size: 13px;
    padding: 6px 14px;
    border-radius: 999px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    cursor: pointer;
  }

  .tabs button.active {
    background: var(--card);
    font-weight: 600;
  }

  .muted {
    color: var(--muted);
  }

  .error,
  .danger {
    color: var(--danger);
  }
</style>
