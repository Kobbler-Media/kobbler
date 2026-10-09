<script lang="ts">
  import { onDestroy } from 'svelte';
  import { Modal, uid } from '@koblibs/ui';
  import type { UnifiedChapter, JobEvent } from '$lib/types';
  import * as api from '$lib/api/client';
  import { showMessage } from '$lib/context/app.svelte';
  import { on, off } from '$lib/services/eventstream.svelte';
  import ChapterEditorPlayer from './ChapterEditorPlayer.svelte';

  interface Props {
    bookId: string;
    onClose: () => void;
    onSaved?: () => void;
  }

  let { bookId, onClose, onSaved }: Props = $props();

  type EditableChapter = {
    id: string;
    chapter_index: number;
    title: string;
    start_seconds: number;
    end_seconds: number;
    file_id: string;
    file_path: string;
    source: string;
    dirty: boolean;
    startError: boolean;
    endError: boolean;
  };

  type FileGroup = {
    fileId: string;
    fileName: string;
    filePath: string;
    chapters: EditableChapter[];
  };

  let chapters = $state<EditableChapter[]>([]);
  let loading = $state(true);
  let saving = $state(false);
  let generating = $state(false);
  let probing = $state(false);
  let detecting = $state(false);
  let detectingText = $state(false);
  let fetchingAsin = $state(false);
  let asinInput = $state('');
  let showAsinInput = $state(false);
  let clearedFileIds = $state<Set<string>>(new Set());
  let dragState = $state<{ chapterId: string; fileId: string } | null>(null);
  let dragOverId = $state<string | null>(null);
  let expandedPlayers = $state<Set<string>>(new Set());
  let playerSeekFns = new Map<string, (seconds: number) => void>();

  const fileGroups = $derived.by(() => {
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
        map.set(ch.file_id, {
          fileId: ch.file_id,
          fileName,
          filePath: ch.file_path,
          chapters: [ch]
        });
      }
    }
    return [...map.values()];
  });

  const isMultiFile = $derived(fileGroups.length > 1);
  const hasDirty = $derived(chapters.some((c) => c.dirty) || clearedFileIds.size > 0);
  const hasErrors = $derived(chapters.some((c) => c.startError || c.endError));
  const totalDuration = $derived.by(() => {
    if (chapters.length === 0) return 0;
    let maxEnd = 0;
    let currentFileId = '';
    let offset = 0;
    let lastFileDur = 0;
    for (const ch of chapters) {
      if (ch.file_id !== currentFileId) {
        offset += lastFileDur;
        lastFileDur = ch.end_seconds;
        currentFileId = ch.file_id;
      }
      if (ch.end_seconds > lastFileDur) lastFileDur = ch.end_seconds;
      const globalEnd = offset + ch.end_seconds;
      if (globalEnd > maxEnd) maxEnd = globalEnd;
    }
    return maxEnd;
  });

  function formatTime(seconds: number): string {
    if (!isFinite(seconds) || seconds < 0) return '0:00';
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = Math.floor(seconds % 60);
    if (h > 0) {
      return `${h}:${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
    }
    return `${m}:${s.toString().padStart(2, '0')}`;
  }

  function parseTime(str: string): number | null {
    const parts = str.trim().split(':').map(Number);
    if (parts.some(isNaN)) return null;
    if (parts.length === 3) return parts[0] * 3600 + parts[1] * 60 + parts[2];
    if (parts.length === 2) return parts[0] * 60 + parts[1];
    if (parts.length === 1) return parts[0];
    return null;
  }

  function fromUnified(ch: UnifiedChapter): EditableChapter {
    return {
      id: ch.id,
      chapter_index: ch.chapter_index,
      title: ch.title ?? '',
      start_seconds: ch.start_seconds,
      end_seconds: ch.end_seconds,
      file_id: ch.file_id,
      file_path: ch.file_path,
      source: ch.source,
      dirty: false,
      startError: false,
      endError: false
    };
  }

  async function loadChapters() {
    loading = true;
    try {
      const unified = await api.getBookChapters(bookId);
      chapters = unified.map(fromUnified);
      clearedFileIds = new Set();
    } catch {
      showMessage('Failed to load chapters');
    } finally {
      loading = false;
    }
  }

  function updateTitle(ch: EditableChapter, value: string) {
    ch.title = value;
    ch.dirty = true;
  }

  function updateStart(ch: EditableChapter, value: string) {
    const parsed = parseTime(value);
    if (parsed !== null) {
      ch.start_seconds = parsed;
      ch.dirty = true;
      ch.startError = false;
    } else {
      ch.startError = true;
    }
  }

  function updateEnd(ch: EditableChapter, value: string) {
    const parsed = parseTime(value);
    if (parsed !== null) {
      ch.end_seconds = parsed;
      ch.dirty = true;
      ch.endError = false;
    } else {
      ch.endError = true;
    }
  }

  function removeChapter(ch: EditableChapter) {
    chapters = chapters.filter((c) => c !== ch);
    const sibling = chapters.find((c) => c.file_id === ch.file_id);
    if (sibling) {
      sibling.dirty = true;
    } else {
      clearedFileIds = new Set([...clearedFileIds, ch.file_id]);
    }
  }

  function addChapter(group: FileGroup) {
    const lastInGroup = group.chapters[group.chapters.length - 1];
    // Best-effort defaults: start where previous ends, end at file end or +5min
    const newStart = lastInGroup ? lastInGroup.end_seconds : 0;
    // Estimate file duration from the max end_seconds in this group
    const maxEndInGroup = group.chapters.reduce((max, c) => Math.max(max, c.end_seconds), 0);
    // Default to remaining time in file, or 5 minutes if we can't tell
    const newEnd = maxEndInGroup > newStart ? maxEndInGroup : newStart + 300;
    const newChapter: EditableChapter = {
      id: uid(),
      chapter_index: group.chapters.length,
      title: `Chapter ${group.chapters.length + 1}`,
      start_seconds: newStart,
      end_seconds: newEnd,
      file_id: group.fileId,
      file_path: group.filePath,
      source: 'manual',
      dirty: true,
      startError: false,
      endError: false
    };
    const lastIdx = chapters.findLastIndex((c) => c.file_id === group.fileId);
    chapters = [...chapters.slice(0, lastIdx + 1), newChapter, ...chapters.slice(lastIdx + 1)];
  }

  // --- Player integration ---

  function togglePlayer(fileId: string) {
    const next = new Set(expandedPlayers);
    if (next.has(fileId)) {
      next.delete(fileId);
    } else {
      next.add(fileId);
    }
    expandedPlayers = next;
  }

  function handleChapterTimeChange(chapterId: string, newStart: number) {
    const ch = chapters.find((c) => c.id === chapterId);
    if (!ch) return;

    ch.start_seconds = newStart;
    ch.dirty = true;

    // Update the previous chapter's end_seconds to avoid gaps/overlaps
    const fileChapters = chapters.filter((c) => c.file_id === ch.file_id);
    const idx = fileChapters.indexOf(ch);
    if (idx > 0) {
      fileChapters[idx - 1].end_seconds = newStart;
      fileChapters[idx - 1].dirty = true;
    }
  }

  function handleAddFromPlayer(fileId: string, startSeconds: number) {
    const group = fileGroups.find((g) => g.fileId === fileId);
    if (!group) return;

    const filePath = group.filePath;
    const sortedFileChapters = [...group.chapters].sort(
      (a, b) => a.start_seconds - b.start_seconds
    );

    // Find next chapter after the insertion point to set proper end_seconds
    const nextChapter = sortedFileChapters.find((c) => c.start_seconds > startSeconds);
    const maxEnd = group.chapters.reduce((max, c) => Math.max(max, c.end_seconds), 0);
    const endSeconds = nextChapter
      ? nextChapter.start_seconds
      : maxEnd > startSeconds
        ? maxEnd
        : startSeconds + 300;

    const newChapter: EditableChapter = {
      id: uid(),
      chapter_index: 0,
      title: `Chapter ${group.chapters.length + 1}`,
      start_seconds: startSeconds,
      end_seconds: endSeconds,
      file_id: fileId,
      file_path: filePath,
      source: 'manual',
      dirty: true,
      startError: false,
      endError: false
    };

    // Update the previous chapter's end_seconds to avoid overlap
    const prevChapter = [...sortedFileChapters]
      .reverse()
      .find((c) => c.start_seconds <= startSeconds);
    if (prevChapter) {
      prevChapter.end_seconds = startSeconds;
      prevChapter.dirty = true;
    }

    // Insert in correct position among this file's chapters
    const fileChapterIndices = chapters
      .map((c, i) => ({ c, i }))
      .filter(({ c }) => c.file_id === fileId);
    const insertAfter = fileChapterIndices.findLast(({ c }) => c.start_seconds <= startSeconds);
    const insertIdx = insertAfter
      ? insertAfter.i + 1
      : (fileChapterIndices[0]?.i ?? chapters.length);
    chapters = [...chapters.slice(0, insertIdx), newChapter, ...chapters.slice(insertIdx)];
  }

  function seekPlayerToChapter(chapter: EditableChapter) {
    // Expand player if collapsed
    if (!expandedPlayers.has(chapter.file_id)) {
      expandedPlayers = new Set([...expandedPlayers, chapter.file_id]);
    }
    // Wait for mount, then seek
    requestAnimationFrame(() => {
      const seekFn = playerSeekFns.get(chapter.file_id);
      seekFn?.(chapter.start_seconds);
    });
  }

  // --- Drag-to-reorder (within same file group only) ---

  function handleDragStart(ch: EditableChapter) {
    dragState = { chapterId: ch.id, fileId: ch.file_id };
  }

  function handleDragOver(e: DragEvent, ch: EditableChapter) {
    if (!dragState || dragState.fileId !== ch.file_id) return;
    e.preventDefault();
    dragOverId = ch.id;
  }

  function handleDragLeave() {
    dragOverId = null;
  }

  function handleDrop(targetCh: EditableChapter) {
    if (!dragState || dragState.fileId !== targetCh.file_id) {
      dragState = null;
      dragOverId = null;
      return;
    }

    const srcIdx = chapters.findIndex((c) => c.id === dragState!.chapterId);
    const dstIdx = chapters.findIndex((c) => c.id === targetCh.id);
    if (srcIdx < 0 || dstIdx < 0 || srcIdx === dstIdx) {
      dragState = null;
      dragOverId = null;
      return;
    }

    const updated = [...chapters];
    const [moved] = updated.splice(srcIdx, 1);
    updated.splice(dstIdx, 0, moved);

    // Mark all chapters in this file group as dirty (indexes changed)
    for (const ch of updated) {
      if (ch.file_id === dragState!.fileId) ch.dirty = true;
    }

    chapters = updated;
    dragState = null;
    dragOverId = null;
  }

  function handleDragEnd() {
    dragState = null;
    dragOverId = null;
  }

  // --- Save ---

  async function handleSave() {
    saving = true;
    try {
      const dirtyFiles = new Set<string>(clearedFileIds);
      for (const ch of chapters) {
        if (ch.dirty) dirtyFiles.add(ch.file_id);
      }

      for (const fileId of dirtyFiles) {
        const fileChapters = chapters
          .filter((c) => c.file_id === fileId)
          .map((c, i) => ({
            chapter_index: i,
            title: c.title || null,
            start_seconds: c.start_seconds,
            end_seconds: c.end_seconds
          }));
        await api.replaceFileChapters(bookId, fileId, fileChapters);
      }
      clearedFileIds = new Set();

      showMessage('Chapters saved');
      onSaved?.();
      onClose();
    } catch {
      showMessage('Failed to save chapters');
    } finally {
      saving = false;
    }
  }

  async function handleGenerate(force: boolean) {
    generating = true;
    try {
      const result = await api.generateChapters(bookId, force);
      showMessage(`Generated ${result.generated} chapter(s)`);
      await loadChapters();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to generate chapters');
    } finally {
      generating = false;
    }
  }

  async function handleProbe() {
    probing = true;
    try {
      await api.probeAudioFiles(bookId);
      showMessage('Probing audio files...');
    } catch {
      showMessage('Failed to enqueue probe');
      probing = false;
    }
    // probing stays true until SSE job completion event
  }

  async function handleDetect() {
    detecting = true;
    try {
      await api.detectChapters(bookId);
      showMessage('Detecting silence chapters...');
    } catch {
      showMessage('Failed to enqueue silence detection');
      detecting = false;
    }
  }

  async function handleDetectText() {
    detectingText = true;
    try {
      await api.detectTextChapters(bookId);
      showMessage('Detecting text chapters...');
    } catch {
      showMessage('Failed to enqueue text detection');
      detectingText = false;
    }
  }

  async function handleFetchAsin() {
    if (!asinInput.trim()) return;
    fetchingAsin = true;
    try {
      const unified = await api.fetchAudnexusChapters(bookId, asinInput.trim());
      chapters = unified.map(fromUnified);
      showAsinInput = false;
      asinInput = '';
      showMessage(`Fetched ${unified.length} chapter(s) from Audnexus`);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to fetch from Audnexus');
    } finally {
      fetchingAsin = false;
    }
  }

  // --- SSE: listen for job completion to auto-reload after probe ---

  let reloadDebounce: ReturnType<typeof setTimeout> | null = null;

  function debouncedReload() {
    if (reloadDebounce) clearTimeout(reloadDebounce);
    reloadDebounce = setTimeout(() => {
      loadChapters();
      reloadDebounce = null;
    }, 500);
  }

  const jobCallback = on('job', (data: unknown) => {
    const event = data as JobEvent;
    // Only care about jobs for this book
    if (event.entity_type !== 'book' || event.entity_id !== bookId) return;

    if (event.job_type === 'audiobook_probe') {
      if (event.status === 'completed') {
        probing = false;
        showMessage('Audio probe complete');
        debouncedReload();
      } else if (event.status === 'failed') {
        probing = false;
        showMessage(event.error ?? 'Audio probe failed');
      }
    }
    if (event.job_type === 'chapter_redetect') {
      if (event.status === 'completed') {
        detecting = false;
        showMessage('Silence chapter detection complete');
        debouncedReload();
      } else if (event.status === 'failed') {
        detecting = false;
        showMessage(event.error ?? 'Silence chapter detection failed');
      }
    }
    if (event.job_type === 'chapter_detect') {
      if (event.status === 'completed') {
        detectingText = false;
        showMessage('Text chapter detection complete');
        debouncedReload();
      } else if (event.status === 'failed') {
        detectingText = false;
        showMessage(event.error ?? 'Text chapter detection failed');
      }
    }
    // Also reload on enrichment (may fetch Audnexus chapters)
    if (event.job_type === 'book_enrich' && event.status === 'completed') {
      debouncedReload();
    }
  });

  onDestroy(() => {
    off('job', jobCallback);
    if (reloadDebounce) clearTimeout(reloadDebounce);
  });

  // Load on mount
  loadChapters();
</script>

<Modal title="Chapter Editor" width="min(90vw, 720px)" {onClose}>
  {#if loading}
    <div class="editor-loading">
      <span class="material-icons-outlined spin">sync</span>
      <span>Loading chapters...</span>
    </div>
  {:else}
    <!-- Toolbar -->
    <div class="editor-toolbar">
      <div class="toolbar-left">
        <span class="chapter-count"
          >{chapters.length} chapter{chapters.length !== 1 ? 's' : ''}</span
        >
        {#if totalDuration > 0}
          <span class="total-duration">{formatTime(totalDuration)}</span>
        {/if}
      </div>
      <div class="toolbar-actions">
        <button
          type="button"
          class="tool-btn"
          class:active={probing}
          title={probing ? 'Probing...' : 'Re-probe metadata & embedded chapters'}
          disabled={probing}
          onclick={handleProbe}
        >
          <span class="material-icons-outlined" class:spin={probing}
            >{probing ? 'sync' : 'hearing'}</span
          >
        </button>
        <button
          type="button"
          class="tool-btn"
          class:active={detecting}
          title={detecting ? 'Detecting...' : 'Detect chapters (silence analysis)'}
          disabled={detecting}
          onclick={handleDetect}
        >
          <span class="material-icons-outlined" class:spin={detecting}
            >{detecting ? 'sync' : 'graphic_eq'}</span
          >
        </button>
        <button
          type="button"
          class="tool-btn"
          class:active={detectingText}
          title={detectingText ? 'Detecting...' : 'Detect chapters (text cues from transcript)'}
          disabled={detectingText}
          onclick={handleDetectText}
        >
          <span class="material-icons-outlined" class:spin={detectingText}
            >{detectingText ? 'sync' : 'text_fields'}</span
          >
        </button>
        <button
          type="button"
          class="tool-btn"
          title="Auto-generate (one per file)"
          disabled={generating}
          onclick={() => handleGenerate(chapters.length > 0)}
        >
          <span class="material-icons-outlined" class:spin={generating}
            >{generating ? 'sync' : 'auto_fix_high'}</span
          >
        </button>
        <button
          type="button"
          class="tool-btn"
          class:active={showAsinInput}
          title="Fetch from Audnexus by ASIN"
          onclick={() => (showAsinInput = !showAsinInput)}
        >
          <span class="material-icons-outlined">cloud_download</span>
        </button>
      </div>
    </div>

    <!-- ASIN fetch row -->
    {#if showAsinInput}
      <div class="asin-row">
        <input
          type="text"
          class="asin-input"
          placeholder="Enter ASIN (e.g. B0DCFM4KGY)"
          bind:value={asinInput}
          onkeydown={(e) => e.key === 'Enter' && handleFetchAsin()}
        />
        <button
          type="button"
          class="btn btn-sm"
          disabled={fetchingAsin || !asinInput.trim()}
          onclick={handleFetchAsin}
        >
          {fetchingAsin ? 'Fetching...' : 'Fetch'}
        </button>
      </div>
    {/if}

    <!-- Chapter list -->
    <div class="chapter-rows">
      {#if chapters.length === 0}
        <div class="empty-state">
          <span class="material-icons-outlined">queue_music</span>
          <p>No chapters yet</p>
          <p class="empty-hint">Auto-generate or fetch from Audnexus</p>
        </div>
      {:else}
        {#each fileGroups as group (group.fileId)}
          <div class="file-header" class:multi={isMultiFile}>
            {#if isMultiFile}
              <span class="material-icons-outlined file-icon">audio_file</span>
              <span class="file-name">{group.fileName}</span>
            {/if}
            <button
              type="button"
              class="player-toggle"
              class:active={expandedPlayers.has(group.fileId)}
              title={expandedPlayers.has(group.fileId) ? 'Hide player' : 'Show player'}
              onclick={() => togglePlayer(group.fileId)}
            >
              <span class="material-icons-outlined"
                >{expandedPlayers.has(group.fileId) ? 'graphic_eq' : 'play_circle'}</span
              >
            </button>
          </div>
          {#if expandedPlayers.has(group.fileId)}
            {@const fid = group.fileId}
            <ChapterEditorPlayer
              {bookId}
              fileId={fid}
              chapters={group.chapters.map((c) => ({
                id: c.id,
                start_seconds: c.start_seconds,
                source: c.source
              }))}
              duration={group.chapters.reduce((max, c) => Math.max(max, c.end_seconds), 0)}
              onChapterTimeChange={handleChapterTimeChange}
              onAddChapter={(t) => handleAddFromPlayer(fid, t)}
              onReady={(seekFn) => playerSeekFns.set(fid, seekFn)}
            />
          {/if}
          {#each group.chapters as chapter, i (chapter.id)}
            <div
              class="chapter-row"
              class:dirty={chapter.dirty}
              class:drag-over={dragOverId === chapter.id}
              draggable="true"
              role="listitem"
              ondragstart={() => handleDragStart(chapter)}
              ondragover={(e) => handleDragOver(e, chapter)}
              ondragleave={handleDragLeave}
              ondrop={() => handleDrop(chapter)}
              ondragend={handleDragEnd}
            >
              <span class="row-grip" title="Drag to reorder">
                <span class="material-icons-outlined">drag_indicator</span>
              </span>
              <span class="row-index">{i + 1}</span>
              <button
                type="button"
                class="row-play"
                title="Preview from this chapter"
                onclick={() => seekPlayerToChapter(chapter)}
              >
                <span class="material-icons-outlined">play_arrow</span>
              </button>
              <input
                type="text"
                class="row-title"
                placeholder="Chapter title"
                value={chapter.title}
                oninput={(e) => updateTitle(chapter, e.currentTarget.value)}
              />
              <input
                type="text"
                class="row-time"
                class:time-error={chapter.startError}
                title="Start time (H:MM:SS or M:SS)"
                value={formatTime(chapter.start_seconds)}
                onchange={(e) => updateStart(chapter, e.currentTarget.value)}
              />
              <span class="row-dash">&ndash;</span>
              <input
                type="text"
                class="row-time"
                class:time-error={chapter.endError}
                title="End time (H:MM:SS or M:SS)"
                value={formatTime(chapter.end_seconds)}
                onchange={(e) => updateEnd(chapter, e.currentTarget.value)}
              />
              <button
                type="button"
                class="row-delete"
                title="Remove chapter"
                onclick={() => removeChapter(chapter)}
              >
                <span class="material-icons-outlined">close</span>
              </button>
            </div>
          {/each}
          <button type="button" class="add-chapter-btn" onclick={() => addChapter(group)}>
            <span class="material-icons-outlined">add</span>
            Add chapter
          </button>
        {/each}
      {/if}
    </div>
  {/if}

  {#snippet footer()}
    <button type="button" class="btn alt" onclick={onClose} disabled={saving}>Cancel</button>
    <button
      type="button"
      class="btn"
      disabled={saving || !hasDirty || hasErrors}
      onclick={handleSave}
    >
      {saving ? 'Saving...' : 'Save Changes'}
    </button>
  {/snippet}
</Modal>

<style>
  .editor-loading {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 40px 0;
    color: var(--muted);
    font-size: 0.9rem;
  }

  .spin {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .editor-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 0 12px;
    border-bottom: 1px solid var(--ring);
    margin-bottom: 12px;
  }

  .toolbar-left {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .chapter-count {
    font-size: 0.8rem;
    font-weight: 600;
    color: var(--ink);
  }

  .total-duration {
    font-size: 0.75rem;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .toolbar-actions {
    display: flex;
    gap: 4px;
  }

  .tool-btn {
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
  }

  .tool-btn:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.06);
    color: var(--ink);
    border-color: var(--ink);
  }

  .tool-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .tool-btn.active {
    background: var(--accent-dim);
    color: var(--accent);
    border-color: var(--accent);
  }

  .tool-btn .material-icons-outlined {
    font-size: 18px;
  }

  .asin-row {
    display: flex;
    gap: 8px;
    padding-bottom: 12px;
    border-bottom: 1px solid var(--ring);
    margin-bottom: 12px;
  }

  .asin-input {
    flex: 1;
    padding: 8px 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg);
    color: var(--ink);
    font-size: 0.85rem;
    font-family: inherit;
  }

  .asin-input:focus {
    outline: none;
    border-color: var(--accent);
  }

  .asin-input::placeholder {
    color: var(--muted);
    opacity: 0.6;
  }

  .btn-sm {
    padding: 8px 16px;
    font-size: 0.8rem;
  }

  .chapter-rows {
    max-height: min(50vh, 400px);
    overflow-y: auto;
    margin: 0 -20px;
    padding: 0 20px;
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 36px 0;
    color: var(--muted);
  }

  .empty-state .material-icons-outlined {
    font-size: 32px;
    opacity: 0.4;
  }

  .empty-state p {
    margin: 0;
    font-size: 0.9rem;
  }

  .empty-hint {
    font-size: 0.8rem !important;
    opacity: 0.6;
  }

  .file-header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 0 4px;
    font-size: 0.72rem;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }

  .file-header.multi {
    border-top: 1px solid rgba(255, 255, 255, 0.06);
    margin-top: 8px;
    padding-top: 10px;
  }

  .file-header.multi:first-child {
    border-top: none;
    margin-top: 0;
    padding-top: 0;
  }

  .player-toggle {
    margin-left: auto;
    width: 26px;
    height: 26px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
  }

  .player-toggle:hover {
    background: rgba(255, 255, 255, 0.06);
    color: var(--ink);
    border-color: var(--ink);
  }

  .player-toggle.active {
    background: var(--accent-dim);
    color: var(--accent);
    border-color: var(--accent);
  }

  .player-toggle .material-icons-outlined {
    font-size: 16px;
  }

  .file-icon {
    font-size: 14px;
  }

  .file-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chapter-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 0;
    border-bottom: 1px solid rgba(255, 255, 255, 0.03);
    transition: background 0.15s;
  }

  .chapter-row.dirty {
    background: rgba(211, 140, 90, 0.04);
    margin: 0 -8px;
    padding: 6px 8px;
    border-radius: 6px;
  }

  .chapter-row.drag-over {
    border-top: 2px solid var(--accent);
    padding-top: 4px;
  }

  .row-grip {
    display: flex;
    align-items: center;
    cursor: grab;
    color: var(--muted);
    opacity: 0.3;
    flex-shrink: 0;
    transition: opacity 0.15s;
  }

  .chapter-row:hover .row-grip {
    opacity: 0.7;
  }

  .row-grip:active {
    cursor: grabbing;
  }

  .row-grip .material-icons-outlined {
    font-size: 16px;
  }

  .row-play {
    width: 22px;
    height: 22px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    opacity: 0;
    transition: all 0.15s;
    flex-shrink: 0;
    padding: 0;
  }

  .chapter-row:hover .row-play {
    opacity: 0.6;
  }

  .row-play:hover {
    opacity: 1 !important;
    color: var(--accent);
    background: var(--accent-dim);
  }

  .row-play .material-icons-outlined {
    font-size: 16px;
  }

  .row-index {
    font-size: 0.72rem;
    color: var(--muted);
    min-width: 20px;
    text-align: center;
    font-variant-numeric: tabular-nums;
    flex-shrink: 0;
  }

  .row-title {
    flex: 1;
    min-width: 0;
    padding: 6px 10px;
    border: 1px solid transparent;
    border-radius: 6px;
    background: transparent;
    color: var(--ink);
    font-size: 0.85rem;
    font-family: inherit;
    transition:
      border-color 0.15s,
      background 0.15s;
  }

  .row-title:hover {
    border-color: var(--ring);
  }

  .row-title:focus {
    outline: none;
    border-color: var(--accent);
    background: var(--bg);
  }

  .row-title::placeholder {
    color: var(--muted);
    opacity: 0.5;
  }

  .row-time {
    width: 64px;
    padding: 6px 8px;
    border: 1px solid transparent;
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    font-size: 0.78rem;
    font-family: inherit;
    font-variant-numeric: tabular-nums;
    text-align: center;
    flex-shrink: 0;
    transition:
      border-color 0.15s,
      background 0.15s;
  }

  .row-time:hover {
    border-color: var(--ring);
  }

  .row-time:focus {
    outline: none;
    border-color: var(--accent);
    background: var(--bg);
    color: var(--ink);
  }

  .row-time.time-error {
    border-color: var(--danger);
    color: var(--danger);
  }

  .row-time.time-error:focus {
    border-color: var(--danger);
  }

  .row-dash {
    color: var(--muted);
    opacity: 0.4;
    flex-shrink: 0;
    font-size: 0.8rem;
  }

  .row-delete {
    width: 24px;
    height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    opacity: 0;
    transition: all 0.15s;
    flex-shrink: 0;
  }

  .chapter-row:hover .row-delete {
    opacity: 0.6;
  }

  .row-delete:hover {
    opacity: 1 !important;
    color: var(--danger);
    background: rgba(239, 68, 68, 0.1);
  }

  .row-delete .material-icons-outlined {
    font-size: 16px;
  }

  .add-chapter-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 12px;
    margin: 4px 0 8px;
    border: 1px dashed var(--ring);
    border-radius: 8px;
    background: transparent;
    color: var(--muted);
    font-size: 0.8rem;
    font-family: inherit;
    cursor: pointer;
    transition: all 0.15s;
    width: 100%;
  }

  .add-chapter-btn:hover {
    border-color: var(--accent);
    color: var(--accent);
    background: var(--accent-dim);
  }

  .add-chapter-btn .material-icons-outlined {
    font-size: 16px;
  }

  @media (max-width: 500px) {
    .row-time {
      width: 52px;
      font-size: 0.72rem;
      padding: 6px 4px;
    }

    .row-title {
      font-size: 0.8rem;
    }

    .chapter-row {
      gap: 4px;
    }

    .row-grip {
      display: none;
    }
  }
</style>
