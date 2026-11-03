<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import BookReader from '#lib/components/BookReader.svelte';
  import ReadAlongControls from '#lib/components/ReadAlongControls.svelte';
  import {
    getBookFileUrl,
    getReaderProgress,
    updateReadingData,
    getTtsStatus,
    getReadAlongData
  } from '#lib/api/client.js';
  import type {
    TtsVoice,
    ReadAlongSyncPoint,
    ReadAlongTranscriptSegment
  } from '#lib/api/client.js';
  import {
    ProgressTracker,
    isTrackingEnabled,
    setTrackingEnabled
  } from '#lib/utils/reader-progress.js';

  import {
    registerCallbacks,
    notifyPageReady,
    startTts,
    startAudiobookSync,
    skipToSentence,
    stop as stopReadAlong,
    getReadAlongState,
    setReadAlongNowPlaying
  } from '#lib/stores/readalong.svelte.js';
  import { onMount, onDestroy, untrack } from 'svelte';
  import { requestWakeLock, releaseWakeLock } from '#lib/utils/wakelock.js';
  import { getUIState } from '#lib/stores/ui.svelte.js';
  import type { Book } from '#lib/types.js';

  interface TocItem {
    label: string;
    href: string;
    subitems?: TocItem[];
  }

  let { data } = $props();
  const initialData = untrack(() => data);
  const book: Book | null = initialData.book;
  const bookId: string = page.params.id ?? '';

  // Reader state
  let reader = $state<BookReader>();
  let toc = $state<TocItem[]>([]);
  let currentChapter = $state('');
  let progressPercent = $state(0);
  let initialCfi = $state<string | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(untrack(() => data).error);
  let tracking = $state(false);
  let showToc = $state(false);
  let showFinished = $state(false);
  let finishedPromptShown = false;
  let readerTheme = $state<'light' | 'dark' | 'sepia'>(getInitialReaderTheme());
  let fontSize = $state(100);

  // Read-along state
  const readAlong = getReadAlongState();
  let ttsAvailable = $state(false);
  let ttsSupportsEmotion = $state(false);
  let ttsVoices = $state<Record<string, TtsVoice[]>>({});
  let audiobookSyncAvailable = $state(false);
  let readAlongSyncPoints = $state<ReadAlongSyncPoint[]>([]);
  let readAlongTranscripts = $state<ReadAlongTranscriptSegment[]>([]);
  let showReadAlongMenu = $state(false);
  let loadingSyncData = $state(false);

  function closeMenusOnOutsideClick(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (showReadAlongMenu && !target.closest('.readalong-menu-wrapper')) {
      showReadAlongMenu = false;
    }
  }

  function getInitialReaderTheme(): 'light' | 'dark' | 'sepia' {
    if (typeof localStorage !== 'undefined') {
      const saved = localStorage.getItem('kobbler-reader-theme');
      if (saved === 'light' || saved === 'dark' || saved === 'sepia') return saved;
    }
    // Fall back to Kobbler's app theme
    const ui = getUIState();
    return ui.theme === 'dark' ? 'dark' : 'light';
  }

  // Progress tracker (only active when tracking is on)
  let tracker: ProgressTracker | null = null;

  // Keep MediaSession metadata fresh so screen-off / lockscreen shows the right
  // book + chapter while TTS is playing.
  $effect(() => {
    if (!book) return;
    setReadAlongNowPlaying({
      title: currentChapter || book.title,
      artist: book.authors?.map((a) => a.name).join(', ') || book.title,
      artwork: book.cover_url ?? undefined
    });
  });

  // Check if book has any format available
  const hasFormat = (book?.formats ?? []).length > 0;
  // Use explicit format from query param if provided, otherwise let backend pick best
  const requestedFormat = page.url.searchParams.get('format') ?? undefined;
  const fileUrl = hasFormat ? getBookFileUrl(bookId, requestedFormat) : '';

  function handleRelocate(detail: { progress: number; cfi: string; tocItem?: { label: string } }) {
    progressPercent = detail.progress;
    if (detail.tocItem) {
      currentChapter = detail.tocItem.label;
    }
    if (tracking && tracker) {
      tracker.update(detail.progress, detail.cfi);
    }
    // Prompt "finished" when reaching end
    if (detail.progress >= 99 && !finishedPromptShown) {
      finishedPromptShown = true;
      showFinished = true;
    }
    // Relocate is the ONLY signal that the page the read-along just turned to
    // is laid out and its text readable. Firing this on load alone meant the
    // continuation after a turn waited on a promise nothing settled, so
    // playback stopped for good after the first page.
    notifyPageReady();
  }

  function handleLoaded(detail: { toc: TocItem[]; metadata: { title?: string } }) {
    toc = detail.toc;
    loading = false;
    notifyPageReady();
  }

  function toggleTracking() {
    tracking = !tracking;
    setTrackingEnabled(bookId, tracking);
    if (tracking) {
      tracker = new ProgressTracker(bookId);
      tracker.start();
    } else {
      tracker?.destroy();
      tracker = null;
    }
  }

  function goToTocItem(href: string) {
    reader?.goTo(href);
    showToc = false;
  }

  function cycleFontSize(delta: number) {
    fontSize = Math.max(60, Math.min(200, fontSize + delta));
    reader?.setFontSize(fontSize);
  }

  function cycleTheme() {
    const themes: Array<'light' | 'dark' | 'sepia'> = ['light', 'dark', 'sepia'];
    const idx = themes.indexOf(readerTheme);
    readerTheme = themes[(idx + 1) % themes.length];
    localStorage.setItem('kobbler-reader-theme', readerTheme);
    reader?.setReaderTheme(readerTheme);
  }

  async function markFinished() {
    showFinished = false;
    try {
      await updateReadingData(bookId, { status: 'finished' });
    } catch {
      // Best effort
    }
  }

  function dismissFinished() {
    showFinished = false;
  }

  function handleClose() {
    stopReadAlong();
    goto(`/library/books/${bookId}`);
  }

  async function activateTts() {
    showReadAlongMenu = false;
    if (!reader) return;
    registerCallbacks({
      highlight: (s) => reader?.highlightSentence(s),
      highlightWord: (localIdx, sentenceText) => reader?.highlightWord(localIdx, sentenceText),
      clearHighlight: () => reader?.clearHighlight(),
      nextPage: () => reader?.nextPage(),
      getPageText: () => reader?.getPageText() ?? '',
      getSpanText: (maxWords) => reader?.getSpanText(maxWords) ?? Promise.resolve(''),
      goToCfi: (cfi) => reader?.goTo(cfi)
    });
    await startTts();
  }

  async function activateAudiobookSync() {
    showReadAlongMenu = false;
    if (!reader) return;

    // Lazy-load sync data if not loaded yet
    if (readAlongSyncPoints.length === 0) {
      loadingSyncData = true;
      try {
        const data = await getReadAlongData(bookId);
        readAlongSyncPoints = data.sync_points;
        readAlongTranscripts = data.transcript_segments;
      } catch {
        loadingSyncData = false;
        return;
      }
      loadingSyncData = false;
    }

    registerCallbacks({
      highlight: (s) => reader?.highlightSentence(s),
      clearHighlight: () => reader?.clearHighlight(),
      nextPage: () => reader?.nextPage(),
      getPageText: () => reader?.getPageText() ?? '',
      getSpanText: (maxWords) => reader?.getSpanText(maxWords) ?? Promise.resolve(''),
      goToCfi: (cfi) => reader?.goTo(cfi)
    });
    startAudiobookSync(readAlongSyncPoints, readAlongTranscripts);
  }

  onMount(async () => {
    requestWakeLock();

    if (!hasFormat) {
      error = 'No file format available for this book';
      loading = false;
      return;
    }

    tracking = isTrackingEnabled(bookId);

    // Load saved progress if tracking
    if (tracking) {
      try {
        const saved = await getReaderProgress(bookId);
        if (saved.cfi) {
          initialCfi = saved.cfi;
        }
      } catch {
        // No saved progress, start from beginning
      }
      tracker = new ProgressTracker(bookId);
      tracker.start();
    }

    // Check read-along availability (non-blocking)
    getTtsStatus()
      .then((status) => {
        ttsAvailable = status.enabled;
        ttsSupportsEmotion = status.supports_emotion ?? false;
        // Group flat voice list by ID prefix (e.g. Kokoro "af_heart" -> "a", "bf_emma" -> "b")
        // For non-prefixed IDs, group under both "a" and "b" so the reader picker always shows them
        const grouped: Record<string, TtsVoice[]> = {};
        let hasLangPrefix = false;
        for (const v of status.voices) {
          const match = v.id.match(/^([a-z])[fm]_/);
          if (match) {
            hasLangPrefix = true;
            (grouped[match[1]] ??= []).push(v);
          }
        }
        if (!hasLangPrefix) {
          // Non-Kokoro service: show all voices under both language keys
          grouped['a'] = status.voices;
          grouped['b'] = status.voices;
        }
        ttsVoices = grouped;
      })
      .catch(() => {});

    // Check if this book has audiobook sync data
    if (book?.has_audiobook) {
      audiobookSyncAvailable = true;
    }
  });

  onDestroy(() => {
    tracker?.destroy();
    stopReadAlong();
    releaseWakeLock();
  });

  // Flush on page unload
  function handleBeforeUnload() {
    tracker?.flush();
  }
