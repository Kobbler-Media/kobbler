<script lang="ts">
  import * as api from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import type { BookCastCharacter, BookCastUnattributedLine } from '#lib/types.js';

  interface Props {
    bookId: string;
    unresolved: number;
    characters: BookCastCharacter[];
    onChanged: () => void;
  }

  let { bookId, unresolved, characters, onChanged }: Props = $props();

  let lines = $state<BookCastUnattributedLine[]>([]);
  let busy = $state(false);
  let speakers = $derived(characters.filter((c) => !c.narrator));

  function key(l: BookCastUnattributedLine): string {
    return `${l.segment_index}:${l.span_start}`;
  }

  async function load() {
    try {
      lines = (await api.getUnattributedCastLines(bookId)).lines;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load the lines');
    }
  }

  $effect(() => {
    void unresolved;
    load();
  });

  async function assign(l: BookCastUnattributedLine, to: string) {
    busy = true;
    try {
      await api.reassignCastLine(bookId, l.segment_index, l.span_start, to || null);
      lines = lines.filter((x) => key(x) !== key(l));
      onChanged();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed');
    } finally {
      busy = false;
    }
  }
</script>

<ul class="needs">
  {#each lines as l (key(l))}
    <li>
      <div class="head">
        <span class="chapter"
          >Ch. {l.chapter_index + 1}{l.chapter_title ? ` · ${l.chapter_title}` : ''}</span
        >
        <span class="quote">“{l.text ?? ''}”</span>
        {#if l.cue}<span class="hint">{l.cue}</span>{/if}
        {#if l.speaker}<span class="hint">pass 1: “{l.speaker}”</span>{/if}
        <span class="spacer"></span>
        <select
          aria-label="Speaker"
          disabled={busy}
          onchange={(e) => {
            const to = e.currentTarget.value;
            if (to !== '-') assign(l, to);
          }}
        >
          <option value="-">Assign to…</option>
          <option value="">Narration</option>
          {#each speakers as c (c.id)}
            <option value={c.id}>{c.name}</option>
          {/each}
        </select>
      </div>
      {#if l.paragraph}<p class="paragraph">{l.paragraph}</p>{/if}
    </li>
  {/each}
</ul>

<style>
  .needs {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 10px;
  }

  .head {
    display: flex;
    align-items: baseline;
    gap: 8px;
    flex-wrap: wrap;
    font-size: 13px;
  }

  .chapter,
  .hint {
    font-size: 11px;
    color: var(--muted);
  }

  .quote {
    font-weight: 600;
  }

  .spacer {
    flex: 1;
  }

  .paragraph {
    margin: 4px 0 0;
    font-size: 12px;
    color: var(--muted);
  }

  select {
    font: inherit;
    font-size: 13px;
  }
</style>
