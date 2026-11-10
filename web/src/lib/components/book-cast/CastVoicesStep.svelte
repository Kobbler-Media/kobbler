<script lang="ts">
  import * as api from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import type { BookCast } from '#lib/types.js';
  import CastCharacterEdit from './CastCharacterEdit.svelte';
  import CastCharacterSummary from './CastCharacterSummary.svelte';
  import CastVoiceStep from './CastVoiceStep.svelte';
  import { firstOpen, voiceSteps, type VoiceStep } from './castReview.js';
  import { isTyping } from './keys.js';

  interface Props {
    bookId: string;
    cast: BookCast;
    onChanged: () => void;
    onDone: () => void;
  }

  let { bookId, cast, onChanged, onDone }: Props = $props();

  let steps = $derived(voiceSteps(cast.characters));
  let picked = $state<string | null>(null);
  let editing = $state(false);
  let approvingAll = $state(false);

  let at = $derived.by(() => {
    const i = steps.findIndex((s) => s.key === picked);
    if (i >= 0) return i;
    const open = firstOpen(steps, -1);
    return open >= 0 ? open : 0;
  });
  let current = $derived<VoiceStep | undefined>(steps[at]);
  let approvedCount = $derived(steps.filter((s) => s.approved).length);
  let allApproved = $derived(steps.length > 0 && approvedCount === steps.length);
  let currentKey = $derived(current?.key);
  let nextOpen = $derived(firstOpen(steps, at));

  $effect(() => {
    void currentKey;
    editing = false;
  });

  function select(i: number) {
    picked = steps[(i + steps.length) % steps.length]?.key ?? null;
  }

  function advance() {
    if (nextOpen >= 0 && nextOpen !== at) select(nextOpen);
  }

  async function approveAll() {
    approvingAll = true;
    try {
      await api.approveAllCastVoices(bookId);
      showMessage('Approving the remaining voices');
      onChanged();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to approve the voices');
    } finally {
      approvingAll = false;
    }
  }

  function group(s: VoiceStep): string {
    if (s.character.narrator) return 'Narrator';
    return s.character.pooled ? 'Minor parts' : 'Cast';
  }

  function stepLabel(s: VoiceStep): string {
    const from = s.voice && s.voice.from_chapter > 0 ? ` · ch. ${s.voice.from_chapter + 1}+` : '';
    const pool = s.sharedWith.length > 0 ? ` +${s.sharedWith.length}` : '';
    return `${s.character.name}${from}${pool}`;
  }

  function onKey(e: KeyboardEvent) {
    if (isTyping(e) || steps.length === 0) return;
    if (e.key === 'ArrowRight') select(at + 1);
    else if (e.key === 'ArrowLeft') select(at - 1);
    else return;
    e.preventDefault();
  }
</script>

<svelte:window onkeydown={onKey} />

