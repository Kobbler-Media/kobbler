<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { VideoChapter, SkipSegment } from '#lib/types.js';
  import { SEGMENT_COLORS, isVisibleSkipSegment, segmentTitle } from './segments';

  interface Props {
    chapters: VideoChapter[];
    skipSegments: SkipSegment[];
    currentTime: number;
    onSeek: (time: number) => void;
    isOpen?: boolean;
    onToggle?: () => void;
  }

  let { chapters, skipSegments, currentTime, onSeek, isOpen, onToggle }: Props = $props();
  let internalOpen = $state(false);
  let open = $derived(isOpen ?? internalOpen);
  let wrapperEl: HTMLDivElement | undefined = $state();

  function handleClickOutside(e: MouseEvent) {
    if (wrapperEl && !wrapperEl.contains(e.target as Node)) {
      if (onToggle && open) onToggle();
      else internalOpen = false;
    }
  }

  onMount(() => {
    document.addEventListener('click', handleClickOutside, true);
  });

  onDestroy(() => {
    document.removeEventListener('click', handleClickOutside, true);
  });

  let currentChapter = $derived(chapters.findLast((ch) => currentTime >= ch.start_seconds) ?? null);
  let detectedSegments = $derived(skipSegments.filter(isVisibleSkipSegment));

  function formatTime(seconds: number): string {
    if (!isFinite(seconds) || seconds < 0) return '0:00';
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = Math.floor(seconds % 60);
    const pad = (n: number) => n.toString().padStart(2, '0');
    return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
  }

  function close() {
    if (onToggle) onToggle();
    else internalOpen = false;
  }

  function handleSelectChapter(chapter: VideoChapter) {
    onSeek(chapter.start_seconds);
    close();
  }

  function handleSelectSegment(seg: SkipSegment) {
    onSeek(seg.start_seconds);
    close();
  }
</script>

{#if chapters.length > 0 || detectedSegments.length > 0}
  <div class="chapter-menu-wrapper" bind:this={wrapperEl}>
    <button
      class="ctrl-btn"
      type="button"
      onclick={() => {
        if (onToggle) onToggle();
        else internalOpen = !internalOpen;
      }}
      aria-label="Chapters"
      title="Chapters"
    >
      <span class="material-icons-outlined">toc</span>
    </button>

    {#if open}
      <div class="chapter-dropdown">
        {#if chapters.length > 0}
          <div class="chapter-dropdown-header">Chapters</div>
          <ul class="chapter-list">
            {#each chapters as chapter (chapter.index)}
              <li>
                <button
                  class="chapter-item"
                  class:active={currentChapter?.index === chapter.index}
                  type="button"
                  onclick={() => handleSelectChapter(chapter)}
                >
                  <span class="ch-time">{formatTime(chapter.start_seconds)}</span>
                  <span class="ch-title">{chapter.title || `Chapter ${chapter.index + 1}`}</span>
                </button>
              </li>
            {/each}
          </ul>
        {/if}
        {#if detectedSegments.length > 0}
          <div class="chapter-dropdown-header">Detected</div>
          <ul class="chapter-list">
            {#each detectedSegments as seg, i (i)}
              <li>
                <button
                  class="chapter-item detected"
                  type="button"
                  onclick={() => handleSelectSegment(seg)}
                >
                  <span class="ch-swatch" style="background: {SEGMENT_COLORS[seg.segment_type]}"
                  ></span>
                  <span class="ch-body">
                    <span class="ch-title">{segmentTitle(seg)}</span>
                    <span class="ch-range"
                      >{formatTime(seg.start_seconds)}–{formatTime(seg.end_seconds)}</span
                    >
                  </span>
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    {/if}
  </div>
{/if}

<style>
  .chapter-menu-wrapper {
    position: relative;
  }

  .ctrl-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 40px;
    height: 40px;
    border-radius: 50%;
    border: none;
    background: transparent;
    color: white;
    cursor: pointer;
    transition: background 0.15s;
    pointer-events: auto;
  }

  .ctrl-btn:hover {
    background: rgba(255, 255, 255, 0.15);
  }

  .chapter-dropdown {
    position: absolute;
    bottom: 100%;
    left: 0;
    margin-bottom: 8px;
    width: 320px;
    max-height: 400px;
    overflow-y: auto;
    background: rgba(0, 0, 0, 0.92);
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.1);
    backdrop-filter: blur(12px);
    pointer-events: auto;
    z-index: 20;
  }

  .chapter-dropdown-header {
    padding: 10px 14px 6px;
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: rgba(255, 255, 255, 0.5);
  }

  .chapter-list {
    list-style: none;
    margin: 0;
    padding: 0 4px 4px;
  }

  .chapter-item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 10px;
    border: none;
    background: transparent;
    color: rgba(255, 255, 255, 0.8);
    font-size: 13px;
    cursor: pointer;
    border-radius: 4px;
    text-align: left;
  }

  .chapter-item:hover {
    background: rgba(255, 255, 255, 0.1);
  }

  .chapter-item.active {
    color: var(--accent, #818cf8);
    font-weight: 600;
  }

  .ch-time {
    font-variant-numeric: tabular-nums;
    color: rgba(255, 255, 255, 0.4);
    min-width: 50px;
    flex-shrink: 0;
  }

  .chapter-item.active .ch-time {
    color: var(--accent, #818cf8);
    opacity: 0.7;
  }

  .ch-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .ch-swatch {
    display: block;
    width: 6px;
    min-width: 6px;
    height: 24px;
    border-radius: 2px;
    flex-shrink: 0;
  }

  .ch-body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    overflow: hidden;
  }

  .ch-range {
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    color: rgba(255, 255, 255, 0.4);
  }

  .chapter-item.detected {
    align-items: center;
  }
</style>
