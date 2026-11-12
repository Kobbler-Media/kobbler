<script lang="ts">
  import { SearchSelect } from '@koblibs/ui';
  import * as api from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import type { BookCastCharacter, BookCastCharacterLine, BookCastSuggestion } from '#lib/types.js';
  import CastBookWindow from './CastBookWindow.svelte';
  import { firstPassName } from './castLines.js';
  import { isTyping } from './keys.js';

  interface Props {
    bookId: string;
    character: BookCastCharacter;
    characters: BookCastCharacter[];
    onChanged: () => void;
    onBack: () => void;
  }

  let { bookId, character, characters, onChanged, onBack }: Props = $props();

  const MOST_ASKED = 120;
  const POLL_MS = 3000;

  type Key = { segment_index: number; span_start: number };

  let lines = $state<BookCastCharacterLine[]>([]);
  let loaded = $state(false);
  let busy = $state(false);
  let focusKey = $state<string | null>(null);
  let selected = $state<string[]>([]);
  let moveTo = $state('');
  let moveName = $state('');
  let version = $state(0);
  let suggestions = $state<Record<string, BookCastSuggestion>>({});
  let asking = $state(false);
  let poller: ReturnType<typeof setTimeout> | undefined;

  let speakers = $derived(characters.filter((c) => !c.narrator));
  let byId = $derived(new Map(characters.map((c) => [c.id, c])));
  let items = $derived(
    speakers
      .filter((c) => c.id !== character.id)
      .map((c) => ({
        value: c.id,
        label: c.name,
        description: c.aliases.length > 0 ? c.aliases.join(', ') : undefined
      }))
  );
  let focused = $derived(lines.find((l) => key(l) === focusKey) ?? null);
  let picked = $derived(lines.filter((l) => selected.includes(key(l))));
  let changes = $derived(
    lines
      .map((l) => suggestions[key(l)])
      .filter((s): s is BookCastSuggestion => !!s && changesSpeaker(s))
  );
  let askCount = $derived(picked.length > 0 ? picked.length : lines.length);

  function key(l: Key): string {
    return `${l.segment_index}:${l.span_start}`;
  }

  function ref(l: Key): Key {
    return { segment_index: l.segment_index, span_start: l.span_start };
  }

  function changesSpeaker(s: BookCastSuggestion): boolean {
    return s.speaker !== 'unknown' && s.character_id !== character.id;
  }

  function existing(name: string): BookCastCharacter | undefined {
    const n = name.trim().toLowerCase();
    return speakers.find((c) => [c.name, ...c.aliases].some((x) => x.toLowerCase() === n));
  }

  async function load() {
    try {
      lines = (await api.getCastCharacterLines(bookId, character.id)).lines;
      if (!lines.some((l) => key(l) === focusKey)) focusKey = lines[0] ? key(lines[0]) : null;
      selected = selected.filter((k) => lines.some((l) => key(l) === k));
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load the lines');
    } finally {
      loaded = true;
    }
  }

  $effect(() => {
    void character.id;
    load();
  });

  $effect(() => () => clearTimeout(poller));

  async function act(work: () => Promise<unknown>, done?: string) {
    busy = true;
    try {
      await work();
      if (done) showMessage(done);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed');
    } finally {
      busy = false;
      onChanged();
      version++;
      await load();
    }
  }

  function reassign(l: Key, to: api.CastLineSpeaker) {
    act(() => api.reassignCastLine(bookId, l.segment_index, l.span_start, to));
  }

  /** Each line to `to`, a character's id, or to a new character of that name. */
  async function move(keys: Key[], to: { id: string } | { name: string }) {
    if ('id' in to) {
      for (const l of keys)
        await api.reassignCastLine(bookId, l.segment_index, l.span_start, to.id);
      return;
    }
    const known = existing(to.name);
    if (known) return move(keys, { id: known.id });
    await api.splitCastCharacter(bookId, character.id, { name: to.name, aliases: [], lines: keys });
  }

  function moveSelected() {
    const keys = picked.map(ref);
    const to = moveTo ? { id: moveTo } : { name: moveName.trim() };
    act(async () => {
      await move(keys, to);
      moveTo = '';
      moveName = '';
      selected = [];
    }, `Moved ${keys.length} line(s)`);
  }

  function target(s: BookCastSuggestion): { id: string } | { name: string } {
    return s.character_id ? { id: s.character_id } : { name: s.speaker };
  }

  function dismiss(s: BookCastSuggestion) {
    const rest = { ...suggestions };
    delete rest[key(s)];
    suggestions = rest;
  }

  function accept(s: BookCastSuggestion) {
    act(async () => {
      await move([ref(s)], target(s));
      dismiss(s);
    });
  }

  function acceptAll() {
    const todo = [...changes];
    act(async () => {
      const groups = new Map<string, BookCastSuggestion[]>();
      for (const s of todo) {
        const k = s.character_id ?? `new:${s.speaker.toLowerCase()}`;
        groups.set(k, [...(groups.get(k) ?? []), s]);
      }
      for (const group of groups.values()) {
        await move(group.map(ref), target(group[0]));
        for (const s of group) dismiss(s);
      }
    }, `Moved ${todo.length} line(s)`);
  }

  async function poll(jobId: string) {
    try {
      const job = await api.getJob(jobId);
      if (job.status === 'pending' || job.status === 'running') {
        poller = setTimeout(() => poll(jobId), POLL_MS);
        return;
      }
      asking = false;
      if (job.status !== 'completed') {
        showMessage(job.error ?? 'The model could not work it out');
        return;
      }
      const got = (job.result?.suggestions ?? []) as BookCastSuggestion[];
      suggestions = { ...suggestions, ...Object.fromEntries(got.map((s) => [key(s), s])) };
      const n = got.filter(changesSpeaker).length;
      showMessage(
        n > 0 ? `${n} line(s) look like someone else` : 'The model agrees with every line'
      );
    } catch (e) {
      asking = false;
      showMessage(e instanceof Error ? e.message : 'Lost track of the model');
    }
  }

  async function ask() {
    asking = true;
    try {
      const { job_id } = await api.suggestCastSpeakers(bookId, character.id, picked.map(ref));
      poll(job_id);
    } catch (e) {
      asking = false;
      showMessage(e instanceof Error ? e.message : 'Failed to ask the model');
    }
  }

  function toggle(l: Key) {
    const k = key(l);
    selected = selected.includes(k) ? selected.filter((x) => x !== k) : [...selected, k];
  }

  function step(delta: number) {
    if (lines.length === 0) return;
    const at = lines.findIndex((l) => key(l) === focusKey);
    const next = Math.min(Math.max(at + delta, 0), lines.length - 1);
    focusKey = key(lines[next]);
    document.querySelector(`[data-line="${focusKey}"]`)?.scrollIntoView({ block: 'nearest' });
  }

  function onKey(e: KeyboardEvent) {
    if (busy || isTyping(e)) return;
    if (e.key === 'ArrowDown' || e.key === 'j') step(1);
    else if (e.key === 'ArrowUp' || e.key === 'k') step(-1);
    else if (e.key === ' ' && focused) toggle(focused);
    else if (e.key === 'Enter' && focused && suggestions[key(focused)]) {
      const s = suggestions[key(focused)];
      if (changesSpeaker(s)) accept(s);
    } else return;
    e.preventDefault();
  }
