<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import DownloadsPage from '#lib/components/pages/DownloadsPage.svelte';
  import { getDownloadsState, fetchDownloads } from '#lib/stores/downloads.svelte.js';
  import { onReconnect, offReconnect } from '#lib/services/eventstream.svelte.js';
  import { getAuthState } from '#lib/stores/auth.svelte.js';

  const auth = getAuthState();

  type ViewFilter = 'all' | 'active' | 'completed' | 'failed';

  // Store state
  const downloadsStore = getDownloadsState();

  // Derived state
  const downloads = $derived(downloadsStore.downloads);
  const loading = $derived(downloadsStore.loading);

  // URL-driven filter state
  const filter = $derived((page.url.searchParams.get('filter') as ViewFilter) || 'all');

  let reconnectHandler: (() => void) | null = null;

  function handleFilterChange(newFilter: ViewFilter) {
    const url = new URL(page.url.href);
    if (newFilter === 'all') {
      url.searchParams.delete('filter');
    } else {
      url.searchParams.set('filter', newFilter);
    }
    goto(url.toString(), { replace: true });
  }

  onMount(() => {
    // Refresh when tab becomes visible after being hidden
    // SSE snapshot doesn't include book enrichment, so we need to re-fetch from API
    reconnectHandler = onReconnect(() => {
      fetchDownloads();
    });

    // Fetch initial downloads from API (with book enrichment)
    // Kobload download events are handled globally by the layout via the unified SSE stream
    fetchDownloads();
  });

  onDestroy(() => {
    if (reconnectHandler) offReconnect(reconnectHandler);
  });
</script>

<svelte:head>
  <title>Downloads — Kobbler</title>
</svelte:head>

{#if !auth.canManageContent}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <DownloadsPage {downloads} {loading} {filter} onFilterChange={handleFilterChange} />
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
