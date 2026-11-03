<script lang="ts">
  import MetaLine from '#lib/components/shared/MetaLine.svelte';
  import type { LivetvChannel, LivetvGuideEntry } from '#lib/types.js';
  import RecordButton from './RecordButton.svelte';
  import { genreTint } from './tint';

  interface Props {
    entry: LivetvGuideEntry;
    channel: LivetvChannel;
    recordBusy?: boolean;
    onRecord: () => void;
    onWatch: () => void;
    onWatchIntent?: () => void;
  }

  let { entry, channel, recordBusy = false, onRecord, onWatch, onWatchIntent }: Props = $props();

  const timeFmt = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' });

  const genre = $derived(entry.genres?.[0] ?? null);

  const durationLabel = $derived.by(() => {
    const mins = Math.round((entry.duration_seconds ?? 0) / 60);
    if (mins <= 0) return null;
    const h = Math.floor(mins / 60);
    const m = mins % 60;
    return h > 0 ? (m > 0 ? `${h}h ${m}m` : `${h}h`) : `${m}m`;
  });

  const timing = $derived([
    `${timeFmt.format(new Date(entry.start_time))} – ${timeFmt.format(new Date(entry.end_time))}`,
    channel.name,
    durationLabel
  ]);

  const subtitle = $derived(
    [entry.episode_number, entry.episode_title].filter(Boolean).join(' · ')
  );
</script>

<div class="detail">
  <div class="thumb">
    {#if entry.image_url}
      <img src={entry.image_url} alt="" />
    {:else}
      <span class="material-icons-outlined">smart_display</span>
    {/if}
  </div>

  <div class="body">
    <div class="title-row">
      <span class="title">{entry.title}</span>
      {#if genre}
        <span class="genre" style:background={genreTint(genre)}>{genre.toUpperCase()}</span>
      {/if}
      <span class="timing"><MetaLine items={timing} /></span>
      {#if entry.repeat_count > 0}
        <span class="repeat"
          >{entry.repeat_count} other airing{entry.repeat_count === 1 ? '' : 's'}</span
        >
      {/if}
    </div>
    {#if subtitle}
      <span class="subtitle">{subtitle}</span>
    {/if}
    <span class="synopsis">
      {entry.synopsis ?? 'No synopsis in the guide feed for this programme.'}
    </span>
  </div>

  <div class="actions">
    <RecordButton
      recording={!!entry.recording}
      busy={recordBusy}
      title={entry.recording ? `Recording: ${entry.recording.status}` : 'Record this programme'}
      onclick={onRecord}
    />
    <button type="button" class="btn-primary watch" onpointerdown={onWatchIntent} onclick={onWatch}>
      <span class="material-icons-outlined">play_arrow</span>
      Watch
    </button>
  </div>
</div>

<style>
  .detail {
    margin-top: 14px;
    display: flex;
    align-items: center;
    gap: 14px;
    border: 1px solid var(--ring);
    border-radius: 12px;
    background: var(--card);
    padding: 12px 14px;
  }

  .thumb {
    width: 104px;
    height: 59px;
    border-radius: 7px;
    background: var(--bg-2);
    border: 1px solid var(--ring);
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
    color: var(--muted);
  }

  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
    flex-grow: 1;
  }

  .title-row {
    display: flex;
    align-items: center;
    gap: 9px;
    flex-wrap: wrap;
  }

  .title {
    font-size: 15px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .genre {
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.04em;
    color: var(--bg);
    border-radius: 3px;
    padding: 2px 5px;
  }

  .timing {
    font-size: 11px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .repeat {
    font-size: 11px;
    color: var(--muted);
    border: 1px solid var(--ring);
    border-radius: 4px;
    padding: 1px 6px;
  }

  .subtitle {
    font-size: 12px;
    color: var(--ink);
  }

  .synopsis {
    font-size: 12px;
    color: var(--muted);
    line-height: 1.5;
    max-width: 720px;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .watch {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 34px;
    padding: 0 16px;
    border-radius: 9px;
  }

  @media (max-width: 900px) {
    .detail {
      flex-wrap: wrap;
    }
  }
</style>
