<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { getAudioStreamUrl } from '#lib/api/client.js';

  interface ChapterMarker {
    id: string;
    start_seconds: number;
    source: string;
  }

  interface Props {
    bookId: string;
    fileId: string;
    chapters: ChapterMarker[];
    duration: number;
    onChapterTimeChange: (chapterId: string, newStartSeconds: number) => void;
    onAddChapter: (startSeconds: number) => void;
    onReady?: (seekTo: (seconds: number) => void) => void;
  }

  let { bookId, fileId, chapters, duration, onChapterTimeChange, onAddChapter, onReady }: Props =
    $props();

  // Local audio element for self-contained playback
  let audio: HTMLAudioElement | undefined = $state();
  let playing = $state(false);
  let currentTime = $state(0);
  let audioDuration = $state(0);
  let audioReady = $state(false);

  // Waveform state
  let canvas: HTMLCanvasElement | undefined = $state();
  let container: HTMLDivElement | undefined = $state();
  let peaks: Int8Array | null = $state(null);
  let containerWidth = $state(0);
  let hoverTime = $state<number | null>(null);
  let seekDragging = $state(false);

  // Chapter marker dragging
  let dragMarker = $state<{ id: string; startX: number; originalTime: number } | null>(null);

  // Resolved accent color for canvas (CSS vars don't work in canvas context)
  let accentColor = '#818cf8';
  let stripEl: HTMLDivElement | undefined = $state();

  const SOURCE_COLORS: Record<string, string> = {
    embedded: '#4ade80',
    transcript: '#60a5fa',
    detected: '#f59e0b',
    provider: '#c084fc',
    filename: '#94a3b8'
  };

  const SOURCE_LABELS: Record<string, string> = {
    embedded: 'Embedded',
    transcript: 'Text cues',
    detected: 'Silence',
    provider: 'Provider',
    filename: 'Filename'
  };

  function sourceColor(source: string): string {
    return SOURCE_COLORS[source] ?? 'rgba(255,255,255,0.6)';
  }

  const effectiveDuration = $derived(audioDuration > 0 ? audioDuration : duration);

  const activeSources = $derived.by(() => {
    const sources = new Set<string>();
    for (const ch of chapters) sources.add(ch.source);
    return [...sources].sort();
  });

  // --- Audio setup ---

  $effect(() => {
    if (!audio || !fileId || !bookId) return;
    const url = getAudioStreamUrl(bookId, fileId);
    if (audio.src !== url && !audio.src.endsWith(url)) {
      audio.src = url;
      audio.load();
      playing = false;
      currentTime = 0;
      audioReady = false;
    }
  });

  function onTimeUpdate() {
    if (audio) currentTime = audio.currentTime;
  }

  function onLoadedMetadata() {
    if (audio) {
      audioDuration = audio.duration;
      audioReady = true;
    }
  }

  function onEnded() {
    playing = false;
  }

  function togglePlay() {
    if (!audio || !audioReady) return;
    if (playing) {
      audio.pause();
      playing = false;
    } else {
      audio.play();
      playing = true;
    }
  }

  function seek(seconds: number) {
    if (!audio) return;
    const clamped = Math.max(0, Math.min(seconds, effectiveDuration));
    audio.currentTime = clamped;
    currentTime = clamped;
  }

  function skip(delta: number) {
    seek(currentTime + delta);
  }

  function seekAndPlay(seconds: number) {
    seek(seconds);
    if (audio && !playing) {
      audio.play();
      playing = true;
    }
  }

  onMount(() => {
    onReady?.(seekAndPlay);

    // Resolve CSS accent color for canvas rendering
    if (stripEl) {
      const computed = getComputedStyle(stripEl).getPropertyValue('--accent').trim();
      if (computed) accentColor = computed;
    }
  });

  // Cleanup: pause and release audio on unmount
  onDestroy(() => {
    if (audio) {
      audio.pause();
      audio.removeAttribute('src');
      audio.load();
    }
  });

  // Keyboard shortcut: 'M' to mark chapter at current position
  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'm' || e.key === 'M') {
      // Don't fire if user is typing in an input
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) {
        return;
      }
      e.preventDefault();
      handleMarkChapter();
    }
  }

  // --- Peaks fetching ---

  $effect(() => {
    if (!fileId || !bookId) {
      peaks = null;
      return;
    }
    const url = `/api/v1/admin/books/${bookId}/audio/${fileId}/peaks`;
    fetch(url)
      .then((r) => {
        if (!r.ok) throw new Error('No peaks');
        return r.arrayBuffer();
      })
      .then((buf) => {
        const data = new Uint8Array(buf);
        if (data.length < 2 || data[0] !== 1) {
          peaks = null;
          return;
        }
        peaks = new Int8Array(buf, 2);
      })
      .catch(() => {
        peaks = null;
      });
  });

  // --- Resize observer ---

  onMount(() => {
    if (!container) return;
    const ro = new ResizeObserver((entries) => {
      containerWidth = entries[0]?.contentRect.width ?? 0;
    });
    ro.observe(container);
    return () => ro.disconnect();
  });

  // --- Waveform rendering ---

  $effect(() => {
    if (!canvas || containerWidth === 0 || effectiveDuration <= 0) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const dpr = window.devicePixelRatio || 1;
    const w = containerWidth;
    const h = 56;
    canvas.width = w * dpr;
    canvas.height = h * dpr;
    canvas.style.width = `${w}px`;
    canvas.style.height = `${h}px`;
    ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, w, h);

    const mid = h / 2;
    const progressFrac = effectiveDuration > 0 ? currentTime / effectiveDuration : 0;

    if (peaks) {
      const numPairs = peaks.length / 2;
      for (let i = 0; i < numPairs; i++) {
        const x = (i / numPairs) * w;
        const barW = Math.max(1, w / numPairs);
        const minVal = peaks[i * 2] / 128;
        const maxVal = peaks[i * 2 + 1] / 128;
        const top = mid - maxVal * mid;
        const bottom = mid - minVal * mid;
        const barH = Math.max(1, bottom - top);

        const frac = i / numPairs;
        ctx.fillStyle = frac <= progressFrac ? accentColor : 'rgba(255, 255, 255, 0.15)';
        ctx.fillRect(x, top, barW, barH);
      }
    } else {
      // Fallback bar
      ctx.fillStyle = 'rgba(255, 255, 255, 0.08)';
      ctx.fillRect(0, mid - 2, w, 4);
      ctx.fillStyle = accentColor;
      ctx.fillRect(0, mid - 2, w * progressFrac, 4);
    }

    // Playback position line
    const posX = progressFrac * w;
    ctx.fillStyle = 'rgba(255, 255, 255, 0.9)';
    ctx.fillRect(posX - 0.5, 0, 1, h);

    // Chapter markers — draw source-colored lines with small triangles at top
    for (const ch of chapters) {
      const x = (ch.start_seconds / effectiveDuration) * w;
      const color = sourceColor(ch.source);
      const isDragging = dragMarker?.id === ch.id;

      ctx.fillStyle = isDragging ? 'white' : color;
      ctx.globalAlpha = isDragging ? 1.0 : 0.8;

      // Vertical line
      ctx.fillRect(x - 0.5, 0, 1, h);

      // Small triangle at top
      ctx.beginPath();
      ctx.moveTo(x, 0);
      ctx.lineTo(x - 4, -1);
      ctx.lineTo(x + 4, -1);
      ctx.lineTo(x, 6);
      ctx.closePath();
      ctx.fill();

      ctx.globalAlpha = 1.0;
    }
  });

  // --- Pointer interactions ---

  function handlePointerDown(e: PointerEvent) {
    if (!container || effectiveDuration <= 0) return;
    const rect = container.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const time = (x / rect.width) * effectiveDuration;

    // Check if near a chapter marker (within 8px)
    for (const ch of chapters) {
      const markerX = (ch.start_seconds / effectiveDuration) * rect.width;
      if (Math.abs(x - markerX) < 8) {
        dragMarker = { id: ch.id, startX: e.clientX, originalTime: ch.start_seconds };
        (e.target as HTMLElement).setPointerCapture(e.pointerId);
        e.preventDefault();
        return;
      }
    }

    // Otherwise, seek
    seekDragging = true;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    seek(time);
  }

  function handlePointerMove(e: PointerEvent) {
    if (!container || effectiveDuration <= 0) return;
    const rect = container.getBoundingClientRect();
    const frac = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));

    if (dragMarker) {
      const newTime = frac * effectiveDuration;
      onChapterTimeChange(dragMarker.id, Math.round(newTime * 10) / 10);
      hoverTime = newTime;
      return;
    }

    hoverTime = frac * effectiveDuration;
    if (seekDragging) {
      seek(frac * effectiveDuration);
    }
  }

  function handlePointerUp() {
    dragMarker = null;
    seekDragging = false;
  }

  function handlePointerLeave() {
    if (!seekDragging && !dragMarker) hoverTime = null;
  }

  function handleMarkChapter() {
    onAddChapter(Math.round(currentTime * 10) / 10);
  }

  function formatTime(seconds: number): string {
    if (!isFinite(seconds) || seconds < 0) return '0:00';
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = Math.floor(seconds % 60);
    const pad = (n: number) => n.toString().padStart(2, '0');
    return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="player-strip" bind:this={stripEl} onkeydown={handleKeydown}>
  <!-- Hidden audio element -->
  <audio
    bind:this={audio}
    ontimeupdate={onTimeUpdate}
    onloadedmetadata={onLoadedMetadata}
    onended={onEnded}
    preload="metadata"
  ></audio>

  <!-- Waveform -->
  <div
    class="waveform"
    bind:this={container}
    onpointerdown={handlePointerDown}
    onpointermove={handlePointerMove}
    onpointerup={handlePointerUp}
    onpointerleave={handlePointerLeave}
    role="slider"
    tabindex="0"
    aria-label="Waveform scrubber"
    aria-valuemin={0}
    aria-valuemax={effectiveDuration}
    aria-valuenow={currentTime}
    class:dragging={seekDragging || !!dragMarker}
  >
    <canvas bind:this={canvas}></canvas>
    {#if hoverTime !== null}
      <div
        class="hover-tip"
        style="left: {effectiveDuration > 0 ? (hoverTime / effectiveDuration) * 100 : 0}%"
      >
        {formatTime(hoverTime)}
      </div>
    {/if}
  </div>

  <!-- Controls row -->
  <div class="controls">
    <div class="controls-left">
      <button
        type="button"
        class="ctrl-btn"
        title="Skip back 5s"
        onclick={() => skip(-5)}
        disabled={!audioReady}
      >
        <span class="material-icons-outlined">replay_5</span>
      </button>
      <button
        type="button"
        class="ctrl-btn play-btn"
        title={playing ? 'Pause' : 'Play'}
        onclick={togglePlay}
        disabled={!audioReady}
      >
        <span class="material-icons-outlined">{playing ? 'pause' : 'play_arrow'}</span>
      </button>
      <button
        type="button"
        class="ctrl-btn"
        title="Skip forward 5s"
        onclick={() => skip(5)}
        disabled={!audioReady}
      >
        <span class="material-icons-outlined">forward_5</span>
      </button>
      <span class="time-display">
        {formatTime(currentTime)}
        <span class="time-sep">/</span>
        {formatTime(effectiveDuration)}
      </span>
    </div>
    <div class="controls-right">
      {#if activeSources.length > 1}
        <div class="source-legend">
          {#each activeSources as src (src)}
            <span class="legend-dot" style="background: {sourceColor(src)}"></span>
            <span class="legend-label">{SOURCE_LABELS[src] ?? src}</span>
          {/each}
        </div>
      {/if}
      <button
        type="button"
        class="ctrl-btn mark-btn"
        title="Mark chapter at current position"
        onclick={handleMarkChapter}
        disabled={!audioReady}
      >
        <span class="material-icons-outlined">add_location_alt</span>
      </button>
    </div>
  </div>
</div>

<style>
  .player-strip {
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: rgba(0, 0, 0, 0.2);
    overflow: hidden;
    margin-bottom: 8px;
  }

  .waveform {
    position: relative;
    width: 100%;
    height: 56px;
    cursor: crosshair;
    touch-action: none;
    overflow: hidden;
  }

  .waveform.dragging {
    cursor: grabbing;
  }

  .waveform canvas {
    display: block;
    width: 100%;
    height: 100%;
  }

  .hover-tip {
    position: absolute;
    top: -2px;
    transform: translate(-50%, -100%);
    padding: 2px 6px;
    background: rgba(0, 0, 0, 0.9);
    border: 1px solid rgba(255, 255, 255, 0.15);
    border-radius: 3px;
    font-size: 10px;
    font-variant-numeric: tabular-nums;
    color: white;
    white-space: nowrap;
    pointer-events: none;
    z-index: 2;
  }

  .controls {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 4px 8px;
    border-top: 1px solid rgba(255, 255, 255, 0.06);
  }

  .controls-left {
    display: flex;
    align-items: center;
    gap: 2px;
  }

  .controls-right {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .ctrl-btn {
    width: 28px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.12s;
  }

  .ctrl-btn:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.08);
    color: var(--ink);
  }

  .ctrl-btn:disabled {
    opacity: 0.3;
    cursor: default;
  }

  .ctrl-btn .material-icons-outlined {
    font-size: 18px;
  }

  .play-btn {
    width: 32px;
    height: 32px;
  }

  .play-btn .material-icons-outlined {
    font-size: 22px;
  }

  .mark-btn {
    color: var(--accent);
  }

  .mark-btn:hover:not(:disabled) {
    background: var(--accent-dim);
    color: var(--accent);
  }

  .time-display {
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    color: var(--muted);
    margin-left: 6px;
    user-select: none;
  }

  .time-sep {
    opacity: 0.4;
    margin: 0 1px;
  }

  .source-legend {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 10px;
    color: var(--muted);
    opacity: 0.7;
  }

  .legend-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .legend-label {
    margin-right: 6px;
  }

  .legend-label:last-child {
    margin-right: 0;
  }

  @media (max-width: 500px) {
    .source-legend {
      display: none;
    }
  }
</style>