<section class="voices">
  <header class="bar">
    <div class="progress">
      <strong>{approvedCount}</strong> of {steps.length} voices approved
      <div class="meter">
        <span style:width="{steps.length ? (approvedCount / steps.length) * 100 : 0}%"></span>
      </div>
    </div>
    <span class="spacer"></span>
    {#if cast.approving}
      <span class="muted">Approving the rest… {cast.approving.progress ?? 0}%</span>
    {:else if !allApproved}
      <button
        type="button"
        class="btn alt small"
        disabled={approvingAll}
        title="Approves every remaining voice with its default take, without auditioning"
        onclick={approveAll}
      >
        Approve the rest as they are
      </button>
    {:else}
      <button type="button" class="btn small" onclick={onDone}>Next: render the audiobook</button>
    {/if}
  </header>

  <div class="layout">
    <nav class="rail" aria-label="Voices">
      {#each steps as s, i (s.key)}
        {#if i === 0 || group(steps[i - 1]) !== group(s)}
          <p class="rail-group">{group(s)}</p>
        {/if}
        <button
          type="button"
          class="rail-item"
          class:active={i === at}
          class:approved={s.approved}
          onclick={() => select(i)}
        >
          <span class="material-icons-outlined">
            {s.approved ? 'check_circle' : 'radio_button_unchecked'}
          </span>
          <span class="rail-name">{stepLabel(s)}</span>
        </button>
      {/each}
    </nav>

    {#if current}
      <div class="main">
        <div class="nav">
          <button
            type="button"
            class="btn alt small"
            onclick={() => select(at - 1)}
            aria-label="Previous voice"
          >
            <span class="material-icons-outlined">chevron_left</span>
          </button>
          <span class="muted">{at + 1} / {steps.length}</span>
          <button
            type="button"
            class="btn alt small"
            onclick={() => select(at + 1)}
            aria-label="Next voice"
          >
            <span class="material-icons-outlined">chevron_right</span>
          </button>
          <span class="spacer"></span>
          {#if nextOpen >= 0 && nextOpen !== at}
            <button type="button" class="btn alt small" onclick={() => select(nextOpen)}>
              Next to approve
            </button>
          {/if}
        </div>

        <div class="card">
          <CastCharacterSummary character={current.character}>
            {#snippet actions()}
              <button type="button" class="btn alt small" onclick={() => (editing = !editing)}>
                {editing ? 'Close' : 'Edit character'}
              </button>
            {/snippet}
          </CastCharacterSummary>
          {#if current.sharedWith.length > 0}
            <p class="shared">
              <span class="material-icons-outlined">group</span>
              A shared voice for small parts — also reads
              {current.sharedWith.map((c) => c.name).join(', ')}.
            </p>
          {/if}
          {#if editing}
            <CastCharacterEdit
              {bookId}
              character={current.character}
              others={cast.characters}
              {onChanged}
              onClose={() => (editing = false)}
            />
          {/if}
        </div>

        {#if current.voice}
          <CastVoiceStep
            {bookId}
            character={current.character}
            voice={current.voice}
            presets={cast.presets}
            {onChanged}
            onApproved={advance}
          />
        {:else}
          <p class="muted">
            No voice has been designed for {current.character.name} yet. Rerun the cast pass from the
            menu to design one.
          </p>
        {/if}
      </div>
    {:else}
      <p class="muted">Nobody in this cast needs a voice.</p>
    {/if}
  </div>
</section>

<style>
  .voices {
    display: grid;
    gap: 14px;
  }

  .bar,
  .nav {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .nav .btn {
    display: inline-flex;
    align-items: center;
  }

  .progress {
    display: grid;
    gap: 4px;
    font-size: 13px;
    min-width: 200px;
  }

  .meter {
    height: 4px;
    border-radius: 999px;
    background: var(--ring);
    overflow: hidden;
  }

  .meter span {
    display: block;
    height: 100%;
    background: var(--success);
    transition: width 0.3s;
  }

  .spacer {
    flex: 1;
  }

  .layout {
    display: grid;
    grid-template-columns: 220px 1fr;
    gap: 20px;
    align-items: start;
  }

  .rail {
    display: grid;
    gap: 2px;
    position: sticky;
    top: 16px;
    max-height: calc(100vh - 32px);
    overflow-y: auto;
  }

  .rail-group {
    margin: 10px 0 4px;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--muted);
  }

  .rail-group:first-child {
    margin-top: 0;
  }

  .rail-item {
    display: flex;
    align-items: center;
    gap: 6px;
    font: inherit;
    font-size: 13px;
    text-align: left;
    padding: 5px 8px;
    border: none;
    border-radius: 8px;
    background: none;
    color: var(--ink);
    cursor: pointer;
  }

  .rail-item:hover {
    background: var(--bg-2);
  }

  .rail-item.active {
    background: var(--card);
    box-shadow: inset 0 0 0 1px var(--ring);
    font-weight: 600;
  }

  .rail-item .material-icons-outlined {
    font-size: 16px;
    color: var(--muted);
  }

  .rail-item.approved .material-icons-outlined {
    color: var(--success);
  }

  .rail-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .main {
    display: grid;
    gap: 12px;
    min-width: 0;
  }

  .card {
    display: grid;
    gap: 12px;
    padding: 18px 20px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 14px;
  }

  .shared {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0;
    font-size: 13px;
    color: var(--muted);
  }

  .shared .material-icons-outlined {
    font-size: 16px;
  }

  .muted {
    margin: 0;
    color: var(--muted);
    font-size: 13px;
  }

  @media (max-width: 760px) {
    .layout {
      grid-template-columns: 1fr;
    }

    .rail {
      display: none;
    }
  }
</style>
