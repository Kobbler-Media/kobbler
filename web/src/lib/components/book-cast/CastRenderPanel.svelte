<script lang="ts">
  import * as api from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import type { BookCast } from '#lib/types.js';
  import { formatDuration } from '#lib/utils/format.js';

  interface Props {
    bookId: string;
    cast: BookCast;
    onChanged: () => void;
    onReview: () => void;
  }

  let { bookId, cast, onChanged, onReview }: Props = $props();

  let starting = $state(false);
  let render = $derived(cast.render);
  let rendering = $derived(cast.state === 'rendering');
  let ready = $derived(render.unapproved.length === 0);
  let todo = $derived(render.chapters.stale + render.chapters.missing);
  let percent = $derived(
    render.chapters.total > 0 ? Math.round((render.chapters.done / render.chapters.total) * 100) : 0
  );
  let staleVoices = $derived.by(() => {
    const counts = new Map<string, number>();
    for (const s of render.stale) {
      for (const v of s.voices) {
        const name = v.name ?? 'a removed character';
        counts.set(name, (counts.get(name) ?? 0) + 1);
      }
    }
    return [...counts].sort((a, b) => b[1] - a[1]);
  });
  let reattributed = $derived(render.stale.filter((s) => s.attribution).length);

  function chapters(n: number): string {
    return n === 1 ? '1 chapter' : `${n} chapters`;
  }

  async function act(work: () => Promise<unknown>, done: string, failed: string) {
    starting = true;
    try {
      await work();
      showMessage(done);
      onChanged();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : failed);
    } finally {
      starting = false;
    }
  }
</script>

<section class="panel render">
  <div class="row">
    <h2>Render the audiobook</h2>
    <span class="spacer"></span>
    <span class="progress-text">
      {render.chapters.done}/{render.chapters.total} chapters up to date
    </span>
    {#if !rendering}
      <button
        type="button"
        class="btn small"
        disabled={starting || !ready || (todo === 0 && render.audiobook !== null)}
        onclick={() =>
          act(
            () => api.startBookCastRender(bookId),
            'Rendering started',
            'Failed to start rendering'
          )}
      >
        {todo > 0 && render.chapters.done > 0 ? `Render ${chapters(todo)}` : 'Render'}
      </button>
    {/if}
  </div>

  {#if rendering || render.chapters.done > 0}
    <div
      class="bar"
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent}
    >
      <span style:width="{percent}%"></span>
    </div>
  {/if}

  {#if todo > 0 && render.eta_seconds !== null}
    <p class="muted">
      About {formatDuration(render.eta_seconds)} of rendering left for {chapters(todo)}.
    </p>
  {/if}

  {#if render.chapters.stale > 0}
    <p class="muted">
      {chapters(render.chapters.stale)} changed since rendered:
      {#if reattributed > 0}
        lines reattributed in {chapters(reattributed)}{staleVoices.length > 0 ? ';' : '.'}
      {/if}
      {#if staleVoices.length > 0}
        new voices for {staleVoices.map(([name, n]) => `${name} (${chapters(n)})`).join(', ')}.
      {/if}
    </p>
  {/if}

  {#if render.audiobook}
    <a class="btn alt small link" href="/library/books/{render.audiobook.book_id}">
      <span class="material-icons-outlined">headphones</span>
      Open the audiobook
    </a>
  {/if}

  {#if !ready}
    <div class="blocked">
      <span class="material-icons-outlined">lock</span>
      <p>
        {render.unapproved.length === 1
          ? '1 voice still needs'
          : `${render.unapproved.length} voices still need`} approving before the book can be rendered.
      </p>
      <span class="spacer"></span>
      <button type="button" class="btn alt small" onclick={onReview}>Review voices</button>
    </div>
  {/if}
</section>

<style>
  .render {
    display: grid;
    gap: 10px;
    padding: 14px 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 14px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 12px;
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

  .progress-text {
    font-size: 13px;
  }

  .bar {
    height: 6px;
    border-radius: 999px;
    background: var(--ring);
    overflow: hidden;
  }

  .bar span {
    display: block;
    height: 100%;
    background: var(--accent);
  }

  .link {
    justify-self: start;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    text-decoration: none;
  }

  .blocked {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    border-radius: 10px;
    background: var(--bg-2);
    font-size: 13px;
  }

  .blocked p {
    margin: 0;
  }

  .blocked .material-icons-outlined {
    font-size: 18px;
    color: var(--muted);
  }

  .muted {
    margin: 0;
    color: var(--muted);
    font-size: 13px;
  }
</style>
