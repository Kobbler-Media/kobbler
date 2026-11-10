<script lang="ts">
  import type { BookCast } from '#lib/types.js';
  import { formatDuration } from '#lib/utils/format.js';

  interface Props {
    cast: BookCast;
    analysing: boolean;
    starting: boolean;
    onResume: () => void;
    onDone: () => void;
  }

  let { cast, analysing, starting, onResume, onDone }: Props = $props();

  const PHASES: Record<string, string> = {
    chapters: 'Reading each chapter and working out who says what',
    bible: 'Gathering the characters and what they are like',
    voices: 'Designing a voice for each character'
  };

  let percent = $derived(
    cast.chapters.total > 0 ? Math.round((cast.chapters.done / cast.chapters.total) * 100) : 0
  );
  let speaking = $derived(cast.characters.filter((c) => !c.narrator).length);
</script>

<section class="panel">
  {#if analysing}
    <h2>{PHASES[cast.state] ?? 'Analysing'}</h2>
    <div
      class="bar"
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent}
    >
      <span style:width="{percent}%"></span>
    </div>
    <p class="muted">
      {cast.chapters.done} of {cast.chapters.total} chapters read
      {#if cast.eta.analysis_seconds !== null}
        · about {formatDuration(cast.eta.analysis_seconds)} left
      {/if}
    </p>
    <p class="muted">
      This runs in the background — you can leave the page. Checking speakers and voices opens once
      it's done.
    </p>
    {#if cast.error}<p class="error">{cast.error}</p>{/if}
    <div class="row">
      <button type="button" class="btn alt small" disabled={starting} onclick={onResume}>
        Nudge it along
      </button>
      <span class="muted small">Restarts the analysis where it stopped, if it stalled.</span>
    </div>
  {:else}
    <h2>Analysis done</h2>
    <p>
      Found <strong>{speaking}</strong> speaking {speaking === 1 ? 'part' : 'parts'} across
      <strong>{cast.lines}</strong> lines in {cast.chapters.total} chapters.
    </p>
    <div class="row">
      <button type="button" class="btn small" onclick={onDone}>Next: check who's speaking</button>
    </div>
  {/if}
</section>

<style>
  .panel {
    display: grid;
    gap: 10px;
    padding: 18px 20px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 14px;
  }

  h2 {
    margin: 0;
    font-size: 16px;
  }

  p {
    margin: 0;
    font-size: 14px;
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
    transition: width 0.4s;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }

  .muted {
    color: var(--muted);
    font-size: 13px;
  }

  .small {
    font-size: 12px;
  }

  .error {
    color: var(--danger);
    font-size: 13px;
  }
</style>
