<script lang="ts">
  import MetaLine from '#lib/components/shared/MetaLine.svelte';
  import { getCastState, endCastSession, castSendMessage } from '#lib/stores/cast.svelte.js';

  const cast = getCastState();

  function formatTime(seconds: number): string {
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = Math.floor(seconds % 60);
    if (h > 0) {
      return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
    }
    return `${m}:${String(s).padStart(2, '0')}`;
  }

  const progressPercent = $derived(
    cast.mediaState && cast.mediaState.duration > 0
      ? (cast.mediaState.currentTime / cast.mediaState.duration) * 100
      : 0
  );

  // Poll receiver status periodically while casting
  let pollInterval: ReturnType<typeof setInterval> | null = null;
  $effect(() => {
    if (cast.connected) {
      pollInterval = setInterval(() => {
        castSendMessage({ type: 'STATUS_REQUEST' });
      }, 2000);
    } else if (pollInterval) {
      clearInterval(pollInterval);
      pollInterval = null;
    }
    return () => {
      if (pollInterval) clearInterval(pollInterval);
    };
  });
</script>

{#if cast.connected && cast.mediaState}
  <div class="cast-mini">
    <div class="cast-progress">
      <div class="cast-progress-fill" style="width: {progressPercent}%"></div>
    </div>

    <div class="cast-content">
      <span class="material-icons-outlined cast-icon">cast_connected</span>

      <div class="cast-info">
        {#if cast.mediaState?.metadata?.title}
          <span class="cast-title">{cast.mediaState.metadata.title}</span>
          <span class="cast-subtitle">
            <MetaLine
              items={[
                cast.deviceName ?? 'Cast device',
                `${formatTime(cast.mediaState.currentTime)} / ${formatTime(cast.mediaState.duration)}`
              ]}
            />
          </span>
        {:else}
          <span class="cast-device">{cast.deviceName ?? 'Cast device'}</span>
          {#if cast.mediaState}
            <span class="cast-time">
              {formatTime(cast.mediaState.currentTime)} / {formatTime(cast.mediaState.duration)}
            </span>
          {/if}
        {/if}
      </div>

      <button
        type="button"
        class="cast-stop-btn"
        onclick={() => endCastSession()}
        title="Stop casting"
      >
        <span class="material-icons-outlined">close</span>
      </button>
    </div>
  </div>
{/if}

<style>
  .cast-mini {
    position: fixed;
    bottom: 0;
    left: 0;
    right: 0;
    padding-bottom: var(--safe-bottom);
    padding-left: var(--safe-left);
    padding-right: var(--safe-right);
    z-index: 150;
    background: var(--surface-2, #1a1a2e);
    border-top: 1px solid rgba(255, 255, 255, 0.08);
  }

  .cast-progress {
    height: 3px;
    background: rgba(255, 255, 255, 0.1);
  }

  .cast-progress-fill {
    height: 100%;
    background: #7c5cfc;
    transition: width 0.3s linear;
  }

  .cast-content {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 16px;
  }

  .cast-icon {
    font-size: 24px;
    color: #7c5cfc;
    flex-shrink: 0;
  }

  .cast-info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .cast-title {
    font-size: 13px;
    font-weight: 500;
    color: rgba(255, 255, 255, 0.9);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .cast-subtitle {
    font-size: 11px;
    color: rgba(255, 255, 255, 0.5);
  }

  .cast-device {
    font-size: 13px;
    font-weight: 500;
    color: rgba(255, 255, 255, 0.9);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .cast-time {
    font-size: 11px;
    color: rgba(255, 255, 255, 0.5);
  }

  .cast-stop-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border: none;
    background: transparent;
    color: rgba(255, 255, 255, 0.6);
    cursor: pointer;
    border-radius: 4px;
    flex-shrink: 0;
  }

  .cast-stop-btn:hover {
    color: #fff;
    background: rgba(255, 255, 255, 0.1);
  }
</style>
