<script lang="ts">
  import type { MediaRoot, MediaRootFileLite } from '#lib/types.js';
  import ContentPageView from '#lib/components/ContentPageView.svelte';
  import MediaRootInfoCard from './MediaRootInfoCard.svelte';
  import MediaRootDirectoryView from './MediaRootDirectoryView.svelte';
  import MediaRootEntityView from './MediaRootEntityView.svelte';
  import MediaRootStatusView from './MediaRootStatusView.svelte';
  import {
    searchMediaRootFiles,
    ignoreMediaRootFile,
    rematchMediaRootFile,
    bulkIgnoreMediaRootFiles,
    bulkRematchMediaRootFiles,
    type MediaRootFileSearchResult
  } from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';

  interface Props {
    root: MediaRoot;
    loading: boolean;
    onClose: () => void;
    onScan: (id: string) => Promise<void>;
    onWriteSidecars?: (id: string) => Promise<void>;
    onWriteImages?: (id: string) => Promise<void>;
    onResolve?: (file: MediaRootFileLite) => void;
    onInspect?: (file: MediaRootFileLite) => void;
    onGroupResolve?: (groupKey: string, rootId: string) => void;
    onReassign?: (groupKey: string, rootId: string) => void;
    onResolveSelected?: (fileIds: string[], rootId: string) => void;
    onBulkResolve?: (fileIds: string[]) => void;
    onBulkReassign?: (fileIds: string[], label: string) => void;
    onReorganize?: (id: string) => void;
    onUpdate?: () => void;
    onEdit?: () => void;
  }

  let {
    root,
    loading,
    onClose,
    onScan,
    onWriteSidecars,
    onWriteImages,
    onResolve,
    onInspect,
    onGroupResolve,
    onReassign,
    onResolveSelected,
    onBulkResolve,
    onBulkReassign,
    onReorganize,
    onUpdate,
    onEdit
  }: Props = $props();

  type ViewMode = 'directory' | 'entity' | 'status';
  let viewMode = $state<ViewMode>('status');
  let statusFilter = $state('all');
  let searchQuery = $state('');
  let searchResults = $state<MediaRootFileSearchResult[] | null>(null);
  let searchTimeout: ReturnType<typeof setTimeout> | undefined;
  const searchLimit = 100;
  let searchHasMore = $state(false);
  let searchLoadingMore = $state(false);

  // Search selection state
  let selectedSearchIds = $state<Set<string>>(new Set());
  let searchBulkLoading = $state(false);
  let selectedSearchCount = $derived(selectedSearchIds.size);
  let allSearchSelected = $derived(
    searchResults != null &&
      searchResults.length > 0 &&
      searchResults.every((r) => selectedSearchIds.has(r.id))
  );

  // Component refs for refreshing
  let entityView = $state<MediaRootEntityView | null>(null);
  let statusView = $state<MediaRootStatusView | null>(null);
  let directoryView = $state<MediaRootDirectoryView | null>(null);

  function handleSearch(query: string) {
    searchQuery = query;
    if (searchTimeout) clearTimeout(searchTimeout);

    if (!query.trim()) {
      searchResults = null;
      selectedSearchIds = new Set();
      return;
    }

    searchTimeout = setTimeout(async () => {
      try {
        const results = await searchMediaRootFiles(root.id, query, searchLimit);
        searchResults = results;
        searchHasMore = results.length === searchLimit;
        selectedSearchIds = new Set();
      } catch {
        searchResults = null;
        searchHasMore = false;
      }
    }, 300);
  }

  function toggleSearchSelectAll() {
    if (allSearchSelected) {
      selectedSearchIds = new Set();
    } else {
      selectedSearchIds = new Set((searchResults ?? []).map((r) => r.id));
    }
  }

  function toggleSearchSelect(id: string) {
    const next = new Set(selectedSearchIds);
    if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }
    selectedSearchIds = next;
  }

  function updateSearchResultStatus(id: string, newStatus: string) {
    if (!searchResults) return;
    searchResults = searchResults.map((r) => (r.id === id ? { ...r, match_status: newStatus } : r));
  }

  async function handleSearchIgnore(id: string) {
    updateSearchResultStatus(id, 'ignored');
    try {
      await ignoreMediaRootFile(id);
    } catch {
      showMessage('Failed to ignore file');
    }
  }

  async function handleSearchRematch(id: string) {
    updateSearchResultStatus(id, 'pending');
    try {
      await rematchMediaRootFile(id);
    } catch {
      showMessage('Failed to rematch file');
    }
  }

  async function handleSearchBulkIgnore() {
    if (searchBulkLoading || selectedSearchCount === 0) return;
    searchBulkLoading = true;
    try {
      const ids = [...selectedSearchIds];
      await bulkIgnoreMediaRootFiles(ids);
      if (searchResults) {
        searchResults = searchResults.map((r) =>
          selectedSearchIds.has(r.id) ? { ...r, match_status: 'ignored' } : r
        );
      }
      selectedSearchIds = new Set();
      showMessage(`Ignored ${ids.length} file${ids.length === 1 ? '' : 's'}`);
    } catch {
      showMessage('Failed to ignore files');
    } finally {
      searchBulkLoading = false;
    }
  }

  async function handleSearchBulkRematch() {
    if (searchBulkLoading || selectedSearchCount === 0) return;
    searchBulkLoading = true;
    try {
      const ids = [...selectedSearchIds];
      await bulkRematchMediaRootFiles(ids);
      if (searchResults) {
        searchResults = searchResults.map((r) =>
          selectedSearchIds.has(r.id) ? { ...r, match_status: 'pending' } : r
        );
      }
      selectedSearchIds = new Set();
      showMessage(`Queued ${ids.length} file${ids.length === 1 ? '' : 's'} for rematch`);
    } catch {
      showMessage('Failed to rematch files');
    } finally {
      searchBulkLoading = false;
    }
  }

  async function loadMoreSearchResults() {
    if (!searchResults || searchLoadingMore || !searchHasMore) return;
    searchLoadingMore = true;
    try {
      const more = await searchMediaRootFiles(
        root.id,
        searchQuery,
        searchLimit,
        searchResults.length
      );
      searchResults = [...searchResults, ...more];
      searchHasMore = more.length === searchLimit;
    } catch {
      showMessage('Failed to load more results');
    } finally {
      searchLoadingMore = false;
    }
  }

  const ACTIONABLE_STATUSES = new Set([
    'pending',
    'review',
    'ambiguous',
    'unmatched',
    'identified'
  ]);

  function selectUnresolved() {
    if (!searchResults) return;
    selectedSearchIds = new Set(
      searchResults.filter((r) => ACTIONABLE_STATUSES.has(r.match_status)).map((r) => r.id)
    );
  }

  function truncateSearchPath(filePath: string): string {
    const parts = filePath.split('/');
    if (parts.length <= 3) return filePath;
    return '.../' + parts.slice(-3, -1).join('/');
  }

  const handleRefresh = async () => {
    await onScan(root.id);
  };

  /**
   * Refresh the currently active view (called by parent after resolve/reorganize).
   * If resolvedFileIds is provided, do a soft refresh: update those files' status
   * locally and refresh counts, without resetting the list scroll position.
   */
  export function refreshActiveView(resolvedFileIds?: string[]) {
    // If search is active, re-run the search to pick up status changes
    if (searchResults && searchQuery.trim()) {
      const currentCount = searchResults.length;
      searchMediaRootFiles(root.id, searchQuery, Math.max(currentCount, searchLimit))
        .then((results) => {
          searchResults = results;
          searchHasMore = results.length === Math.max(currentCount, searchLimit);
        })
        .catch(() => {});
    }

    if (resolvedFileIds && resolvedFileIds.length > 0) {
      // Soft refresh: optimistic update + counts only
      if (viewMode === 'status') statusView?.markResolved(resolvedFileIds);
      else if (viewMode === 'entity') entityView?.refresh();
      else if (viewMode === 'directory') directoryView?.refresh();
    } else {
      // Hard refresh for major operations like scan/reorganize
      if (viewMode === 'entity') entityView?.refresh();
      else if (viewMode === 'status') statusView?.refresh();
      else if (viewMode === 'directory') directoryView?.refresh();
    }
  }
