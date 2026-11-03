<script lang="ts">
  import type { MediaRootFile, MediaRootFileLite } from '#lib/types.js';
  import {
    addMediaRootFileToLibrary,
    ignoreMediaRootFile,
    rematchMediaRootFile
  } from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { formatSize } from '#lib/utils/format.js';

  interface Props {
    file: MediaRootFileLite;
    label?: string;
    onResolve?: (file: MediaRootFileLite) => void;
    onInspect?: (file: MediaRootFileLite) => void;
    onStatusChange?: (fileId: string, newStatus: MediaRootFile['match_status']) => void;
  }

  let { file, label, onResolve, onInspect, onStatusChange }: Props = $props();

  const STATUS_ICONS: Record<string, { icon: string; color: string }> = {
    added: { icon: 'check_circle', color: 'var(--success)' },
    identified: { icon: 'check_circle', color: 'var(--success)' },
    review: { icon: 'task_alt', color: 'var(--info, #3b82f6)' },
    ambiguous: { icon: 'help', color: 'var(--warning)' },
    unmatched: { icon: 'remove_circle_outline', color: 'var(--muted)' },
    pending: { icon: 'schedule', color: '#60a5fa' },
    ignored: { icon: 'block', color: 'var(--skipped)' }
  };

  const displayLabel = $derived(label ?? file.file_name);

  async function handleIgnore() {
    onStatusChange?.(file.id, 'ignored');
    try {
      await ignoreMediaRootFile(file.id);
    } catch {
      onStatusChange?.(file.id, file.match_status);
    }
  }

  async function handleRematch() {
    const prev = file.match_status;
    onStatusChange?.(file.id, 'pending');
    try {
      await rematchMediaRootFile(file.id);
    } catch {
      onStatusChange?.(file.id, prev);
    }
  }

  async function handleAddToLibrary() {
    onStatusChange?.(file.id, 'added');
    try {
      await addMediaRootFileToLibrary(file.id);
    } catch {
      onStatusChange?.(file.id, file.match_status);
      showMessage('Failed to add to library');
    }
  }
</script>

<div class="file-row">
  {#if STATUS_ICONS[file.match_status]}
    <span
      class="status-icon"
      style="color: {STATUS_ICONS[file.match_status].color}"
      title={file.match_status}
    >
      <span class="material-icons-outlined">{STATUS_ICONS[file.match_status].icon}</span>
    </span>
  {/if}
  <span class="file-label" title={file.file_path}>{displayLabel}</span>
  <span class="file-size">{file.file_size != null ? formatSize(file.file_size) : '--'}</span>
  <span class="file-actions">
    {#if file.match_status !== 'added' && file.match_status !== 'ignored' && onResolve}
      <button
        type="button"
        class="icon-action icon-resolve"
        title="Resolve match"
        onclick={() => onResolve?.(file)}
      >
        <span class="material-icons-outlined">manage_search</span>
      </button>
    {/if}
    {#if file.match_status === 'review'}
      <button
        type="button"
        class="icon-action icon-add"
        title="Add to library"
        onclick={handleAddToLibrary}
      >
        <span class="material-icons-outlined">library_add</span>
      </button>
    {/if}
    {#if onInspect}
      <button
        type="button"
        class="icon-action"
        title="File details"
        onclick={() => onInspect?.(file)}
      >
        <span class="material-icons-outlined">help_outline</span>
      </button>
    {/if}
    <button type="button" class="icon-action" title="Ignore" onclick={handleIgnore}>
      <span class="material-icons-outlined">block</span>
    </button>
    <button type="button" class="icon-action" title="Rematch" onclick={handleRematch}>
      <span class="material-icons-outlined">refresh</span>
    </button>
  </span>
</div>

<style>
  .file-row {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 8px;
    border-bottom: 1px solid color-mix(in srgb, var(--ring) 40%, transparent);
  }

  .file-row:hover {
    background: rgba(255, 255, 255, 0.02);
  }

  .status-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .status-icon :global(.material-icons-outlined) {
    font-size: 15px;
  }

  .file-label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
  }

  .file-size {
    font-size: 11px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
    flex-shrink: 0;
  }

  .file-actions {
    display: flex;
    align-items: center;
    gap: 2px;
    flex-shrink: 0;
  }

  .icon-action {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
  }

  .icon-action:hover {
    background: var(--bg-2);
    color: var(--ink);
  }

  .icon-action :global(.material-icons-outlined) {
    font-size: 14px;
  }

  .icon-resolve {
    border-color: var(--warning);
    color: var(--warning);
  }

  .icon-resolve:hover {
    background: rgba(249, 115, 22, 0.12);
    color: var(--warning);
  }

  .icon-add {
    border-color: var(--success, #22c55e);
    color: var(--success, #22c55e);
  }

  .icon-add:hover {
    background: rgba(34, 197, 94, 0.12);
    color: var(--success, #22c55e);
  }
</style>
