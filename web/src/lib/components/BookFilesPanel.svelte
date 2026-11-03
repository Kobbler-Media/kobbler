<script lang="ts">
  import { goto } from '$app/navigation';
  import * as api from '#lib/api/client.js';
  import type { BookFileInfo, AudioFileInfo, AudioSummary } from '#lib/types.js';
  import type { SearchSelectItem } from '@koblibs/ui';
  import { showMessage } from '#lib/context/app.svelte.js';
  import OverflowMenu from './shared/OverflowMenu.svelte';
  import MetaLine from './shared/MetaLine.svelte';
  import MoveFileModal from './shared/MoveFileModal.svelte';

  interface Props {
    bookId: string;
    /** Called when a file is deleted and book may need refresh */
    onFileDeleted?: () => void;
    /** Called when replace/search is triggered */
    onSearchReleases?: () => void;
  }

  let { bookId, onFileDeleted, onSearchReleases }: Props = $props();

  // State
  let files = $state<BookFileInfo[]>([]);
  let audioFiles = $state<AudioFileInfo[]>([]);
  let audioSummary = $state<AudioSummary | null>(null);
  let audioExpanded = $state(false);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let deletingFileId = $state<string | null>(null);
  let movingFileId = $state<string | null>(null);
  let rejecting = $state(false);

  // Load files when bookId changes
  $effect(() => {
    loadFiles();
  });

  const loadFiles = async () => {
    loading = true;
    error = null;
    try {
      const response = await api.getBookFiles(bookId);
      files = response.files;
      audioFiles = response.audio_files ?? [];
      audioSummary = response.audio_summary ?? null;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load files';
    } finally {
      loading = false;
    }
  };

  const handleDelete = async (fileId: string) => {
    if (!confirm('Delete this file? This cannot be undone.')) return;

    deletingFileId = fileId;
    try {
      await api.deleteBookFile(bookId, fileId);
      files = files.filter((f) => f.id !== fileId);
      showMessage('File deleted');
      onFileDeleted?.();
    } catch (e) {
      const msg = e instanceof Error ? e.message : 'Delete failed';
      showMessage(msg);
    } finally {
      deletingFileId = null;
    }
  };

  const handleReplace = async (fileId: string) => {
    if (!confirm('Replace this file? The current file will be deleted and a new search started.'))
      return;
    try {
      await api.replaceFile('book', bookId, fileId);
      showMessage('Replacement search started');
      await loadFiles();
      onFileDeleted?.();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Replace failed');
    }
  };

  const handleRejectAndResearch = async () => {
    rejecting = true;
    try {
      await api.rejectDownload('book', bookId);
      showMessage('Download rejected — searching for new release');
      onSearchReleases?.();
    } catch (e) {
      const msg = e instanceof Error ? e.message : 'Reject failed';
      showMessage(msg);
    } finally {
      rejecting = false;
    }
  };

  const handleMoveFile = (fileId: string) => {
    movingFileId = fileId;
  };

  const doMoveFile = async (targetBookId: string) => {
    if (!movingFileId) return;
    await api.moveBookFile(bookId, movingFileId, targetBookId);
    movingFileId = null;
    await loadFiles();
    onFileDeleted?.();
  };

  const searchBooksForMove = async (query: string): Promise<SearchSelectItem[]> => {
    const resp = await api.getBooks({ search: query, limit: 20 });
    return resp.items.filter((b) => b.id !== bookId).map((b) => ({ value: b.id, label: b.title }));
  };

  const formatSize = (bytes: number | null): string => {
    if (!bytes) return 'Unknown';
    const mb = bytes / 1024 / 1024;
    if (mb < 1024) return `${mb.toFixed(1)} MB`;
    return `${(mb / 1024).toFixed(2)} GB`;
  };

  const formatDuration = (seconds: number | null): string => {
    if (!seconds) return '';
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    if (h > 0) return `${h}h ${m}m`;
    return `${m}m`;
  };
</script>

