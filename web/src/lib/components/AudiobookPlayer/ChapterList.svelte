<script lang="ts">
  import type { UnifiedChapter } from '$lib/types';
  import {
    getAudioPlayerState,
    seekToUnifiedChapter,
    formatTime
  } from '$lib/stores/audioplayer.svelte';

  const player = getAudioPlayerState();

  const SOURCE_PRIORITY = ['embedded', 'provider', 'transcript', 'detected', 'filename'];

  const sourceIcons: Record<string, string> = {
    embedded: 'library_music',
    transcript: 'smart_toy',
    detected: 'graphic_eq',
    provider: 'cloud_download',
    filename: 'insert_drive_file'
  };

  const sourceLabels: Record<string, string> = {
    embedded: 'Embedded',
    transcript: 'Text cues',
    detected: 'Silence',
    provider: 'Provider',
    filename: 'Filename'
  };

  const sourceColors: Record<string, string> = {
    embedded: '#4ade80',
    transcript: '#60a5fa',
    detected: '#f59e0b',
    provider: '#c084fc',
    filename: '#94a3b8'
  };

  function sourceIcon(source: string): string {
    return sourceIcons[source] ?? 'info';
  }

  function sourceLabel(source: string): string {
    return sourceLabels[source] ?? source;
  }

  // Compute available sources from all chapters
  const availableSources = $derived.by(() => {
    const sources = new Set<string>();
    for (const ch of player.unifiedChapters) {
      if (ch.source) sources.add(ch.source);
    }
    return SOURCE_PRIORITY.filter((s) => sources.has(s));
  });

  // Track active sources — default to highest-priority one
  let activeSources = $state<Set<string>>(new Set());
  let lastAvailableKey = '';

  // Re-initialize active sources when available sources change (new book loaded)
  $effect(() => {
    const key = availableSources.join(',');
    if (key !== lastAvailableKey) {
      lastAvailableKey = key;
      if (availableSources.length > 0) {
        activeSources = new Set([availableSources[0]]);
      } else {
        activeSources = new Set();
      }
    }
  });

  function toggleSource(source: string) {
    const next = new Set(activeSources);
    if (next.has(source)) {
      // Don't allow deselecting the last active source
      if (next.size > 1) {
        next.delete(source);
      }
    } else {
      next.add(source);
    }
    activeSources = next;
  }

  // Filter chapters by active sources
  const filteredChapters = $derived.by(() => {
    if (activeSources.size === 0) return player.unifiedChapters;
    return player.unifiedChapters.filter((ch) => activeSources.has(ch.source));
  });

  function chapterDuration(ch: UnifiedChapter): string {
    return formatTime(ch.end_seconds - ch.start_seconds);
  }

  function isCurrentChapter(ch: UnifiedChapter): boolean {
    return player.currentChapter?.id === ch.id;
  }

  // Group unified chapters by file for multi-file books
  type FileGroup = { fileId: string; filePath: string; chapters: UnifiedChapter[] };

  function groupByFile(chapters: UnifiedChapter[]): FileGroup[] {
    const map = new Map<string, FileGroup>();
    for (const ch of chapters) {
      const existing = map.get(ch.file_id);
      if (existing) {
        existing.chapters.push(ch);
      } else {
        const fileName =
          ch.file_path
            .split('/')
            .pop()
            ?.replace(/\.[^.]+$/, '') ?? ch.file_path;
        map.set(ch.file_id, { fileId: ch.file_id, filePath: fileName, chapters: [ch] });
      }
    }
    return [...map.values()];
  }

  const fileGroups = $derived(groupByFile(filteredChapters));
  const isMultiFile = $derived(fileGroups.length > 1);
  const showSourceFilter = $derived(availableSources.length > 1);
</script>

