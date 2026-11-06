<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { getDevicesState } from '#lib/stores/devices.svelte.js';
  import { loadDevices, loadActivity, ensureDevices } from '#lib/context/app.svelte.js';
  import {
    getDashboard,
    getNasHealth,
    getVisionHealth,
    getProxyLogs,
    type ProxyLogEntry
  } from '#lib/api/client.js';
  import type { NasHealthSnapshot, VisionHealthSnapshot } from '#lib/types.js';
  import { createDelayedLoading } from '#lib/utils/delayed-loading.svelte.js';
  import { onDebounced, off, onReconnect, offReconnect } from '#lib/services/eventstream.svelte.js';
  import { watchStateVersion } from '#lib/stores/watchState.svelte.js';
  import type { RecentBook, RecentMovie, RecentTvEntry, ReadNextItem } from '#lib/types.js';
  import { formatDuration, formatEpisodeCode } from '#lib/utils/format.js';
  import { mediaTypeChip } from '#lib/utils/chip-mappings.js';
  import IconChip from '#lib/components/shared/IconChip.svelte';
  import MediaCard from '#lib/components/shared/MediaCard.svelte';
  import ScrollRow from '#lib/components/shared/ScrollRow.svelte';
  import { getFormatFilter } from '#lib/stores/format-filter.svelte.js';

  const devicesStore = getDevicesState();
  const activityEvents = $derived(devicesStore.activityEvents);

  const loadingState = createDelayedLoading(true);
  let recentBooks = $state<RecentBook[]>([]);
  let recentEbooks = $state<RecentBook[]>([]);
  let recentAudiobooks = $state<RecentBook[]>([]);
  // The sidebar's format filter decides which shelf "recently added" is about: an
  // audiobook added to a book shelved years ago is new here and nowhere else.
  const shownBooks = $derived(
    { all: recentBooks, ebook: recentEbooks, audiobook: recentAudiobooks }[getFormatFilter()]
  );
  let recentMovies = $state<RecentMovie[]>([]);
  let recentTv = $state<RecentTvEntry[]>([]);
  let recentSports = $state<RecentTvEntry[]>([]);
  let readNext = $state<ReadNextItem[]>([]);
  let recentProxyLogs = $state<ProxyLogEntry[]>([]);
  let nasHealth = $state<NasHealthSnapshot | null>(null);
  let nasHealthPollTimer: ReturnType<typeof setInterval> | null = null;
  let visionHealth = $state<VisionHealthSnapshot | null>(null);

  let libraryCallback: ((data: any) => void) | null = null;
  let tvCallback: ((data: any) => void) | null = null;
  let movieCallback: ((data: any) => void) | null = null;
  let reconnectHandler: (() => void) | null = null;

  async function refreshDashboard(forceDevices = false) {
    await Promise.all([
      getDashboard().then((res) => {
        recentBooks = res.recent_books;
        recentEbooks = res.recent_ebooks;
        recentAudiobooks = res.recent_audiobooks;
        recentMovies = res.recent_movies;
        recentTv = res.recent_tv;
        recentSports = res.recent_sports;
        readNext = res.read_next ?? [];
      }),
      forceDevices ? loadDevices() : ensureDevices(),
      loadActivity(),
      getProxyLogs({ limit: 5 }).then((res) => {
        recentProxyLogs = res.logs;
      })
    ]);
  }

  async function refreshNasHealth() {
    try {
      const snap = await getNasHealth();
      nasHealth = snap;
      // Stop polling if the feature is disabled server-side — no point hammering
      // the endpoint every minute if the monitor isn't running.
      if (snap.status === 'disabled' && nasHealthPollTimer) {
        clearInterval(nasHealthPollTimer);
        nasHealthPollTimer = null;
      }
    } catch {
      // Swallow — a missing/unauthorized endpoint shouldn't break the dashboard.
      nasHealth = null;
    }
  }

  async function refreshVisionHealth() {
    try {
      visionHealth = await getVisionHealth();
    } catch {
      visionHealth = null;
    }
  }

  onMount(async () => {
    reconnectHandler = onReconnect(() => {
      refreshDashboard(true);
    });

    libraryCallback = onDebounced('library_updated', () => refreshDashboard(), 300);
    tvCallback = onDebounced('tv_show_updated', () => refreshDashboard(), 300);
    movieCallback = onDebounced('movie_updated', () => refreshDashboard(), 300);

    await refreshDashboard();
    loadingState.loading = false;

    await refreshNasHealth();
    if (nasHealth && nasHealth.status !== 'disabled') {
      nasHealthPollTimer = setInterval(refreshNasHealth, 60_000);
    }

    await refreshVisionHealth();
  });

  onDestroy(() => {
    if (libraryCallback) off('library_updated', libraryCallback);
    if (tvCallback) off('tv_show_updated', tvCallback);
    if (movieCallback) off('movie_updated', movieCallback);
    if (reconnectHandler) offReconnect(reconnectHandler);
    if (nasHealthPollTimer) clearInterval(nasHealthPollTimer);
  });

  let lastWatchVersion = -1;
  $effect(() => {
    const v = watchStateVersion();
    if (lastWatchVersion !== -1 && v !== lastWatchVersion) refreshDashboard();
    lastWatchVersion = v;
  });

  const nasBannerVisible = $derived(
    nasHealth !== null && (nasHealth.status === 'degraded' || nasHealth.status === 'unreachable')
  );

  const visionBannerVisible = $derived(
    visionHealth !== null &&
      (visionHealth.status === 'failing' || visionHealth.status === 'degraded')
  );

  const recentActivity = $derived(activityEvents.slice(0, 5));

  function formatRelativeTime(dateStr: string): string {
    const date = new Date(dateStr);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffMins = Math.floor(diffMs / 60000);
    const diffHours = Math.floor(diffMins / 60);
    const diffDays = Math.floor(diffHours / 24);

    if (diffMins < 1) return 'just now';
    if (diffMins < 60) return `${diffMins}m ago`;
    if (diffHours < 24) return `${diffHours}h ago`;
    if (diffDays < 7) return `${diffDays}d ago`;
    return date.toLocaleDateString();
  }

  const EVENT_ICONS: Record<string, string> = {
    imported: 'download_done',
    deleted: 'delete',
    grabbed: 'cloud_download',
    failed: 'error',
    synced: 'sync',
    merged: 'merge_type',
    delivered: 'send'
  };

  function getStatusClass(status: number | null): string {
    if (status === null) return 'status-info';
    if (status >= 200 && status < 300) return 'status-success';
    if (status >= 400 && status < 500) return 'status-warning';
    if (status >= 500) return 'status-error';
    return 'status-info';
  }

  function getMethodClass(method: string): string {
    switch (method) {
      case 'GET':
        return 'method-get';
      case 'POST':
        return 'method-post';
      case 'PUT':
        return 'method-put';
      case 'DELETE':
        return 'method-delete';
      default:
        return 'method-other';
    }
  }

  function progressPercent(item: ReadNextItem): number {
    if (item.progress_percent != null) return Math.min(100, item.progress_percent);
    if (
      item.position_seconds != null &&
      item.duration_seconds != null &&
      item.duration_seconds > 0
    ) {
      return Math.min(100, (item.position_seconds / item.duration_seconds) * 100);
    }
    return 0;
  }

  function progressLabel(item: ReadNextItem): string {
    if (item.position_seconds != null && item.duration_seconds != null) {
      return `${formatDuration(item.position_seconds)} / ${formatDuration(item.duration_seconds)}`;
    }
    if (item.progress_percent != null) {
      return `${Math.round(item.progress_percent)}%`;
    }
    return '';
  }

  function hasProgress(item: ReadNextItem): boolean {
    return item.progress_percent != null || item.position_seconds != null;
  }

  function episodeLabel(entry: RecentTvEntry): string {
    const code = formatEpisodeCode(entry.season_identifier, entry.episode_identifier);
    return entry.episode_title ? `${code} - ${entry.episode_title}` : code;
  }

  function sportLabel(entry: RecentTvEntry): string {
    const eventTitle = entry.episode_title
      ? entry.episode_title.startsWith(entry.show_title)
        ? entry.episode_title.slice(entry.show_title.length).trim()
        : entry.episode_title
      : '';
    return eventTitle ? `${entry.season_identifier} ${eventTitle}` : entry.season_identifier;
  }
