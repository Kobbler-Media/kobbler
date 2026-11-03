<script lang="ts">
  import { onDestroy } from 'svelte';
  import type { NextEpisodeInfo } from '#lib/types.js';

  interface Props {
    nextEpisode: NextEpisodeInfo;
    visible: boolean;
    onPlayNow: () => void;
    onCancel: () => void;
    /** When true (video ended), start a 10s countdown to auto-play. */
    autoPlay: boolean;
  }

  let { nextEpisode, visible, onPlayNow, onCancel, autoPlay }: Props = $props();

  let countdown = $state(5);
  let countdownInterval: ReturnType<typeof setInterval> | null = null;

  $effect(() => {
    if (autoPlay) {
      countdown = 5;
      countdownInterval = setInterval(() => {
        countdown -= 1;
        if (countdown <= 0) {
          if (countdownInterval) clearInterval(countdownInterval);
          onPlayNow();
        }
      }, 1000);
    } else {
      if (countdownInterval) clearInterval(countdownInterval);
      countdownInterval = null;
    }
  });

  onDestroy(() => {
    if (countdownInterval) clearInterval(countdownInterval);
  });
</script>

{#if visible}
  <div class="up-next-overlay">
    <div class="up-next-card">
      {#if nextEpisode.still_url}
        <img class="up-next-thumb" src={nextEpisode.still_url} alt="" />
      {/if}
      <div class="up-next-info">
        <span class="up-next-label">Up Next</span>
        <span class="up-next-title">{nextEpisode.title || 'Next Episode'}</span>
        <span class="up-next-meta">
          {nextEpisode.show_title} - S{nextEpisode.season_identifier}E{nextEpisode.episode_identifier}
        </span>
      </div>
      <div class="up-next-actions">
        {#if autoPlay}
          <span class="countdown">Playing in {countdown}s</span>
        {/if}
        <button class="play-now-btn" type="button" onclick={onPlayNow}>Play Now</button>
        <button class="cancel-btn" type="button" onclick={onCancel}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .up-next-overlay {
    position: absolute;
    bottom: 100px;
    right: 24px;
    z-index: 10;
  }

  .up-next-card {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px;
    background: rgba(0, 0, 0, 0.85);
    border-radius: 8px;
    backdrop-filter: blur(12px);
    border: 1px solid rgba(255, 255, 255, 0.1);
    max-width: 400px;
  }

  .up-next-thumb {
    width: 120px;
    height: 68px;
    object-fit: cover;
    border-radius: 4px;
    flex-shrink: 0;
  }

  .up-next-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .up-next-label {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: rgba(255, 255, 255, 0.5);
  }

  .up-next-title {
    font-size: 14px;
    font-weight: 600;
    color: white;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .up-next-meta {
    font-size: 12px;
    color: rgba(255, 255, 255, 0.6);
  }

  .up-next-actions {
    display: flex;
    flex-direction: column;
    gap: 4px;
    align-items: center;
    flex-shrink: 0;
  }

  .countdown {
    font-size: 11px;
    color: rgba(255, 255, 255, 0.5);
  }

  .play-now-btn {
    padding: 6px 16px;
    border: none;
    border-radius: 4px;
    background: var(--accent, #818cf8);
    color: white;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }

  .play-now-btn:hover {
    filter: brightness(1.1);
  }

  .cancel-btn {
    padding: 4px 12px;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: rgba(255, 255, 255, 0.6);
    font-size: 12px;
    cursor: pointer;
  }

  .cancel-btn:hover {
    color: white;
  }
</style>
