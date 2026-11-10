<script lang="ts">
  import * as api from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import type { BookCast, BookCastUnattributedLine } from '#lib/types.js';
  import CastLineContext from './CastLineContext.svelte';
  import CastSpeakerPicker from './CastSpeakerPicker.svelte';
  import { suggestSpeakers } from './castReview.js';
  import { isTyping } from './keys.js';

  interface Props {
    bookId: string;
    cast: BookCast;
    onChanged: () => void;
    onDone: () => void;
  }

  let { bookId, cast, onChanged, onDone }: Props = $props();

  type Item =
    | { kind: 'speaker'; key: string; speaker: string; count: number }
    | { kind: 'line'; key: string; line: BookCastUnattributedLine };

  let lines = $state<BookCastUnattributedLine[]>([]);
  let loaded = $state(false);
  let index = $state(0);
  let busy = $state(false);
  let expanded = $state(false);
  let attachTo = $state('');

  let items = $derived<Item[]>([
    ...cast.unattached.map((u) => ({
      kind: 'speaker' as const,
      key: `s:${u.speaker}`,
      speaker: u.speaker,
      count: u.lines
    })),
    ...lines.map((l) => ({ kind: 'line' as const, key: `l:${lineKey(l)}`, line: l }))
  ]);
  let at = $derived(Math.min(index, Math.max(items.length - 1, 0)));
  let item = $derived(items[at]);
  let examples = $derived(
    item?.kind === 'speaker'
      ? lines.filter((l) => l.speaker?.toLowerCase() === item.speaker.toLowerCase()).slice(0, 3)
      : []
  );
  let suggestions = $derived.by(() => {
    if (!item) return [];
    if (item.kind === 'speaker') {
      return suggestSpeakers(cast.characters, {
        speaker: item.speaker,
        text: examples.map((l) => surrounding(l)).join(' ')
      });
    }
    return suggestSpeakers(cast.characters, {
      speaker: item.line.speaker,
      text: surrounding(item.line)
    });
  });

  function lineKey(l: BookCastUnattributedLine): string {
    return `${l.segment_index}:${l.span_start}`;
  }

  function surrounding(l: BookCastUnattributedLine): string {
    const c = l.context;
    if (!c) return l.text ?? '';
    return [c.before, c.pre, c.quote, c.post, c.after].filter(Boolean).join(' ');
  }

  async function loadLines() {
    try {
      lines = (await api.getUnattributedCastLines(bookId)).lines;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load the lines');
    } finally {
      loaded = true;
    }
  }

  let unresolved = $derived(cast.unresolved + cast.unattached.length);
  let itemKey = $derived(item?.key);

  $effect(() => {
    void unresolved;
    loadLines();
  });

  $effect(() => {
    void itemKey;
    expanded = false;
    attachTo = '';
  });

  async function resolve(work: () => Promise<unknown>, after: () => void) {
    busy = true;
    try {
      await work();
      after();
      onChanged();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed');
    } finally {
      busy = false;
    }
  }

  function assign(to: api.CastLineSpeaker) {
    if (item?.kind !== 'line') return;
    const l = item.line;
    resolve(
      () => api.reassignCastLine(bookId, l.segment_index, l.span_start, to),
      () => (lines = lines.filter((x) => lineKey(x) !== lineKey(l)))
    );
  }

  function attach(characterId: string) {
    if (item?.kind !== 'speaker') return;
    const speaker = item.speaker;
    resolve(
      async () => {
        const res = await api.attachCastSpeaker(bookId, speaker, characterId);
        showMessage(`“${speaker}” attached — ${res.attached} line(s)`);
      },
      () => {}
    );
  }

  function pick(characterId: string) {
    if (item?.kind === 'speaker') attachTo = characterId;
    else assign(characterId);
  }

  function go(delta: number) {
    if (items.length === 0) return;
    index = (at + delta + items.length) % items.length;
  }

  function onKey(e: KeyboardEvent) {
    if (busy || !item || isTyping(e)) return;
    const n = Number(e.key);
    if (n >= 1 && n <= 9 && suggestions[n - 1]) pick(suggestions[n - 1].id);
    else if (e.key === 'ArrowRight' || e.key === 's') go(1);
    else if (e.key === 'ArrowLeft') go(-1);
    else if (item.kind === 'line' && e.key === 'n') assign(null);
    else if (item.kind === 'line' && e.key === 'x') assign('not_speech');
    else if (e.key === 'c') expanded = !expanded;
    else if (e.key === 'Enter' && attachTo) attach(attachTo);
    else return;
    e.preventDefault();
  }
</script>

<svelte:window onkeydown={onKey} />