</script>

<svelte:head>
  <title>Dashboard — Kobbler</title>
</svelte:head>

<div class="dashboard">
  <div class="dashboard-header">
    <h1>Dashboard</h1>
    <p class="muted">Welcome to Kobbler</p>
  </div>

  {#if nasBannerVisible && nasHealth}
    <div class="nas-banner" class:unreachable={nasHealth.status === 'unreachable'}>
      <span class="material-icons-outlined">warning</span>
      <div class="nas-banner-body">
        {#if nasHealth.status === 'unreachable'}
          <strong>NAS monitoring unreachable.</strong>
          {nasHealth.note ?? 'Cannot read /proc/fs/cifs/Stats from the api container.'}
        {:else}
          <strong>NAS contention detected.</strong>
          {nasHealth.reconnects_last_hour} SMB share reconnect{nasHealth.reconnects_last_hour === 1
            ? ''
            : 's'} in the last hour. Try lowering <em>Max concurrent I/O</em> on affected media roots
          in settings.
        {/if}
      </div>
    </div>
  {/if}

  {#if visionBannerVisible && visionHealth}
    <div class="nas-banner" class:unreachable={visionHealth.status === 'failing'}>
      <span class="material-icons-outlined">warning</span>
      <div class="nas-banner-body">
        {#if visionHealth.status === 'failing'}
          <strong>Visual skip detection is not working.</strong>
        {:else}
          <strong>Visual skip detection is degrading.</strong>
        {/if}
        {visionHealth.note}
        {#if visionHealth.last_error}
          <div class="banner-detail">{visionHealth.last_error}</div>
        {/if}
        <div class="banner-detail">
          <a href="/settings/processing">Check the vision service URL in Processing settings</a>
        </div>
      </div>
    </div>
  {/if}

  {#if loadingState.showLoading}
    <div class="loading-card">Loading...</div>
  {:else if loadingState.loading}
    <!-- Content loading but not shown yet (< 150ms) -->
  {:else}
    <!-- Movies -->
    {#if recentMovies.length > 0}
      <ScrollRow title="Recently Added Movies" viewAllHref="/movies">
        {#each recentMovies as movie (movie.id)}
          <MediaCard posterUrl={movie.poster_url} placeholderIcon="movie" href="/movies/{movie.id}">
            <strong class="card-title">{movie.title}</strong>
            <div class="card-sub">{movie.year ?? ''}</div>
          </MediaCard>
        {/each}
      </ScrollRow>
    {/if}

    <!-- TV -->
    {#if recentTv.length > 0}
      <ScrollRow title="Recently Added TV Shows" viewAllHref="/tv">
        {#each recentTv as entry (entry.episode_id)}
          <MediaCard
            posterUrl={entry.show_poster_url}
            placeholderIcon="tv"
            href="/tv/{entry.show_id}"
          >
            <strong class="card-title">{entry.show_title}</strong>
            <div class="card-sub">{episodeLabel(entry)}</div>
          </MediaCard>
        {/each}
      </ScrollRow>
    {/if}

    <!-- Sports -->
    {#if recentSports.length > 0}
      <ScrollRow title="Recently Added Sports" viewAllHref="/sports">
        {#each recentSports as entry (entry.episode_id)}
          <MediaCard
            posterUrl={entry.show_poster_url}
            placeholderIcon="sports_soccer"
            href="/sports/{entry.show_id}"
          >
            <strong class="card-title">{entry.show_title}</strong>
            <div class="card-sub">{sportLabel(entry)}</div>
          </MediaCard>
        {/each}
      </ScrollRow>
    {/if}

    <!-- What to Read -->
    {#if readNext.length > 0}
      <ScrollRow title="What to Read">
        {#each readNext as item (item.media_id + ':' + item.media_type)}
          <MediaCard
            posterUrl={item.image_url}
            placeholderIcon={item.media_type === 'audiobook' ? 'headphones' : 'auto_stories'}
            href={item.link}
          >
            {#snippet posterOverlay()}
              <span class="chip-pos-left">
                <IconChip chip={mediaTypeChip(item.media_type)} />
              </span>
              {#if hasProgress(item)}
                <div class="progress-track">
                  <div class="progress-fill" style="width: {progressPercent(item)}%"></div>
                </div>
              {/if}
            {/snippet}
            <strong class="card-title">{item.title}</strong>
            <div class="card-sub">{item.subtitle ?? progressLabel(item)}</div>
          </MediaCard>
        {/each}
      </ScrollRow>
    {/if}

    <!-- Books -->
    {#if shownBooks.length > 0}
      <ScrollRow
        title={getFormatFilter() === 'audiobook' ? 'Audiobooks' : 'Books'}
        viewAllHref="/library"
      >
        {#each shownBooks as book (book.id)}
          <MediaCard
            posterUrl={book.cover_url}
            placeholderIcon="auto_stories"
            href="/library/books/{book.id}"
          >
            <strong class="card-title">{book.title}</strong>
            <div class="card-sub">{book.authors || 'Unknown'}</div>
          </MediaCard>
        {/each}
      </ScrollRow>
    {/if}

    <!-- Quick Actions -->
    <div class="quick-actions">
      <div class="section-header">
        <h2>Quick Actions</h2>
      </div>
      <div class="actions-grid">
        <a href="/library" class="action-card">
          <svg viewBox="0 0 24 24" width="20" height="20" fill="currentColor">
            <path
              d="M6 4h12a2 2 0 0 1 2 2v12h-2V6H6V4zm0 4h10a2 2 0 0 1 2 2v10H6a2 2 0 0 1-2-2V10a2 2 0 0 1 2-2zm0 2v8h10v-8H6z"
            ></path>
          </svg>
          <span>Books</span>
        </a>
        <a href="/movies" class="action-card">
          <svg viewBox="0 0 24 24" width="20" height="20" fill="currentColor">
            <path
              d="M18 4l2 4h-3l-2-4h-2l2 4h-3l-2-4H8l2 4H7L5 4H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V4h-4z"
            ></path>
          </svg>
          <span>Movies</span>
        </a>
        <a href="/tv" class="action-card">
          <svg viewBox="0 0 24 24" width="20" height="20" fill="currentColor">
            <path
              d="M21 3H3c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h5v2h8v-2h5c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 14H3V5h18v12z"
            ></path>
          </svg>
          <span>TV Shows</span>
        </a>
        <a href="/sports" class="action-card">
          <svg viewBox="0 0 24 24" width="20" height="20" fill="currentColor">
            <path
              d="M19 5h-2V3H7v2H5c-1.1 0-2 .9-2 2v1c0 2.55 1.92 4.63 4.39 4.94.63 1.5 1.98 2.63 3.61 2.96V19H7v2h10v-2h-4v-3.1c1.63-.33 2.98-1.46 3.61-2.96C19.08 12.63 21 10.55 21 8V7c0-1.1-.9-2-2-2zM5 8V7h2v3.82C5.84 10.4 5 9.3 5 8zm14 0c0 1.3-.84 2.4-2 2.82V7h2v1z"
            ></path>
          </svg>
          <span>Sports</span>
        </a>
      </div>
    </div>

    <!-- Activity Feed -->
    <div class="activity-section">
      <div class="section-header">
        <h2>Recent Activity</h2>
        <a href="/activity" class="view-all">View all</a>
      </div>

      {#if recentActivity.length === 0}
        <div class="activity-empty muted">No recent activity</div>
      {:else}
        <div class="activity-list">
          {#each recentActivity as event (event.id)}
            <div class="activity-item">
              <div class="activity-icon">
                <span class="material-icons-outlined" style="font-size: 14px">
                  {EVENT_ICONS[event.event_kind] ?? 'info'}
                </span>
              </div>
              <div class="activity-content">
                <span class="activity-description">{event.title}</span>
                {#if event.subtitle}
                  <span class="activity-meta muted"> - {event.subtitle}</span>
                {/if}
              </div>
              <div class="activity-time muted">
                {formatRelativeTime(event.created_at)}
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </div>

    <!-- Proxy Logs -->
    <div class="activity-section">
      <div class="section-header">
        <h2>Recent Proxy Logs</h2>
        <a href="/proxy-logs" class="view-all">View all</a>
      </div>

      {#if recentProxyLogs.length === 0}
        <div class="activity-empty muted">No recent proxy logs</div>
      {:else}
        <div class="activity-list">
          {#each recentProxyLogs as log (log.id)}
            <div class="activity-item">
              <span class="method-badge {getMethodClass(log.request_method)}"
                >{log.request_method}</span
              >
              <div class="activity-content">
                <span class="activity-description proxy-path">{log.request_path}</span>
              </div>
              <span class="status-badge {getStatusClass(log.response_status)}"
                >{log.response_status ?? '—'}</span
              >
              <div class="activity-time muted">
                {formatRelativeTime(log.created_at)}
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .dashboard {
    padding-top: 8px;
  }

  .dashboard-header {
    margin-bottom: 24px;
    padding: 0 24px;
  }

  .dashboard-header h1 {
    margin: 0 0 4px;
    font-size: 24px;
  }

  .dashboard-header p {
    margin: 0;
    font-size: 14px;
  }

  .nas-banner {
    margin: 0 24px 20px;
    padding: 12px 16px;
    border-radius: 10px;
    border: 1px solid #e0a800;
    background: rgba(255, 193, 7, 0.12);
    color: var(--ink);
    display: flex;
    align-items: flex-start;
    gap: 12px;
    font-size: 13px;
    line-height: 1.45;
  }

  .nas-banner.unreachable {
    border-color: #c82333;
    background: rgba(220, 53, 69, 0.14);
  }

  .nas-banner .material-icons-outlined {
    font-size: 20px;
    flex-shrink: 0;
    color: #c88500;
  }

  .nas-banner.unreachable .material-icons-outlined {
    color: #c82333;
  }

  .nas-banner-body {
    flex: 1;
  }

  .banner-detail {
    margin-top: 4px;
    font-size: 12px;
    opacity: 0.85;
    word-break: break-word;
  }

  .banner-detail a {
    color: inherit;
  }

  .nas-banner-body strong {
    display: block;
    margin-bottom: 2px;
  }

  /* Quick Actions */
  .quick-actions {
    margin-bottom: 32px;
    padding: 0 24px;
  }

  .quick-actions .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 16px;
  }

  .quick-actions .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .actions-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
    gap: 12px;
  }

  .action-card {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 14px 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    text-decoration: none;
    color: var(--ink);
    font-size: 14px;
    transition:
      border-color 0.15s,
      background 0.15s;
  }

  .action-card:hover {
    border-color: var(--accent);
    background: rgba(211, 140, 90, 0.08);
  }

  :global([data-theme='light']) .action-card:hover {
    background: rgba(200, 122, 70, 0.08);
  }

  .action-card svg {
    color: var(--accent);
    flex-shrink: 0;
  }

  /* Activity (de-emphasized) */
  .activity-section {
    margin-bottom: 24px;
    padding: 0 24px;
    opacity: 0.85;
  }

  .activity-section .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 16px;
  }

  .activity-section .section-header h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 500;
  }

  .view-all {
    font-size: 13px;
    color: var(--accent);
    text-decoration: none;
  }

  .view-all:hover {
    text-decoration: underline;
  }

  .activity-list {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    overflow: hidden;
  }

  .activity-item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    border-bottom: 1px solid var(--ring);
  }

  .activity-item:last-child {
    border-bottom: none;
  }

  .activity-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    background: rgba(255, 255, 255, 0.05);
    border-radius: 6px;
    color: var(--muted);
    flex-shrink: 0;
  }

  :global([data-theme='light']) .activity-icon {
    background: rgba(0, 0, 0, 0.05);
  }

  .activity-content {
    flex: 1;
    min-width: 0;
    font-size: 13px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .activity-description {
    color: var(--ink);
  }

  .activity-meta {
    font-size: 12px;
  }

  .activity-time {
    font-size: 11px;
    flex-shrink: 0;
  }

  .activity-empty {
    font-size: 13px;
    padding: 16px;
    text-align: center;
    background: var(--card);
    border: 1px dashed var(--ring);
    border-radius: 12px;
  }

  /* Proxy log badges */
  .method-badge {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 48px;
    padding: 2px 6px;
    font-size: 10px;
    font-weight: 600;
    border-radius: 4px;
    text-transform: uppercase;
    flex-shrink: 0;
  }

  .method-get {
    background: rgba(59, 130, 246, 0.15);
    color: #60a5fa;
  }

  .method-post {
    background: rgba(34, 197, 94, 0.15);
    color: #4ade80;
  }

  .method-put {
    background: rgba(249, 115, 22, 0.15);
    color: #fb923c;
  }

  .method-delete {
    background: rgba(239, 68, 68, 0.15);
    color: #f87171;
  }

  .method-other {
    background: rgba(148, 163, 184, 0.15);
    color: #94a3b8;
  }

  .status-badge {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 32px;
    padding: 2px 6px;
    font-size: 11px;
    font-weight: 500;
    border-radius: 4px;
    flex-shrink: 0;
  }

  .status-success {
    background: rgba(34, 197, 94, 0.15);
    color: #4ade80;
  }

  .status-warning {
    background: rgba(249, 115, 22, 0.15);
    color: #fb923c;
  }

  .status-error {
    background: rgba(239, 68, 68, 0.15);
    color: #f87171;
  }

  .status-info {
    background: rgba(148, 163, 184, 0.15);
    color: #94a3b8;
  }

  .proxy-path {
    font-family: ui-monospace, monospace;
    font-size: 12px;
  }

  .loading-card {
    padding: 48px;
    margin: 0 24px;
    text-align: center;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
  }

  /* Mobile responsive */
  @media (max-width: 768px) {
    .dashboard-header {
      padding: 0 16px;
    }

    .quick-actions {
      padding: 0 16px;
    }

    .activity-section {
      padding: 0 16px;
    }

    .actions-grid {
      grid-template-columns: 1fr 1fr;
    }

    .action-card {
      padding: 12px;
      font-size: 13px;
    }
  }
</style>
