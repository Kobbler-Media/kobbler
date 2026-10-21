<script lang="ts">
  import { formatDuration } from '$lib/utils/format';

  interface Props {
    mode?: 'file' | 'stream';
    positionSeconds: number | null;
    totalSeconds: number | null;
    watched: boolean;
    loading: boolean;
    variant: 'hero' | 'panel';
    onPlay: () => void;
    onPickStream?: () => void;
    label?: string;
  }

  let {
    mode = 'file',
    positionSeconds,
    totalSeconds,
    watched,
    loading,
    variant,
    onPlay,
    onPickStream,
    label
  }: Props = $props();

  const canResume = $derived(positionSeconds != null && positionSeconds > 0 && !watched);

  const progressPercent = $derived.by(() => {
    if (!canResume || !totalSeconds || totalSeconds <= 0) return 0;
    return Math.min(100, (positionSeconds! / totalSeconds) * 100);
  });

  const remainingSeconds = $derived.by(() => {
    if (!canResume || !totalSeconds || totalSeconds <= 0) return null;
    const left = totalSeconds - positionSeconds!;
    return left > 30 ? left : null;
  });

  const buttonLabel = $derived.by(() => {
    if (label) return label;
    if (mode === 'stream') return canResume ? 'Resume Stream' : 'Stream';
    return canResume ? 'Resume' : 'Play';
  });
</script>

<div class="playback-area" class:hero={variant === 'hero'} class:panel={variant === 'panel'}>
  <div class="play-row" class:split={mode === 'stream' && onPickStream}>
    <button class="play-btn" type="button" onclick={onPlay} disabled={loading}>
      {#if loading}
        <span class="material-icons-outlined spin">refresh</span>
      {:else}
        <span class="material-icons-outlined">
          {mode === 'stream' ? 'play_circle' : 'play_arrow'}
        </span>
      {/if}
      {buttonLabel}
    </button>
    {#if mode === 'stream' && onPickStream}
      <button
        class="pick-btn"
        type="button"
        onclick={onPickStream}
        title="Choose a release"
        aria-label="Choose a release"
      >
        <span class="material-icons-outlined">menu</span>
      </button>
    {/if}
  </div>

  {#if canResume}
    <div class="progress-section">
      <div class="progress-track">
        <div class="progress-fill" style="width: {progressPercent}%"></div>
      </div>
      <div class="progress-labels">
        <span class="progress-label">
          {formatDuration(positionSeconds!)}{totalSeconds
            ? ` / ${formatDuration(totalSeconds)}`
            : ''}
        </span>
        {#if remainingSeconds != null}
          <span class="progress-label">{formatDuration(remainingSeconds)} left</span>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .playback-area {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .playback-area.panel {
    background: var(--bg-2);
    border: 1px solid var(--ring);
    border-radius: 10px;
    padding: 10px 12px;
  }

  .play-row {
    display: inline-flex;
    align-items: stretch;
  }

  /* Only the split control clips: the plain button's hover shadow has to escape. */
  .play-row.split {
    border-radius: 100px;
    overflow: hidden;
  }

  .panel .play-row {
    display: flex;
  }

  .play-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    border-radius: 100px;
    border: none;
    background: var(--accent);
    color: var(--bg);
    cursor: pointer;
    font-weight: 600;
    transition: all 0.2s cubic-bezier(0.16, 1, 0.3, 1);
  }

  .split .play-btn {
    border-radius: 0;
  }

  .hero .play-btn {
    padding: 9px 24px;
    font-size: 14px;
    font-weight: 700;
  }

  .panel .play-btn {
    padding: 7px 14px;
    font-size: 13px;
    width: 100%;
  }

  .play-btn:hover:not(:disabled) {
    filter: brightness(1.1);
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(211, 140, 90, 0.25);
  }

  /* A split control moves as one piece, so the halves must not lift apart. */
  .split .play-btn:hover:not(:disabled) {
    transform: none;
    box-shadow: none;
  }

  .play-btn:active:not(:disabled) {
    transform: translateY(0);
  }

  .play-btn:disabled {
    opacity: 0.7;
    cursor: not-allowed;
  }

  .hero .play-btn .material-icons-outlined {
    font-size: 20px;
  }

  .panel .play-btn .material-icons-outlined {
    font-size: 16px;
  }

  .pick-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 0 12px;
    border: none;
    border-left: 1px solid rgba(255, 255, 255, 0.2);
    background: var(--accent);
    color: var(--bg);
    cursor: pointer;
    transition: all 0.2s;
  }

  .pick-btn:hover {
    filter: brightness(1.15);
  }

  .pick-btn .material-icons-outlined {
    font-size: 18px;
  }

  .progress-section {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .progress-track {
    height: 3px;
    background: var(--ring);
    border-radius: 2px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 2px;
    transition: width 0.3s ease;
  }

  .progress-labels {
    display: flex;
    justify-content: space-between;
    gap: 12px;
  }

  .progress-label {
    font-size: 11px;
    color: var(--muted);
    font-weight: 500;
    font-variant-numeric: tabular-nums;
  }

  .spin {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
