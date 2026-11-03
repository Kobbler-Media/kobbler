<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { page } from '$app/state';
  import ProxyLogsPage from '#lib/components/pages/ProxyLogsPage.svelte';
  import { getLibraryState } from '#lib/stores/library.svelte.js';
  import { getDevicesState } from '#lib/stores/devices.svelte.js';
  import { loadProxyLogs, copyText } from '#lib/context/app.svelte.js';
  import { updateUrlParams, getNumParam } from '#lib/utils/url-state.js';
  import type { ProxyLogsQuery } from '#lib/api/client.js';

  // Store state
  const library = getLibraryState();
  const devicesStore = getDevicesState();

  // Derived state
  const loadState = $derived(library.loadState);
  const proxyLogs = $derived(devicesStore.proxyLogs);
  const proxyLogsTotal = $derived(devicesStore.proxyLogsTotal);
  const proxyLogsOffset = $derived(devicesStore.proxyLogsOffset);

  // Local state
  let loading = $state(false);

  // URL state
  const urlOffset = $derived(getNumParam(page.url.searchParams, 'offset', 0));

  const urlPath = $derived(page.url.searchParams.get('path') || '');
  const urlMethod = $derived(page.url.searchParams.get('method') || '');
  const urlSeverity = $derived(page.url.searchParams.get('severity') || '');

  // Handle scroll-to-top event - reload from beginning of list
  const handleScrollToTop = () => {
    loadProxyLogs({
      offset: 0,
      path: urlPath || undefined,
      method: urlMethod || undefined,
      severity: urlSeverity || undefined,
      limit: 50
    });
  };

  onMount(async () => {
    window.addEventListener('scrolltotop', handleScrollToTop);
    // Load with URL params
    await loadProxyLogs({
      offset: urlOffset,
      path: urlPath || undefined,
      method: urlMethod || undefined,
      severity: urlSeverity || undefined,
      limit: 50
    });
  });

  onDestroy(() => {
    window.removeEventListener('scrolltotop', handleScrollToTop);
  });

  // Handler for loading proxy logs (called by the component)
  const handleLoadProxyLogs = async (query: ProxyLogsQuery, _append: boolean) => {
    loading = true;
    try {
      // Update URL params
      updateUrlParams({
        offset: query.offset || null,
        path: query.path || null,
        method: query.method || null,
        severity: query.severity || null
      });
      await loadProxyLogs(query);
    } finally {
      loading = false;
    }
  };

  const handleCopyText = async (value: string, label: string) => {
    await copyText(value, label);
  };
</script>

<svelte:head>
  <title>Proxy Logs — Kobbler</title>
</svelte:head>

<ProxyLogsPage
  {proxyLogs}
  total={proxyLogsTotal}
  offset={proxyLogsOffset}
  {loading}
  loadState={loadState.proxyLogs}
  onLoadProxyLogs={handleLoadProxyLogs}
  onCopyText={handleCopyText}
/>
