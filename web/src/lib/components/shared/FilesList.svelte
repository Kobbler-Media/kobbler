<script lang="ts">
  import { formatSize } from '$lib/utils/format';
  import QualityBadge from './QualityBadge.svelte';
  import FileActionsDropdown from './FileActionsDropdown.svelte';
  import UpgradeStatusBadge from './UpgradeStatusBadge.svelte';

  interface FileItem {
    id: string;
    file_path: string;
    file_size: number | null;
    quality?: string | null;
    source_type?: string | null;
    source_metadata?: Record<string, unknown> | null;
  }

  interface Props {
    files: FileItem[];
    deletingFileId: string | null;
    startingPlaybackFileId: string | null;
    upgradeUntil?: string | null;
    onPlay: (fileId: string) => void;
    onDelete: (fileId: string) => void;
    onMove?: ((fileId: string) => void) | undefined;
    onSearchReleases?: (() => void) | undefined;
    onRejectAndResearch?: (() => void) | undefined;
    onReplace?: ((fileId: string) => void) | undefined;
    rejecting?: boolean;
  }

  let {
    files,
    deletingFileId,
    startingPlaybackFileId,
    upgradeUntil = null,
    onPlay,
    onDelete,
    onMove,
    onSearchReleases,
    onRejectAndResearch,
    onReplace,
    rejecting = false
  }: Props = $props();
</script>

{#if files.length > 0}
  <UpgradeStatusBadge {upgradeUntil} />
  <div class="files-list">
    {#each files as file (file.id)}
      {@const isDeleting = deletingFileId === file.id}
      {@const isStarting = startingPlaybackFileId === file.id}
      <div class="file-item">
        <div class="file-info">
          <span class="file-name" title={file.file_path}>{file.file_path.split('/').pop()}</span>
          {#if file.file_size}
            <span class="file-size">{formatSize(file.file_size)}</span>
          {/if}
          {#if file.source_metadata?.release_title}
            <span class="release-title" title={String(file.source_metadata.release_title)}>
              {file.source_metadata.release_title}
            </span>
          {/if}
        </div>
        <div class="file-actions">
          <button
            class="icon-btn play-file-btn"
            title="Play"
            onclick={() => onPlay(file.id)}
            disabled={isStarting}
          >
            {#if isStarting}
              <span class="material-icons-outlined spin">refresh</span>
            {:else}
              <span class="material-icons-outlined">play_arrow</span>
            {/if}
          </button>
          <QualityBadge quality={file.quality ?? null} source={file.source_type ?? null} />
          {#if onMove}
            <button class="icon-btn" title="Move to another entity" onclick={() => onMove(file.id)}>
              <span class="material-icons-outlined">swap_horiz</span>
            </button>
          {/if}
          {#if onSearchReleases || onRejectAndResearch}
            <FileActionsDropdown
              {onSearchReleases}
              {onRejectAndResearch}
              onReplace={onReplace ? () => onReplace(file.id) : undefined}
              {rejecting}
            />
          {/if}
          <button
            class="icon-btn"
            title="Remove file"
            onclick={() => onDelete(file.id)}
            disabled={isDeleting}
          >
            {#if isDeleting}
              <span class="material-icons-outlined">hourglass_empty</span>
            {:else}
              <span class="material-icons-outlined">delete_outline</span>
            {/if}
          </button>
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .files-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .file-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    padding: 10px 14px;
    background: var(--bg-2);
    border-radius: 10px;
    border: 1px solid var(--ring);
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
    font-weight: 600;
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .file-size {
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
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  .icon-btn {
    width: 28px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .icon-btn :global(.material-icons-outlined) {
    font-size: 16px;
  }

  .icon-btn:hover:not(:disabled) {
    background: var(--bg);
    color: var(--ink);
  }

  .icon-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .play-file-btn {
    border-color: var(--accent);
    color: var(--accent);
  }

  .play-file-btn:hover:not(:disabled) {
    background: var(--accent-dim);
  }

  .spin {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