<div class="chapter-list">
  <div class="chapter-header">
    <h3 class="chapter-heading">
      <span class="material-icons-outlined">queue_music</span>
      Chapters
    </h3>
    {#if showSourceFilter}
      <div class="source-filter">
        {#each availableSources as source (source)}
          <button
            type="button"
            class="source-btn"
            class:active={activeSources.has(source)}
            title={sourceLabel(source)}
            onclick={() => toggleSource(source)}
          >
            <span
              class="source-dot"
              style="background: {sourceColors[source] ?? 'rgba(255,255,255,0.5)'}"
            ></span>
            <span class="material-icons-outlined">{sourceIcon(source)}</span>
          </button>
        {/each}
      </div>
    {/if}
  </div>

  {#if filteredChapters.length === 0}
    <p class="no-chapters">No chapter information available</p>
  {:else}
    <ul class="chapters">
      {#each fileGroups as group (group.fileId)}
        {#if isMultiFile}
          <li class="file-separator">
            <span class="file-icon material-icons-outlined">audio_file</span>
            <span class="file-name">{group.filePath}</span>
          </li>
        {/if}
        {#each group.chapters as chapter (chapter.id)}
          <li>
            <button
              type="button"
              class="chapter-item"
              class:active={isCurrentChapter(chapter)}
              onclick={() => seekToUnifiedChapter(chapter)}
            >
              <span class="chapter-index">{chapter.chapter_index + 1}</span>
              <span class="chapter-info">
                <span class="chapter-title-row">
                  <span class="chapter-title"
                    >{chapter.title || `Chapter ${chapter.chapter_index + 1}`}</span
                  >
                  {#if showSourceFilter && chapter.source && chapter.source !== 'embedded'}
                    <span class="source-badge" title={sourceLabel(chapter.source)}>
                      <span class="material-icons-outlined">{sourceIcon(chapter.source)}</span>
                    </span>
                  {/if}
                </span>
                <span class="chapter-duration">{chapterDuration(chapter)}</span>
              </span>
              {#if isCurrentChapter(chapter)}
                <span class="now-playing-indicator"></span>
              {/if}
            </button>
          </li>
        {/each}
      {/each}
    </ul>
  {/if}
</div>

<style>
  .chapter-list {
    display: flex;
    flex-direction: column;
    max-height: 100%;
    width: 100%;
    min-width: 0;
    overflow: hidden;
  }

  .chapter-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
    padding: 0 4px;
  }

  .chapter-heading {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 0.9rem;
    font-weight: 600;
    color: var(--ink);
    margin: 0;
  }

  .chapter-heading .material-icons-outlined {
    font-size: 18px;
  }

  .source-filter {
    display: flex;
    gap: 2px;
  }

  .source-btn {
    display: flex;
    align-items: center;
    gap: 3px;
    padding: 3px 6px;
    border: 1px solid transparent;
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
    opacity: 0.4;
  }

  .source-btn:hover {
    opacity: 0.8;
    background: rgba(255, 255, 255, 0.05);
  }

  .source-btn.active {
    opacity: 1;
    border-color: var(--ring);
    background: rgba(255, 255, 255, 0.06);
  }

  .source-btn .material-icons-outlined {
    font-size: 14px;
  }

  .source-dot {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .no-chapters {
    color: var(--muted);
    font-size: 0.85rem;
    padding: 12px 4px;
    margin: 0;
  }

  .chapters {
    list-style: none;
    padding: 0;
    margin: 0;
    overflow-y: auto;
    flex: 1;
  }

  .chapters li {
    margin: 0;
  }

  .file-separator {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 12px 4px;
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.03em;
    border-top: 1px solid rgba(255, 255, 255, 0.06);
    margin-top: 4px;
  }

  .file-separator:first-child {
    border-top: none;
    margin-top: 0;
  }

  .file-icon {
    font-size: 14px;
  }

  .file-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chapter-item {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    background: none;
    border: none;
    color: var(--ink);
    cursor: pointer;
    border-radius: 8px;
    text-align: left;
    transition: background 0.15s;
  }

  .chapter-item:hover {
    background: rgba(255, 255, 255, 0.05);
  }

  .chapter-item.active {
    background: var(--accent-dim);
  }

  .chapter-index {
    font-size: 0.75rem;
    color: var(--muted);
    min-width: 20px;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }

  .chapter-item.active .chapter-index {
    color: var(--accent);
    font-weight: 600;
  }

  .chapter-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }

  .chapter-title-row {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }

  .chapter-title {
    font-size: 0.85rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .source-badge {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    color: var(--muted);
    cursor: help;
  }

  .source-badge .material-icons-outlined {
    font-size: 14px;
  }

  .chapter-item.active .chapter-title {
    color: var(--accent);
    font-weight: 500;
  }

  .chapter-duration {
    font-size: 0.75rem;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .now-playing-indicator {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent);
    flex-shrink: 0;
    animation: pulse 2s ease-in-out infinite;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.4;
    }
  }
</style>