</script>

<svelte:window onkeydown={onKey} />

<section class="reading">
  <header class="bar">
    <button type="button" class="btn alt small" onclick={onBack}>
      <span class="material-icons-outlined">arrow_back</span> All characters
    </button>
    <h2>{character.name}</h2>
    <span class="muted">{lines.length} {lines.length === 1 ? 'line' : 'lines'}</span>
    <span class="spacer"></span>
    <button
      type="button"
      class="btn small"
      disabled={busy || asking || askCount === 0 || askCount > MOST_ASKED}
      title={askCount > MOST_ASKED
        ? `Select at most ${MOST_ASKED} lines to ask about`
        : 'The model reads each line in its scene and says who it thinks speaks it'}
      onclick={ask}
    >
      <span class="material-icons-outlined">psychology</span>
      {asking
        ? 'Working it out…'
        : picked.length > 0
          ? `Ask the model about ${picked.length}`
          : 'Ask the model who says these'}
    </button>
  </header>

  <div class="window">
    <CastBookWindow {bookId} {characters} focus={focused ? ref(focused) : null} {version} />
  </div>

  {#if changes.length > 0}
    <div class="banner">
      <span class="material-icons-outlined">psychology</span>
      <span>The model thinks {changes.length} line(s) are someone else.</span>
      <span class="spacer"></span>
      <button type="button" class="btn small" disabled={busy} onclick={acceptAll}>
        Accept all
      </button>
      <button type="button" class="btn alt small" onclick={() => (suggestions = {})}>
        Dismiss
      </button>
    </div>
  {/if}

  {#if picked.length > 0}
    <div class="move">
      <span>Move {picked.length} line(s) to</span>
      <div class="pick">
        <SearchSelect
          {items}
          bind:value={moveTo}
          placeholder="Someone in the cast…"
          disabled={busy}
        />
      </div>
      {#if !moveTo}
        <span class="muted">or</span>
        <input bind:value={moveName} placeholder="A new character's name" />
      {/if}
      <button
        type="button"
        class="btn small"
        disabled={busy || (!moveTo && !moveName.trim())}
        onclick={moveSelected}
      >
        Move
      </button>
      <button type="button" class="btn alt small" onclick={() => (selected = [])}>Clear</button>
    </div>
  {/if}

  {#if !loaded}
    <p class="muted">Loading…</p>
  {:else if lines.length === 0}
    <p class="muted">{character.name} has no lines left.</p>
  {:else}
    <ol class="list">
      {#each lines as l (key(l))}
        {@const s = suggestions[key(l)]}
        {@const guess = firstPassName(l, character)}
        <li
          class="line"
          class:focus={key(l) === focusKey}
          class:picked={selected.includes(key(l))}
          data-line={key(l)}
        >
          <input
            type="checkbox"
            aria-label="Select line"
            checked={selected.includes(key(l))}
            onchange={() => toggle(l)}
          />
          <button type="button" class="text" onclick={() => (focusKey = key(l))}>
            <span class="where"
              >Ch. {l.chapter_index + 1}{l.chapter_title ? ` · ${l.chapter_title}` : ''}</span
            >
            <span class="quote">“{l.text ?? ''}”</span>
            {#if guess || l.source === 'user'}
              <span class="where">
                {#if guess}First guess: {guess}{/if}
                {#if l.source === 'user'}{guess ? ' · ' : ''}set by hand{/if}
              </span>
            {/if}
          </button>
          {#if s}
            <span class="suggestion" class:agrees={!changesSpeaker(s)} title={s.reason}>
              <span class="material-icons-outlined">psychology</span>
              {#if !changesSpeaker(s)}
                {s.speaker === 'unknown' ? "Can't tell" : 'Agrees'}
              {:else}
                {s.character_id ? (byId.get(s.character_id)?.name ?? s.speaker) : s.speaker}
                {#if !s.character_id}<em>new</em>{/if}
                <button
                  type="button"
                  class="icon"
                  disabled={busy}
                  aria-label="Accept"
                  onclick={() => accept(s)}
                >
                  <span class="material-icons-outlined">check</span>
                </button>
              {/if}
              <button type="button" class="icon" aria-label="Dismiss" onclick={() => dismiss(s)}>
                <span class="material-icons-outlined">close</span>
              </button>
            </span>
          {/if}
          <select
            class="speaker"
            aria-label="Speaker"
            value={character.id}
            disabled={busy}
            onchange={(e) => reassign(l, e.currentTarget.value || null)}
          >
            <option value="">Narration</option>
            <option value="not_speech">Not speech</option>
            {#each speakers as c (c.id)}
              <option value={c.id}>{c.name}</option>
            {/each}
          </select>
        </li>
      {/each}
    </ol>
    <p class="keys">
      Keys: <kbd>↑</kbd>/<kbd>↓</kbd> move · <kbd>Space</kbd> select · <kbd>↵</kbd> accept the suggestion
    </p>
  {/if}
</section>

<style>
  .reading {
    display: grid;
    gap: 12px;
  }

  .bar,
  .banner,
  .move {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .bar .btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }

  h2 {
    margin: 0;
    font-size: 18px;
  }

  .spacer {
    flex: 1;
  }

  .window {
    position: sticky;
    top: 0;
    z-index: 2;
    background: var(--bg);
    padding-bottom: 4px;
  }

  .banner,
  .move {
    padding: 10px 14px;
    border: 1px solid var(--accent);
    border-radius: 10px;
    font-size: 13px;
  }

  .pick {
    min-width: 220px;
  }

  input:not([type='checkbox']),
  select {
    font: inherit;
    font-size: 13px;
    color: var(--ink);
  }

  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 4px;
  }

  .line {
    display: grid;
    grid-template-columns: auto 1fr auto auto;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    border: 1px solid transparent;
    border-radius: 10px;
    background: var(--card);
  }

  .line.focus {
    border-color: var(--accent);
  }

  .line.picked {
    background: color-mix(in srgb, var(--accent) 10%, var(--card));
  }

  .text {
    display: grid;
    gap: 2px;
    min-width: 0;
    font: inherit;
    text-align: left;
    color: var(--ink);
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
  }

  .quote {
    font-family: Georgia, 'Iowan Old Style', 'Times New Roman', serif;
    font-size: 15px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .where,
  .muted,
  .keys {
    font-size: 12px;
    color: var(--muted);
  }

  .suggestion {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    padding: 2px 4px 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--accent);
    white-space: nowrap;
  }

  .suggestion.agrees {
    border-color: var(--ring);
    color: var(--muted);
  }

  .suggestion .material-icons-outlined {
    font-size: 15px;
  }

  .icon {
    display: inline-flex;
    padding: 2px;
    border: none;
    background: none;
    color: inherit;
    cursor: pointer;
    border-radius: 50%;
  }

  .icon:hover {
    background: var(--bg-2);
  }

  .speaker {
    max-width: 160px;
  }

  kbd {
    font-family: ui-monospace, monospace;
    font-size: 10px;
    border: 1px solid var(--ring);
    border-radius: 4px;
    padding: 0 4px;
  }

  @media (max-width: 640px) {
    .line {
      grid-template-columns: auto 1fr;
    }

    .suggestion,
    .speaker {
      grid-column: 2;
      justify-self: start;
    }
  }
</style>