<section class="step">
  {#if !loaded}
    <p class="muted">Loading…</p>
  {:else if !item}
    <div class="done">
      <span class="material-icons-outlined">task_alt</span>
      <h2>Every line has a speaker</h2>
      <p class="muted">Nothing left to sort out here.</p>
      <button type="button" class="btn" onclick={onDone}>Next: voices</button>
    </div>
  {:else}
    <header class="bar">
      <span class="count">{at + 1} of {items.length} to sort</span>
      <span class="spacer"></span>
      <button type="button" class="btn alt small" onclick={() => go(-1)} aria-label="Previous">
        <span class="material-icons-outlined">chevron_left</span>
      </button>
      <button type="button" class="btn alt small" onclick={() => go(1)}>
        Skip <span class="material-icons-outlined">chevron_right</span>
      </button>
    </header>

    {#if item.kind === 'speaker'}
      <div class="card">
        <p class="eyebrow">Unknown speaker</p>
        <h2>“{item.speaker}”</h2>
        <p class="muted">
          Says {item.count === 1 ? '1 line' : `${item.count} lines`} but isn't anyone in the cast. Who
          is it?
        </p>
        {#each examples as l (lineKey(l))}
          <div class="example">
            <span class="where">Chapter {l.chapter_index + 1}</span>
            <CastLineContext context={l.context} fallback={l.text} />
          </div>
        {/each}
      </div>
      <CastSpeakerPicker characters={cast.characters} {suggestions} disabled={busy} onPick={pick} />
      {#if attachTo}
        {@const target = cast.characters.find((c) => c.id === attachTo)}
        <div class="confirm">
          <span>Attach every “{item.speaker}” line to <strong>{target?.name}</strong>?</span>
          <button type="button" class="btn small" disabled={busy} onclick={() => attach(attachTo)}>
            <kbd>↵</kbd> Attach
          </button>
          <button type="button" class="btn alt small" onclick={() => (attachTo = '')}>Cancel</button
          >
        </div>
      {/if}
    {:else}
      {@const l = item.line}
      <div class="card">
        <p class="eyebrow">
          Chapter {l.chapter_index + 1}{l.chapter_title ? ` · ${l.chapter_title}` : ''}
        </p>
        <CastLineContext context={l.context} fallback={l.text} bind:expanded />
        {#if l.speaker || l.cue}
          <p class="hints">
            {#if l.speaker}<span>First guess: <strong>{l.speaker}</strong></span>{/if}
            {#if l.cue}<span>Delivered: <em>{l.cue}</em></span>{/if}
          </p>
        {/if}
      </div>
      <p class="ask">Who says this?</p>
      <CastSpeakerPicker characters={cast.characters} {suggestions} disabled={busy} onPick={pick} />
      <div class="row">
        <button type="button" class="btn alt small" disabled={busy} onclick={() => assign(null)}>
          <kbd>N</kbd> Narration
        </button>
        <button
          type="button"
          class="btn alt small"
          disabled={busy}
          onclick={() => assign('not_speech')}
        >
          <kbd>X</kbd> Not speech
        </button>
      </div>
    {/if}
    <p class="keys">
      Keys: <kbd>1</kbd>–<kbd>9</kbd> pick · <kbd>←</kbd>/<kbd>→</kbd> move · <kbd>C</kbd> context
    </p>
  {/if}
</section>

<style>
  .step {
    display: grid;
    gap: 14px;
  }

  .bar,
  .row,
  .confirm {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .bar .btn {
    display: inline-flex;
    align-items: center;
  }

  .count {
    font-size: 13px;
    color: var(--muted);
  }

  .spacer {
    flex: 1;
  }

  .card {
    display: grid;
    gap: 10px;
    padding: 18px 20px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 14px;
  }

  .eyebrow {
    margin: 0;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--muted);
  }

  h2 {
    margin: 0;
    font-size: 20px;
  }

  .example {
    display: grid;
    gap: 4px;
    padding-top: 10px;
    border-top: 1px solid var(--ring);
  }

  .where {
    font-size: 11px;
    color: var(--muted);
  }

  .hints {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
    margin: 0;
    font-size: 12px;
    color: var(--muted);
  }

  .hints strong,
  .hints em {
    color: var(--ink);
  }

  .ask {
    margin: 0;
    font-weight: 600;
  }

  .confirm {
    padding: 10px 14px;
    border: 1px solid var(--accent);
    border-radius: 10px;
    font-size: 13px;
  }

  .done {
    display: grid;
    justify-items: center;
    gap: 8px;
    padding: 40px 16px;
    text-align: center;
  }

  .done .material-icons-outlined {
    font-size: 40px;
    color: var(--success);
  }

  .muted {
    margin: 0;
    color: var(--muted);
    font-size: 13px;
  }

  .keys {
    margin: 0;
    font-size: 11px;
    color: var(--muted);
  }

  kbd {
    font-family: ui-monospace, monospace;
    font-size: 10px;
    border: 1px solid var(--ring);
    border-radius: 4px;
    padding: 0 4px;
  }
</style>
