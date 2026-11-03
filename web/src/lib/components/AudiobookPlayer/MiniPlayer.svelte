<script lang="ts">
  import { goto } from '$app/navigation';
  import AudioControls from './AudioControls.svelte';
  import CastButton from '#lib/components/CastButton.svelte';
  import {
    getAudioPlayerState,
    seekToGlobal,
    skipForward,
    skipBackward,
    stop,
    formatTime
  } from '#lib/stores/audioplayer.svelte.js';

  const player = getAudioPlayerState();

  const displayDuration = $derived(
    player.totalDuration > 0 ? player.totalDuration : player.duration
  );
  const displayCurrent = $derived(
    player.totalDuration > 0 ? player.globalCurrentTime : player.currentTime
  );

  const progressPercent = $derived(
    displayDuration > 0 ? (displayCurrent / displayDuration) * 100 : 0
  );

  function handleProgressClick(e: MouseEvent) {
    const target = e.currentTarget as HTMLElement;
    const rect = target.getBoundingClientRect();
    const pct = (e.clientX - rect.left) / rect.width;
    seekToGlobal(pct * displayDuration);
  }

  function handleProgressKeydown(e: KeyboardEvent) {
    if (e.key === 'ArrowRight') skipForward(10);
    else if (e.key === 'ArrowLeft') skipBackward(10);
  }

  function expandPlayer() {
    if (player.currentBook) {
      goto(`/library/books/${player.currentBook.id}/listen`);
    }
  }
</script>

{#if player.isActive}
  <div class="mini-player">
    <!-- Progress bar -->
    <div
      class="mini-progress"
      role="slider"
      tabindex="0"
      aria-label="Playback progress"
      aria-valuemin={0}
      aria-valuemax={displayDuration}
      aria-valuenow={displayCurrent}
      onclick={handleProgressClick}
      onkeydown={handleProgressKeydown}
    >
      <div class="mini-progress-fill" style="width: {progressPercent}%"></div>
    </div>

    <div class="mini-content">
      <!-- Cover art -->
      {#if player.currentBook?.cover_url}
        <button type="button" class="mini-cover-btn" onclick={expandPlayer}>
          <img src={player.currentBook.cover_url} alt="" class="mini-cover" />
        </button>
      {/if}

      <!-- Info -->
      <button type="button" class="mini-info" onclick={expandPlayer}>
        <span class="mini-title">{player.currentBook?.title ?? 'Unknown'}</span>
        <span class="mini-subtitle">
          {player.currentChapter?.title || ''}
          {#if player.currentChapter?.title}
            &middot;
          {/if}
          {formatTime(displayCurrent)}
        </span>
      </button>

      <!-- Controls -->
      <div class="mini-controls">
        <AudioControls />
      </div>

      <!-- Cast -->
      <CastButton class="mini-action-btn" />

      <!-- Expand / Close -->
      <button type="button" class="mini-action-btn" onclick={expandPlayer} title="Full player">
        <span class="material-icons-outlined">open_in_full</span>
      </button>

      <button type="button" class="mini-action-btn" onclick={stop} title="Close player">
        <span class="material-icons-outlined">close</span>
      </button>
    </div>
  </div>
{/if}

<style>
  .mini-player {
    position: fixed;
    bottom: 0;
    left: 0;
    right: 0;
    padding-bottom: var(--safe-bottom);
    padding-left: var(--safe-left);
    padding-right: var(--safe-right);
    z-index: 50;
    background: var(--card);
    border-top: 1px solid var(--ring);
    box-shadow: 0 -4px 20px rgba(0, 0, 0, 0.3);
  }

  .mini-progress {
    height: 3px;
    background: rgba(255, 255, 255, 0.08);
    cursor: pointer;
    position: relative;
  }

  .mini-progress:hover {
    height: 5px;
  }

  .mini-progress-fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.25s linear;
    border-radius: 0 1px 1px 0;
  }

  .mini-content {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    min-height: 56px;
  }

  .mini-cover-btn {
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    flex-shrink: 0;
  }

  .mini-cover {
    width: 40px;
    height: 40px;
    border-radius: 6px;
    object-fit: cover;
  }

  .mini-info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    background: none;
    border: none;
    color: var(--ink);
    cursor: pointer;
    text-align: left;
    padding: 0;
  }

  .mini-title {
    font-size: 0.85rem;
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .mini-subtitle {
    font-size: 0.75rem;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .mini-controls {
    flex-shrink: 0;
  }

  .mini-action-btn {
    background: none;
    border: none;
    color: var(--ink);
    cursor: pointer;
    padding: 6px;
    border-radius: 6px;
    opacity: 0.6;
    display: flex;
    align-items: center;
    transition:
      opacity 0.15s,
      background 0.15s;
    flex-shrink: 0;
  }

  .mini-action-btn .material-icons-outlined {
    font-size: 18px;
  }

  .mini-action-btn:hover {
    opacity: 1;
    background: rgba(255, 255, 255, 0.08);
  }

  /* On mobile, hide full controls to save space */
  @media (max-width: 768px) {
    .mini-controls {
      display: none;
    }
  }
</style>
