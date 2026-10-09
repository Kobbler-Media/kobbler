<script lang="ts">
  import { untrack } from 'svelte';
  import type { MediaRootFile } from '$lib/types';
  import {
    addMediaRootFileToLibrary,
    listMediaRootFiles,
    ignoreMediaRootFile,
    rematchMediaRootFile,
    bulkIgnoreMediaRootFiles,
    bulkRematchMediaRootFiles
  } from '$lib/api/client';
  import { showMessage } from '$lib/context/app.svelte';
  import { InfiniteScroll } from '@koblibs/ui';

  interface Props {
    rootId: string;
    onResolve?: (file: MediaRootFile) => void;
    onInspect?: (file: MediaRootFile) => void;
    onBulkResolve?: (fileIds: string[]) => void;
    onBulkReassign?: (fileIds: string[], label: string) => void;
  }

  let { rootId, onResolve, onInspect, onBulkResolve, onBulkReassign }: Props = $props();

  const PAGE_SIZE = 50;
  const STATUSES = [
    'all',
    'added',
    'review',
    'ambiguous',
    'unmatched',
    'pending',
    'ignored'
  ] as const;

  const STATUS_ICONS: Record<string, { icon: string; color: string }> = {
    added: { icon: 'check_circle', color: 'var(--success)' },
    identified: { icon: 'check_circle', color: 'var(--success)' },
    review: { icon: 'task_alt', color: 'var(--info, #3b82f6)' },
    ambiguous: { icon: 'help', color: 'var(--warning)' },
    unmatched: { icon: 'remove_circle_outline', color: 'var(--muted)' },
    pending: { icon: 'schedule', color: '#60a5fa' },
    ignored: { icon: 'block', color: 'var(--skipped)' }
  };

  const TYPE_ICONS: Record<string, string> = {
    book: 'menu_book',
    tv_episode: 'tv',
    movie: 'movie'
  };

  let files = $state<MediaRootFile[]>([]);
  let loading = $state(false);
  let hasMore = $state(false);
  let statusFilter = $state<string>('all');
  let offset = $state(0);
  let selectedIds = $state<Set<string>>(new Set());
  let bulkLoading = $state(false);

  let selectedCount = $derived(selectedIds.size);
  let allSelected = $derived(files.length > 0 && files.every((f) => selectedIds.has(f.id)));

  async function loadFiles(reset = false) {
    if (loading) return;
    loading = true;
    const newOffset = reset ? 0 : offset;
    try {
      const result = await listMediaRootFiles(rootId, {
        status: statusFilter === 'all' ? undefined : statusFilter,
        limit: PAGE_SIZE,
        offset: newOffset
      });
      const items = result.items;
      if (reset) {
        files = items;
        offset = items.length;
        selectedIds = new Set();
      } else {
        files = [...files, ...items];
        offset = newOffset + items.length;
      }
      hasMore = items.length === PAGE_SIZE;
    } catch (e) {
      console.error('Failed to load media root files:', e);
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    void rootId;
    void statusFilter;
    untrack(() => loadFiles(true));
  });

  function formatSize(bytes: number | null): string {
    if (!bytes) return '--';
    const units = ['B', 'KB', 'MB', 'GB'];
    let size = bytes;
    let i = 0;
    while (size >= 1024 && i < units.length - 1) {
      size /= 1024;
      i++;
    }
    return `${size.toFixed(i > 0 ? 1 : 0)} ${units[i]}`;
  }

  function toggleSelectAll() {
    if (allSelected) {
      selectedIds = new Set();
    } else {
      selectedIds = new Set(files.map((f) => f.id));
    }
  }

  function toggleSelect(id: string) {
    const next = new Set(selectedIds);
    if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }
    selectedIds = next;
  }

  async function handleBulkIgnore() {
    if (bulkLoading || selectedCount === 0) return;
    bulkLoading = true;
    try {
      const ids = [...selectedIds];
      await bulkIgnoreMediaRootFiles(ids);
      files = files.map((f) => (selectedIds.has(f.id) ? { ...f, match_status: 'ignored' } : f));
      selectedIds = new Set();
      showMessage(`Ignored ${ids.length} file${ids.length === 1 ? '' : 's'}`);
    } catch {
      showMessage('Failed to ignore files');
    } finally {
      bulkLoading = false;
    }
  }

  async function handleBulkRematch() {
    if (bulkLoading || selectedCount === 0) return;
    bulkLoading = true;
    try {
      const ids = [...selectedIds];
      await bulkRematchMediaRootFiles(ids);
      files = files.map((f) => (selectedIds.has(f.id) ? { ...f, match_status: 'pending' } : f));
      selectedIds = new Set();
      showMessage(`Queued ${ids.length} file${ids.length === 1 ? '' : 's'} for rematch`);
    } catch {
      showMessage('Failed to rematch files');
    } finally {
      bulkLoading = false;
    }
  }

  async function handleIgnore(file: MediaRootFile) {
    await ignoreMediaRootFile(file.id);
    files = files.map((f) => (f.id === file.id ? { ...f, match_status: 'ignored' } : f));
  }

  async function handleRematch(file: MediaRootFile) {
    await rematchMediaRootFile(file.id);
    files = files.map((f) => (f.id === file.id ? { ...f, match_status: 'pending' } : f));
  }

  async function handleAddToLibrary(file: MediaRootFile) {
    const prev = file.match_status;
    files = files.map((f) => (f.id === file.id ? { ...f, match_status: 'added' } : f));
    try {
      await addMediaRootFileToLibrary(file.id);
    } catch {
      files = files.map((f) => (f.id === file.id ? { ...f, match_status: prev } : f));
      showMessage('Failed to add to library');
    }
  }
