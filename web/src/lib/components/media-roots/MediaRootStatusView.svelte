<script lang="ts">
  import type { MediaRootFile, MediaRootFileLite } from '#lib/types.js';
  import {
    addMediaRootFileToLibrary,
    ignoreMediaRootFile,
    rematchMediaRootFile,
    bulkResolveMediaRootFiles,
    bulkIgnoreMediaRootFiles,
    bulkRematchMediaRootFiles,
    matchMediaRootToLibrary,
    listStatusCounts,
    listMediaRootFilesLite,
    reinterpretMediaRootFiles
  } from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { untrack } from 'svelte';
  import { formatSize } from '#lib/utils/format.js';
  import { getJobsState } from '#lib/stores/jobs.svelte.js';
  import { InfiniteScroll } from '@koblibs/ui';

  const PAGE_SIZE = 50;

  interface Props {
    rootId: string;
    onResolve?: (file: MediaRootFileLite) => void;
    onInspect?: (file: MediaRootFileLite) => void;
    onBulkResolve?: (fileIds: string[]) => void;
    onBulkReassign?: (fileIds: string[], label: string) => void;
    statusFilter: string;
    onStatusFilterChange: (status: string) => void;
    onUpdate?: () => void;
  }

  let {
    rootId,
    onResolve,
    onInspect,
    onBulkResolve,
    onBulkReassign,
    statusFilter,
    onStatusFilterChange,
    onUpdate
  }: Props = $props();

  let files = $state<MediaRootFileLite[]>([]);
  let statusCounts = $state<Record<string, number>>({});
  let offset = $state(0);
  let hasMore = $state(false);
  let loadingFiles = $state(false);

  let selectedIds = $state<Set<string>>(new Set());
  let bulkLoading = $state(false);
  let selectedCount = $derived(selectedIds.size);
  let allSelected = $derived(files.length > 0 && files.every((f) => selectedIds.has(f.id)));

  let addingAll = $state(false);
  let addJobId = $state<string | null>(null);
  let addTotal = $state(0);
  let addResolved = $state(0);
  let matchingToLibrary = $state(false);
  let matchJobId = $state<string | null>(null);
  let matchTotal = $state(0);
  let matchResolved = $state(0);
  let reinterpreting = $state(false);
  const jobStore = getJobsState();

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

  async function loadCounts() {
    try {
      const resp = await listStatusCounts(rootId);
      statusCounts = resp.counts;
    } catch {
      // ignore
    }
  }

  async function loadFiles(reset = false) {
    if (loadingFiles) return;
    loadingFiles = true;
    const newOffset = reset ? 0 : offset;
    try {
      const status = statusFilter === 'all' ? undefined : statusFilter;
      const resp = await listMediaRootFilesLite(rootId, {
        status,
        limit: PAGE_SIZE,
        offset: newOffset
      });
      if (reset) {
        files = resp.items;
        offset = resp.items.length;
        selectedIds = new Set();
      } else {
        files = [...files, ...resp.items];
        offset = newOffset + resp.items.length;
      }
      hasMore = resp.items.length === PAGE_SIZE;
    } catch {
      showMessage('Failed to load files');
    } finally {
      loadingFiles = false;
    }
  }

  export function refresh() {
    loadCounts();
    loadFiles(true);
  }

  /** Soft refresh: update specific files as resolved without resetting the list. */
  export function markResolved(fileIds: string[]) {
    const idSet = new Set(fileIds);
    for (const f of files) {
      if (idSet.has(f.id)) {
        adjustCounts(f.match_status, 'added');
      }
    }
    files = files.map((f) => (idSet.has(f.id) ? { ...f, match_status: 'added' } : f));
    // Remove from view if current filter doesn't show 'added'
    if (statusFilter !== 'all' && statusFilter !== 'added') {
      files = files.filter((f) => !idSet.has(f.id));
    }
    // Deselect resolved files
    const next = new Set(selectedIds);
    for (const id of fileIds) next.delete(id);
    selectedIds = next;
    // Refresh counts from server to stay accurate
    loadCounts();
  }

  // Load counts once on mount, and reload files when rootId or statusFilter changes
  $effect(() => {
    const _id = rootId;
    untrack(() => loadCounts());
  });

  $effect(() => {
    const _id = rootId;
    const _filter = statusFilter;
    untrack(() => loadFiles(true));
  });

  const allCount = $derived(Object.values(statusCounts).reduce((sum, c) => sum + c, 0));

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
      // Count old statuses for adjusting counts
      const oldCounts: Record<string, number> = {};
      for (const f of files) {
        if (selectedIds.has(f.id)) {
          oldCounts[f.match_status] = (oldCounts[f.match_status] ?? 0) + 1;
        }
      }
      await bulkIgnoreMediaRootFiles(ids);
      files = files.map((f) => (selectedIds.has(f.id) ? { ...f, match_status: 'ignored' } : f));
      // Adjust counts
      for (const [status, count] of Object.entries(oldCounts)) {
        adjustCounts(status, 'ignored');
        // adjustCounts handles one at a time, call for each
        for (let i = 1; i < count; i++) adjustCounts(status, 'ignored');
      }
      // Remove from view if filter doesn't match
      if (statusFilter !== 'all' && statusFilter !== 'ignored') {
        files = files.filter((f) => !selectedIds.has(f.id) || f.match_status === statusFilter);
      }
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
      const oldCounts: Record<string, number> = {};
      for (const f of files) {
        if (selectedIds.has(f.id)) {
          oldCounts[f.match_status] = (oldCounts[f.match_status] ?? 0) + 1;
        }
      }
      await bulkRematchMediaRootFiles(ids);
      files = files.map((f) => (selectedIds.has(f.id) ? { ...f, match_status: 'pending' } : f));
      for (const [status, count] of Object.entries(oldCounts)) {
        for (let i = 0; i < count; i++) adjustCounts(status, 'pending');
      }
      if (statusFilter !== 'all' && statusFilter !== 'pending') {
        files = files.filter((f) => !selectedIds.has(f.id) || f.match_status === statusFilter);
      }
      selectedIds = new Set();
      showMessage(`Queued ${ids.length} file${ids.length === 1 ? '' : 's'} for rematch`);
    } catch {
      showMessage('Failed to rematch files');
    } finally {
      bulkLoading = false;
    }
  }

  function truncatePath(filePath: string): string {
    const parts = filePath.split('/');
    if (parts.length <= 4) return filePath;
    return '.../' + parts.slice(-3).join('/');
  }

  async function handleIgnore(file: MediaRootFileLite) {
    const oldStatus = file.match_status;
    updateFileStatus(file.id, 'ignored');
    try {
      await ignoreMediaRootFile(file.id);
      adjustCounts(oldStatus, 'ignored');
    } catch {
      updateFileStatus(file.id, oldStatus);
    }
  }

  async function handleRematch(file: MediaRootFileLite) {
    const oldStatus = file.match_status;
    updateFileStatus(file.id, 'pending');
    try {
      await rematchMediaRootFile(file.id);
      adjustCounts(oldStatus, 'pending');
    } catch {
      updateFileStatus(file.id, oldStatus);
    }
  }

  async function handleAddToLibrary(file: MediaRootFileLite) {
    const oldStatus = file.match_status;
    updateFileStatus(file.id, 'added');
    try {
      await addMediaRootFileToLibrary(file.id);
      adjustCounts(oldStatus, 'added');
      onUpdate?.();
    } catch {
      updateFileStatus(file.id, oldStatus);
      showMessage('Failed to add to library');
    }
  }

  function updateFileStatus(fileId: string, newStatus: MediaRootFile['match_status']) {
    files = files.map((f) => (f.id === fileId ? { ...f, match_status: newStatus } : f));
    // If the file no longer matches the current filter, remove it
    if (statusFilter !== 'all' && newStatus !== statusFilter) {
      files = files.filter((f) => f.id !== fileId);
    }
  }

  function adjustCounts(oldStatus: string, newStatus: string) {
    statusCounts = {
      ...statusCounts,
      [oldStatus]: Math.max((statusCounts[oldStatus] ?? 1) - 1, 0),
      [newStatus]: (statusCounts[newStatus] ?? 0) + 1
    };
  }

  const reviewCount = $derived(statusCounts['review'] ?? 0);

  async function handleAddAllReview() {
    if (reviewCount === 0) return;
    addingAll = true;
    addTotal = reviewCount;
    addResolved = 0;
    try {
      // Fetch review file IDs
      // Fetch all review file IDs, paginating if needed
      const ids: string[] = [];
      let offset = 0;
      const pageSize = 5000;
      while (true) {
        const resp = await listMediaRootFilesLite(rootId, {
          status: 'review',
          limit: pageSize,
          offset
        });
        for (const f of resp.items) ids.push(f.id);
        if (resp.items.length < pageSize) break;
        offset += pageSize;
      }
      if (!ids.length) {
        addingAll = false;
        return;
      }
      const { job_id } = await bulkResolveMediaRootFiles(ids);
      addJobId = job_id;
    } catch {
      showMessage('Add failed');
      addingAll = false;
      addJobId = null;
    }
  }

  async function handleMatchToLibrary() {
    if (reviewCount === 0) return;
    matchingToLibrary = true;
    matchTotal = 0;
    matchResolved = 0;
    try {
      const { job_id, file_count } = await matchMediaRootToLibrary(rootId);
      if (file_count === 0) {
        showMessage('No review files match library entities');
        matchingToLibrary = false;
        return;
      }
      matchTotal = file_count;
      matchJobId = job_id;
    } catch {
      showMessage('Match to library failed');
      matchingToLibrary = false;
      matchJobId = null;
    }
  }

  async function handleReinterpret() {
    const filterCount = statusFilter === 'all' ? allCount : (statusCounts[statusFilter] ?? 0);
    if (filterCount === 0 || reinterpreting) return;
    reinterpreting = true;
    try {
      const status = statusFilter === 'all' ? undefined : statusFilter;
      const { count } = await reinterpretMediaRootFiles(rootId, status);
      showMessage(`Queued ${count} file${count === 1 ? '' : 's'} for reinterpretation`);
      // Refresh after a brief delay to let the job start
      loadCounts();
      loadFiles(true);
    } catch {
      showMessage('Failed to reinterpret files');
    } finally {
      reinterpreting = false;
    }
  }

  // Watch job store for progress/completion of match-to-library job
  $effect(() => {
    if (!matchJobId) return;
    const job = jobStore.jobs.find((j) => j.id === matchJobId);
    if (!job) return;

    if (job.result && typeof job.result === 'object') {
      const r = job.result as Record<string, unknown>;
      if (typeof r.resolved === 'number') matchResolved = r.resolved;
    }

    if (job.status === 'completed') {
      const r = (job.result ?? {}) as Record<string, unknown>;
      const resolved = typeof r.resolved === 'number' ? r.resolved : 0;
      const failed = typeof r.failed === 'number' ? r.failed : 0;
      if (resolved > 0) {
        showMessage(`Matched ${resolved} ${resolved === 1 ? 'file' : 'files'} to library`);
        untrack(() => {
          loadCounts();
          loadFiles(true);
        });
        onUpdate?.();
      }
      if (failed > 0) showMessage(`${failed} failed to match`);
      matchingToLibrary = false;
      matchJobId = null;
    } else if (job.status === 'failed') {
      showMessage(job.error ?? 'Match to library failed');
      matchingToLibrary = false;
      matchJobId = null;
    }
  });

  // Watch job store for progress/completion of the bulk resolve job
  $effect(() => {
    if (!addJobId) return;
    const job = jobStore.jobs.find((j) => j.id === addJobId);
    if (!job) return;

    if (job.result && typeof job.result === 'object') {
      const r = job.result as Record<string, unknown>;
      if (typeof r.resolved === 'number') addResolved = r.resolved;
    }

    if (job.status === 'completed') {
      const r = (job.result ?? {}) as Record<string, unknown>;
      const resolved = typeof r.resolved === 'number' ? r.resolved : 0;
      const failed = typeof r.failed === 'number' ? r.failed : 0;
      if (resolved > 0) {
        showMessage(`Added ${resolved} ${resolved === 1 ? 'file' : 'files'}`);
        untrack(() => {
          // Refresh counts and file list
          loadCounts();
          loadFiles(true);
        });
        onUpdate?.();
      }
      if (failed > 0) showMessage(`${failed} failed to add`);
      addingAll = false;
      addJobId = null;
    } else if (job.status === 'failed') {
      showMessage(job.error ?? 'Add failed');
      addingAll = false;
      addJobId = null;
    }
  });
