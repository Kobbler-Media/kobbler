<script lang="ts">
  import MetaLine from '$lib/components/shared/MetaLine.svelte';
  import type { LivetvRecording, LivetvSeriesRule } from '$lib/types';

  interface Props {
    recording: LivetvRecording;
    rule: LivetvSeriesRule | null;
    busy?: boolean;
    onCancel: (recording: LivetvRecording) => void;
  }

  let { recording, rule, busy = false, onCancel }: Props = $props();

  function clock(iso: string): string {
    const d = new Date(iso);
    return isNaN(d.getTime())
      ? ''
      : d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }

  const padding = $derived.by(() => {
    const before = Math.round((recording.padding_before_secs ?? 0) / 60);
    const after = Math.round((recording.padding_after_secs ?? 0) / 60);
    if (!before && !after) return null;
    return `+${before} / +${after} min padding`;
  });

  const meta = $derived([
    recording.episode_number,
    recording.channel_name,
    `${clock(recording.scheduled_start)} – ${clock(recording.scheduled_end)}`,
    padding
  ]);
</script>

<article class="row">
  <span class="time">{clock(recording.scheduled_start)}</span>
  <div class="thumb">
    {#if recording.image_url}
      <img src={recording.image_url} alt="" loading="lazy" />
    {:else}
      <span class="material-icons-outlined">tv</span>
    {/if}
  </div>
  <div class="body">
    <div class="line">
      <span class="title">{recording.title}</span>
      {#if rule}
        <a class="rule-chip" href="/livetv/series-rules?rule={rule.id}">{rule.title}</a>
      {:else}
        <span class="rule-chip plain">one-off</span>
      {/if}
      {#if recording.moved_to_guide_id}
        <span class="moved">moved by a clash</span>
      {/if}
    </div>
    <span class="meta"><MetaLine items={meta} /></span>
  </div>
  <button type="button" class="ghost-btn" disabled={busy} onclick={() => onCancel(recording)}>
    Cancel
  </button>
</article>

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 13px;
    padding: 11px 14px;
    border-radius: 11px;
    border: 1px solid var(--ring);
    background: var(--card);
  }

  .time {
    width: 52px;
    flex-shrink: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
    font-variant-numeric: tabular-nums;
  }

  .thumb {
    width: 72px;
    height: 41px;
    flex-shrink: 0;
    border-radius: 6px;
    border: 1px solid var(--ring);
    background: var(--bg-2);
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
    font-size: 18px;
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

  .title {
    font-size: 13.5px;
    font-weight: 600;
    letter-spacing: -0.01em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .rule-chip {
    font-size: 10.5px;
    font-weight: 600;
    color: var(--accent);
    border: 1px solid var(--accent);
    background: var(--accent-dim);
    border-radius: 5px;
    padding: 1px 7px;
    text-decoration: none;
    flex-shrink: 0;
  }

  .rule-chip.plain {
    color: var(--muted);
    border-color: var(--ring);
    background: transparent;
  }

  .moved {
    font-size: 10.5px;
    font-weight: 600;
    color: #8b5cf6;
    flex-shrink: 0;
  }

  .meta {
    font-size: 11.5px;
    color: var(--muted);
  }

  .ghost-btn {
    height: 28px;
    padding: 0 12px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    font-size: 11.5px;
    font-weight: 600;
    cursor: pointer;
    flex-shrink: 0;
  }

  .ghost-btn:hover:not(:disabled) {
    color: var(--ink);
    border-color: var(--muted);
  }

  .ghost-btn:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
</style>
