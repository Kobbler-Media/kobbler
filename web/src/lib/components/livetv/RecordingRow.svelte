<script lang="ts">
  import MetaLine from '#lib/components/shared/MetaLine.svelte';
  import { formatSize } from '#lib/utils/format.js';
  import type { LivetvRecording } from '#lib/types.js';

  interface Props {
    recording: LivetvRecording;
    busy?: boolean;
    onPlay: (recording: LivetvRecording) => void;
    onCancel?: (recording: LivetvRecording) => void;
  }

  let { recording, busy = false, onPlay, onCancel }: Props = $props();

  const inProgress = $derived(recording.is_in_progress || recording.status === 'recording');

  function clock(iso: string | null): string {
    if (!iso) return '';
    const d = new Date(iso);
    if (isNaN(d.getTime())) return '';
    return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }

  function weekday(iso: string | null): string {
    if (!iso) return '';
    const d = new Date(iso);
    if (isNaN(d.getTime())) return '';
    const today = new Date();
    if (d.toDateString() === today.toDateString()) return '';
    return d.toLocaleDateString([], { weekday: 'short' });
  }

  const meta = $derived.by(() => {
    const parts: string[] = [];
    if (recording.episode_number) parts.push(recording.episode_number);
    parts.push(recording.channel_name);
    const day = weekday(recording.scheduled_start);
    const from = clock(recording.actual_start ?? recording.scheduled_start);
    const to = clock(recording.scheduled_end);
    parts.push(`${day ? `${day} ` : ''}${from} – ${to}`);
    return parts;
  });

  const detail = $derived.by(() => {
    if (inProgress) {
      const mins =
        recording.seconds_on_disk != null ? Math.floor(recording.seconds_on_disk / 60) : null;
      if (!recording.playable_from_start) return ['buffering — not yet playable from the start'];
      return [
        mins != null ? `watch from the start — ${mins} min already on disk` : 'watch from the start'
      ];
    }
    const parts: string[] = [];
    if (recording.duration_seconds) {
      const h = Math.floor(recording.duration_seconds / 3600);
      const m = Math.round((recording.duration_seconds % 3600) / 60);
      parts.push(h > 0 ? `${h} h${m ? ` ${m}` : ''}` : `${m} min`);
    }
    const breaks = recording.ad_breaks?.length ?? 0;
    parts.push(
      breaks > 0 ? `${breaks} ad break${breaks === 1 ? '' : 's'} marked` : 'no ads detected'
    );
    if (recording.size_bytes) parts.push(formatSize(recording.size_bytes));
    return parts;
  });

  const progressPct = $derived.by(() => {
    if (!inProgress) return 100;
    const start = Date.parse(recording.actual_start ?? recording.scheduled_start);
    const end = Date.parse(recording.scheduled_end);
    if (!isFinite(start) || !isFinite(end) || end <= start) return 0;
    return Math.min(100, Math.max(0, ((Date.now() - start) / (end - start)) * 100));
  });

  const failed = $derived(recording.status === 'failed');
</script>

<article class="row" class:live={inProgress} class:failed>
  <div class="thumb">
    {#if recording.image_url}
      <img src={recording.image_url} alt="" loading="lazy" />
    {:else}
      <span class="material-icons-outlined">tv</span>
    {/if}
  </div>

  <div class="body">
    <div class="line">
      {#if inProgress}
        <span class="rec-badge"><span class="dot"></span>REC</span>
      {/if}
      <span class="title">{recording.title}</span>
    </div>
    <span class="meta"><MetaLine items={meta} /></span>
    <span class="detail">
      {#if failed}{recording.error ?? 'Recording failed'}{:else}<MetaLine items={detail} />{/if}
    </span>
    <div class="progress-track">
      <div class="progress-fill" class:live={inProgress} style:width="{progressPct}%"></div>
    </div>
  </div>

  <div class="actions">
    {#if onCancel}
      <button type="button" class="ghost-btn" disabled={busy} onclick={() => onCancel?.(recording)}>
        {inProgress ? 'Stop' : 'Delete'}
      </button>
    {/if}
    <button
      type="button"
      class="play-btn"
      class:primary={inProgress}
      disabled={busy || failed || (inProgress && !recording.playable_from_start)}
      onclick={() => onPlay(recording)}
    >
      <span class="material-icons-outlined">play_arrow</span>
      {inProgress ? 'Watch from start' : 'Play'}
    </button>
  </div>
</article>

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 13px;
    padding: 12px 14px;
    border-radius: 11px;
    border: 1px solid var(--ring);
    background: var(--card);
  }

  .row.live {
    border-color: var(--danger);
  }

  .row.failed {
    border-color: var(--warning);
  }

  .thumb {
    width: 88px;
    height: 50px;
    border-radius: 6px;
    background: var(--bg-2);
    border: 1px solid var(--ring);
    flex-shrink: 0;
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--muted);
  }

  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .thumb .material-icons-outlined {
    font-size: 20px;
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex-grow: 1;
  }

  .line {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 0;
  }

  .rec-badge {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.05em;
    color: var(--danger);
    flex-shrink: 0;
  }

  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--danger);
  }

  .title {
    font-size: 14px;
    font-weight: 600;
    letter-spacing: -0.01em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .meta {
    font-size: 11.5px;
    color: var(--muted);
  }

  .detail {
    font-size: 11px;
    color: #5f6874;
  }

  .progress-track {
    height: 3px;
    border-radius: 2px;
    background: var(--ring);
    margin-top: 8px;
    overflow: hidden;
  }

  .progress-fill {
    height: 3px;
    border-radius: 2px;
    background: #3f4753;
  }

  .progress-fill.live {
    background: var(--danger);
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .play-btn,
  .ghost-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 30px;
    padding: 0 13px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 11.5px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }

  .play-btn .material-icons-outlined {
    font-size: 15px;
  }

  .play-btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--bg);
    font-weight: 700;
  }

  .play-btn:hover:not(:disabled),
  .ghost-btn:hover:not(:disabled) {
    border-color: var(--muted);
  }

  .play-btn:disabled,
  .ghost-btn:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .ghost-btn {
    color: var(--muted);
  }
</style>
