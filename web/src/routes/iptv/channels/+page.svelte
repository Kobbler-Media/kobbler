<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import { onReconnect, offReconnect } from '#lib/services/eventstream.svelte.js';
  import ChannelGrid from '#lib/components/iptv/ChannelGrid.svelte';
  import ChannelDetailSidebar from '#lib/components/iptv/ChannelDetailSidebar.svelte';
  import BidirectionalScroll from '#lib/components/BidirectionalScroll.svelte';
  import { captureScrollAnchor, restoreScrollAnchor } from '#lib/utils/scroll-anchor.js';
  import { ExpandableSidebar } from '@koblibs/ui';
  import ErrorBanner from '#lib/components/shared/ErrorBanner.svelte';
  import { showMessage } from '#lib/context/app.svelte.js';
  import * as api from '#lib/api/client.js';
  import type { IptvChannel, IptvGroup } from '#lib/types.js';
  import {
    pushEntity,
    clearStack,
    getNavigationState,
    updateCurrentEntityData,
    setLoadingEntity
  } from '#lib/stores/navigation.svelte.js';

  let channels = $state<IptvChannel[]>([]);
  let groups = $state<IptvGroup[]>([]);
  let favouriteGroupIds = $state<string[]>([]);
  let loading = $state(true);
  let error = $state('');
  let activeGroupSlug = $state<string | null>(null);
  let searchQuery = $state('');
  let localSearchQuery = $state('');
  let showAllLanguages = $state(false);
  let totalCount = $state(0);
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let loadGeneration = 0;

  const PAGE_SIZE = 100;
  const hasMore = $derived(channels.length < totalCount);

  const nav = getNavigationState();

  let reconnectHandler: (() => void) | null = null;

  async function loadChannels(options?: { append?: boolean; limit?: number }) {
    const gen = ++loadGeneration;
    loading = true;
    try {
      const params: Record<string, string | number | boolean | undefined> = {};
      if (searchQuery.trim()) params.search = searchQuery.trim();
      if (activeGroupSlug) params.category = activeGroupSlug;
      if (showAllLanguages) params.language = 'all';
      params.limit = options?.limit ?? PAGE_SIZE;
      if (options?.append) params.offset = channels.length;

      const result = await api.getIptvChannels(params);
      if (gen !== loadGeneration) return;

      if (options?.append) {
        const existingIds = new Set(channels.map((c) => c.id));
        const uniqueNew = result.items.filter((c) => !existingIds.has(c.id));
        channels = [...channels, ...uniqueNew];
      } else {
        channels = result.items;
      }
      totalCount = result.total;
      error = '';
    } catch (e) {
      if (gen !== loadGeneration) return;
      showMessage('Failed to load channels');
      console.error('Failed to load channels:', e);
      error = 'Failed to load data. Please check your connection and try again.';
    } finally {
      if (gen === loadGeneration) loading = false;
    }
  }

  async function loadGroups() {
    try {
      const [g, favIds] = await Promise.all([api.getIptvGroups(), api.getIptvFavouriteGroups()]);
      groups = g;
      favouriteGroupIds = favIds;
    } catch {
      groups = [];
      favouriteGroupIds = [];
    }
  }

  async function handleGroupFavouriteToggle(groupId: string, isFav: boolean) {
    try {
      if (isFav) {
        await api.addIptvFavouriteGroup(groupId);
        favouriteGroupIds = [...favouriteGroupIds, groupId];
      } else {
        await api.removeIptvFavouriteGroup(groupId);
        favouriteGroupIds = favouriteGroupIds.filter((id) => id !== groupId);
      }
    } catch {
      showMessage('Failed to update group favourite');
    }
  }

  /**
   * Reload without collapsing an appended list back to one page: that shrinks the grid
   * and the browser clamps scrollTop into unrelated channels. Re-request the window at
   * its current size, then pin the first visible card back to the same offset.
   */
  function reloadInPlace() {
    const anchor = captureScrollAnchor();
    loadChannels({ limit: Math.max(channels.length, PAGE_SIZE) }).then(() =>
      restoreScrollAnchor(anchor)
    );
  }

  onMount(() => {
    loadChannels();
    loadGroups();

    reconnectHandler = onReconnect(() => {
      reloadInPlace();
      loadGroups();
    });
  });

  onDestroy(() => {
    if (reconnectHandler) offReconnect(reconnectHandler);
    clearTimeout(searchTimer);
  });

  function handleSelectChannel(channel: IptvChannel) {
    if (window.matchMedia('(max-width: 768px)').matches) {
      goto(`/iptv/${channel.id}`);
    } else {
      pushEntity('iptv_channel', channel.id, channel);
    }
  }

  function handleGroupChange(slug: string | null) {
    activeGroupSlug = slug;
    totalCount = 0;
    loadChannels();
  }

  function handleSearch(query: string) {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      searchQuery = query;
      totalCount = 0;
      loadChannels();
    }, 300);
  }

  function handleLanguageToggle() {
    showAllLanguages = !showAllLanguages;
    totalCount = 0;
    loadChannels();
  }

  function loadMoreChannels() {
    if (!hasMore || loading) return;
    loadChannels({ append: true });
  }

  function handleCloseSidebar() {
    clearStack();
  }

  function handleExpandChannel() {
    const entity = nav.current;
    if (entity?.type === 'iptv_channel') {
      clearStack();
      goto(`/iptv/${entity.id}`);
    }
  }

  function handlePlay() {
    const entity = nav.current;
    if (entity?.type === 'iptv_channel') {
      clearStack();
      goto(`/iptv/${entity.id}`);
    }
  }

  function handlePlaySource(sourceId: string) {
    const entity = nav.current;
    if (entity?.type === 'iptv_channel') {
      clearStack();
      goto(`/iptv/${entity.id}?source=${sourceId}`);
    }
  }

  // Load entity data when nav stack changes
  $effect(() => {
    const entity = nav.current;
    if (!entity) return;
    if (entity.data) return;

    if (entity.type === 'iptv_channel') {
      loadNavChannelData(entity.id);
    }
  });

  async function loadNavChannelData(channelId: string) {
    setLoadingEntity(true);
    try {
      const channel = await api.getIptvChannel(channelId);
      updateCurrentEntityData(channel);
    } catch (e) {
      console.error('Failed to load channel:', e);
    } finally {
      setLoadingEntity(false);
    }
  }

  const selectedChannelId = $derived(nav.current?.type === 'iptv_channel' ? nav.current.id : null);

  const sidebarTitle = $derived.by(() => {
    const entity = nav.current;
    if (!entity) return 'Details';
    if (entity.type === 'iptv_channel' && entity.data) {
      return (entity.data as IptvChannel).name;
    }
    return 'Details';
  });

  const countDisplay = $derived(
    totalCount > 0 ? `${totalCount} channels` : `${channels.length} channels`
  );
