<script lang="ts">
  import { getAudioPlayerState, seekToGlobal, formatTime } from '$lib/stores/audioplayer.svelte';

  interface TranscriptSegment {
    id: string;
    file_id: string;
    segment_index: number;
    start_seconds: number;
    end_seconds: number;
    text: string;
    chapter_title?: string;
  }

  let { segments }: { segments: TranscriptSegment[] } = $props();

  const player = getAudioPlayerState();

  // Project segments onto the global book timeline: segment.start_seconds is
  // file-relative, so add the file's cumulative offset for monotonic global
  // times across the whole transcript.
  const globalSegments = $derived.by(() => {
    return segments.map((s) => {
      const offset = player.globalTimeForFile(s.file_id, 0);
      return {
        ...s,
        global_start: offset + s.start_seconds,
        global_end: offset + s.end_seconds
      };
    });
  });

  const activeIndex = $derived.by(() => {
    if (globalSegments.length === 0) return -1;
    const time = player.globalCurrentTime;
    let lo = 0;
    let hi = globalSegments.length - 1;
    let result = -1;
    while (lo <= hi) {
      const mid = (lo + hi) >>> 1;
      if (globalSegments[mid].global_start <= time) {
        result = mid;
        lo = mid + 1;
      } else {
        hi = mid - 1;
      }
    }
    if (result >= 0 && time <= globalSegments[result].global_end) return result;
    return -1;
  });

  // Auto-scroll to keep active segment visible
  $effect(() => {
    if (activeIndex < 0) return;
    const el = document.querySelector(`[data-segment-index="${activeIndex}"]`);
    if (el) {
      el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    }
  });

  function segmentClass(index: number): string {
    if (index === activeIndex) return 'segment active';
    if (activeIndex >= 0 && index < activeIndex) return 'segment past';
    return 'segment future';
  }
</script>

<div class="transcript-subtitles">
  {#each globalSegments as segment, i (segment.id)}
    <button
      type="button"
      class={segmentClass(i)}
      data-segment-index={i}
      onclick={() => seekToGlobal(segment.global_start)}
    >
      <span class="timestamp">{formatTime(segment.global_start)}</span>
      <span class="text">{segment.text}</span>
    </button>
  {/each}
</div>

<style>
  .transcript-subtitles {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-height: 0;
    height: 100%;
    overflow-y: auto;
    scroll-behavior: smooth;
    padding: 4px;
  }

  .segment {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 8px 12px;
    border: none;
    border-radius: 8px;
    cursor: pointer;
    text-align: left;
    color: var(--ink);
    background: none;
    transition:
      background 0.2s ease,
      opacity 0.2s ease,
      font-size 0.2s ease;
  }

  .segment:hover {
    background: rgba(255, 255, 255, 0.06);
  }

  .segment.active {
    background: color-mix(in srgb, var(--accent) 15%, transparent);
    font-size: 1.02rem;
  }

  .segment.past {
    opacity: 0.55;
  }

  .segment.future {
    opacity: 0.4;
  }

  .timestamp {
    flex-shrink: 0;
    font-family: 'JetBrains Mono', 'Fira Code', 'SF Mono', monospace;
    font-variant-numeric: tabular-nums;
    font-size: 0.72rem;
    color: var(--muted);
    padding-top: 2px;
    min-width: 42px;
  }

  .segment.active .timestamp {
    color: var(--accent);
    font-weight: 600;
  }

  .text {
    font-size: 0.88rem;
    line-height: 1.5;
  }

  .segment.active .text {
    font-weight: 500;
  }
</style>
