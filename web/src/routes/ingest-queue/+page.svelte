<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import IngestQueuePage from '#lib/components/pages/IngestQueuePage.svelte';
  import { getAuthState } from '#lib/stores/auth.svelte.js';
  import { getLibraryState } from '#lib/stores/library.svelte.js';
  import { showMessage, loadSeries } from '#lib/context/app.svelte.js';
  import { updateUrlParams } from '#lib/utils/url-state.js';
  import type { IngestQueueGroup, IngestQueueStatusCounts, DeviceSummary } from '#lib/types.js';
  import * as api from '#lib/api/client.js';
  import { on, off, onReconnect, offReconnect } from '#lib/services/eventstream.svelte.js';

  // Import the shared type
  import type { BookFormData } from '#lib/components/ingest/types.js';

  const auth = getAuthState();

  const apiBase = import.meta.env.VITE_API_BASE_URL || '';

  // Debounce timer for live updates
  let reloadTimeout: ReturnType<typeof setTimeout> | null = null;
  let ingestCallback: ((data: any) => void) | null = null;
  let reconnectHandler: (() => void) | null = null;

  // Store state
  const library = getLibraryState();

  // Derived state
  const seriesList = $derived(library.seriesList);
  const loadState = $derived(library.loadState);

  // Local state
  let loading = $state(false);
  let groups = $state<IngestQueueGroup[]>([]);
  let totalGroups = $state(0);
  let offset = $state(0);
  let search = $state('');
  let status = $state('pending');
  let statusCounts = $state<IngestQueueStatusCounts>({
    extracting: 0,
    pending: 0,
    converting: 0,
    approved: 0,
    rejected: 0,
    error: 0
  });
  let devices = $state<DeviceSummary[]>([]);

  const hasMore = $derived(offset + groups.length < totalGroups);

  const loadDevices = async () => {
    try {
      devices = await api.getDevices();
    } catch {
      // Ignore errors loading devices
    }
  };

  // URL state
  const urlStatus = $derived(page.url.searchParams.get('status') || 'pending');

  const urlSearch = $derived(page.url.searchParams.get('q') || '');

  const loadQueue = async (newOffset = 0, append = false) => {
    loading = true;
    try {
      const url = new URL(`${apiBase}/api/v1/admin/ingest-queue`, window.location.origin);
      url.searchParams.set('limit', '50');
      url.searchParams.set('offset', String(newOffset));
      if (search.trim()) {
        url.searchParams.set('search', search.trim());
      }
      if (status && status !== 'all') {
        url.searchParams.set('status', status);
      }

      const resp = await fetch(url.toString(), { credentials: 'include' });
      if (resp.ok) {
        const data = await resp.json();
        if (append) {
          groups = [...groups, ...data.groups];
        } else {
          groups = data.groups;
        }
        totalGroups = data.total_groups;
        offset = newOffset;
        statusCounts = data.status_counts;
      }
    } finally {
      loading = false;
    }
  };

  // Handle scroll-to-top event - reload from beginning of list
  const handleScrollToTop = () => {
    loadQueue(0);
  };

  onMount(async () => {
    // Redirect old ?item= URLs to new dedicated route
    const urlItem = page.url.searchParams.get('item');
    if (urlItem) {
      goto(`/ingest-queue/${urlItem}`, { replace: true });
      return;
    }

    window.addEventListener('scrolltotop', handleScrollToTop);
    status = urlStatus;
    search = urlSearch;

    // Register for ingest queue events from the unified SSE stream
    ingestCallback = on('ingest_queue', () => {
      // Debounce reload — grouping logic is too complex to patch locally
      if (reloadTimeout) clearTimeout(reloadTimeout);
      reloadTimeout = setTimeout(() => {
        loadQueue(offset);
        reloadTimeout = null;
      }, 500);
    });

    // Refresh when tab becomes visible after being hidden
    reconnectHandler = onReconnect(() => {
      loadQueue(offset);
    });

    await Promise.all([loadQueue(), loadSeries(), loadDevices()]);
  });

  onDestroy(() => {
    window.removeEventListener('scrolltotop', handleScrollToTop);
    if (ingestCallback) off('ingest_queue', ingestCallback);
    if (reconnectHandler) offReconnect(reconnectHandler);
    if (reloadTimeout) {
      clearTimeout(reloadTimeout);
    }
  });

  // Handlers
  const handleApprove = async (id: string, deviceIds?: string[]) => {
    loading = true;
    try {
      const resp = await fetch(`${apiBase}/api/v1/admin/ingest-queue/${id}/approve`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'include',
        body: JSON.stringify({ device_ids: deviceIds })
      });
      if (resp.ok) {
        showMessage('Item approved');
        await loadQueue(offset);
      } else {
        showMessage('Approve failed');
      }
    } finally {
      loading = false;
    }
  };

  const handleReject = async (id: string) => {
    loading = true;
    try {
      const resp = await fetch(`${apiBase}/api/v1/admin/ingest-queue/${id}/reject`, {
        method: 'POST',
        credentials: 'include'
      });
      if (resp.ok) {
        showMessage('Item rejected');
        await loadQueue(offset);
      } else {
        showMessage('Reject failed');
      }
    } finally {
      loading = false;
    }
  };

  const handleRestore = async (id: string) => {
    loading = true;
    try {
      await api.restoreIngestQueueItem(id);
      showMessage('Item restored');
      await loadQueue(offset);
    } catch {
      showMessage('Restore failed');
    } finally {
      loading = false;
    }
  };

  const handleDelete = async (id: string) => {
    loading = true;
    try {
      const resp = await fetch(`${apiBase}/api/v1/admin/ingest-queue/${id}`, {
        method: 'DELETE',
        credentials: 'include'
      });
      if (resp.ok) {
        showMessage('Item deleted');
        await loadQueue(offset);
      } else {
        showMessage('Delete failed');
      }
    } finally {
      loading = false;
    }
  };

  const handleExtractMetadata = async (id: string) => {
    loading = true;
    try {
      const resp = await fetch(`${apiBase}/api/v1/admin/ingest-queue/${id}/extract-metadata`, {
        method: 'POST',
        credentials: 'include'
      });
      if (resp.ok) {
        showMessage('Metadata extraction queued');
      } else {
        showMessage('Extract failed');
      }
    } finally {
      loading = false;
    }
  };

  const handleUpdateMetadata = async (id: string, data: BookFormData, coverFile?: File) => {
    loading = true;
    try {
      // Build BookUpdate payload for the library PATCH endpoint
      const payload: Record<string, unknown> = {
        title: data.title,
        authors: data.authors,
        description: data.description ?? undefined,
        language: data.language ?? undefined,
        publisher: data.publisher ?? undefined,
        pubdate: data.pubdate ?? undefined,
        series_name: data.series_name ?? undefined,
        series_id: data.series_id ?? undefined,
        series_hardcover_id: data.series_hardcover_id ?? undefined,
        series_index: data.series_index ?? undefined,
        identifiers: data.identifiers,
        page_count: data.page_count ?? undefined,
        genres: data.genres,
        rating: data.rating ?? undefined,
        cover_url: !coverFile ? (data.cover_url ?? undefined) : undefined
      };

      // PATCH library book (works for both library and ingest items)
      const resp = await fetch(`${apiBase}/api/v1/library/books/${id}`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'include',
        body: JSON.stringify(payload)
      });

      if (!resp.ok) {
        showMessage('Update failed');
        return;
      }

      // Upload cover if a file was selected
      if (coverFile) {
        const formData = new FormData();
        formData.append('cover', coverFile);
        const coverResp = await fetch(`${apiBase}/api/v1/admin/ingest-queue/${id}/cover`, {
          method: 'PUT',
          credentials: 'include',
          body: formData
        });
        if (!coverResp.ok) {
          showMessage('Metadata updated but cover upload failed');
          await loadQueue(offset);
          return;
        }
      }

      showMessage('Metadata updated');
      await loadQueue(offset);
    } finally {
      loading = false;
    }
  };

  const handleBatchReject = async (ids: string[]) => {
    loading = true;
    try {
      for (const id of ids) {
        await fetch(`${apiBase}/api/v1/admin/ingest-queue/${id}/reject`, {
          method: 'POST',
          credentials: 'include'
        });
      }
      showMessage(`${ids.length} items rejected`);
      await loadQueue(offset);
    } finally {
      loading = false;
    }
  };

  const handleUploadBook = async (file: File) => {
    loading = true;
    try {
      const formData = new FormData();
      formData.append('file', file);
      const resp = await fetch(`${apiBase}/api/v1/admin/ingest-queue/upload`, {
        method: 'POST',
        credentials: 'include',
        body: formData
      });
      if (resp.ok) {
        showMessage('Book uploaded');
        await loadQueue();
      } else {
        showMessage('Upload failed');
      }
    } finally {
      loading = false;
    }
  };

  const handleLoadMore = () => {
    if (hasMore && !loading) {
      loadQueue(offset + groups.length, true);
    }
  };

  const handleSearch = (newSearch: string) => {
    search = newSearch;
    updateUrlParams({ q: newSearch || null });
    loadQueue(0);
  };

  const handleStatusFilter = (newStatus: string) => {
    status = newStatus;
    updateUrlParams({ status: newStatus !== 'pending' ? newStatus : null });
    loadQueue(0);
  };
</script>

<svelte:head>
  <title>Ingest Queue — Kobbler</title>
</svelte:head>

{#if !auth.canManageContent}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <IngestQueuePage
    {groups}
    {totalGroups}
    {hasMore}
    {statusCounts}
    {loading}
    loadState={loadState.ingestQueue}
    {seriesList}
    {devices}
    onApprove={handleApprove}
    onReject={handleReject}
    onRestore={handleRestore}
    onDelete={handleDelete}
    onExtractMetadata={handleExtractMetadata}
    onUpdateMetadata={handleUpdateMetadata}
    onBatchReject={handleBatchReject}
    onUploadBook={handleUploadBook}
    onLoadMore={handleLoadMore}
    onSearch={handleSearch}
    onStatusFilter={handleStatusFilter}
  />
{/if}

<style>
  .permission-denied {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 4rem 2rem;
    color: var(--muted);
    text-align: center;
  }

  .permission-denied h2 {
    font-size: 1.25rem;
    margin-bottom: 0.5rem;
    color: var(--ink);
  }
</style>
