<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import ActivityPage from '$lib/components/pages/ActivityPage.svelte';
  import { getLibraryState } from '$lib/stores/library.svelte';
  import { getDevicesState } from '$lib/stores/devices.svelte';
  import { loadActivity, loadMoreActivity } from '$lib/context/app.svelte';
  import { onDebounced, off, onReconnect, offReconnect } from '$lib/services/eventstream.svelte';

  // Store state
  const library = getLibraryState();
  const devicesStore = getDevicesState();

  // Derived state
  const activityEvents = $derived(devicesStore.activityEvents);
  const activityNextCursor = $derived(devicesStore.activityNextCursor);
  const activityTotal = $derived(devicesStore.activityTotal);
  const loadState = $derived(library.loadState);

  // Local state
  let loading = $state(false);
  let activityCallback: ((data: any) => void) | null = null;
  let libraryCallback: ((data: any) => void) | null = null;
  let movieCallback: ((data: any) => void) | null = null;
  let tvCallback: ((data: any) => void) | null = null;
  let reconnectHandler: (() => void) | null = null;

  // Handle scroll-to-top event - reload from beginning of list
  const handleScrollToTop = () => {
    loadActivity();
  };

  onMount(async () => {
    window.addEventListener('scrolltotop', handleScrollToTop);

    // Live-reload activity when new entries are added
    const reloadIfFirstPage = () => {
      if (activityNextCursor) return;
      loadActivity();
    };
    activityCallback = onDebounced('activity_log_updated', reloadIfFirstPage, 300);
    libraryCallback = onDebounced('library_updated', reloadIfFirstPage, 300);
    movieCallback = onDebounced('movie_updated', reloadIfFirstPage, 300);
    tvCallback = onDebounced('tv_show_updated', reloadIfFirstPage, 300);

    // Refresh when tab becomes visible after being hidden
    reconnectHandler = onReconnect(() => {
      if (!activityNextCursor) {
        loadActivity();
      }
    });

    await loadActivity();
  });

  onDestroy(() => {
    window.removeEventListener('scrolltotop', handleScrollToTop);
    if (activityCallback) off('activity_log_updated', activityCallback);
    if (libraryCallback) off('library_updated', libraryCallback);
    if (movieCallback) off('movie_updated', movieCallback);
    if (tvCallback) off('tv_show_updated', tvCallback);
    if (reconnectHandler) offReconnect(reconnectHandler);
  });

  // Handlers
  const handleLoadMore = () => {
    loadMoreActivity();
  };

  const handleViewEntity = (url: string) => {
    goto(url);
  };
</script>

<svelte:head>
  <title>Activity — Kobbler</title>
</svelte:head>

<ActivityPage
  events={activityEvents}
  nextCursor={activityNextCursor}
  {loading}
  loadState={loadState.activity}
  {activityTotal}
  onLoadMore={handleLoadMore}
  onViewEntity={handleViewEntity}
/>
