<script lang="ts">
  import * as api from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import type { BookCastCharacter, BookCastLine } from '#lib/types.js';

  interface Props {
    bookId: string;
    chapters: number;
    characters: BookCastCharacter[];
    onChanged: () => void;
  }

  let { bookId, chapters, characters, onChanged }: Props = $props();

  let chapter = $state(0);
  let lines = $state<BookCastLine[]>([]);
  let loading = $state(false);
  let selected = $state<string[]>([]);
  let splitName = $state('');
  let busy = $state(false);

  let byId = $derived(new Map(characters.map((c) => [c.id, c])));
  let speakers = $derived(characters.filter((c) => !c.narrator));
  let picked = $derived(lines.filter((l) => selected.includes(key(l))));
  let splitFrom = $derived.by(() => {
    const owners = new Set(picked.map((l) => l.character_id));
    const [only] = owners;
    return owners.size === 1 && only ? byId.get(only) : undefined;
  });

  function key(l: BookCastLine): string {
    return `${l.segment_index}:${l.span_start}`;
  }

  async function load(n: number) {
    loading = true;
    try {
      lines = (await api.getBookCastChapter(bookId, n)).lines;
      selected = [];
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load lines');
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    load(chapter);
  });

  async function act(work: () => Promise<unknown>, done?: string) {
    busy = true;
    try {
      await work();
      if (done) showMessage(done);
      await load(chapter);
      onChanged();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed');
    } finally {
      busy = false;
    }
  }

  function voiceName(l: BookCastLine): string {
    const v = l.voice;
    if (!v) return 'no voice yet';
    const who = byId.get(v.character_id)?.name ?? '?';
    return v.from_chapter > 0 ? `${who} (from ch. ${v.from_chapter + 1})` : who;
  }
</script>

<div class="lines">
  <div class="toolbar">
    <label class="field">
      <span>Chapter</span>
      <select bind:value={chapter}>
        {#each Array.from({ length: chapters }, (_, i) => i) as n (n)}
          <option value={n}>{n + 1}</option>
        {/each}
      </select>
    </label>
    {#if splitFrom}
      <label class="field grow">
        <span>Split {picked.length} line(s) from {splitFrom.name} into</span>
        <input bind:value={splitName} placeholder="New character's name" />
      </label>
      <button
        type="button"
        class="btn small"
        disabled={busy || !splitName.trim()}
        onclick={() =>
          act(async () => {
            await api.splitCastCharacter(bookId, splitFrom!.id, {
              name: splitName.trim(),
              aliases: [],
              lines: picked.map((l) => ({
                segment_index: l.segment_index,
                span_start: l.span_start
              }))
            });
            splitName = '';
          }, 'Split')}
      >
        Split
      </button>
    {/if}
  </div>

  {#if loading}
    <p class="muted">Loading…</p>
  {:else if lines.length === 0}
    <p class="muted">No dialogue in this chapter.</p>
  {:else}
    <table>
      <thead>
        <tr>
          <th></th>
          <th>Line</th>
          <th>Speaker</th>
          <th>Read by</th>
        </tr>
      </thead>
      <tbody>
        {#each lines as l (key(l))}
          <tr class:user={l.source === 'user'}>
            <td>
              <input
                type="checkbox"
                aria-label="Select line"
                disabled={!l.character_id}
                bind:group={selected}
                value={key(l)}
              />
            </td>
            <td class="text">
              “{l.text ?? ''}”
              {#if l.cue}<span class="cue">{l.cue}</span>{/if}
            </td>
            <td>
              <select
                aria-label="Speaker"
                value={l.character_id ?? ''}
                disabled={busy}
                onchange={(e) =>
                  act(() =>
                    api.reassignCastLine(
                      bookId,
                      l.segment_index,
                      l.span_start,
                      e.currentTarget.value || null
                    )
                  )}
              >
                <option value=""
                  >Narration{l.speaker && !l.character_id ? ` (“${l.speaker}”)` : ''}</option
                >
                {#each speakers as c (c.id)}
                  <option value={c.id}>{c.name}</option>
                {/each}
              </select>
              <span class="source">{l.source}</span>
            </td>
            <td class="muted">{voiceName(l)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .lines {
    display: grid;
    gap: 12px;
  }

  .toolbar {
    display: flex;
    gap: 12px;
    align-items: flex-end;
    flex-wrap: wrap;
  }

  .field {
    display: grid;
    gap: 4px;
    font-size: 12px;
    color: var(--muted);
  }

  .field select,
  .field input,
  td select {
    font: inherit;
    font-size: 13px;
    color: var(--ink);
  }

  .grow {
    flex: 1;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }

  th {
    text-align: left;
    font-size: 11px;
    color: var(--muted);
    font-weight: 500;
    padding: 4px 6px;
  }

  td {
    padding: 6px;
    border-top: 1px solid var(--ring);
    vertical-align: top;
  }

  tr.user td {
    background: color-mix(in srgb, var(--ring) 30%, transparent);
  }

  .text {
    width: 55%;
  }

  .cue {
    display: inline-block;
    margin-left: 6px;
    font-size: 11px;
    color: var(--muted);
    font-style: italic;
  }

  .source {
    display: block;
    font-size: 10px;
    color: var(--muted);
  }

  .muted {
    color: var(--muted);
    font-size: 12px;
  }
</style>