<div class="files-section">
  <div class="label">File</div>

  {#if loading}
    <div class="file-box empty">
      <span class="muted">Loading...</span>
    </div>
  {:else if error}
    <div class="file-box empty">
      <span class="error-text">{error}</span>
    </div>
  {:else if files.length === 0 && !audioSummary}
    <!-- No files - show search button -->
    <div class="file-box empty">
      <span class="muted">No file</span>
      {#if onSearchReleases}
        <button class="icon-btn small" title="Search for releases" onclick={onSearchReleases}>
          <span class="material-icons-outlined">search</span>
        </button>
      {/if}
    </div>
  {:else}
    <!-- Show files -->
    {#each files as file (file.id)}
      {@const isDeleting = deletingFileId === file.id}
      <div class="file-box" class:unavailable={!file.available}>
        <div class="file-info">
          <div class="file-name" title={file.file_path}>{file.filename}</div>
          <div class="file-meta">
            <MetaLine items={[file.format, formatSize(file.size_bytes)]} />
          </div>
          {#if file.source_metadata?.release_title}
            <div class="release-title" title={file.source_metadata.release_title}>
              {file.source_metadata.release_title}
            </div>
          {/if}
        </div>
        <div class="file-actions">
          <button
            class="icon-btn small"
            title="Read in browser"
            aria-label="Read in browser"
            onclick={() => goto(`/library/books/${bookId}/read?format=${file.format}`)}
          >
            <span class="material-icons-outlined">auto_stories</span>
          </button>
          <OverflowMenu
            variant="icon"
            icon="more_vert"
            label="File actions"
            busy={isDeleting || rejecting}
            items={[
              onSearchReleases && {
                label: 'Search releases',
                icon: 'search',
                onSelect: onSearchReleases
              },
              onSearchReleases && {
                label: 'Reject & re-search',
                icon: 'block',
                danger: true,
                onSelect: handleRejectAndResearch
              },
              onSearchReleases && {
                label: 'Replace file',
                icon: 'find_replace',
                danger: true,
                onSelect: () => handleReplace(file.id)
              },
              {
                label: 'Move to another book',
                icon: 'drive_file_move',
                onSelect: () => handleMoveFile(file.id)
              },
              {
                label: 'Remove file',
                icon: 'delete_outline',
                danger: true,
                onSelect: () => handleDelete(file.id)
              }
            ]}
          />
        </div>
      </div>
    {/each}
  {/if}

  {#if audioSummary}
    <div class="label" style="margin-top: 12px;">Audiobook</div>
    <div class="file-box audio-summary">
      <div class="file-info">
        <div class="audio-summary-row">
          <span class="material-icons-outlined audio-icon">headphones</span>
          <span class="audio-summary-text">
            <MetaLine
              items={[
                `${audioSummary.count} ${audioSummary.format} file${audioSummary.count !== 1 ? 's' : ''}`,
                formatSize(audioSummary.total_size),
                formatDuration(audioSummary.total_duration_seconds)
              ]}
            />
          </span>
        </div>
      </div>
      <div class="file-actions">
        <a
          class="icon-btn small"
          href="/library/books/{bookId}/listen"
          title="Listen"
          aria-label="Listen"
        >
          <span class="material-icons-outlined">play_arrow</span>
        </a>
        {#if audioFiles.length > 1}
          <button
            class="icon-btn small"
            title={audioExpanded ? 'Collapse' : 'Expand'}
            onclick={() => (audioExpanded = !audioExpanded)}
          >
            <span class="material-icons-outlined">
              {audioExpanded ? 'expand_less' : 'expand_more'}
            </span>
          </button>
        {/if}
      </div>
    </div>
    {#if audioExpanded}
      {#each audioFiles as af (af.id)}
        <div class="file-box audio-file" class:unavailable={!af.available}>
          <div class="file-info">
            <div class="file-name">{af.filename}</div>
            <div class="file-meta">
              <MetaLine
                items={[af.format, formatSize(af.file_size), formatDuration(af.duration_seconds)]}
              />
            </div>
          </div>
        </div>
      {/each}
    {/if}
  {/if}
</div>

{#if movingFileId}
  <MoveFileModal
    title="Move File to Book"
    searchFn={searchBooksForMove}
    onMove={doMoveFile}
    onClose={() => (movingFileId = null)}
  />
{/if}

<style>
  .files-section {
    margin-bottom: 16px;
  }

  .label {
    font-size: 10px;
    text-transform: uppercase;
    color: var(--muted);
    font-weight: 700;
    letter-spacing: 0.5px;
    margin-bottom: 6px;
  }

  .file-box {
    background: rgba(0, 0, 0, 0.2);
    border: 1px solid var(--ring);
    border-radius: 8px;
    padding: 12px;
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 8px;
  }

  .file-box:last-child {
    margin-bottom: 0;
  }

  .file-box.empty {
    justify-content: center;
    gap: 12px;
  }

  .file-box.unavailable {
    opacity: 0.6;
    border-color: var(--danger);
  }

  :global([data-theme='light']) .file-box {
    background: rgba(0, 0, 0, 0.05);
  }

  .file-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }

  .file-name {
    font-size: 12px;
    font-family: monospace;
    word-break: break-all;
  }

  .file-meta {
    font-size: 11px;
    color: var(--muted);
  }

  .release-title {
    font-size: 10px;
    color: var(--muted);
    opacity: 0.7;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .file-actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
  }

  .icon-btn {
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
  }

  .icon-btn:hover:not(:disabled) {
    background: var(--bg);
    color: var(--ink);
  }

  .icon-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .icon-btn.small {
    width: 28px;
    height: 28px;
  }

  .icon-btn.small .material-icons-outlined {
    font-size: 16px;
  }

  .muted {
    color: var(--muted);
    font-size: 12px;
  }

  .error-text {
    color: var(--danger);
    font-size: 12px;
  }

  .audio-summary-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .audio-icon {
    font-size: 16px;
    color: var(--accent);
  }

  .audio-summary-text {
    font-size: 12px;
  }

  .audio-file {
    padding: 8px 12px;
    margin-left: 16px;
    border-left: 2px solid var(--accent);
  }
</style>
