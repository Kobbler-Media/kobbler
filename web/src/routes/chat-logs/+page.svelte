<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import ChatLogsPage from '$lib/components/pages/ChatLogsPage.svelte';
  import {
    getChatLogs,
    getUsers,
    type ChatLogsQuery,
    type ChatTurnLogEntry
  } from '$lib/api/client';
  import { copyText } from '$lib/context/app.svelte';
  import { updateUrlParams } from '$lib/utils/url-state';
  import type { UserSummary } from '$lib/types';

  const PAGE_SIZE = 50;

  let logs = $state<ChatTurnLogEntry[]>([]);
  let total = $state(0);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let users = $state<UserSummary[]>([]);

  const filters = $derived<ChatLogsQuery>({
    q: $page.url.searchParams.get('q') || undefined,
    transport: $page.url.searchParams.get('transport') || undefined,
    status: ($page.url.searchParams.get('status') as 'ok' | 'failed') || undefined,
    stage: ($page.url.searchParams.get('stage') as 'extract' | 'resolve' | 'action') || undefined,
    user_id: $page.url.searchParams.get('user_id') || undefined,
    since: $page.url.searchParams.get('since') || undefined,
    until: $page.url.searchParams.get('until') || undefined
  });

  const load = async (offset: number, active: ChatLogsQuery = filters) => {
    loading = true;
    error = null;
    try {
      const response = await getChatLogs({ ...active, limit: PAGE_SIZE, offset });
      logs = offset === 0 ? response.logs : [...logs, ...response.logs];
      total = response.total;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load turn logs';
    } finally {
      loading = false;
    }
  };

  onMount(async () => {
    users = await getUsers().catch(() => []);
  });

  $effect(() => {
    const activeFilters = filters;
    load(0, activeFilters);
  });

  const handleChangeFilters = (next: Partial<ChatLogsQuery>) => {
    updateUrlParams(next);
  };

  const handleLoadMore = () => load(logs.length);
</script>

<svelte:head>
  <title>Chat Turn Logs — Kobbler</title>
</svelte:head>

<ChatLogsPage
  {logs}
  {total}
  {loading}
  {error}
  {users}
  {filters}
  onChangeFilters={handleChangeFilters}
  onLoadMore={handleLoadMore}
  onCopyText={copyText}
/>
