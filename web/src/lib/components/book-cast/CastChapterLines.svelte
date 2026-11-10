<script lang="ts">
  import * as api from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import type { BookCastCharacter, BookCastLine } from '#lib/types.js';
  import { speakerHue } from './castCharacter.js';
  import { firstPassName, paragraphs } from './castLines.js';

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
  let script = $derived(paragraphs(lines));
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

  function said(l: BookCastLine): api.CastLineSpeaker {
    return l.not_speech ? 'not_speech' : l.character_id;
  }

  function toggle(l: BookCastLine) {
    if (!l.character_id) return;
    const k = key(l);
    selected = selected.includes(k) ? selected.filter((x) => x !== k) : [...selected, k];
  }

  function details(l: BookCastLine): string {
    const character = l.character_id ? byId.get(l.character_id) : undefined;
    const parts: string[] = [];
    const guess = firstPassName(l, character);
    if (guess) parts.push(`First guess: ${guess}`);
    if (l.source === 'user') parts.push('Set by hand');
    if (l.cue) parts.push(`Delivered ${l.cue}`);
    const v = l.voice;
    if (v) {
      const who = byId.get(v.character_id)?.name ?? '?';
      parts.push(`Read by ${who}${v.from_chapter > 0 ? ` (from ch. ${v.from_chapter + 1})` : ''}`);
    }
    return parts.join(' · ');
  }
</script>

<div class="lines">
  <div class="toolbar">
    <button
      type="button"
      class="btn alt small"
      disabled={chapter === 0}
      onclick={() => chapter--}
      aria-label="Previous chapter"
    >
      <span class="material-icons-outlined">chevron_left</span>
    </button>
    <label class="chapter">
      <span>Chapter</span>
      <select bind:value={chapter}>
        {#each Array.from({ length: chapters }, (_, i) => i) as n (n)}
          <option value={n}>{n + 1}</option>
        {/each}
      </select>
      <span class="of">of {chapters}</span>
    </label>
    <button
      type="button"
      class="btn alt small"
      disabled={chapter >= chapters - 1}
      onclick={() => chapter++}
      aria-label="Next chapter"
    >
      <span class="material-icons-outlined">chevron_right</span>
    </button>
    <span class="spacer"></span>
    <span class="hint">Click lines to select them for a split</span>
  </div>

  {#if picked.length > 0}
    <div class="split">
      {#if splitFrom}
        <span>Split {picked.length} line(s) from <strong>{splitFrom.name}</strong> into</span>
        <input bind:value={splitName} placeholder="New character's name" />
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
      {:else}
        <span>Pick lines from a single character to split them off.</span>
      {/if}
      <button type="button" class="btn alt small" onclick={() => (selected = [])}>Clear</button>
    </div>
  {/if}

  {#if loading}
    <p class="muted">Loading…</p>
  {:else if script.length === 0}
    <p class="muted">No dialogue in this chapter.</p>
  {:else}
    <div class="script">
      {#each script as p (p.segment_index)}
        <p class="para">
          {#each p.pieces as piece, i (i)}
            {#if piece.line}
              {@const l = piece.line}
              {@const who = l.character_id ? byId.get(l.character_id) : undefined}
              <span
                class="line"
                class:not-speech={l.not_speech}
                class:unassigned={!who && !l.not_speech}
                class:selected={selected.includes(key(l))}
                style:--hue={who ? speakerHue(who.id) : null}
              >
                <button
                  type="button"
                  class="quote"
                  title={details(l)}
                  disabled={!l.character_id}
                  onclick={() => toggle(l)}>{piece.text}</button
                ><select
                  class="tag"
                  aria-label="Speaker"
                  value={said(l) ?? ''}
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
                  <option value="">Narration</option>
                  <option value="not_speech">Not speech</option>
                  {#each speakers as c (c.id)}
                    <option value={c.id}>{c.name}</option>
                  {/each}
                </select>{#if l.source === 'user'}<span
                    class="material-icons-outlined by-hand"
                    title="Set by hand">edit</span
                  >{/if}
              </span>
            {:else}
              {piece.text}
            {/if}
          {/each}
        </p>
      {/each}
    </div>
  {/if}
</div>

<style>
  .lines {
    display: grid;
    gap: 12px;
  }

  .toolbar,
  .split {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
  }

  .toolbar .btn {
    display: inline-flex;
    align-items: center;
  }

  .chapter {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
  }

  .of,
  .hint {
    font-size: 12px;
    color: var(--muted);
  }

  select,
  input {
    font: inherit;
    font-size: 13px;
    color: var(--ink);
  }

  .spacer {
    flex: 1;
  }

  .split {
    padding: 10px 14px;
    border: 1px solid var(--accent);
    border-radius: 10px;
    font-size: 13px;
  }

  .script {
    display: grid;
    gap: 14px;
    padding: 20px clamp(16px, 4vw, 40px);
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 14px;
    max-width: 760px;
  }

  .para {
    margin: 0;
    font-family: Georgia, 'Iowan Old Style', 'Times New Roman', serif;
    font-size: 16px;
    line-height: 1.9;
  }

  .line {
    --hue: 0;
    --tint: hsl(var(--hue) 70% 55%);
  }

  .quote {
    font: inherit;
    color: var(--ink);
    padding: 1px 3px;
    border: none;
    border-radius: 4px;
    background: color-mix(in srgb, var(--tint) 18%, transparent);
    box-shadow: inset 0 -2px 0 var(--tint);
    cursor: pointer;
    text-align: left;
    box-decoration-break: clone;
    -webkit-box-decoration-break: clone;
  }

  .quote:disabled {
    cursor: default;
  }

  .unassigned .quote {
    background: color-mix(in srgb, var(--warning) 20%, transparent);
    box-shadow: inset 0 -2px 0 var(--warning);
  }

  .not-speech .quote {
    background: none;
    box-shadow: none;
    color: var(--muted);
  }

  .selected .quote {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  .tag {
    margin-left: 4px;
    font-family: system-ui, sans-serif;
    font-size: 11px;
    padding: 0 2px;
    border-radius: 999px;
    border: 1px solid color-mix(in srgb, var(--tint) 60%, var(--ring));
    background: var(--bg-2);
    vertical-align: 2px;
    max-width: 140px;
  }

  .unassigned .tag {
    border-color: var(--warning);
  }

  .by-hand {
    font-size: 13px;
    color: var(--muted);
    vertical-align: -2px;
    margin-left: 2px;
  }

  .muted {
    color: var(--muted);
    font-size: 13px;
  }
</style>
