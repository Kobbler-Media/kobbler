<script lang="ts">
  import { onDestroy } from 'svelte';
  import * as api from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import type {
    BookCastCharacter,
    BookCastPresetChoice,
    BookCastTake,
    BookCastVoice
  } from '#lib/types.js';

  interface Props {
    bookId: string;
    character: BookCastCharacter;
    voice: BookCastVoice;
    presets: BookCastPresetChoice[];
    onChanged: () => void;
  }

  let { bookId, character, voice, presets, onChanged }: Props = $props();

  const SEEDS = 4;

  type Take = { seed: number; url: string | null; error: string | null };

  let description = $derived(voice.description ?? '');
  let designed = $derived(voice.preset_id !== null && voice.preset_id === voice.design_preset_id);
  let choice = $derived(designed ? '' : (voice.preset_id ?? ''));
  let approved = $derived(voice.approved_at !== null);
  let busy = $state(false);
  let lines = $state<BookCastTake[] | null>(null);
  let lineIndex = $state(0);
  let withCue = $state(false);
  let takes = $state<Take[]>([]);
  let rendering = $state(false);

  let line = $derived(lines?.[lineIndex] ?? null);

  function releaseTakes() {
    for (const t of takes) if (t.url) URL.revokeObjectURL(t.url);
    takes = [];
  }

  onDestroy(releaseTakes);

  async function act(work: () => Promise<unknown>, done?: string) {
    busy = true;
    try {
      await work();
      if (done) showMessage(done);
      onChanged();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed');
    } finally {
      busy = false;
    }
  }

  async function loadLines() {
    try {
      const res = await api.getCastAuditionLines(bookId, character.id, voice.from_chapter);
      lines = res.lines;
      lineIndex = 0;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load lines');
    }
  }

  async function audition() {
    if (!line) return;
    releaseTakes();
    rendering = true;
    const picked = line;
    takes = Array.from({ length: SEEDS }, () => ({
      seed: Math.floor(Math.random() * 2_147_483_647),
      url: null,
      error: null
    }));
    await Promise.all(
      takes.map(async (take, i) => {
        try {
          const blob = await api.renderCastAudition(bookId, character.id, voice.from_chapter, {
            segment_index: picked.segment_index,
            span_start: picked.span_start,
            seed: take.seed,
            with_cue: withCue
          });
          takes[i].url = URL.createObjectURL(blob);
        } catch (e) {
          takes[i].error = e instanceof Error ? e.message : 'Failed';
        }
      })
    );
    rendering = false;
  }

  function approve(take: Take | null) {
    act(
      () =>
        api.approveCastVoice(
          bookId,
          character.id,
          voice.from_chapter,
          take && line
            ? {
                segment_index: line.segment_index,
                span_start: line.span_start,
                seed: take.seed,
                with_cue: withCue
              }
            : {}
        ),
      `${character.name}'s voice approved`
    );
  }
</script>

<div class="step" class:approved>
  <div class="step-head">
    <span class="step-title">
      {voice.from_chapter === 0 ? 'Voice' : `From chapter ${voice.from_chapter + 1}`}
    </span>
    {#if approved}
      <span class="pill">Approved</span>
    {/if}
    <span class="spacer"></span>
    {#if approved}
      <button
        type="button"
        class="btn alt small"
        disabled={busy}
        onclick={() => act(() => api.unapproveCastVoice(bookId, character.id, voice.from_chapter))}
      >
        Unapprove
      </button>
    {/if}
    {#if voice.from_chapter > 0}
      <button
        type="button"
        class="btn alt small"
        disabled={busy}
        onclick={() =>
          act(() => api.removeCastVoiceChange(bookId, character.id, voice.from_chapter))}
      >
        Remove change
      </button>
    {/if}
  </div>

  <label class="field">
    <span>Voice from</span>
    <select
      value={choice}
      disabled={busy}
      onchange={(e) =>
        act(() =>
          api.updateCastVoice(bookId, character.id, voice.from_chapter, {
            preset_id: e.currentTarget.value || null
          })
        )}
    >
      <option value=""
        >Description{voice.preset_name && designed ? ` (${voice.preset_name})` : ''}</option
      >
      {#each presets as p (p.id)}
        <option value={p.id}>{p.name}{p.has_clip ? ' (clone)' : ''}</option>
      {/each}
    </select>
  </label>

  {#if designed}
    <label class="field">
      <span>Description</span>
      <textarea rows="2" bind:value={description} disabled={busy}></textarea>
    </label>
    {#if description.trim() !== (voice.description ?? '')}
      {#if approved}
        <p class="muted">Saving a new description takes the approval back; approve a take again.</p>
      {/if}
      <button
        type="button"
        class="btn alt small"
        disabled={busy || !description.trim()}
        onclick={() =>
          act(() => api.updateCastVoice(bookId, character.id, voice.from_chapter, { description }))}
      >
        Save description
      </button>
    {/if}
  {:else if !approved}
    <button type="button" class="btn alt small" disabled={busy} onclick={() => approve(null)}>
      Approve this preset
    </button>
  {/if}

  {#if lines === null}
    <button type="button" class="btn alt small" onclick={loadLines}>Audition</button>
  {:else if lines.length === 0}
    <p class="muted">No lines to audition on.</p>
  {:else}
    <div class="audition">
      <label class="field">
        <span>Line</span>
        <select bind:value={lineIndex}>
          {#each lines as l, i (`${l.segment_index}:${l.span_start}`)}
            <option value={i}>{l.text.length > 90 ? `${l.text.slice(0, 90)}…` : l.text}</option>
          {/each}
        </select>
      </label>
      {#if line?.cue}
        <label class="check">
          <input type="checkbox" bind:checked={withCue} />
          <span>Deliver as “{line.cue}”</span>
        </label>
      {/if}
      <button type="button" class="btn small" disabled={rendering} onclick={audition}>
        {rendering ? 'Rendering…' : `Render ${SEEDS} takes`}
      </button>
      {#if takes.length > 0}
        <ul class="takes">
          {#each takes as take (take.seed)}
            <li>
              <span class="seed">seed {take.seed}</span>
              {#if take.url}
                <audio controls src={take.url}></audio>
                {#if designed}
                  <button
                    type="button"
                    class="btn alt small"
                    disabled={busy}
                    onclick={() => approve(take)}
                  >
                    {approved ? 'Approve this take instead' : 'Approve this take'}
                  </button>
                {/if}
              {:else if take.error}
                <span class="error">{take.error}</span>
              {:else}
                <span class="muted">Rendering…</span>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
</div>

<style>
  .step {
    display: grid;
    gap: 8px;
    padding: 10px 12px;
    border: 1px solid var(--ring);
    border-radius: 10px;
  }

  .step.approved {
    border-color: var(--accent, var(--ring));
  }

  .step-head {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .step-title {
    font-weight: 600;
    font-size: 13px;
  }

  .spacer {
    flex: 1;
  }

  .pill {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--ring);
  }

  .field {
    display: grid;
    gap: 4px;
    font-size: 12px;
    color: var(--muted);
  }

  .field select,
  .field textarea {
    font: inherit;
    font-size: 13px;
    color: var(--ink);
  }

  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
  }

  .audition {
    display: grid;
    gap: 8px;
  }

  .takes {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 6px;
  }

  .takes li {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .seed {
    font-size: 11px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
    min-width: 110px;
  }

  .muted {
    color: var(--muted);
    font-size: 12px;
    margin: 0;
  }

  .error {
    color: var(--danger);
    font-size: 12px;
  }
</style>
