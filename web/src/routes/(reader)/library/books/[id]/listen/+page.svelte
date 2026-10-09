<script lang="ts">
  import { goto } from '$app/navigation';
  import { untrack, onMount, onDestroy } from 'svelte';
  import { requestWakeLock, releaseWakeLock } from '$lib/utils/wakelock';
  import AudioControls from '$lib/components/AudiobookPlayer/AudioControls.svelte';
  import ChapterBookmarkTabs from '$lib/components/AudiobookPlayer/ChapterBookmarkTabs.svelte';
  import WaveformBar from '$lib/components/AudiobookPlayer/WaveformBar.svelte';
  import TranscriptSearch from '$lib/components/AudiobookPlayer/TranscriptSearch.svelte';
  import TranscriptSubtitles from '$lib/components/AudiobookPlayer/TranscriptSubtitles.svelte';
  import TranscriptReader from '$lib/components/AudiobookPlayer/TranscriptReader.svelte';
  import {
    hasTranscript,
    transcribeAudiobook,
    getReaderProgress,
    getSyncPosition,
    getTranscriptSegments
  } from '$lib/api/client';
  import type { SyncPositionResult, ReadAlongTranscriptSegment } from '$lib/api/client';
  import ChapterEditor from '$lib/components/ChapterEditor.svelte';
  import {
    getAudioPlayerState,
    play,
    seek,
    seekToGlobal,
    skipForward,
    skipBackward,
    togglePlayPause,
    setRate,
    stop as stopPlayer,
    formatTime,
    recordInteraction
  } from '$lib/stores/audioplayer.svelte';
  import type { Book } from '$lib/types';

  let { data } = $props();
  const initialData = untrack(() => data);
  const book: Book | null = initialData.book;
  const audioFileIds: string[] = initialData.audioFileIds ?? [];
  const audioFileDurations: number[] = initialData.audioFileDurations ?? [];
  const audioFileIntroOutro: Record<
    string,
    { intro_end_seconds?: number | null; outro_start_seconds?: number | null }
  > = initialData.audioFileIntroOutro ?? {};
  const loadError: string | null = initialData.error;

  const player = getAudioPlayerState();

  let error = $state<string | null>(loadError);
  let showChapterEditor = $state(false);
  let isFullscreen = $state(false);
  let pageEl = $state<HTMLElement | null>(null);
  let hasAudioTranscript = $state(false);
  let transcriptMode = $state('off');
  let transcribing = $state(false);
  let syncOffer = $state<SyncPositionResult | null>(null);
  let syncDismissed = $state(false);
  type ViewMode = 'cover' | 'subtitles' | 'reader';

  function getInitialViewMode(): ViewMode {
    if (typeof localStorage === 'undefined') return 'cover';
    const saved = localStorage.getItem('kobbler-listen-view');
    if (saved === 'subtitles' || saved === 'reader') return saved;
    // Migrate old boolean preference
    if (localStorage.getItem('kobbler-listen-subtitles') === 'true') return 'subtitles';
    return 'cover';
  }

  let viewMode = $state<ViewMode>(getInitialViewMode());
  let transcriptSegments = $state<ReadAlongTranscriptSegment[]>([]);

  // Intro/outro boundaries for the currently playing file
  const currentFileIntroOutro = $derived.by(() => {
    const fid = player.currentFileId;
    if (!fid) return null;
    return audioFileIntroOutro[fid] ?? null;
  });
  const introEndSeconds = $derived(currentFileIntroOutro?.intro_end_seconds ?? null);
  const outroStartSeconds = $derived(currentFileIntroOutro?.outro_start_seconds ?? null);

  function handleClose() {
    // If paused, treat back as a full close so the mini player goes away too.
    if (!player.playing) {
      stopPlayer();
    }
    if (book) {
      goto(`/library/books/${book.id}`);
    } else {
      goto('/library');
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    recordInteraction();
    // Don't interfere with form inputs
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;

    switch (e.key) {
      case ' ':
        e.preventDefault();
        togglePlayPause();
        break;
      case 'ArrowLeft':
        e.preventDefault();
        skipBackward(30);
        break;
      case 'ArrowRight':
        e.preventDefault();
        skipForward(30);
        break;
      case '[':
        setRate(Math.max(0.5, player.playbackRate - 0.25));
        break;
      case ']':
        setRate(Math.min(3, player.playbackRate + 0.25));
        break;
      case 'f':
        toggleFullscreen();
        break;
    }
  }

  function cycleViewMode() {
    const hasSegments = transcriptSegments.length > 0;
    if (!hasSegments) return;
    const modes: ViewMode[] = ['cover', 'subtitles', 'reader'];
    const next = modes[(modes.indexOf(viewMode) + 1) % modes.length];
    viewMode = next;
    localStorage.setItem('kobbler-listen-view', next);
  }

  function toggleFullscreen() {
    if (!pageEl) return;
    if (document.fullscreenElement) {
      screen.orientation?.unlock();
      document.exitFullscreen();
    } else {
      pageEl
        .requestFullscreen()
        .then(() => {
          (screen.orientation as any)?.lock?.('landscape')?.catch?.(() => {});
        })
        .catch(() => {
          // Fullscreen not supported or denied
        });
    }
  }

  function handleFullscreenChange() {
    isFullscreen = !!document.fullscreenElement;
  }

  function handleBeforeUnload() {
    // Position is auto-saved by the store
  }

  onDestroy(() => {
    releaseWakeLock();
    document.removeEventListener('fullscreenchange', handleFullscreenChange);
  });

  onMount(async () => {
    requestWakeLock();
    document.addEventListener('fullscreenchange', handleFullscreenChange);

    if (!book || audioFileIds.length === 0) {
      if (!error) error = 'No audiobook file found for this book';
      return;
    }

    // Only start playing if not already playing this book
    if (player.currentBook?.id !== book.id) {
      try {
        await play(book, audioFileIds, audioFileDurations);
      } catch {
        error = 'Failed to start audiobook playback';
      }
    }

    // Check transcript availability
    try {
      const status = await hasTranscript(book.id);
      hasAudioTranscript = status.has_audio_transcript;
      transcriptMode = status.transcript_mode;
    } catch {
      // Not critical
    }

    // Load transcript segments if available
    if (hasAudioTranscript) {
      try {
        transcriptSegments = await getTranscriptSegments(book.id);
      } catch {
        // Not critical
      }
    }

    try {
      const progress = await getReaderProgress(book.id);
      if (progress.cfi) {
        const result = await getSyncPosition(book.id, { from: 'ebook', cfi: progress.cfi });
        if (result && result.seconds != null && result.file_id) {
          const targetGlobal = player.globalTimeForFile(result.file_id, result.seconds);
          if (Math.abs(targetGlobal - player.globalCurrentTime) > 30) {
            syncOffer = result;
          }
        }
      }
    } catch {
      // Not critical
    }
  });
</script>

<svelte:window
  onbeforeunload={handleBeforeUnload}
  onkeydown={handleKeydown}
  onclick={recordInteraction}
  ontouchstart={recordInteraction}
/>

<svelte:head>
  <title>{book?.title ?? 'Audiobook'} — Listen — Kobbler</title>
</svelte:head>

<div class="listen-page" bind:this={pageEl}>
  <!-- Header -->
  <header class="listen-header">
    <button type="button" class="header-btn" onclick={handleClose} title="Back to book">
      <span class="material-icons-outlined">arrow_back</span>
    </button>
    <span class="header-title">Now Playing</span>
    <div class="header-spacer"></div>
    {#if hasAudioTranscript}
      <button
        type="button"
        class="header-btn"
        class:active={viewMode !== 'cover'}
        onclick={cycleViewMode}
        title={viewMode === 'cover'
          ? 'Show subtitles'
          : viewMode === 'subtitles'
            ? 'Show reader view'
            : 'Show cover art'}
      >
        <span class="material-icons-outlined">
          {viewMode === 'reader' ? 'auto_stories' : 'subtitles'}
        </span>
      </button>
    {/if}
    <button
      type="button"
      class="header-btn"
      onclick={() => (showChapterEditor = true)}
      title="Edit chapters"
    >
      <span class="material-icons-outlined">edit_note</span>
    </button>
    <button
      type="button"
      class="header-btn"
      onclick={toggleFullscreen}
      title={isFullscreen ? 'Exit fullscreen' : 'Fullscreen'}
    >
      <span class="material-icons-outlined">
        {isFullscreen ? 'fullscreen_exit' : 'fullscreen'}
      </span>
    </button>
  </header>

  {#if error}
    <div class="listen-error">
      <p>{error}</p>
      <button type="button" class="btn btn-sec" onclick={handleClose}>Back to book</button>
    </div>
  {:else}
    <div
      class="listen-body"
      class:two-col={viewMode === 'cover'}
      class:scroll={viewMode === 'cover'}
      class:fit={viewMode !== 'cover'}
    >
      <!-- Cross-format sync banner -->
      {#if syncOffer && !syncDismissed && syncOffer.seconds != null}
        <div class="sync-banner">
          <div class="sync-banner-text">
            <span class="material-icons-outlined">menu_book</span>
            <span>You were reading nearby. Continue listening from there?</span>
          </div>
          {#if syncOffer.text_snippet}
            <p class="sync-snippet">"{syncOffer.text_snippet}"</p>
          {/if}
          <div class="sync-banner-actions">
            <button type="button" class="btn btn-sec btn-sm" onclick={() => (syncDismissed = true)}>
              Dismiss
            </button>
            <button
              type="button"
              class="btn btn-primary btn-sm"
              onclick={() => {
                if (syncOffer?.seconds != null && syncOffer.file_id) {
                  seekToGlobal(player.globalTimeForFile(syncOffer.file_id, syncOffer.seconds));
                }
                syncDismissed = true;
              }}
            >
              Jump to position
            </button>
          </div>
        </div>
      {/if}

      {#if viewMode === 'reader' && transcriptSegments.length > 0}
        <div class="reader-section">
          <TranscriptReader segments={transcriptSegments} />
        </div>
      {:else if viewMode === 'subtitles' && transcriptSegments.length > 0}
        <div class="subtitle-section">
          <TranscriptSubtitles segments={transcriptSegments} />
        </div>
      {:else}
        <!-- Cover art section -->
        <div class="cover-section">
          {#if book?.cover_url}
            <img src={book.cover_url} alt={book.title} class="cover-art" />
          {:else}
            <div class="cover-placeholder">
              <span class="cover-placeholder-text">No Cover</span>
            </div>
          {/if}
        </div>
      {/if}

      <!-- Player panel: groups controls for two-column desktop layout -->
      <div class="player-panel">
        {#if viewMode !== 'reader'}
          <!-- Track info -->
          <div class="track-info">
            <h1 class="track-title">{book?.title ?? 'Unknown'}</h1>
            {#if book?.authors?.length}
              <p class="track-author">{book.authors.map((a) => a.name).join(', ')}</p>
            {/if}
            {#if player.currentChapter?.title}
              <p class="track-chapter">{player.currentChapter.title}</p>
            {/if}
          </div>
        {/if}

        <!-- Progress bar (waveform) — global book timeline across all files -->
        <div class="progress-section">
          <WaveformBar
            bookId={book?.id ?? ''}
            files={player.fileIds.map((id, i) => ({
              id,
              duration: player.fileDurations[i] ?? 0
            }))}
            currentTime={player.globalCurrentTime}
            duration={player.totalDuration}
            chapters={player.unifiedChapters.map((c) => ({
              start_seconds: c.global_start_seconds,
              title: c.title
            }))}
            bookmarks={[]}
            onSeek={seekToGlobal}
          />
          <div class="progress-times">
            <span>{formatTime(player.globalCurrentTime)}</span>
            <span
              >-{formatTime(
                (player.totalDuration - player.globalCurrentTime) / player.playbackRate
              )}</span
            >
          </div>
        </div>

        <!-- Skip intro/credits buttons -->
        {#if introEndSeconds != null && player.currentTime < introEndSeconds}
          <button type="button" class="skip-btn" onclick={() => seek(introEndSeconds!)}>
            <span class="material-icons-outlined">skip_next</span>
            Skip Intro
          </button>
        {/if}
        {#if outroStartSeconds != null && player.currentTime >= outroStartSeconds}
          <button
            type="button"
            class="skip-btn"
            onclick={() => {
              if (player.trackCount > 1 && player.currentTrackIndex < player.trackCount - 1) {
                seek(player.duration);
              }
            }}
          >
            <span class="material-icons-outlined">skip_next</span>
            Skip Credits
          </button>
        {/if}

        <!-- Playback controls (sleep timer inline on full player) -->
        <div class="controls-section">
          <AudioControls showVolume showSleepTimer large />
        </div>

        {#if viewMode !== 'reader'}
          <!-- Transcript search -->
          {#if hasAudioTranscript && book}
            <div class="transcript-section">
              <TranscriptSearch bookId={book.id} />
            </div>
          {:else if book && !hasAudioTranscript && transcriptMode !== 'off'}
            <div class="transcript-section">
              <button
                type="button"
                class="transcribe-btn"
                disabled={transcribing}
                onclick={async () => {
                  transcribing = true;
                  try {
                    await transcribeAudiobook(book.id);
                  } catch {
                    /* job status shows progress */
                  }
                }}
              >
                <span class="material-icons-outlined">mic</span>
                {transcribing ? 'Transcription started...' : 'Transcribe for search'}
              </button>
            </div>
          {/if}

          <!-- Chapters + Bookmarks (tabbed, collapsible). Default collapsed
               in subtitles mode so the transcript has room to breathe. -->
          {#if book}
            <div class="tabs-section">
              <ChapterBookmarkTabs
                bookId={book.id}
                currentFileId={player.currentFileId}
                currentTime={player.currentTime}
                defaultExpanded={viewMode === 'cover'}
              />
            </div>
          {/if}
        {/if}
      </div>
    </div>
  {/if}
</div>

{#if showChapterEditor && book}
  <ChapterEditor bookId={book.id} onClose={() => (showChapterEditor = false)} />
{/if}

<style>
  .listen-page {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    background: var(--bg);
    color: var(--ink);
    overflow: hidden;
  }

  .listen-page:fullscreen {
    width: 100vw;
    height: 100vh;
    height: 100dvh;
  }

  .listen-header {
    display: flex;
    align-items: center;
    padding: 8px 12px;
    gap: 8px;
    flex-shrink: 0;
    border-bottom: 1px solid var(--ring);
  }

  .header-btn {
    background: none;
    border: none;
    color: var(--ink);
    cursor: pointer;
    padding: 6px;
    border-radius: 6px;
    display: flex;
    align-items: center;
    opacity: 0.8;
    transition:
      opacity 0.15s,
      background 0.15s;
  }

  .header-btn.active {
    color: var(--accent);
    opacity: 1;
  }

  .header-btn:hover {
    opacity: 1;
    background: rgba(255, 255, 255, 0.08);
  }

  .header-title {
    font-size: 0.9rem;
    font-weight: 500;
    opacity: 0.7;
  }

  .header-spacer {
    flex: 1;
  }

  .listen-error {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    flex: 1;
    gap: 16px;
    padding: 24px;
    text-align: center;
  }

  .listen-error p {
    color: var(--muted);
    font-size: 0.95rem;
    margin: 0;
  }

  /* ── Mobile-first: single column ── */

  .listen-body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 24px 16px 40px;
    gap: 20px;
  }

  /* Cover mode keeps the natural scrolling layout; subtitle/reader modes
     use a fixed-height layout so subtitles fill available space and the
     player controls + tabs stay visible. */
  .listen-body.scroll {
    overflow-y: auto;
  }

  .listen-body.fit {
    overflow: hidden;
  }

  .player-panel {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 20px;
    width: 100%;
  }

  .listen-body.fit .player-panel {
    flex-shrink: 0;
  }

  .sync-banner {
    width: 100%;
    max-width: 480px;
    background: var(--card);
    border: 1px solid var(--accent);
    border-radius: 12px;
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .sync-banner-text {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 0.85rem;
    font-weight: 500;
    color: var(--ink);
  }

  .sync-banner-text .material-icons-outlined {
    font-size: 18px;
    color: var(--accent);
    flex-shrink: 0;
  }

  .sync-snippet {
    font-size: 0.8rem;
    color: var(--muted);
    margin: 0;
    font-style: italic;
    line-height: 1.4;
    overflow: hidden;
    text-overflow: ellipsis;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }

  .sync-banner-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .btn-sm {
    font-size: 0.8rem;
    padding: 6px 14px;
  }

  .subtitle-section {
    width: 100%;
    max-width: 480px;
    flex: 1;
    min-height: 200px;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .reader-section {
    width: 100%;
    flex: 1;
    min-height: 0;
    overflow: hidden;
    display: flex;
  }

  .cover-section {
    flex-shrink: 0;
    display: flex;
    justify-content: center;
  }

  .cover-art {
    width: min(280px, 60vw);
    height: auto;
    aspect-ratio: 2 / 3;
    border-radius: 12px;
    object-fit: cover;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.5);
  }

  .cover-placeholder {
    width: min(280px, 60vw);
    aspect-ratio: 2 / 3;
    border-radius: 12px;
    background: var(--card);
    border: 1px solid var(--ring);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .cover-placeholder-text {
    color: var(--muted);
    font-size: 0.9rem;
  }

  .track-info {
    text-align: center;
    max-width: 400px;
  }

  .track-title {
    font-size: 1.2rem;
    font-weight: 600;
    margin: 0;
    line-height: 1.3;
  }

  .track-author {
    font-size: 0.9rem;
    color: var(--muted);
    margin: 4px 0 0;
  }

  .track-chapter {
    font-size: 0.85rem;
    color: var(--accent);
    margin: 6px 0 0;
    font-weight: 500;
  }

  .progress-section {
    width: 100%;
    max-width: 480px;
  }

  .progress-times {
    display: flex;
    justify-content: space-between;
    margin-top: 6px;
    font-size: 0.75rem;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .skip-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 8px 20px;
    background: var(--accent);
    color: #000;
    border: none;
    border-radius: 8px;
    font-size: 0.85rem;
    font-weight: 600;
    cursor: pointer;
    transition: opacity 0.15s;
  }

  .skip-btn:hover {
    opacity: 0.85;
  }

  .skip-btn .material-icons-outlined {
    font-size: 18px;
  }

  .controls-section {
    padding: 4px 0;
  }

  .transcript-section {
    width: 100%;
    max-width: 480px;
  }

  .tabs-section {
    width: 100%;
    max-width: 480px;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  /* In subtitle/reader mode the page doesn't scroll, so cap the tabs panel
     so an expanded chapter list doesn't push controls off-screen. */
  .listen-body.fit .tabs-section {
    max-height: 40vh;
  }

  .transcribe-btn {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 10px 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 10px;
    color: var(--ink);
    cursor: pointer;
    font-size: 0.85rem;
    opacity: 0.7;
    transition:
      opacity 0.15s,
      background 0.15s;
  }

  .transcribe-btn:hover:not(:disabled) {
    opacity: 1;
    background: rgba(255, 255, 255, 0.06);
  }

  .transcribe-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .transcribe-btn .material-icons-outlined {
    font-size: 18px;
  }

  /* ── Small phones ── */

  @media (max-width: 600px) {
    .listen-body {
      padding: 16px 12px 32px;
      gap: 16px;
    }

    .player-panel {
      gap: 16px;
    }

    .cover-art {
      width: min(220px, 55vw);
    }

    .cover-placeholder {
      width: min(220px, 55vw);
    }

    .track-title {
      font-size: 1.05rem;
    }
  }

  @media (min-height: 800px) {
    .listen-body {
      gap: 24px;
    }
  }

  /* ── Tablet: wider single column ── */

  @media (min-width: 768px) {
    .sync-banner,
    .progress-section,
    .transcript-section,
    .tabs-section,
    .subtitle-section {
      max-width: 560px;
    }
  }

  /* ── Desktop: two-column layout when cover is visible ── */

  @media (min-width: 920px) {
    .listen-body.two-col {
      display: grid;
      grid-template-columns: auto 1fr;
      grid-template-rows: auto 1fr;
      gap: 0 48px;
      align-items: start;
      padding: 32px 40px 40px;
      overflow: hidden;
    }

    /* Sync banner spans full width above the columns */
    .two-col .sync-banner {
      grid-column: 1 / -1;
      max-width: 720px;
      justify-self: center;
      margin-bottom: 24px;
    }

    /* Cover pinned to left column, vertically centered */
    .two-col .cover-section {
      grid-row: 2;
      grid-column: 1;
      align-self: center;
      position: sticky;
      top: 32px;
    }

    .two-col .cover-art {
      width: clamp(240px, 28vw, 380px);
      border-radius: 14px;
      box-shadow:
        0 16px 48px rgba(0, 0, 0, 0.5),
        0 0 0 1px rgba(255, 255, 255, 0.05);
    }

    .two-col .cover-placeholder {
      width: clamp(240px, 28vw, 380px);
      border-radius: 14px;
    }

    /* Player panel fills right column and scrolls independently */
    .two-col .player-panel {
      grid-row: 2;
      grid-column: 2;
      align-items: stretch;
      overflow-y: auto;
      max-height: 100%;
      padding-right: 8px;
      gap: 24px;
    }

    .two-col .track-info {
      text-align: left;
      max-width: none;
    }

    .two-col .track-title {
      font-size: 1.5rem;
    }

    .two-col .track-author {
      font-size: 0.95rem;
    }

    .two-col .progress-section {
      max-width: none;
    }

    .two-col .controls-section {
      padding: 8px 0;
    }

    .two-col .transcript-section,
    .two-col .tabs-section {
      max-width: none;
    }

    /* Non-two-col desktop: wider single column for subtitle/reader modes */
    .listen-body:not(.two-col) {
      padding: 24px 32px 40px;
    }

    .listen-body:not(.two-col) .subtitle-section {
      max-width: 640px;
    }

    .listen-body:not(.two-col) .progress-section,
    .listen-body:not(.two-col) .transcript-section,
    .listen-body:not(.two-col) .tabs-section {
      max-width: 640px;
    }
  }

  /* ── Wide desktop: more breathing room ── */

  @media (min-width: 1200px) {
    .listen-body.two-col {
      gap: 0 64px;
      padding: 40px 60px 48px;
    }

    .two-col .cover-art {
      width: clamp(300px, 24vw, 420px);
    }

    .two-col .cover-placeholder {
      width: clamp(300px, 24vw, 420px);
    }

    .two-col .track-title {
      font-size: 1.65rem;
    }

    .two-col .player-panel {
      gap: 28px;
    }
  }
</style>
