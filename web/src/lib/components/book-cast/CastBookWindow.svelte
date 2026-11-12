<script lang="ts">
  import { tick } from 'svelte';
  import * as api from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import type { BookCastCharacter, BookCastText } from '#lib/types.js';
  import { speakerHue } from './castCharacter.js';
  import { cutQuotes } from './castLines.js';

  interface Props {
    bookId: string;
    characters: BookCastCharacter[];
    /** The line to read, centred in the window. */
    focus: { segment_index: number; span_start: number } | null;
    /** Bumped by the parent after an edit, to read the window again. */
    version?: number;
  }

  let { bookId, characters, focus, version = 0 }: Props = $props();

  const AROUND = 6;
  const STEP = 12;
  const MOST = 200;

  let from = $state(0);
  let to = $state(0);
  let text = $state<BookCastText | null>(null);
  let loading = $state(false);
  let box = $state<HTMLDivElement>();

  let byId = $derived(new Map(characters.map((c) => [c.id, c])));
  let paragraphs = $derived.by(() => {
    if (!text) return [];
    const all = text.paragraphs;
    const bySegment = new Map<number, BookCastText['lines']>();
    for (const l of text.lines)
      bySegment.set(l.segment_index, [...(bySegment.get(l.segment_index) ?? []), l]);
    return all.map((p, i) => ({
      ...p,
      newChapter: i > 0 && p.chapter_index !== all[i - 1].chapter_index,
      pieces: cutQuotes(p.text, bySegment.get(p.segment_index) ?? [])
    }));
  });
  let atStart = $derived(text?.first == null || from <= text.first);
  let atEnd = $derived(text?.last == null || to >= text.last);

  async function load(a: number, b: number, centre: boolean) {
    loading = true;
    try {
      text = await api.getCastText(bookId, a, b);
      from = a;
      to = b;
      if (centre) {
        await tick();
        const mark = box?.querySelector<HTMLElement>('[data-focus]');
        if (box && mark) box.scrollTop = mark.offsetTop - box.clientHeight / 2;
      }
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load the book');
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    void version;
    if (focus) load(Math.max(0, focus.segment_index - AROUND), focus.segment_index + AROUND, true);
  });

  async function earlier() {
    const a = Math.max(text?.first ?? 0, from - STEP);
    const keep = box ? box.scrollHeight - box.scrollTop : 0;
    await load(a, Math.min(to, a + MOST - 1), false);
    await tick();
    if (box) box.scrollTop = box.scrollHeight - keep;
  }

  function later() {
    const b = to + STEP;
    load(Math.max(from, b - MOST + 1), b, false);
  }

  function isFocus(l: { segment_index: number; span_start: number }): boolean {
    return !!focus && l.segment_index === focus.segment_index && l.span_start === focus.span_start;
  }
</script>

<div class="window">
  <div class="scroll" bind:this={box}>
    {#if !focus}
      <p class="muted">Pick a line to read it in the book.</p>
    {:else}
      {#if !atStart}
        <button type="button" class="more" disabled={loading} onclick={earlier}>
          <span class="material-icons-outlined">expand_less</span> Earlier
        </button>
      {/if}
      {#each paragraphs as p (p.segment_index)}
        {#if p.newChapter}<hr />{/if}
        <p class="para">
          {#each p.pieces as piece, i (i)}
            {#if piece.line}
              {@const who = piece.line.character_id ? byId.get(piece.line.character_id) : undefined}
              <span
                class="quote"
                class:focus={isFocus(piece.line)}
                class:narrated={!who}
                data-focus={isFocus(piece.line) || undefined}
                style:--hue={who ? speakerHue(who.id) : null}
                title={who?.name ?? (piece.line.not_speech ? 'Not speech' : 'Narration')}
                >{piece.text}</span
              >{#if who && !who.narrator}<span class="who" style:--hue={speakerHue(who.id)}
                  >{who.name}</span
                >{/if}
            {:else}
              {piece.text}
            {/if}
          {/each}
        </p>
      {/each}
      {#if !atEnd}
        <button type="button" class="more" disabled={loading} onclick={later}>
          <span class="material-icons-outlined">expand_more</span> Later
        </button>
      {/if}
    {/if}
  </div>
</div>

<style>
  .window {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 14px;
    overflow: hidden;
  }

  .scroll {
    position: relative;
    display: grid;
    gap: 12px;
    max-height: min(46vh, 520px);
    overflow-y: auto;
    padding: 16px clamp(14px, 3vw, 32px);
  }

  .para {
    margin: 0;
    font-family: Georgia, 'Iowan Old Style', 'Times New Roman', serif;
    font-size: 15px;
    line-height: 1.8;
  }

  .quote {
    --hue: 0;
    --tint: hsl(var(--hue) 70% 55%);
    padding: 1px 2px;
    border-radius: 4px;
    background: color-mix(in srgb, var(--tint) 14%, transparent);
    box-decoration-break: clone;
    -webkit-box-decoration-break: clone;
  }

  .quote.narrated {
    background: none;
    color: var(--muted);
  }

  .quote.focus {
    background: color-mix(in srgb, var(--accent) 28%, transparent);
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  .who {
    --hue: 0;
    margin-left: 4px;
    font-family: system-ui, sans-serif;
    font-size: 10px;
    padding: 0 6px;
    border-radius: 999px;
    border: 1px solid color-mix(in srgb, hsl(var(--hue) 70% 55%) 60%, var(--ring));
    color: var(--muted);
    vertical-align: 2px;
    white-space: nowrap;
  }

  hr {
    width: 100%;
    border: none;
    border-top: 1px dashed var(--ring);
    margin: 4px 0;
  }

  .more {
    justify-self: center;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font: inherit;
    font-size: 12px;
    color: var(--muted);
    background: none;
    border: none;
    cursor: pointer;
  }

  .more .material-icons-outlined {
    font-size: 16px;
  }

  .muted {
    margin: 0;
    color: var(--muted);
    font-size: 13px;
  }
</style>
