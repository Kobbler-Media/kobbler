<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { ExpandableSidebar } from '@koblibs/ui';
  import ErrorBanner from '$lib/components/shared/ErrorBanner.svelte';
  import GroupPills from '$lib/components/iptv/GroupPills.svelte';
  import EpgGrid from '$lib/components/iptv/EpgGrid.svelte';
  import RecordButton from '$lib/components/iptv/RecordButton.svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import * as api from '$lib/api/client';
  import type { IptvChannel, IptvEpgEntry, IptvGroup } from '$lib/types';

  let channels = $state<IptvChannel[]>([]);
  let programs = $state<IptvEpgEntry[]>([]);
  let groups = $state<IptvGroup[]>([]);
  let selectedGroup = $state<string | null>(null);
  let loading = $state(true);
  let error = $state('');
  let searchQuery = $state('');
  let searchResults = $state<IptvEpgEntry[]>([]);
  let searching = $state(false);
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let refreshInterval: ReturnType<typeof setInterval> | undefined;

  // Sidebar state
  let sidebarOpen = $state(false);
  let selectedProgram = $state<IptvEpgEntry | null>(null);

  // Time window: 6 hours (1h back, 5h forward)
  const WINDOW_BACK_MS = 60 * 60000;
  const WINDOW_FORWARD_MS = 5 * 60 * 60000;
  const SHIFT_MS = 2 * 60 * 60000;

  let windowStart = $state(new Date(Date.now() - WINDOW_BACK_MS));
  let windowEnd = $state(new Date(Date.now() + WINDOW_FORWARD_MS));

  // Mobile detection
  const isMobile = $derived(
    typeof window !== 'undefined' && window.matchMedia('(max-width: 768px)').matches
  );

  // Current/next programme per channel for mobile view
  const mobileChannelPrograms = $derived.by(() => {
    if (!isMobile) return new Map<string, { current?: IptvEpgEntry; next?: IptvEpgEntry }>();
    const now = Date.now();
    const map = new Map<string, { current?: IptvEpgEntry; next?: IptvEpgEntry }>();
    for (const ch of channels) {
      const chProgs = programs
        .filter((p) => p.channel_id === ch.id)
        .sort((a, b) => new Date(a.start_time).getTime() - new Date(b.start_time).getTime());
      const current = chProgs.find(
        (p) => now >= new Date(p.start_time).getTime() && now < new Date(p.stop_time).getTime()
      );
      const next = chProgs.find((p) => new Date(p.start_time).getTime() > now);
      map.set(ch.id, { current, next });
    }
    return map;
  });

  async function loadData() {
    loading = true;
    error = '';
    try {
      const [channelsRes, groupsRes] = await Promise.all([
        api.getIptvChannels({
          category: selectedGroup ?? undefined,
          limit: 50
        }),
        api.getIptvGroups()
      ]);

      channels = channelsRes.items;
      groups = groupsRes;

      if (channels.length > 0) {
        const channelIds = channels.map((c) => c.id).join(',');
        const gridRes = await api.getIptvEpgGrid({
          from: windowStart.toISOString(),
          to: windowEnd.toISOString(),
          channel_ids: channelIds
        });
        // Flatten grid into a flat array of EPG entries, attaching channel_name
        programs = gridRes.grid.flatMap((g) =>
          g.programs.map((p) => ({ ...p, channel_name: g.channel.name }))
        );
      } else {
        programs = [];
      }
    } catch (e) {
      console.error('Failed to load guide:', e);
      error = 'Failed to load guide data. Please check your connection and try again.';
    } finally {
      loading = false;
    }
  }

  function handleGroupSelect(groupId: string | null) {
    selectedGroup = groupId;
    loadData();
  }

  function handleSearch(query: string) {
    searchQuery = query;
    clearTimeout(searchTimer);
    if (!query.trim()) {
      searchResults = [];
      searching = false;
      return;
    }
    searching = true;
    searchTimer = setTimeout(async () => {
      try {
        searchResults = await api.searchIptvEpg(query.trim());
      } catch {
        showMessage('Search failed');
        searchResults = [];
      } finally {
        searching = false;
      }
    }, 300);
  }

  function shiftTimeWindow(direction: 'back' | 'forward') {
    const shift = direction === 'back' ? -SHIFT_MS : SHIFT_MS;
    windowStart = new Date(windowStart.getTime() + shift);
    windowEnd = new Date(windowEnd.getTime() + shift);
    loadData();
  }

  function jumpToNow() {
    windowStart = new Date(Date.now() - WINDOW_BACK_MS);
    windowEnd = new Date(Date.now() + WINDOW_FORWARD_MS);
    loadData();
  }

  function handleProgramClick(program: IptvEpgEntry) {
    selectedProgram = program;
    sidebarOpen = true;
  }

  function handleCloseSidebar() {
    sidebarOpen = false;
    selectedProgram = null;
  }

  async function handleCreateWatch() {
    if (!selectedProgram) return;
    try {
      await api.createIptvWatch({
        title_pattern: selectedProgram.title,
        match_mode: 'contains'
      });
      showMessage('Watch created');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to create watch');
    }
  }

  function formatProgramTime(dateStr: string): string {
    return new Date(dateStr).toLocaleTimeString(undefined, {
      hour: '2-digit',
      minute: '2-digit'
    });
  }

  function formatTimeRange(start: string, stop: string): string {
    return `${formatProgramTime(start)} - ${formatProgramTime(stop)}`;
  }

  function isLive(program: IptvEpgEntry): boolean {
    const now = Date.now();
    return (
      now >= new Date(program.start_time).getTime() && now < new Date(program.stop_time).getTime()
    );
  }

  onMount(() => {
    loadData();
    // Auto-refresh every 5 minutes
    refreshInterval = setInterval(
      () => {
        loadData();
      },
      5 * 60 * 1000
    );
    return () => clearTimeout(searchTimer);
  });

  onDestroy(() => {
    if (refreshInterval) clearInterval(refreshInterval);
  });
