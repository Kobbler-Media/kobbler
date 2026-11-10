<script lang="ts">
  import { page } from '$app/state';
  import * as api from '#lib/api/client.js';
  import { ApiError } from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { getAuthState } from '#lib/stores/auth.svelte.js';
  import type { BookCast } from '#lib/types.js';
  import OverflowMenu from '#lib/components/shared/OverflowMenu.svelte';
  import CastStepper, { type CastStep } from '#lib/components/book-cast/CastStepper.svelte';
  import CastAnalysisStep from '#lib/components/book-cast/CastAnalysisStep.svelte';
  import CastSpeakersStep from '#lib/components/book-cast/CastSpeakersStep.svelte';
  import CastVoicesStep from '#lib/components/book-cast/CastVoicesStep.svelte';
  import CastRenderPanel from '#lib/components/book-cast/CastRenderPanel.svelte';
  import CastCharacterList from '#lib/components/book-cast/CastCharacterList.svelte';
  import CastChapterLines from '#lib/components/book-cast/CastChapterLines.svelte';
  import { voiceSteps } from '#lib/components/book-cast/castReview.js';

  type View = 'analyse' | 'speakers' | 'voices' | 'render' | 'characters' | 'lines';

  const auth = getAuthState();
  let bookId = $derived(page.params.id ?? '');
  let cast = $state<BookCast | null>(null);
  let missing = $state(false);
  let loading = $state(true);
  let starting = $state(false);
  let chosen = $state<View | null>(null);

  const ANALYSING = ['chapters', 'bible', 'voices'];
  const POLL_MS = 10_000;
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
  let rendering = $derived(cast?.state === 'rendering');
  let polling = $derived(analysing || rendering || !!cast?.approving);
  let voices = $derived(voiceSteps(cast?.characters ?? []));
  let approvedVoices = $derived(voices.filter((v) => v.approved).length);
  let toSort = $derived((cast?.unattached.length ?? 0) + (cast?.unresolved ?? 0));

  let steps = $derived.by<CastStep[]>(() => {
    if (!cast) return [];
    const locked = analysing;
    const r = cast.render;
    const renderTodo = r.chapters.stale + r.chapters.missing;
    return [
      {
        id: 'analyse',
        title: 'Analyse',
        detail: analysing ? `${cast.chapters.done}/${cast.chapters.total} chapters` : 'Done',
        status: analysing ? 'working' : 'done'
      },
      {
        id: 'speakers',
        title: "Who's speaking",
        detail: locked ? 'After analysis' : toSort > 0 ? `${toSort} to sort` : 'All sorted',
        status: locked ? 'locked' : toSort > 0 ? 'todo' : 'done'
      },
      {
        id: 'voices',
        title: 'Voices',
        detail: locked ? 'After analysis' : `${approvedVoices}/${voices.length} approved`,
        status: locked
          ? 'locked'
          : cast.approving
            ? 'working'
            : approvedVoices < voices.length
              ? 'todo'
              : 'done'
      },
      {
        id: 'render',
        title: 'Audiobook',
        detail: locked
          ? 'After analysis'
          : rendering
            ? `Rendering ${r.chapters.done}/${r.chapters.total}`
            : r.audiobook && renderTodo === 0
              ? 'Up to date'
              : r.audiobook
                ? `${renderTodo} chapters to update`
                : 'Not rendered yet',
        status: locked
          ? 'locked'
          : rendering
            ? 'working'
            : r.audiobook && renderTodo === 0
              ? 'done'
              : 'todo'
      }
    ];
  });

  let view = $derived.by<View>(() => {
    if (chosen && !(analysing && ['speakers', 'voices', 'render'].includes(chosen))) return chosen;
    const next = steps.find((s) => s.status !== 'done');
    return (next?.id as View | undefined) ?? 'render';
  });

  let menu = $derived(
    cast && !rendering
      ? [
          !analysing && {
            label: 'Rerun the cast pass',
            icon: 'groups',
            title: RERUN_WARNING.bible,
            onSelect: () => start({ rerun: 'bible' })
          },
          !analysing && {
            label: 'Rerun the whole analysis',
            icon: 'refresh',
            title: RERUN_WARNING.chapters,
            onSelect: () => start({ rerun: 'chapters' })
          },
          {
            label: 'Start over from scratch',
            icon: 'restart_alt',
            danger: true,
            title: REDO_WARNING,
            onSelect: () => start({ fresh: true })
          }
        ]
      : []
  );

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
    if (!polling) return;
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
      chosen = null;
      await load();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to start');
    } finally {
      starting = false;
    }
  }

  function go(v: string) {
    chosen = v as View;
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
    <h1>Full-cast audiobook</h1>
    <span class="spacer"></span>
    {#if menu.length > 0}
      <OverflowMenu items={menu} busy={starting} label="Analysis options" />
    {/if}
  </header>

  {#if !auth.isAdmin}
    <p class="muted">Only an admin can cast a book.</p>
  {:else if loading}
    <p class="muted">Loading…</p>
  {:else if missing}
    <section class="intro">
      <span class="material-icons-outlined hero">record_voice_over</span>
      <h2>Turn this book into a full-cast audiobook</h2>
      <p class="muted">
        Every character gets a voice of their own, with a narrator for everything else.
      </p>
      <ol class="how">
        <li><strong>Analyse</strong> — Kobbler reads the book and works out who says each line.</li>
        <li><strong>Check who's speaking</strong> — sort out any lines it couldn't place.</li>
        <li><strong>Approve voices</strong> — listen to each character and pick a take.</li>
        <li><strong>Render</strong> — the audiobook lands in your library.</li>
      </ol>
      <button type="button" class="btn" disabled={starting} onclick={() => start()}>
        Start the analysis
      </button>
      <p class="muted small">
        Analysis runs in the background and can take a while for a long book.
      </p>
    </section>
  {:else if cast}
    <CastStepper
      {steps}
      extras={analysing
        ? []
        : [
            { id: 'characters', title: 'All characters' },
            { id: 'lines', title: 'Read by chapter' }
          ]}
      active={view}
      onSelect={go}
    />

    {#if cast.state === 'failed' && cast.error}
      <p class="failed">
        <span class="material-icons-outlined">error</span>
        {cast.error}
      </p>
    {/if}

    {#if view === 'analyse'}
      <CastAnalysisStep
        {cast}
        {analysing}
        {starting}
        onResume={() => start()}
        onDone={() => go('speakers')}
      />
    {:else if view === 'speakers'}
      <CastSpeakersStep {bookId} {cast} onChanged={load} onDone={() => go('voices')} />
    {:else if view === 'voices'}
      <CastVoicesStep {bookId} {cast} onChanged={load} onDone={() => go('render')} />
    {:else if view === 'render'}
      <CastRenderPanel {bookId} {cast} onChanged={load} onReview={() => go('voices')} />
    {:else if view === 'characters'}
      <CastCharacterList {bookId} {cast} onChanged={load} />
    {:else}
      <CastChapterLines
        {bookId}
        chapters={cast.chapters.total}
        characters={cast.characters}
        onChanged={load}
      />
    {/if}
  {/if}
</div>

<style>
  .cast-page {
    display: grid;
    gap: 18px;
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

  .spacer {
    flex: 1;
  }

  .intro {
    display: grid;
    justify-items: center;
    gap: 12px;
    padding: 40px 24px;
    text-align: center;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
  }

  .intro h2 {
    margin: 0;
    font-size: 20px;
  }

  .intro p {
    margin: 0;
  }

  .hero {
    font-size: 44px;
    color: var(--accent);
  }

  .how {
    display: grid;
    gap: 8px;
    margin: 8px 0;
    padding-left: 20px;
    text-align: left;
    font-size: 14px;
    max-width: 520px;
  }

  .failed {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    padding: 10px 14px;
    border-radius: 10px;
    border: 1px solid var(--danger);
    color: var(--danger);
    font-size: 13px;
  }

  .muted {
    color: var(--muted);
  }

  .small {
    font-size: 12px;
  }
</style>
