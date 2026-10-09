<script lang="ts">
  import ConfidenceDots from './ConfidenceDots.svelte';
  import { networkTint } from './tint';
  import type { LivetvOnNowItem } from '$lib/types';

  interface Props {
    item: LivetvOnNowItem;
    compact?: boolean;
    busy?: boolean;
    onWatch: (item: LivetvOnNowItem) => void;
    onRecord: (item: LivetvOnNowItem) => void;
  }

  let { item, compact = false, busy = false, onWatch, onRecord }: Props = $props();

  const tint = $derived(networkTint(item.channel.network ?? item.channel.name));

  const progress = $derived(Math.min(100, Math.max(0, item.guide.progress_pct ?? 0)));

  const confidence = $derived(
    item.confidence == null ? null : Math.max(1, Math.min(4, Math.round(item.confidence * 4)))
  );

  function clock(iso: string): string {
    const d = new Date(iso);
    return isNaN(d.getTime())
      ? ''
      : d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }

  const timing = $derived(
    [
      `${clock(item.guide.start_time)} – ${clock(item.guide.end_time)}`,
      item.minutes_left != null ? `${item.minutes_left} min left` : null,
      item.guide.episode_number
    ]
      .filter(Boolean)
      .join(' · ')
  );
</script>

<article class="card" class:compact>
  <div
    class="banner"
    style:background="radial-gradient(120% 105% at 25% 18%, {tint}33 0%, rgba(0,0,0,0) 62%),
    linear-gradient(160deg, #14171d 0%, #0d1014 100%)"
  >
    {#if item.guide.image_url}
      <img class="still" src={item.guide.image_url} alt="" loading="lazy" />
      <span class="scrim"></span>
    {/if}
    <span class="channel">
      <span class="bar" style:background={tint}></span>
      {item.channel.name}
    </span>
    <div class="progress">
      <div class="fill" style:width="{progress}%" style:background={tint}></div>
    </div>
  </div>

  <div class="body">
    <span class="title">{item.guide.title}</span>
    <span class="timing">{timing}</span>
    {#if !compact && item.guide.synopsis}
      <span class="synopsis">{item.guide.synopsis}</span>
    {/if}

    <div class="spacer"></div>

    <div class="reason">
      {#if confidence != null}
        <ConfidenceDots filled={confidence} />
      {/if}
      <span>{item.reason ?? 'on now'}</span>
    </div>

    <div class="actions">
      <button type="button" class="watch" disabled={busy} onclick={() => onWatch(item)}>
        <span class="material-icons-outlined">play_arrow</span>
        Watch
      </button>
      <button type="button" class="record" disabled={busy} onclick={() => onRecord(item)}>
        <span class="ring"><span class="core"></span></span>
        Record
      </button>
    </div>
  </div>
</article>

<style>
  .card {
    width: 340px;
    flex-shrink: 0;
    box-sizing: border-box;
    border: 1px solid var(--ring);
    border-radius: 12px;
    background: var(--card);
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .card.compact {
    width: 258px;
  }

  .banner {
    height: 96px;
    position: relative;
    display: flex;
    align-items: flex-end;
    padding: 10px;
  }

  .card.compact .banner {
    height: 78px;
  }

  .still {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .scrim {
    position: absolute;
    inset: 0;
    background: linear-gradient(180deg, rgba(0, 0, 0, 0.15) 0%, rgba(0, 0, 0, 0.62) 100%);
  }

  .channel {
    position: relative;
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 4px 9px;
    border-radius: 7px;
    background: rgba(0, 0, 0, 0.55);
    font-size: 11px;
    font-weight: 600;
    color: #f3f1ee;
  }

  .bar {
    width: 3px;
    height: 13px;
    border-radius: 2px;
  }

  .progress {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 3px;
    background: rgba(255, 255, 255, 0.12);
  }

  .fill {
    height: 3px;
  }

  .body {
    padding: 11px 12px 12px;
    display: flex;
    flex-direction: column;
    gap: 5px;
    flex-grow: 1;
  }

  .title {
    font-size: 13.5px;
    font-weight: 600;
    letter-spacing: -0.01em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .timing {
    font-size: 11px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .synopsis {
    font-size: 11.5px;
    color: var(--muted);
    line-height: 1.5;
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }

  .spacer {
    flex-grow: 1;
  }

  .reason {
    display: flex;
    align-items: center;
    gap: 7px;
    padding-top: 8px;
    margin-top: 2px;
    border-top: 1px solid var(--ring);
    font-size: 10.5px;
    color: #5f6874;
    line-height: 1.4;
  }

  .actions {
    display: flex;
    gap: 6px;
    margin-top: 8px;
  }

  .watch,
  .record {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 11px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 11.5px;
    font-weight: 600;
    cursor: pointer;
  }

  .watch .material-icons-outlined {
    font-size: 15px;
  }

  .watch:hover:not(:disabled) {
    border-color: var(--accent);
  }

  .record:hover:not(:disabled) {
    border-color: var(--danger);
  }

  .watch:disabled,
  .record:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .ring {
    width: 11px;
    height: 11px;
    border-radius: 50%;
    border: 2px solid var(--danger);
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .core {
    width: 4px;
    height: 4px;
    border-radius: 50%;
    background: var(--danger);
  }
</style>