</script>

<svelte:head>
  <title>TV Guide — Kobbler</title>
</svelte:head>

{#if error}
  <div class="error-wrapper">
    <ErrorBanner message={error} onRetry={loadData} onDismiss={() => (error = '')} />
  </div>
{/if}

<div class="guide-page page-container">
  <div class="toolbar">
    <div class="toolbar-left">
      <a href="/iptv" class="back-btn" aria-label="Back to Live TV">
        <span class="material-icons-outlined">arrow_back</span>
      </a>
      <h1>TV Guide</h1>
    </div>
    <div class="toolbar-right">
      <div class="search-box">
        <span class="material-icons-outlined search-icon">search</span>
        <input
          type="text"
          placeholder="Search programmes..."
          value={searchQuery}
          oninput={(e) => handleSearch((e.target as HTMLInputElement).value)}
        />
        {#if searchQuery}
          <button
            type="button"
            class="clear-btn"
            aria-label="Clear search"
            onclick={() => handleSearch('')}
          >
            <span class="material-icons-outlined" style="font-size: 14px;">close</span>
          </button>
        {/if}
      </div>
    </div>
  </div>

  <div class="filter-row">
    <GroupPills {groups} selected={selectedGroup} onselect={handleGroupSelect} />
  </div>

  <!-- Time navigation -->
  <div class="time-nav">
    <button
      type="button"
      class="time-nav-btn"
      aria-label="Earlier"
      onclick={() => shiftTimeWindow('back')}
    >
      <span class="material-icons-outlined">chevron_left</span>
    </button>
    <button type="button" class="time-nav-now" onclick={jumpToNow}>Now</button>
    <span class="time-nav-range">
      {windowStart.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}
      {formatProgramTime(windowStart.toISOString())} -
      {formatProgramTime(windowEnd.toISOString())}
    </span>
    <button
      type="button"
      class="time-nav-btn"
      aria-label="Later"
      onclick={() => shiftTimeWindow('forward')}
    >
      <span class="material-icons-outlined">chevron_right</span>
    </button>
  </div>

  <!-- Search results overlay -->
  {#if searchQuery.trim()}
    <div class="search-results">
      <h3>Search Results</h3>
      {#if searching}
        <div class="search-loading">Searching...</div>
      {:else if searchResults.length === 0}
        <p class="search-empty">No programmes found</p>
      {:else}
        <div class="search-list">
          {#each searchResults as result (result.id)}
            <button
              type="button"
              class="search-result-item"
              onclick={() => handleProgramClick(result)}
            >
              <div class="search-result-info">
                <span class="search-result-title">{result.title}</span>
                <span class="search-result-meta">
                  {result.channel_name} -- {formatTimeRange(result.start_time, result.stop_time)}
                </span>
              </div>
              {#if isLive(result)}
                <span class="live-badge">LIVE</span>
              {/if}
            </button>
          {/each}
        </div>
      {/if}
    </div>
  {:else if loading && channels.length === 0}
    <!-- Skeleton loader -->
    <div class="skeleton-grid">
      <div class="skeleton-channels">
        {#each Array(8) as _, i (i)}
          <div class="skeleton-channel">
            <div class="skeleton-icon"></div>
            <div class="skeleton-text"></div>
          </div>
        {/each}
      </div>
      <div class="skeleton-programs">
        {#each Array(8) as _, i (i)}
          <div class="skeleton-row">
            {#each Array(4) as _, j (j)}
              <div class="skeleton-cell" style="width: {20 + Math.random() * 30}%;"></div>
            {/each}
          </div>
        {/each}
      </div>
    </div>
  {:else if channels.length === 0}
    <div class="empty-state">
      <span class="material-icons-outlined" style="font-size: 40px; opacity: 0.3;">tv_off</span>
      <p>No channels available</p>
      <p class="empty-hint">Add an IPTV provider in settings to get started.</p>
    </div>
  {:else}
    <!-- Desktop: EPG grid -->
    <div class="epg-desktop">
      <EpgGrid
        {channels}
        {programs}
        startTime={windowStart}
        endTime={windowEnd}
        onprogramclick={handleProgramClick}
      />
    </div>

    <!-- Mobile: simplified list -->
    <div class="epg-mobile">
      {#each channels as channel (channel.id)}
        {@const chData = mobileChannelPrograms.get(channel.id)}
        <button
          type="button"
          class="mobile-channel-item"
          onclick={() => {
            if (chData?.current) handleProgramClick(chData.current);
          }}
        >
          <div class="mobile-ch-info">
            {#if channel.icon_url}
              <img src={channel.icon_url} alt="" class="mobile-ch-icon" />
            {/if}
            <span class="mobile-ch-name">{channel.name}</span>
          </div>
          <div class="mobile-ch-programs">
            {#if chData?.current}
              <div class="mobile-current">
                <span class="live-dot"></span>
                <span class="mobile-prog-title">{chData.current.title}</span>
              </div>
            {:else}
              <span class="mobile-no-epg">No EPG</span>
            {/if}
            {#if chData?.next}
              <span class="mobile-next">Next: {chData.next.title}</span>
            {/if}
          </div>
        </button>
      {/each}
    </div>
  {/if}
</div>

<ExpandableSidebar
  open={sidebarOpen}
  title={selectedProgram?.title ?? 'Programme Details'}
  onClose={handleCloseSidebar}
>
  {#if selectedProgram}
    <div class="sidebar-content">
      <div class="sidebar-channel">
        <span class="material-icons-outlined" style="font-size: 16px;">tv</span>
        {selectedProgram.channel_name}
      </div>

      <div class="sidebar-time">
        <span class="material-icons-outlined" style="font-size: 16px;">schedule</span>
        {formatTimeRange(selectedProgram.start_time, selectedProgram.stop_time)}
        {#if isLive(selectedProgram)}
          <span class="live-badge">LIVE</span>
        {/if}
      </div>

      {#if selectedProgram.description}
        <p class="sidebar-desc">{selectedProgram.description}</p>
      {/if}

      {#if selectedProgram.categories.length > 0}
        <div class="sidebar-cats">
          {#each selectedProgram.categories as cat (cat)}
            <span class="cat-pill">{cat}</span>
          {/each}
        </div>
      {/if}

      <div class="sidebar-actions">
        <RecordButton
          channelId={selectedProgram.channel_id}
          title={selectedProgram.title}
          startTime={selectedProgram.start_time}
          endTime={selectedProgram.stop_time}
          epgId={selectedProgram.id}
        />

        <a href="/iptv/{selectedProgram.channel_id}" class="action-btn watch-btn">
          <span class="material-icons-outlined" style="font-size: 16px;">play_arrow</span>
          Watch Now
        </a>

        <button type="button" class="action-btn watch-create-btn" onclick={handleCreateWatch}>
          <span class="material-icons-outlined" style="font-size: 16px;">visibility</span>
          Add Watch
        </button>
      </div>
    </div>
  {/if}
</ExpandableSidebar>

<style>
  .error-wrapper {
    padding: 12px 24px 0;
  }

  .guide-page {
    max-width: 1400px;
    margin: 0 auto;
    padding: 24px;
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
  }

  .toolbar-left h1 {
    font-size: 24px;
    font-weight: 700;
    margin: 0;
  }

  .toolbar-right {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .back-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    text-decoration: none;
    transition: all 0.15s;
  }

  .back-btn:hover {
    border-color: var(--accent);
    color: var(--accent);
  }

  .back-btn :global(.material-icons-outlined) {
    font-size: 18px;
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
    font-size: 16px;
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

  .filter-row {
    margin-bottom: 12px;
  }

  /* Time navigation */
  .time-nav {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 16px;
  }

  .time-nav-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--card);
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
  }

  .time-nav-btn:hover {
    border-color: var(--accent);
    color: var(--ink);
  }

  .time-nav-btn :global(.material-icons-outlined) {
    font-size: 18px;
  }

  .time-nav-now {
    padding: 5px 14px;
    border-radius: 8px;
    border: 1px solid var(--accent);
    background: var(--accent);
    color: var(--bg);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: opacity 0.15s;
  }

  .time-nav-now:hover {
    opacity: 0.9;
  }

  .time-nav-range {
    font-size: 13px;
    color: var(--muted);
    font-weight: 500;
  }

  /* Search results */
  .search-results {
    padding: 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
  }

  .search-results h3 {
    font-size: 14px;
    font-weight: 600;
    margin: 0 0 12px;
  }

  .search-loading,
  .search-empty {
    font-size: 13px;
    color: var(--muted);
    padding: 16px 0;
  }

  .search-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 400px;
    overflow-y: auto;
  }

  .search-result-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 12px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--ink);
    cursor: pointer;
    text-align: left;
    font: inherit;
    transition: background 0.15s;
    width: 100%;
  }

  .search-result-item:hover {
    background: var(--bg-2);
  }

  .search-result-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .search-result-title {
    font-size: 14px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .search-result-meta {
    font-size: 12px;
    color: var(--muted);
  }

  .live-badge {
    font-size: 9px;
    font-weight: 800;
    padding: 2px 6px;
    border-radius: 4px;
    background: var(--accent);
    color: var(--bg);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    flex-shrink: 0;
  }

  /* Skeleton loader */
  .skeleton-grid {
    display: flex;
    border: 1px solid var(--ring);
    border-radius: 12px;
    overflow: hidden;
    background: var(--card);
  }

  .skeleton-channels {
    width: 160px;
    border-right: 1px solid var(--ring);
  }

  .skeleton-channel {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 48px;
    padding: 0 12px;
    border-bottom: 1px solid var(--ring);
  }

  .skeleton-icon {
    width: 24px;
    height: 24px;
    border-radius: 4px;
    background: var(--ring);
    animation: skeleton-pulse 1.5s ease-in-out infinite;
  }

  .skeleton-text {
    height: 12px;
    flex: 1;
    border-radius: 4px;
    background: var(--ring);
    animation: skeleton-pulse 1.5s ease-in-out infinite;
  }

  .skeleton-programs {
    flex: 1;
    overflow: hidden;
  }

  .skeleton-row {
    display: flex;
    gap: 4px;
    height: 48px;
    padding: 6px 4px;
    border-bottom: 1px solid var(--ring);
    align-items: center;
  }

  .skeleton-cell {
    height: 32px;
    border-radius: 6px;
    background: var(--ring);
    animation: skeleton-pulse 1.5s ease-in-out infinite;
    flex-shrink: 0;
  }

  @keyframes skeleton-pulse {
    0%,
    100% {
      opacity: 0.4;
    }
    50% {
      opacity: 0.7;
    }
  }

  /* Empty state */
  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 64px 24px;
    color: var(--muted);
  }

  .empty-hint {
    font-size: 13px;
    margin: 0;
  }

  .empty-state p {
    margin: 0;
  }

  /* Desktop EPG grid */
  .epg-desktop {
    display: block;
  }

  /* Mobile list view */
  .epg-mobile {
    display: none;
  }

  .mobile-channel-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    width: 100%;
    padding: 12px 14px;
    border: none;
    border-bottom: 1px solid var(--ring);
    background: var(--card);
    color: var(--ink);
    cursor: pointer;
    text-align: left;
    font: inherit;
    transition: background 0.15s;
  }

  .mobile-channel-item:first-child {
    border-radius: 10px 10px 0 0;
  }

  .mobile-channel-item:last-child {
    border-bottom: none;
    border-radius: 0 0 10px 10px;
  }

  .mobile-channel-item:only-child {
    border-radius: 10px;
  }

  .mobile-channel-item:hover {
    background: var(--bg-2);
  }

  .mobile-ch-info {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-shrink: 0;
    min-width: 120px;
  }

  .mobile-ch-icon {
    width: 28px;
    height: 28px;
    border-radius: 4px;
    object-fit: contain;
    flex-shrink: 0;
  }

  .mobile-ch-name {
    font-size: 13px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .mobile-ch-programs {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    text-align: right;
    flex: 1;
  }

  .mobile-current {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 6px;
  }

  .live-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
    flex-shrink: 0;
  }

  .mobile-prog-title {
    font-size: 13px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .mobile-next {
    font-size: 11px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .mobile-no-epg {
    font-size: 12px;
    color: var(--muted);
  }

  /* Sidebar content */
  .sidebar-content {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 4px 0;
  }

  .sidebar-channel {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 14px;
    font-weight: 500;
    color: var(--accent);
  }

  .sidebar-time {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--muted);
  }

  .sidebar-desc {
    font-size: 14px;
    line-height: 1.5;
    color: var(--ink);
    margin: 4px 0 0;
  }

  .sidebar-cats {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }

  .cat-pill {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--muted);
  }

  :global([data-theme='light']) .cat-pill {
    background: rgba(0, 0, 0, 0.06);
  }

  .sidebar-actions {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 8px;
    padding-top: 12px;
    border-top: 1px solid var(--ring);
  }

  .action-btn {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
    text-decoration: none;
  }

  .watch-btn {
    background: var(--accent);
    color: var(--bg);
    border: 1px solid var(--accent);
  }

  .watch-btn:hover {
    opacity: 0.9;
  }

  .watch-create-btn {
    background: transparent;
    color: var(--ink);
    border: 1px solid var(--ring);
  }

  .watch-create-btn:hover {
    border-color: var(--accent);
    color: var(--accent);
  }

  @media (max-width: 768px) {
    .guide-page {
      padding: 16px;
    }

    .toolbar {
      flex-wrap: wrap;
      gap: 8px;
    }

    .toolbar-right {
      flex: 1 1 100%;
    }

    .search-box {
      max-width: none;
      flex: 1;
    }

    .time-nav {
      flex-wrap: wrap;
      gap: 6px;
    }

    .epg-desktop {
      display: none;
    }

    .epg-mobile {
      display: block;
      border: 1px solid var(--ring);
      border-radius: 10px;
      overflow: hidden;
    }
  }
</style>
