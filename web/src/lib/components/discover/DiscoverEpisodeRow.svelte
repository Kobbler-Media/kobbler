<script lang="ts">
  import type { DiscoverEpisode } from '$lib/api/client';
  import EpisodeSummary from '../tv/EpisodeSummary.svelte';

  interface Props {
    episode: DiscoverEpisode;
    canStream: boolean;
    wanting: boolean;
    onStream: () => void;
    onChooseRelease: () => void;
    onWant: () => void;
  }

  let { episode, canStream, wanting, onStream, onChooseRelease, onWant }: Props = $props();

  const today = new Date().toISOString().slice(0, 10);
  const upcoming = $derived(!!episode.air_date && episode.air_date.slice(0, 10) > today);
</script>

<div class="episode-row">
  <div class="episode-body">
    <EpisodeSummary
      number={episode.episode_number}
      title={episode.title ?? `Episode ${episode.episode_number}`}
      airDate={episode.air_date}
      runtime={episode.runtime}
      stillUrl={episode.still_url}
      synopsis={episode.description}
    />
  </div>
  <div class="episode-actions">
    {#if canStream && !upcoming}
      <button class="action-btn stream-btn" type="button" title="Stream" onclick={onStream}>
        <span class="material-icons-outlined">play_arrow</span>
      </button>
      <button class="action-btn" type="button" title="Choose a release" onclick={onChooseRelease}>
        <span class="material-icons-outlined">tune</span>
      </button>
    {/if}
    <button
      class="action-btn want-btn"
      type="button"
      title="Want this episode"
      disabled={wanting}
      onclick={onWant}
    >
      <span class="material-icons-outlined"
        >{wanting ? 'hourglass_empty' : 'add_circle_outline'}</span
      >
    </button>
  </div>
</div>

<style>
  .episode-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--ring);
    transition: background 0.15s ease;
  }

  .episode-row:last-child {
    border-bottom: none;
  }

  .episode-row:hover {
    background: var(--bg-2);
  }

  .episode-body {
    display: flex;
    align-items: center;
    gap: 12px;
    flex: 1;
    min-width: 0;
  }

  .episode-actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
  }

  .action-btn {
    width: 28px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .action-btn .material-icons-outlined {
    font-size: 16px;
  }

  .action-btn:hover:not(:disabled) {
    background: var(--bg-2);
    color: var(--ink);
  }

  .action-btn:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .stream-btn {
    border-color: var(--accent);
    color: var(--accent);
  }

  .stream-btn:hover {
    background: var(--accent-dim);
  }

  .want-btn:hover:not(:disabled) {
    color: var(--wanted, #f59e0b);
    border-color: var(--wanted, #f59e0b);
  }

  @media (max-width: 480px) {
    .episode-row {
      gap: 8px;
      padding: 8px 10px;
    }

    .episode-body {
      gap: 8px;
    }
  }
</style>
