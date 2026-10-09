<script lang="ts">
  import { onMount } from 'svelte';

  type FileEntry = { id: string; duration: number };

  interface Props {
    bookId: string;
    files: FileEntry[];
    currentTime: number;
    duration: number;
    chapters: Array<{ start_seconds: number; title?: string | null }>;
    bookmarks: Array<{ position_seconds: number }>;
    onSeek: (time: number) => void;
  }

  let { bookId, files, currentTime, duration, chapters, bookmarks, onSeek }: Props = $props();

  let canvas: HTMLCanvasElement | undefined = $state();
  let container: HTMLDivElement | undefined = $state();
  // Per-file peaks payloads (without the 2-byte header). null entry = peaks
  // unavailable for that file → we degrade to the flat fallback bar.
  let perFilePeaks = $state<(Int8Array | null)[]>([]);
  let dragging = $state(false);
  let hoverTime = $state<number | null>(null);
  let containerWidth = $state(0);

  let accentColor = '#818cf8';
  onMount(() => {
    if (container) {
      const computed = getComputedStyle(container).getPropertyValue('--accent').trim();
      if (computed) accentColor = computed;
    }
  });

  // Stable ID list so we re-fetch only when the file set changes.
  const fileIdsKey = $derived(files.map((f) => f.id).join('|'));

  $effect(() => {
    if (!bookId || files.length === 0) {
      perFilePeaks = [];
      return;
    }
    let cancelled = false;
    const ids = fileIdsKey.split('|').filter(Boolean);
    Promise.all(
      ids.map((fid) =>
        fetch(`/api/v1/admin/books/${bookId}/audio/${fid}/peaks`)
          .then((r) => (r.ok ? r.arrayBuffer() : null))
          .then((buf) => {
            if (!buf) return null;
            const data = new Uint8Array(buf);
            if (data.length < 2 || data[0] !== 1) return null;
            // data[1] is peaks_per_second; the same value is currently emitted
            // for every file (4) so we can concatenate without rescaling.
            return new Int8Array(buf, 2);
          })
          .catch(() => null)
      )
    ).then((results) => {
      if (!cancelled) perFilePeaks = results;
    });
    return () => {
      cancelled = true;
    };
  });

  onMount(() => {
    if (!container) return;
    const ro = new ResizeObserver((entries) => {
      containerWidth = entries[0]?.contentRect.width ?? 0;
    });
    ro.observe(container);
    return () => ro.disconnect();
  });

  // Cumulative offsets in seconds for file boundaries on the global timeline.
  const fileOffsets = $derived.by(() => {
    const out: number[] = new Array(files.length);
    let acc = 0;
    for (let i = 0; i < files.length; i++) {
      out[i] = acc;
      acc += files[i].duration || 0;
    }
    return out;
  });

  const peaksReady = $derived(
    perFilePeaks.length === files.length &&
      perFilePeaks.length > 0 &&
      perFilePeaks.every((p) => p !== null)
  );

  $effect(() => {
    if (!canvas || !peaksReady || containerWidth === 0 || duration <= 0) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const dpr = window.devicePixelRatio || 1;
    const w = containerWidth;
    const h = 48;
    canvas.width = w * dpr;
    canvas.height = h * dpr;
    canvas.style.width = `${w}px`;
    canvas.style.height = `${h}px`;
    ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, w, h);

    let totalPairs = 0;
    for (const p of perFilePeaks) totalPairs += (p?.length ?? 0) / 2;
    if (totalPairs === 0) return;

    const mid = h / 2;
    const progressFrac = duration > 0 ? currentTime / duration : 0;
    const barW = Math.max(1, w / totalPairs);

    let pairIdx = 0;
    for (const peaks of perFilePeaks) {
      if (!peaks) continue;
      const numPairs = peaks.length / 2;
      for (let i = 0; i < numPairs; i++) {
        const x = (pairIdx / totalPairs) * w;
        const minVal = peaks[i * 2] / 128;
        const maxVal = peaks[i * 2 + 1] / 128;
        const top = mid - maxVal * mid;
        const bottom = mid - minVal * mid;
        const barH = Math.max(1, bottom - top);
        const frac = pairIdx / totalPairs;
        ctx.fillStyle = frac <= progressFrac ? accentColor : 'rgba(255, 255, 255, 0.2)';
        ctx.fillRect(x, top, barW, barH);
        pairIdx++;
      }
    }

    // File-boundary ticks (skip 0 — that's the start of the bar).
    for (let i = 1; i < fileOffsets.length; i++) {
      const x = (fileOffsets[i] / duration) * w;
      ctx.fillStyle = 'rgba(255, 255, 255, 0.18)';
      ctx.fillRect(x - 0.5, h * 0.25, 1, h * 0.5);
    }

    // Chapter markers (start_seconds is global).
    for (const ch of chapters) {
      const x = (ch.start_seconds / duration) * w;
      ctx.fillStyle = 'rgba(255, 255, 255, 0.5)';
      ctx.fillRect(x - 0.5, 0, 1, h);
    }

    for (const bm of bookmarks) {
      const x = (bm.position_seconds / duration) * w;
      ctx.fillStyle = '#f59e0b';
      ctx.beginPath();
      ctx.arc(x, 4, 3, 0, Math.PI * 2);
      ctx.fill();
    }
  });

  function handlePointerDown(e: PointerEvent) {
    dragging = true;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    seekToPointer(e);
  }

  function handlePointerMove(e: PointerEvent) {
    if (!container || duration <= 0) return;
    const rect = container.getBoundingClientRect();
    const frac = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    hoverTime = frac * duration;
    if (dragging) {
      seekToPointer(e);
    }
  }

  function handlePointerUp() {
    dragging = false;
  }

  function handlePointerLeave() {
    if (!dragging) hoverTime = null;
  }

  function seekToPointer(e: PointerEvent) {
    if (!container || duration <= 0) return;
    const rect = container.getBoundingClientRect();
    const frac = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    onSeek(frac * duration);
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

<div
  class="waveform-container"
  bind:this={container}
  onpointerdown={handlePointerDown}
  onpointermove={handlePointerMove}
  onpointerup={handlePointerUp}
  onpointerleave={handlePointerLeave}
  role="slider"
  tabindex="0"
  aria-label="Playback progress"
  aria-valuemin={0}
  aria-valuemax={duration}
  aria-valuenow={currentTime}
>
  {#if peaksReady}
    <canvas bind:this={canvas}></canvas>
  {:else}
    <div class="fallback-bar">
      <div
        class="fallback-fill"
        style="width: {duration > 0 ? (currentTime / duration) * 100 : 0}%"
      ></div>
    </div>
  {/if}

  {#if hoverTime !== null}
    <div class="hover-tooltip" style="left: {duration > 0 ? (hoverTime / duration) * 100 : 0}%">
      {formatTime(hoverTime)}
    </div>
  {/if}
</div>

<style>
  .waveform-container {
    position: relative;
    width: 100%;
    height: 48px;
    cursor: pointer;
    touch-action: none;
    border-radius: 4px;
    overflow: hidden;
  }

  canvas {
    display: block;
    width: 100%;
    height: 100%;
  }

  .fallback-bar {
    width: 100%;
    height: 6px;
    margin-top: 21px;
    background: rgba(255, 255, 255, 0.1);
    border-radius: 3px;
    overflow: hidden;
  }

  .fallback-fill {
    height: 100%;
    background: var(--accent, #818cf8);
    border-radius: 3px;
    transition: width 0.1s linear;
  }

  .hover-tooltip {
    position: absolute;
    bottom: 100%;
    transform: translateX(-50%);
    margin-bottom: 4px;
    padding: 2px 8px;
    background: rgba(0, 0, 0, 0.85);
    border-radius: 4px;
    font-size: 12px;
    color: white;
    white-space: nowrap;
    pointer-events: none;
  }
</style>