</script>

<svelte:head>
  <title>Channels — Kobbler</title>
</svelte:head>

{#if error}
  <div class="error-wrapper">
    <ErrorBanner message={error} onRetry={() => loadChannels()} onDismiss={() => (error = '')} />
  </div>
{/if}

<div class="channels-page page-container">
  <!-- Toolbar -->
  <div class="toolbar">
    <div class="toolbar-left">
      <div class="view-controls">
        <button
          class="view-tab"
          class:active={activeGroupSlug === null}
          type="button"
          onclick={() => handleGroupChange(null)}
        >
          All
        </button>
        {#each groups as group (group.id)}
          <div class="tab-wrapper">
            <button
              class="view-tab"
              class:active={activeGroupSlug === group.slug}
              type="button"
              onclick={() => handleGroupChange(group.slug)}
            >
              {group.name}
              {#if group.channel_count > 0}
                <span class="tab-count">{group.channel_count}</span>
              {/if}
            </button>
            <button
              class="tab-fav"
              class:active={favouriteGroupIds.includes(group.id)}
              type="button"
              onclick={() =>
                handleGroupFavouriteToggle(group.id, !favouriteGroupIds.includes(group.id))}
              title={favouriteGroupIds.includes(group.id)
                ? 'Remove from favourites'
                : 'Add to favourites'}
            >
              <svg
                viewBox="0 0 24 24"
                width="11"
                height="11"
                fill={favouriteGroupIds.includes(group.id) ? 'currentColor' : 'none'}
                stroke="currentColor"
                stroke-width="2"
              >
                <path
                  d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z"
                ></path>
              </svg>
            </button>
          </div>
        {/each}
      </div>
    </div>

    <div class="toolbar-right">
      <button
        class="lang-pill"
        class:active={showAllLanguages}
        type="button"
        onclick={handleLanguageToggle}
      >
        {showAllLanguages ? 'All Languages' : 'English'}
      </button>

      <div class="search-box">
        <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor" class="search-icon">
          <path
            d="M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z"
          ></path>
        </svg>
        <input
          type="text"
          placeholder="Search channels..."
          value={localSearchQuery}
          oninput={(e) => {
            localSearchQuery = (e.target as HTMLInputElement).value;
            handleSearch(localSearchQuery);
          }}
        />
        {#if localSearchQuery}
          <button
            type="button"
            class="clear-btn"
            aria-label="Clear search"
            onclick={() => {
              localSearchQuery = '';
              handleSearch('');
            }}
          >
            <svg viewBox="0 0 24 24" width="12" height="12" fill="currentColor">
              <path
                d="M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"
              ></path>
            </svg>
          </button>
        {/if}
      </div>
    </div>
  </div>

  <!-- Main channel grid -->
  {#if loading && channels.length === 0}
    <div class="loading-card">Loading channels...</div>
  {:else if channels.length === 0}
    <div class="empty-state">
      {#if localSearchQuery.trim()}
        <h3>No results</h3>
        <p class="muted">No channels match your search.</p>
      {:else}
        <h3>No channels found</h3>
        <p class="muted">Configure an IPTV provider in settings to get started.</p>
      {/if}
    </div>
  {:else}
    <div class="section-header">
      <h2>Channels</h2>
      <span class="count">{countDisplay}</span>
    </div>

    <div class="grid-container">
      <BidirectionalScroll
        {hasMore}
        hasPrevious={false}
        {loading}
        onLoadMore={loadMoreChannels}
        onLoadPrevious={() => {}}
      >
        <ChannelGrid {channels} {selectedChannelId} onSelect={handleSelectChannel} />
      </BidirectionalScroll>
    </div>
  {/if}
</div>

<ExpandableSidebar
  open={nav.isOpen}
  title={sidebarTitle}
  onClose={handleCloseSidebar}
  onExpand={nav.current?.type === 'iptv_channel' ? handleExpandChannel : undefined}
>
  {#if nav.current?.type === 'iptv_channel' && nav.current.data}
    <ChannelDetailSidebar
      channel={nav.current.data as IptvChannel}
      onPlay={handlePlay}
      onPlaySource={handlePlaySource}
    />
  {:else if nav.isOpen}
    <div class="loading-sidebar">Loading...</div>
  {/if}
</ExpandableSidebar>

<style>
  .error-wrapper {
    padding: 12px 24px 0;
  }

  .toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
    margin-bottom: 16px;
  }

  .toolbar-left {
    display: flex;
    align-items: center;
    gap: 12px;
    overflow-x: auto;
  }

  .toolbar-right {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .view-controls {
    display: flex;
    gap: 2px;
    background: var(--card);
    padding: 4px;
    border-radius: 8px;
    border: 1px solid var(--ring);
  }

  .tab-wrapper {
    display: flex;
    align-items: center;
    gap: 2px;
  }

  .tab-count {
    font-size: 10px;
    opacity: 0.6;
    margin-left: 2px;
  }

  .tab-fav {
    display: flex;
    align-items: center;
    padding: 2px;
    border: none;
    background: none;
    color: var(--ink-2);
    cursor: pointer;
    opacity: 0.4;
    transition: all 0.15s;
  }

  .tab-fav:hover,
  .tab-fav.active {
    opacity: 1;
    color: #ef4444;
  }

  .view-tab {
    padding: 6px 12px;
    border-radius: 6px;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    color: var(--muted);
    border: none;
    background: none;
    transition: background 0.15s;
    white-space: nowrap;
  }

  .view-tab:hover {
    background: rgba(255, 255, 255, 0.05);
    color: var(--ink);
  }

  .view-tab.active {
    background: var(--bg-2);
    color: var(--ink);
  }

  .lang-pill {
    padding: 6px 12px;
    border-radius: 20px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s;
    white-space: nowrap;
  }

  .lang-pill:hover {
    border-color: var(--accent);
  }

  .lang-pill.active {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--bg);
  }

  .search-box {
    display: flex;
    align-items: center;
    gap: 8px;
    background: var(--bg);
    border: 1px solid var(--ring);
    border-radius: 10px;
    padding: 6px 10px;
    flex: 1;
    max-width: 280px;
  }

  .search-box:focus-within {
    border-color: var(--accent);
  }

  .search-box input {
    border: none;
    background: transparent;
    outline: none;
    font-size: 13px;
    color: var(--ink);
    flex: 1;
    min-width: 0;
  }

  .search-box input::placeholder {
    color: var(--muted);
  }

  .search-icon {
    color: var(--muted);
    flex-shrink: 0;
  }

  .clear-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    color: var(--muted);
    cursor: pointer;
    padding: 2px;
    border-radius: 4px;
  }

  .clear-btn:hover {
    color: var(--ink);
    background: var(--bg-2);
  }

  /* Section header + grid */
  .section-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 16px;
    gap: 16px;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .count {
    color: var(--muted);
    font-size: 13px;
  }

  .grid-container {
    flex: 1;
    min-width: 0;
    padding-bottom: 120px;
  }

  .empty-state {
    border: none;
    opacity: 0.7;
    margin-top: 24px;
  }

  .empty-state h3 {
    margin: 0 0 8px;
  }

  .muted {
    color: var(--muted);
    font-size: 13px;
  }

  .loading-card {
    border: none;
    background: none;
    color: var(--muted);
    padding: 24px;
  }

  .loading-sidebar {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 40px;
    color: var(--muted);
    font-size: 13px;
  }

  @media (max-width: 768px) {
    .toolbar {
      flex-wrap: wrap;
      gap: 8px;
    }

    .toolbar-left {
      flex: 1 1 100%;
    }

    .toolbar-right {
      flex: 1 1 100%;
    }

    .search-box {
      flex: 1;
      max-width: none;
    }
  }
</style>