</script>

<ContentPageView title="Media Root" {onClose} onRefresh={handleRefresh} {onEdit}>
  <div class="detail-grid">
    <div class="main-column">
      <div class="search-bar">
        <span class="material-icons-outlined search-icon">search</span>
        <input
          type="text"
          class="search-input"
          placeholder="Search files..."
          value={searchQuery}
          oninput={(e) => handleSearch(e.currentTarget.value)}
        />
        {#if searchQuery}
          <button class="clear-btn" type="button" onclick={() => handleSearch('')}>
            <span class="material-icons-outlined">close</span>
          </button>
        {/if}
      </div>

      {#if searchResults}
        <div class="search-results">
          <div class="search-results-header">
            <input
              type="checkbox"
              checked={allSearchSelected}
              onchange={toggleSearchSelectAll}
              aria-label="Select all results"
            />
            <span>{searchResults.length} result{searchResults.length !== 1 ? 's' : ''}</span>
            <button class="clear-search-btn" type="button" onclick={selectUnresolved}>
              Select unresolved
            </button>
            <button class="clear-search-btn" type="button" onclick={() => handleSearch('')}>
              Clear search
            </button>
          </div>

          {#if selectedSearchCount > 0}
            <div class="bulk-bar">
              <span class="bulk-count">{selectedSearchCount} selected</span>
              <button
                type="button"
                class="bulk-btn bulk-ignore"
                disabled={searchBulkLoading}
                onclick={handleSearchBulkIgnore}
              >
                <span class="material-icons-outlined">block</span>
                Ignore
              </button>
              <button
                type="button"
                class="bulk-btn bulk-rematch"
                disabled={searchBulkLoading}
                onclick={handleSearchBulkRematch}
              >
                <span class="material-icons-outlined">refresh</span>
                Rematch
              </button>
              {#if onBulkResolve}
                <button
                  type="button"
                  class="bulk-btn bulk-resolve"
                  disabled={searchBulkLoading}
                  onclick={() => onBulkResolve?.([...selectedSearchIds])}
                >
                  <span class="material-icons-outlined">manage_search</span>
                  Resolve as
                </button>
              {/if}
              {#if onBulkReassign}
                <button
                  type="button"
                  class="bulk-btn bulk-reassign"
                  disabled={searchBulkLoading}
                  onclick={() => onBulkReassign?.([...selectedSearchIds], 'Selected files')}
                >
                  <span class="material-icons-outlined">drive_file_move</span>
                  Reassign
                </button>
              {/if}
              <button
                type="button"
                class="bulk-btn"
                onclick={() => (selectedSearchIds = new Set())}
              >
                Clear
              </button>
            </div>
          {/if}

          {#each searchResults as result (result.id)}
            <div class="search-result-row" class:selected={selectedSearchIds.has(result.id)}>
              <input
                type="checkbox"
                checked={selectedSearchIds.has(result.id)}
                onchange={() => toggleSearchSelect(result.id)}
                aria-label="Select {result.file_name}"
              />
              <div class="sr-info">
                <span class="sr-name" title={result.file_path}>{result.file_name}</span>
                <span class="sr-path">{truncateSearchPath(result.file_path)}</span>
              </div>
              <span class="sr-badge badge-{result.match_status}">{result.match_status}</span>
              {#if result.series_name}
                <span class="sr-meta">{result.series_name}</span>
              {/if}
              {#if result.show_name}
                <span class="sr-meta">{result.show_name}</span>
              {:else if result.movie_title}
                <span class="sr-meta">{result.movie_title}</span>
              {:else if result.book_title && result.book_title !== result.series_name}
                <span class="sr-meta">{result.book_title}</span>
              {/if}
              <span class="sr-actions">
                {#if result.match_status !== 'added' && result.match_status !== 'ignored' && onBulkResolve}
                  <button
                    type="button"
                    class="icon-action icon-resolve"
                    title="Resolve match"
                    onclick={() => onBulkResolve?.([result.id])}
                  >
                    <span class="material-icons-outlined">manage_search</span>
                  </button>
                {/if}
                {#if onBulkReassign}
                  <button
                    type="button"
                    class="icon-action icon-reassign"
                    title="Reassign"
                    onclick={() => onBulkReassign?.([result.id], result.file_name)}
                  >
                    <span class="material-icons-outlined">drive_file_move</span>
                  </button>
                {/if}
                <button
                  type="button"
                  class="icon-action"
                  title="Ignore"
                  onclick={() => handleSearchIgnore(result.id)}
                >
                  <span class="material-icons-outlined">block</span>
                </button>
                <button
                  type="button"
                  class="icon-action"
                  title="Rematch"
                  onclick={() => handleSearchRematch(result.id)}
                >
                  <span class="material-icons-outlined">refresh</span>
                </button>
              </span>
            </div>
          {/each}
          {#if searchHasMore}
            <button
              type="button"
              class="load-more-btn"
              disabled={searchLoadingMore}
              onclick={loadMoreSearchResults}
            >
              {searchLoadingMore ? 'Loading...' : 'Load more results'}
            </button>
          {/if}
        </div>
      {:else}
        <div class="view-tabs">
          <button
            type="button"
            class="view-tab"
            class:active={viewMode === 'directory'}
            onclick={() => (viewMode = 'directory')}
          >
            <span class="material-icons-outlined">folder</span>
            Directory
          </button>
          <button
            type="button"
            class="view-tab"
            class:active={viewMode === 'entity'}
            onclick={() => (viewMode = 'entity')}
          >
            <span class="material-icons-outlined">category</span>
            By Entity
          </button>
          <button
            type="button"
            class="view-tab"
            class:active={viewMode === 'status'}
            onclick={() => (viewMode = 'status')}
          >
            <span class="material-icons-outlined">list</span>
            By Status
          </button>
        </div>

        {#if loading}
          <div class="loading">Loading...</div>
        {:else if viewMode === 'directory'}
          <MediaRootDirectoryView
            bind:this={directoryView}
            rootId={root.id}
            rootPath={root.path}
            {onResolve}
            {onInspect}
            {onResolveSelected}
          />
        {:else if viewMode === 'entity'}
          <MediaRootEntityView
            bind:this={entityView}
            rootId={root.id}
            mediaType={root.media_type}
            {onResolve}
            {onInspect}
            {onGroupResolve}
            {onReassign}
            {onResolveSelected}
            {onUpdate}
          />
        {:else}
          <MediaRootStatusView
            bind:this={statusView}
            rootId={root.id}
            {onResolve}
            {onInspect}
            {onBulkResolve}
            {onBulkReassign}
            {statusFilter}
            onStatusFilterChange={(s) => (statusFilter = s)}
            {onUpdate}
          />
        {/if}
      {/if}
    </div>

    <div class="sidebar-column">
      <MediaRootInfoCard
        {root}
        {loading}
        {onScan}
        {onWriteSidecars}
        {onWriteImages}
        {onReorganize}
        {onUpdate}
      />
    </div>
  </div>
</ContentPageView>

<style>
  .detail-grid {
    display: grid;
    grid-template-columns: 1fr 340px;
    gap: 24px;
    padding: 24px;
    align-items: start;
  }

  .main-column {
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-width: 0;
  }

  .sidebar-column {
    position: sticky;
    top: 80px;
  }

  .search-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: var(--card);
  }

  .search-icon {
    font-size: 20px;
    color: var(--muted);
  }

  .search-input {
    flex: 1;
    border: none;
    background: none;
    color: var(--ink);
    font-size: 14px;
    outline: none;
  }

  .search-input::placeholder {
    color: var(--muted);
  }

  .clear-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border: none;
    background: none;
    color: var(--muted);
    cursor: pointer;
    border-radius: 4px;
  }

  .clear-btn:hover {
    color: var(--ink);
  }

  .clear-btn :global(.material-icons-outlined) {
    font-size: 18px;
  }

  .view-tabs {
    display: flex;
    gap: 2px;
    background: var(--card);
    padding: 4px;
    border-radius: 8px;
    border: 1px solid var(--ring);
  }

  .view-tab {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    color: var(--muted);
    border: none;
    background: none;
    transition: background 0.15s;
  }

  .view-tab :global(.material-icons-outlined) {
    font-size: 16px;
  }

  .view-tab:hover {
    background: rgba(255, 255, 255, 0.05);
    color: var(--ink);
  }

  .view-tab.active {
    background: var(--bg-2);
    color: var(--ink);
  }

  .loading,
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 48px;
    color: var(--muted);
    text-align: center;
  }

  .search-results {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .search-results-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 0;
    font-size: 12px;
    color: var(--muted);
  }

  .search-results-header span {
    flex: 1;
  }

  .search-results-header input[type='checkbox'] {
    cursor: pointer;
    accent-color: var(--info, #3b82f6);
  }

  .clear-search-btn {
    padding: 4px 8px;
    border-radius: 6px;
    font-size: 11px;
    font-weight: 600;
    border: 1px solid var(--ring);
    background: none;
    color: var(--muted);
    cursor: pointer;
    flex-shrink: 0;
  }

  .clear-search-btn:hover {
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
    margin-bottom: 4px;
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

  .search-result-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border-radius: 6px;
    font-size: 13px;
  }

  .search-result-row:hover {
    background: rgba(255, 255, 255, 0.03);
  }

  .search-result-row.selected {
    background: rgba(59, 130, 246, 0.06);
  }

  .search-result-row input[type='checkbox'] {
    cursor: pointer;
    accent-color: var(--info, #3b82f6);
    flex-shrink: 0;
  }

  .sr-info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .sr-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .sr-path {
    font-size: 11px;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    opacity: 0.7;
  }

  .sr-actions {
    display: flex;
    gap: 2px;
    flex-shrink: 0;
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

  .icon-reassign {
    border-color: var(--accent);
    color: var(--accent);
  }

  .icon-reassign:hover {
    background: rgba(255, 255, 255, 0.06);
    color: var(--accent);
  }

  .load-more-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 10px;
    margin-top: 4px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--card);
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
  }

  .load-more-btn:hover:not(:disabled) {
    color: var(--ink);
    background: var(--bg-2);
  }

  .load-more-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .sr-badge {
    padding: 2px 6px;
    border-radius: 4px;
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    flex-shrink: 0;
  }

  .badge-matched,
  .badge-identified {
    background: rgba(34, 197, 94, 0.15);
    color: var(--success);
  }
  .badge-review {
    background: rgba(59, 130, 246, 0.15);
    color: var(--info, #3b82f6);
  }
  .badge-ambiguous {
    background: rgba(249, 115, 22, 0.15);
    color: var(--warning);
  }
  .badge-unmatched {
    background: rgba(255, 255, 255, 0.06);
    color: var(--muted);
  }
  .badge-pending {
    background: rgba(96, 165, 250, 0.15);
    color: #60a5fa;
  }
  .badge-ignored {
    background: rgba(255, 255, 255, 0.04);
    color: var(--skipped);
    opacity: 0.7;
  }

  .sr-meta {
    font-size: 11px;
    color: var(--muted);
    flex-shrink: 0;
  }

  @media (max-width: 900px) {
    .detail-grid {
      grid-template-columns: 1fr;
    }

    .sidebar-column {
      position: static;
      order: -1;
    }
  }
</style>
