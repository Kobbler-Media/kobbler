<script lang="ts">
  import { onMount } from 'svelte';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { cancelJob, getTranscriptBacklog, startTranscriptRefresh } from '#lib/api/client.js';
  import type { TranscriptBacklog, TranscriptBacklogTotals } from '#lib/types.js';

  let { transcriptionOff }: { transcriptionOff: boolean } = $props();

  const POLL_MS = 10_000;

  let backlog = $state<TranscriptBacklog | null>(null);
  let busy = $state(false);

  const job = $derived(backlog?.refresh_job ?? null);
  const pendingFiles = $derived((backlog?.outdated.files ?? 0) + (backlog?.missing.files ?? 0));
  const remainingHours = $derived(job?.result?.remaining_hours);

  const hours = (h: number) => `${h.toLocaleString(undefined, { maximumFractionDigits: 1 })} h`;
  const summary = (t: TranscriptBacklogTotals) =>
    `${t.books.toLocaleString()} book${t.books === 1 ? '' : 's'} · ${hours(t.hours)} of audio`;

  async function refresh() {
    try {
      backlog = await getTranscriptBacklog();
    } catch {
      backlog = null;
    }
  }

  async function start() {
    busy = true;
    try {
      await startTranscriptRefresh();
      showMessage('Re-transcription started');
      await refresh();
    } catch (e) {
      showMessage(
        e instanceof Error && e.message.includes('409')
          ? 'Transcription is off, or a run is already going'
          : 'Failed to start re-transcription'
      );
    } finally {
      busy = false;
    }
  }

  async function cancel() {
    if (!job) return;
    busy = true;
    try {
      await cancelJob(job.id);
      showMessage('Re-transcription cancelled');
      await refresh();
    } catch {
      showMessage('Failed to cancel re-transcription');
    } finally {
      busy = false;
    }
  }

  onMount(() => {
    refresh();
    const timer = setInterval(() => {
      if (job) refresh();
    }, POLL_MS);
    return () => clearInterval(timer);
  });
</script>

<div class="refresh" data-field="transcript_refresh">
  <span class="heading">Transcript backlog</span>
  {#if backlog}
    <dl class="totals">
      <dt>Outdated</dt>
      <dd>{summary(backlog.outdated)}</dd>
      <dt>Not yet transcribed</dt>
      <dd>{summary(backlog.missing)}</dd>
    </dl>
  {:else}
    <span class="hint">Backlog unavailable</span>
  {/if}

  {#if job}
    <div class="run">
      <div
        class="bar"
        role="progressbar"
        aria-valuemin="0"
        aria-valuemax="100"
        aria-valuenow={job.progress ?? 0}
      >
        <div class="fill" style="width: {job.progress ?? 0}%"></div>
      </div>
      <span class="hint">
        {job.status === 'pending' ? 'Waiting to start' : `${job.progress ?? 0}% done`}
        {#if job.result?.batches}· batch {job.result.batches}{/if}
        {#if remainingHours !== undefined}· {hours(remainingHours)} left{/if}
      </span>
      <button type="button" class="btn-action" disabled={busy} onclick={cancel}>Cancel</button>
    </div>
  {:else}
    <div class="run">
      <button
        type="button"
        class="btn-action"
        disabled={busy || transcriptionOff || pendingFiles === 0}
        onclick={start}
      >
        {busy
          ? 'Starting...'
          : (backlog?.outdated.files ?? 0) > 0
            ? 'Re-transcribe outdated'
            : 'Transcribe missing'}
      </button>
    </div>
  {/if}
  <span class="hint">
    New audiobooks are transcribed automatically. Outdated transcripts are only redone from here, a
    day of audio per batch with each batch waiting for the last, so a run can take days.
  </span>
</div>

<style>
  .refresh {
    display: grid;
    gap: 6px;
  }

  .heading {
    font-size: 12px;
    color: var(--muted);
  }

  .hint {
    font-size: 11px;
    color: var(--muted);
  }

  .totals {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 2px 12px;
    margin: 0;
    font-size: 13px;
  }

  .totals dt {
    color: var(--muted);
  }

  .totals dd {
    margin: 0;
    color: var(--ink);
  }

  .run {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }

  .bar {
    flex: 1 1 160px;
    max-width: 280px;
    height: 6px;
    border-radius: 999px;
    background: var(--ring);
    overflow: hidden;
  }

  .fill {
    height: 100%;
    background: var(--accent);
  }

  .btn-action {
    padding: 6px 12px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
  }

  .btn-action:hover:not(:disabled) {
    background: var(--ring);
  }

  .btn-action:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
</style>