</script>

<svelte:window onbeforeunload={handleBeforeUnload} onclick={closeMenusOnOutsideClick} />

<svelte:head>
  <title>{book?.title ?? 'Reader'} — Kobbler</title>
</svelte:head>

<div class="reader-page" class:readalong-active={readAlong.mode !== 'off'} data-theme={readerTheme}>
  <!-- Toolbar -->
  <header class="reader-toolbar">
    <div class="toolbar-left">
      <button type="button" class="toolbar-btn" onclick={handleClose} title="Close reader"
        ><span class="material-icons-outlined">close</span></button
      >

      <button
        type="button"
        class="toolbar-btn"
        onclick={() => (showToc = !showToc)}
        title="Table of contents"
      >
        <span class="material-icons-outlined">toc</span>
      </button>
      <span class="toolbar-title">{currentChapter || book?.title || ''}</span>
    </div>
    <div class="toolbar-right">
      <button
        type="button"
        class="toolbar-btn"
        class:active={tracking}
        onclick={toggleTracking}
        title={tracking ? 'Tracking progress (click to disable)' : 'Track reading progress'}
      >
        <span class="material-icons-outlined">{tracking ? 'bookmark' : 'bookmark_border'}</span>
      </button>
      <button
        type="button"
        class="toolbar-btn"
        onclick={() => cycleFontSize(-10)}
        title="Decrease font size"
      >
        <span class="material-icons-outlined" style="font-size: 16px">text_decrease</span>
      </button>
      <button
        type="button"
        class="toolbar-btn"
        onclick={() => cycleFontSize(10)}
        title="Increase font size"
      >
        <span class="material-icons-outlined" style="font-size: 20px">text_increase</span>
      </button>
      <button type="button" class="toolbar-btn" onclick={cycleTheme} title="Cycle theme">
        <span class="material-icons-outlined">
          {readerTheme === 'dark'
            ? 'dark_mode'
            : readerTheme === 'sepia'
              ? 'auto_stories'
              : 'light_mode'}
        </span>
      </button>
      <!-- Read-along button -->
      {#if (ttsAvailable || audiobookSyncAvailable) && readAlong.mode === 'off'}
        <div class="readalong-menu-wrapper">
          <button
            type="button"
            class="toolbar-btn"
            onclick={() => (showReadAlongMenu = !showReadAlongMenu)}
            title="Read along"
          >
            <span class="material-icons-outlined">record_voice_over</span>
          </button>
          {#if showReadAlongMenu}
            <div class="readalong-menu">
              {#if ttsAvailable}
                <button type="button" class="readalong-menu-item" onclick={activateTts}>
                  <span class="material-icons-outlined">record_voice_over</span>
                  Read Aloud (TTS)
                </button>
              {/if}
              {#if audiobookSyncAvailable}
                <button
                  type="button"
                  class="readalong-menu-item"
                  onclick={activateAudiobookSync}
                  disabled={loadingSyncData}
                >
                  <span class="material-icons-outlined">
                    {loadingSyncData ? 'hourglass_empty' : 'headphones'}
                  </span>
                  {loadingSyncData ? 'Loading…' : 'Listen Along (Audiobook)'}
                </button>
              {/if}
            </div>
          {/if}
        </div>
      {/if}
      <span class="toolbar-progress">{Math.round(progressPercent)}%</span>
    </div>
  </header>

  <!-- TOC sidebar -->
  {#if showToc}
    <div class="toc-overlay" role="presentation" onclick={() => (showToc = false)}></div>

    <nav class="toc-panel">
      <h2>Contents</h2>
      <ul>
        {#each toc as item, i (`${i}:${item.href}`)}
          <li>
            <button type="button" onclick={() => goToTocItem(item.href)}>{item.label}</button>
            {#if item.subitems?.length}
              <ul>
                {#each item.subitems as sub, j (`${i}:${j}:${sub.href}`)}
                  <li>
                    <button type="button" onclick={() => goToTocItem(sub.href)}>{sub.label}</button>
                  </li>
                {/each}
              </ul>
            {/if}
          </li>
        {/each}
      </ul>
    </nav>
  {/if}

  <!-- Reader content -->
  <div class="reader-content">
    {#if error}
      <div class="reader-error">
        <span class="material-icons-outlined">error_outline</span>
        <p>{error}</p>
        <button type="button" class="btn btn-sec" onclick={handleClose}>Back to book</button>
      </div>
    {:else if loading}
      <div class="reader-loading">
        <div class="spinner"></div>
        <p>Loading book…</p>
      </div>
    {/if}
    {#if hasFormat && !error}
      <BookReader
        bind:this={reader}
        {fileUrl}
        {initialCfi}
        initialTheme={readerTheme}
        onRelocate={handleRelocate}
        onLoaded={handleLoaded}
        onSentenceTap={readAlong.mode === 'tts' ? skipToSentence : undefined}
      />
    {/if}
  </div>

  <!-- Progress bar -->
  <div class="reader-progress-bar">
    <div class="reader-progress-fill" style="width: {progressPercent}%"></div>
  </div>

  <!-- Read-along controls -->
  {#if readAlong.mode !== 'off'}
    <ReadAlongControls voices={ttsVoices} supportsEmotion={ttsSupportsEmotion} />
  {/if}

  <!-- Finished prompt -->
  {#if showFinished}
    <div class="finished-overlay">
      <div class="finished-dialog">
        <span class="material-icons-outlined finished-icon">check_circle</span>
        <h3>Finished reading?</h3>
        <p>Mark <strong>{book?.title}</strong> as finished?</p>
        <div class="finished-actions">
          <button type="button" class="btn btn-primary" onclick={markFinished}>
            Mark as Finished
          </button>
          <button type="button" class="btn btn-sec" onclick={dismissFinished}>
            Keep Reading
          </button>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .reader-page {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: #ffffff;
    color: #1a1a1a;
  }

  .reader-page[data-theme='dark'] {
    background: #000000;
    color: #e0e0e0;
    --readalong-bg: rgba(0, 0, 0, 0.85);
    --readalong-border: rgba(255, 255, 255, 0.1);
  }

  .reader-page[data-theme='sepia'] {
    background: #f4ecd8;
    color: #5b4636;
    --readalong-bg: rgba(210, 190, 160, 0.92);
    --readalong-border: rgba(91, 70, 54, 0.15);
  }

  .reader-page[data-theme='light'] {
    --readalong-bg: rgba(245, 245, 245, 0.92);
    --readalong-border: rgba(0, 0, 0, 0.1);
  }

  .reader-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 4px 8px;
    border-bottom: 1px solid rgba(128, 128, 128, 0.2);
    flex-shrink: 0;
    gap: 4px;
    min-height: 40px;
  }

  .toolbar-left,
  .toolbar-right {
    display: flex;
    align-items: center;
    gap: 2px;
  }

  .toolbar-title {
    font-size: 0.85rem;
    opacity: 0.7;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 300px;
  }

  .toolbar-progress {
    font-size: 0.8rem;
    opacity: 0.6;
    min-width: 3em;
    text-align: right;
  }

  .toolbar-btn {
    background: none;
    border: none;
    cursor: pointer;
    padding: 4px 6px;
    border-radius: 4px;
    color: inherit;
    opacity: 0.7;
    display: flex;
    align-items: center;
  }

  .toolbar-btn:hover {
    opacity: 1;
    background: rgba(128, 128, 128, 0.15);
  }

  .toolbar-btn.active {
    opacity: 1;
    color: var(--accent, #4a90d9);
  }

  .reader-content {
    flex: 1;
    overflow: hidden;
    position: relative;
  }

  .reader-loading,
  .reader-error {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    gap: 12px;
    opacity: 0.7;
  }

  .reader-error .material-icons-outlined {
    font-size: 48px;
    opacity: 0.5;
  }

  .spinner {
    width: 32px;
    height: 32px;
    border: 3px solid rgba(128, 128, 128, 0.3);
    border-top-color: currentColor;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .reader-progress-bar {
    height: 3px;
    background: rgba(128, 128, 128, 0.2);
    flex-shrink: 0;
  }

  .reader-progress-fill {
    height: 100%;
    background: var(--accent, #4a90d9);
    transition: width 0.3s ease;
  }

  /* TOC panel */
  .toc-overlay {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.3);
    z-index: 10;
  }

  .toc-panel {
    position: absolute;
    top: 0;
    left: 0;
    bottom: 0;
    width: min(320px, 80vw);
    background: inherit;
    z-index: 11;
    overflow-y: auto;
    padding: 16px;
    box-shadow: 2px 0 12px rgba(0, 0, 0, 0.2);
  }

  .toc-panel h2 {
    font-size: 1rem;
    margin: 0 0 12px;
  }

  .toc-panel ul {
    list-style: none;
    padding: 0;
    margin: 0;
  }

  .toc-panel li {
    margin: 0;
  }

  .toc-panel button {
    background: none;
    border: none;
    color: inherit;
    cursor: pointer;
    padding: 6px 8px;
    width: 100%;
    text-align: left;
    border-radius: 4px;
    font-size: 0.9rem;
  }

  .toc-panel button:hover {
    background: rgba(128, 128, 128, 0.15);
  }

  .toc-panel ul ul {
    padding-left: 16px;
  }

  .toc-panel ul ul button {
    font-size: 0.85rem;
    opacity: 0.8;
  }

  /* Finished dialog */
  .finished-overlay {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 20;
  }

  .finished-dialog {
    background: inherit;
    padding: 32px;
    border-radius: 12px;
    text-align: center;
    max-width: 360px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.3);
  }

  .finished-icon {
    font-size: 48px;
    color: var(--accent, #4a90d9);
  }

  .finished-dialog h3 {
    margin: 8px 0 4px;
  }

  .finished-dialog p {
    margin: 0 0 20px;
    opacity: 0.8;
    font-size: 0.9rem;
  }

  .finished-actions {
    display: flex;
    gap: 8px;
    justify-content: center;
  }

  /* Read-along dropdown menu */
  .readalong-menu-wrapper {
    position: relative;
  }

  .readalong-menu {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 4px;
    background: var(--card, #1a1a1a);
    border: 1px solid rgba(128, 128, 128, 0.2);
    border-radius: 8px;
    padding: 4px;
    min-width: 200px;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.3);
    z-index: 30;
  }

  .readalong-menu-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 12px;
    background: none;
    border: none;
    color: inherit;
    cursor: pointer;
    border-radius: 6px;
    font-size: 0.85rem;
    text-align: left;
  }

  .readalong-menu-item:hover:not(:disabled) {
    background: rgba(128, 128, 128, 0.15);
  }

  .readalong-menu-item:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .readalong-menu-item .material-icons-outlined {
    font-size: 18px;
    opacity: 0.8;
  }

  /* Push content up when read-along controls are visible */
  .reader-page.readalong-active {
    padding-bottom: 52px;
  }

  @media (max-width: 600px) {
    .toolbar-title {
      display: none;
    }
  }
</style>