</script>

<div class="file-list">
  <div class="filter-tabs">
    {#each STATUSES as s (s)}
      <button
        type="button"
        class="filter-tab"
        class:active={statusFilter === s}
        onclick={() => (statusFilter = s)}
        >{s === 'all' ? 'All' : s.charAt(0).toUpperCase() + s.slice(1)}</button
      >
    {/each}
  </div>

  {#if selectedCount > 0}
    <div class="bulk-bar">
      <span class="bulk-count">{selectedCount} selected</span>
      <button
        type="button"
        class="bulk-btn bulk-ignore"
        disabled={bulkLoading}
        onclick={handleBulkIgnore}
      >
        <span class="material-icons-outlined">block</span>
        Ignore
      </button>
      <button
        type="button"
        class="bulk-btn bulk-rematch"
        disabled={bulkLoading}
        onclick={handleBulkRematch}
      >
        <span class="material-icons-outlined">refresh</span>
        Rematch
      </button>
      {#if onBulkResolve}
        <button
          type="button"
          class="bulk-btn bulk-resolve"
          disabled={bulkLoading}
          onclick={() => onBulkResolve?.([...selectedIds])}
        >
          <span class="material-icons-outlined">manage_search</span>
          Resolve as
        </button>
      {/if}
      {#if onBulkReassign}
        <button
          type="button"
          class="bulk-btn bulk-reassign"
          disabled={bulkLoading}
          onclick={() => onBulkReassign?.([...selectedIds], 'Selected files')}
        >
          <span class="material-icons-outlined">swap_horiz</span>
          Reassign
        </button>
      {/if}
      <button type="button" class="bulk-btn" onclick={() => (selectedIds = new Set())}>
        Clear
      </button>
    </div>
  {/if}

  {#if files.length === 0 && !loading}
    <div class="empty">No files found.</div>
  {:else}
    <div class="table-scroll">
      <table class="file-table">
        <thead>
          <tr>
            <th class="col-check">
              <input
                type="checkbox"
                checked={allSelected}
                onchange={toggleSelectAll}
                aria-label="Select all"
              />
            </th>
            <th class="col-status-icon"></th>
            <th>File</th>
            <th class="col-type-icon"></th>
            <th class="col-size">Size</th>
            <th class="col-actions"></th>
          </tr>
        </thead>
        <tbody>
          {#each files as file (file.id)}
            <tr class:selected={selectedIds.has(file.id)}>
              <td class="col-check">
                <input
                  type="checkbox"
                  checked={selectedIds.has(file.id)}
                  onchange={() => toggleSelect(file.id)}
                  aria-label="Select {file.file_name}"
                />
              </td>
              <td class="col-status-icon">
                {#if STATUS_ICONS[file.match_status]}
                  <span
                    class="status-icon"
                    style="color: {STATUS_ICONS[file.match_status].color}"
                    title={file.match_status}
                  >
                    <span class="material-icons-outlined"
                      >{STATUS_ICONS[file.match_status].icon}</span
                    >
                  </span>
                {/if}
              </td>
              <td class="cell-name" title={file.file_path}>{file.file_name}</td>
              <td class="col-type-icon">
                {#if file.entity_type && TYPE_ICONS[file.entity_type]}
                  <span class="type-icon" title={file.entity_type}>
                    <span class="material-icons-outlined">{TYPE_ICONS[file.entity_type]}</span>
                  </span>
                {:else}
                  <span class="type-icon muted">--</span>
                {/if}
              </td>
              <td class="col-size muted">{formatSize(file.file_size)}</td>
              <td class="col-actions">
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
                    onclick={() => handleAddToLibrary(file)}
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
                <button
                  type="button"
                  class="icon-action"
                  title="Ignore"
                  onclick={() => handleIgnore(file)}
                >
                  <span class="material-icons-outlined">block</span>
                </button>
                <button
                  type="button"
                  class="icon-action"
                  title="Rematch"
                  onclick={() => handleRematch(file)}
                >
                  <span class="material-icons-outlined">refresh</span>
                </button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}

  <InfiniteScroll {hasMore} {loading} onLoadMore={() => loadFiles()} />
</div>

<style>
  .file-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .filter-tabs {
    display: flex;
    gap: 2px;
    background: var(--card);
    padding: 4px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    flex-wrap: wrap;
  }

  .filter-tab {
    padding: 5px 10px;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    color: var(--muted);
    border: none;
    background: none;
    white-space: nowrap;
    transition: background 0.15s;
  }

  .filter-tab:hover {
    background: rgba(255, 255, 255, 0.05);
    color: var(--ink);
  }

  .filter-tab.active {
    background: var(--bg-2);
    color: var(--ink);
  }

  .bulk-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 8px;
  }

  .bulk-count {
    font-size: 12px;
    font-weight: 600;
    color: var(--ink);
    margin-right: auto;
  }

  .bulk-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 4px 10px;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
  }

  .bulk-btn:hover:not(:disabled) {
    background: var(--bg-2);
    color: var(--ink);
  }

  .bulk-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .bulk-btn :global(.material-icons-outlined) {
    font-size: 14px;
  }

  .bulk-ignore {
    border-color: var(--warning);
    color: var(--warning);
  }

  .bulk-ignore:hover:not(:disabled) {
    background: rgba(249, 115, 22, 0.12);
    color: var(--warning);
  }

  .bulk-rematch {
    border-color: var(--info, #3b82f6);
    color: var(--info, #3b82f6);
  }

  .bulk-rematch:hover:not(:disabled) {
    background: rgba(59, 130, 246, 0.12);
    color: var(--info, #3b82f6);
  }

  .bulk-resolve {
    border-color: var(--warning);
    color: var(--warning);
  }

  .bulk-resolve:hover:not(:disabled) {
    background: rgba(249, 115, 22, 0.12);
    color: var(--warning);
  }

  .bulk-reassign {
    border-color: var(--accent);
    color: var(--accent);
  }

  .bulk-reassign:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.06);
    color: var(--accent);
  }

  .table-scroll {
    overflow-x: auto;
  }

  .file-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }

  .file-table th {
    text-align: left;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--muted);
    padding: 6px 8px;
    border-bottom: 1px solid var(--ring);
  }

  .file-table td {
    padding: 8px;
    border-bottom: 1px solid var(--ring);
    color: var(--ink);
    vertical-align: middle;
  }

  .file-table tr:hover td {
    background: rgba(255, 255, 255, 0.02);
  }

  .file-table tr.selected td {
    background: rgba(59, 130, 246, 0.06);
  }

  .col-check {
    width: 28px;
    text-align: center;
  }

  .col-check input[type='checkbox'] {
    cursor: pointer;
    accent-color: var(--info, #3b82f6);
  }

  .col-status-icon {
    width: 28px;
    text-align: center;
  }

  .status-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }

  .status-icon :global(.material-icons-outlined) {
    font-size: 16px;
  }

  .cell-name {
    max-width: 300px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .col-type-icon {
    width: 28px;
    text-align: center;
  }

  .type-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--muted);
  }

  .type-icon :global(.material-icons-outlined) {
    font-size: 16px;
  }

  .col-size {
    width: 60px;
    font-size: 12px;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .col-actions {
    white-space: nowrap;
    text-align: right;
  }

  .muted {
    color: var(--muted);
  }

  .icon-action {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
    margin-left: 2px;
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

  .empty {
    padding: 32px;
    text-align: center;
    color: var(--muted);
    font-size: 13px;
  }
</style>
