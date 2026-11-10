<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
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
    onApproved?: () => void;
  }

  let { bookId, character, voice, presets, onChanged, onApproved }: Props = $props();

  const SEEDS = 4;

  type Take = { seed: number; url: string | null; error: string | null };

  let description = $derived(voice.description ?? '');
  let designed = $derived(voice.preset_id !== null && voice.preset_id === voice.design_preset_id);
  let choice = $derived(designed ? '' : (voice.preset_id ?? ''));
  let approved = $derived(voice.approved_at !== null);
  let dirty = $derived(description.trim() !== (voice.description ?? ''));
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

  async function act(work: () => Promise<unknown>, done?: string): Promise<boolean> {
    busy = true;
    try {
      await work();
      if (done) showMessage(done);
      onChanged();
      return true;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed');
      return false;
    } finally {
      busy = false;
    }
  }

  async function loadLines(characterId: string, fromChapter: number) {
    try {
      const res = await api.getCastAuditionLines(bookId, characterId, fromChapter);
      lines = res.lines;
      lineIndex = 0;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to load lines');
    }
  }

  let stepKey = $derived(`${character.id}:${voice.from_chapter}`);

  $effect(() => {
    void stepKey;
    untrack(() => {
      lines = null;
      releaseTakes();
      loadLines(character.id, voice.from_chapter);
    });
  });

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

  async function approve(take: Take | null) {
    const ok = await act(
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
    if (ok) onApproved?.();
  }
</script>

<div class="step" class:approved>
  <div class="step-head">
    <span class="step-title">
      {voice.from_chapter === 0 ? 'Voice' : `Voice from chapter ${voice.from_chapter + 1}`}
    </span>
    {#if approved}
      <span class="pill"><span class="material-icons-outlined">check</span>Approved</span>
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

  <div class="stage">
    <span class="stage-num">1</span>
    <div class="stage-body">
      <h4>How they sound</h4>
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
            >A description{voice.preset_name && designed ? ` (${voice.preset_name})` : ''}</option
          >
          {#each presets as p (p.id)}
            <option value={p.id}>Preset: {p.name}{p.has_clip ? ' (clone)' : ''}</option>
          {/each}
        </select>
      </label>
      {#if designed}
        <textarea rows="3" bind:value={description} disabled={busy} aria-label="Voice description"
        ></textarea>
        {#if dirty}
          <div class="row">
            <button
              type="button"
              class="btn alt small"
              disabled={busy || !description.trim()}
              onclick={() =>
                act(() =>
                  api.updateCastVoice(bookId, character.id, voice.from_chapter, { description })
                )}
            >
              Save description
            </button>
            {#if approved}
              <span class="muted">Saving takes the approval back — approve a new take after.</span>
            {/if}
          </div>
        {/if}
      {:else if !approved}
        <div class="row">
          <button type="button" class="btn small" disabled={busy} onclick={() => approve(null)}>
            Approve this preset
          </button>
        </div>
      {/if}
    </div>
  </div>

  <div class="stage">
    <span class="stage-num">2</span>
    <div class="stage-body">
      <h4>Hear it on one of their lines</h4>
      {#if lines === null}
        <p class="muted">Loading lines…</p>
      {:else if lines.length === 0}
        <p class="muted">No lines to audition on.</p>
      {:else}
        <div class="choices" role="radiogroup" aria-label="Audition line">
          {#each lines as l, i (`${l.segment_index}:${l.span_start}`)}
            <label class="choice" class:picked={i === lineIndex}>
              <input
                type="radio"
                name="line-{voice.character_id}-{voice.from_chapter}"
                value={i}
                bind:group={lineIndex}
              />
              <span class="quote">“{l.text}”</span>
              <span class="meta">
                Ch. {l.chapter_index + 1}{l.cue ? ` · ${l.cue}` : ''}
              </span>
            </label>
          {/each}
        </div>
        <div class="row">
          {#if line?.cue}
            <label class="check">
              <input type="checkbox" bind:checked={withCue} />
              <span>Deliver it {line.cue}</span>
            </label>
          {/if}
          <span class="spacer"></span>
          <button type="button" class="btn small" disabled={rendering} onclick={audition}>
            <span class="material-icons-outlined">graphic_eq</span>
            {rendering ? 'Rendering…' : `Render ${SEEDS} takes`}
          </button>
        </div>
      {/if}
    </div>
  </div>

  {#if takes.length > 0}
    <div class="stage">
      <span class="stage-num">3</span>
      <div class="stage-body">
        <h4>{designed ? 'Pick the take that sounds right' : 'Listen'}</h4>
        <ul class="takes">
          {#each takes as take, i (take.seed)}
            <li>
              <span class="take-label">Take {i + 1}</span>
              {#if take.url}
                <audio controls src={take.url}></audio>
                {#if designed}
                  <button
                    type="button"
                    class="btn small"
                    disabled={busy}
                    onclick={() => approve(take)}
                  >
                    {approved ? 'Use this one instead' : 'Approve this take'}
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
      </div>
    </div>
  {/if}
</div>

<style>
  .step {
    display: grid;
    gap: 14px;
    padding: 14px 16px;
    border: 1px solid var(--ring);
    border-radius: 12px;
  }

  .step.approved {
    border-color: color-mix(in srgb, var(--success) 60%, var(--ring));
  }

  .step-head {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .step-title {
    font-weight: 600;
    font-size: 14px;
  }

  .spacer {
    flex: 1;
  }

  .pill {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    font-size: 11px;
    padding: 2px 8px 2px 4px;
    border-radius: 999px;
    color: var(--success);
    border: 1px solid currentColor;
  }

  .pill .material-icons-outlined {
    font-size: 14px;
  }

  .stage {
    display: grid;
    grid-template-columns: 24px 1fr;
    gap: 10px;
  }

  .stage-num {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border-radius: 50%;
    background: var(--bg-2);
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
  }

  .stage-body {
    display: grid;
    gap: 8px;
    min-width: 0;
  }

  h4 {
    margin: 2px 0 0;
    font-size: 13px;
    font-weight: 600;
  }

  .field {
    display: grid;
    gap: 4px;
    font-size: 12px;
    color: var(--muted);
  }

  select,
  textarea {
    font: inherit;
    font-size: 13px;
    color: var(--ink);
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .row .btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }

  .choices {
    display: grid;
    gap: 6px;
  }

  .choice {
    display: grid;
    grid-template-columns: auto 1fr;
    column-gap: 10px;
    align-items: baseline;
    padding: 8px 12px;
    border: 1px solid var(--ring);
    border-radius: 10px;
    cursor: pointer;
  }

  .choice.picked {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 8%, transparent);
  }

  .choice input {
    grid-row: span 2;
  }

  .quote {
    font-family: Georgia, 'Iowan Old Style', 'Times New Roman', serif;
    font-size: 14px;
    line-height: 1.5;
  }

  .meta {
    font-size: 11px;
    color: var(--muted);
  }

  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
  }

  .takes {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 8px;
  }

  .takes li {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }

  .takes audio {
    height: 36px;
    max-width: 100%;
  }

  .take-label {
    font-size: 12px;
    color: var(--muted);
    min-width: 48px;
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