</script>

<div class="status-view">
  <div class="filter-bar">
    <div class="filter-tabs">
      {#each STATUSES as s (s)}
        {@const count = s === 'all' ? allCount : (statusCounts[s] ?? 0)}
        <button
          type="button"
          class="filter-tab"
          class:active={statusFilter === s}
          onclick={() => onStatusFilterChange(s)}
        >
          {s === 'all' ? 'All' : s.charAt(0).toUpperCase() + s.slice(1)}
          {#if count > 0}
            <span class="tab-count">{count}</span>
          {/if}
        </button>
      {/each}
    </div>
    {#if statusFilter !== 'pending'}
      {@const filterCount = statusFilter === 'all' ? allCount : (statusCounts[statusFilter] ?? 0)}
      {#if filterCount > 0}
        <button
          type="button"
          class="btn-reinterpret"
          disabled={reinterpreting}
          onclick={handleReinterpret}
        >
          {#if reinterpreting}
            <span class="material-icons-outlined spin">sync</span>
            Reinterpreting...
          {:else}
            <span class="material-icons-outlined">refresh</span>
            Reinterpret ({filterCount})
          {/if}
        </button>
      {/if}
    {/if}
    {#if statusFilter === 'review' && reviewCount > 0}
      <button
        type="button"
        class="btn-match-library"
        disabled={matchingToLibrary}
        onclick={handleMatchToLibrary}
      >
        {#if matchingToLibrary}
          <span class="material-icons-outlined spin">sync</span>
          Matching... ({matchResolved}/{matchTotal})
        {:else}
          <span class="material-icons-outlined">library_add_check</span>
          Match to Library
        {/if}
      </button>
      <button type="button" class="btn-add-all" disabled={addingAll} onclick={handleAddAllReview}>
        {#if addingAll}
          <span class="material-icons-outlined spin">sync</span>
          Adding... ({addResolved}/{addTotal})
        {:else}
          <span class="material-icons-outlined">library_add</span>
          Add All ({reviewCount})
        {/if}
      </button>
    {/if}
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
          <span class="material-icons-outlined">drive_file_move</span>
          Reassign
        </button>
      {/if}
      <button type="button" class="bulk-btn" onclick={() => (selectedIds = new Set())}>
        Clear
      </button>
    </div>
  {/if}

  {#if files.length === 0 && !loadingFiles}
    <div class="empty">No files match this filter.</div>
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
            <th>File Name</th>
            <th class="col-path">Path</th>
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
              <td class="cell-name" title={file.file_name}>{file.file_name}</td>
              <td class="col-path cell-path" title={file.file_path}
                >{truncatePath(file.file_path)}</td
              >
              <td class="col-type-icon">
                {#if file.entity_type && TYPE_ICONS[file.entity_type]}
                  <span class="type-icon" title={file.entity_type}>
                    <span class="material-icons-outlined">{TYPE_ICONS[file.entity_type]}</span>
                  </span>
                {:else}
                  <span class="type-icon muted">--</span>
                {/if}
              </td>
              <td class="col-size muted">
                {file.file_size != null ? formatSize(file.file_size) : '--'}
              </td>
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
      <InfiniteScroll {hasMore} loading={loadingFiles} onLoadMore={() => loadFiles(false)} />
    </div>
  {/if}
</div>

<style>
  .status-view {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .filter-bar {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .btn-reinterpret {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 5px 12px;
    border-radius: 6px;
    border: 1px solid var(--muted);
    background: transparent;
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
    flex-shrink: 0;
    transition:
      opacity 0.15s,
      transform 0.15s;
  }

  .btn-reinterpret :global(.material-icons-outlined) {
    font-size: 15px;
  }

  .btn-reinterpret:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.06);
    color: var(--ink);
    transform: translateY(-1px);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.2);
  }

  .btn-reinterpret:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-match-library {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 5px 12px;
    border-radius: 6px;
    border: 1px solid var(--info, #3b82f6);
    background: transparent;
    color: var(--info, #3b82f6);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
    flex-shrink: 0;
    transition:
      opacity 0.15s,
      transform 0.15s;
  }

  .btn-match-library :global(.material-icons-outlined) {
    font-size: 15px;
  }

  .btn-match-library:hover:not(:disabled) {
    background: rgba(59, 130, 246, 0.1);
    transform: translateY(-1px);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.2);
  }

  .btn-match-library:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-add-all {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 5px 12px;
    border-radius: 6px;
    border: none;
    background: var(--accent);
    color: #1d130c;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
    flex-shrink: 0;
    transition:
      opacity 0.15s,
      transform 0.15s;
  }

  .btn-add-all :global(.material-icons-outlined) {
    font-size: 15px;
  }

  .btn-add-all:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.2);
  }

  .btn-add-all:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  .spin {
    animation: spin 1s linear infinite;
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
    display: inline-flex;
    align-items: center;
    gap: 6px;
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

  .tab-count {
    font-size: 10px;
    padding: 1px 5px;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.08);
    font-variant-numeric: tabular-nums;
  }

  .filter-tab.active .tab-count {
    background: rgba(255, 255, 255, 0.12);
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

  .col-check {
    width: 28px;
    text-align: center;
  }

  .col-check input[type='checkbox'] {
    cursor: pointer;
    accent-color: var(--info, #3b82f6);
  }

  .file-table tr.selected td {
    background: rgba(59, 130, 246, 0.06);
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
    max-width: 240px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .col-path {
    max-width: 200px;
  }

  .cell-path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    color: var(--muted);
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
