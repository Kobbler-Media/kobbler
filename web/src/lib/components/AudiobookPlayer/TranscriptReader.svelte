<script lang="ts">
  import { getAudioPlayerState, seekToGlobal } from '#lib/stores/audioplayer.svelte.js';

  interface TranscriptSegment {
    id: string;
    file_id: string;
    segment_index: number;
    start_seconds: number;
    end_seconds: number;
    text: string;
    chapter_title?: string;
  }

  type GlobalSegment = TranscriptSegment & {
    global_start: number;
    global_end: number;
    global_index: number;
  };

  interface Paragraph {
    chapter_title?: string;
    segments: GlobalSegment[];
  }

  let { segments }: { segments: TranscriptSegment[] } = $props();

  const player = getAudioPlayerState();

  const PAUSE_THRESHOLD = 2.0;
  const MAX_SEGMENTS_PER_PARA = 6;

  // Project to the global book timeline so paragraph breaks (gap detection)
  // and the active-segment search work uniformly across file boundaries.
  const globalSegments = $derived.by<GlobalSegment[]>(() => {
    return segments.map((s, i) => {
      const offset = player.globalTimeForFile(s.file_id, 0);
      return {
        ...s,
        global_start: offset + s.start_seconds,
        global_end: offset + s.end_seconds,
        global_index: i
      };
    });
  });

  const paragraphs = $derived.by(() => {
    if (globalSegments.length === 0) return [] as Paragraph[];

    const result: Paragraph[] = [];
    let current: Paragraph = { segments: [], chapter_title: globalSegments[0].chapter_title };

    for (let i = 0; i < globalSegments.length; i++) {
      const seg = globalSegments[i];
      const prev = i > 0 ? globalSegments[i - 1] : null;

      const chapterChanged = prev && seg.chapter_title !== prev.chapter_title;
      const longPause = prev && seg.global_start - prev.global_end > PAUSE_THRESHOLD;
      const tooLong = current.segments.length >= MAX_SEGMENTS_PER_PARA;

      if (current.segments.length > 0 && (chapterChanged || longPause || tooLong)) {
        result.push(current);
        current = { segments: [], chapter_title: chapterChanged ? seg.chapter_title : undefined };
      } else if (chapterChanged) {
        current.chapter_title = seg.chapter_title;
      }

      current.segments.push(seg);
    }

    if (current.segments.length > 0) result.push(current);
    return result;
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

  const activeSegmentId = $derived(activeIndex >= 0 ? globalSegments[activeIndex].id : '');

  $effect(() => {
    if (!activeSegmentId) return;
    const el = document.getElementById(`tr-seg-${activeSegmentId}`);
    if (el) {
      el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    }
  });

  function segmentState(seg: GlobalSegment): 'active' | 'past' | 'future' {
    if (seg.id === activeSegmentId) return 'active';
    if (activeIndex >= 0 && seg.global_index < activeIndex) return 'past';
    return 'future';
  }
</script>

<div class="transcript-reader">
  {#each paragraphs as para, pi (pi)}
    {#if para.chapter_title}
      <h2 class="chapter-heading">{para.chapter_title}</h2>
    {/if}
    <p class="para">
      {#each para.segments as seg (seg.id)}
        <button
          type="button"
          id="tr-seg-{seg.id}"
          class="sentence {segmentState(seg)}"
          onclick={() => seekToGlobal(seg.global_start)}>{seg.text}</button
        >
      {/each}
    </p>
  {/each}
</div>

<style>
  .transcript-reader {
    width: 100%;
    max-width: 640px;
    margin: 0 auto;
    padding: 24px 20px 80px;
    overflow-y: auto;
    flex: 1;
    min-height: 0;
  }

  .chapter-heading {
    font-size: 1.15rem;
    font-weight: 600;
    color: var(--ink);
    margin: 32px 0 12px;
    padding-bottom: 8px;
    border-bottom: 1px solid var(--ring);
    opacity: 0.85;
  }

  .chapter-heading:first-child {
    margin-top: 0;
  }

  .para {
    margin: 0 0 20px;
    line-height: 1.8;
    font-size: 1.05rem;
  }

  .sentence {
    all: unset;
    cursor: pointer;
    border-radius: 3px;
    transition:
      color 0.2s ease,
      background 0.2s ease;
    padding: 1px 0;
    margin-right: 0.25em;
  }

  .sentence.active {
    color: var(--ink);
    background: color-mix(in srgb, var(--accent) 20%, transparent);
    border-radius: 3px;
    padding: 1px 2px;
    margin: 0 -2px;
  }

  .sentence.past {
    color: var(--muted);
  }

  .sentence.future {
    color: var(--ink);
    opacity: 0.55;
  }

  .sentence:hover {
    background: rgba(255, 255, 255, 0.08);
  }

  @media (max-width: 600px) {
    .transcript-reader {
      padding: 16px 14px 60px;
    }

    .para {
      font-size: 0.95rem;
      line-height: 1.7;
    }
  }
</style>
