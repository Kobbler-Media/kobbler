<script lang="ts">
  import type { EnrichedDownload } from '$lib/api/client';
  import * as downloadsStore from '$lib/stores/downloads.svelte';

  type ViewFilter = 'all' | 'active' | 'completed' | 'failed';

  interface Props {
    downloads: EnrichedDownload[];
    loading?: boolean;
    filter?: ViewFilter;
    onFilterChange?: (filter: ViewFilter) => void;
  }

  let { downloads, loading = false, filter = 'all', onFilterChange }: Props = $props();

  // Split downloads into sections
  const allActiveDownloads = $derived(
    downloads.filter(
      (d) =>
        d.status === 'pending' ||
        d.status === 'downloading' ||
        d.status === 'queued' ||
        d.status === 'processing' ||
        d.status === 'searching'
    )
  );

  const allCompletedDownloads = $derived(
    downloads.filter(
      (d) => d.status === 'completed' || d.status === 'imported' || d.status === 'needs_review'
    )
  );

  const allFailedDownloads = $derived(
    downloads.filter((d) => d.status === 'failed' || d.status === 'error')
  );

  // Filtered views based on current filter
  const activeDownloads = $derived(
    filter === 'all' || filter === 'active' ? allActiveDownloads : []
  );

  const completedDownloads = $derived(
    filter === 'all'
      ? allCompletedDownloads.slice(0, 10)
      : filter === 'completed'
        ? allCompletedDownloads
        : []
  );

  const failedDownloads = $derived(
    filter === 'all'
      ? allFailedDownloads.slice(0, 10)
      : filter === 'failed'
        ? allFailedDownloads
        : []
  );

  // Show "View all" only when there are more items than displayed
  const showActiveViewAll = $derived(filter === 'all' && allActiveDownloads.length > 0);
  const showCompletedViewAll = $derived(
    filter === 'all' && allCompletedDownloads.length > completedDownloads.length
  );
  const showFailedViewAll = $derived(
    filter === 'all' && allFailedDownloads.length > failedDownloads.length
  );

  function handleViewAll(section: ViewFilter) {
    onFilterChange?.(section);
  }

  function handleBackToAll() {
    onFilterChange?.('all');
  }

  // Format relative time
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

  // Format file size
  function formatSize(bytes: number | null | undefined): string {
    if (!bytes) return '';
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  }

  // Check if a download can be cancelled
  function canCancel(download: EnrichedDownload): boolean {
    return (
      download.status === 'queued' ||
      download.status === 'downloading' ||
      download.status === 'pending' ||
      download.status === 'searching'
    );
  }

  // Check if a download can be retried
  function canRetry(download: EnrichedDownload): boolean {
    return download.status === 'failed' || download.status === 'error';
  }

  // Check if a download has a callback error (completed but failed to ingest)
  function hasCallbackError(download: EnrichedDownload): boolean {
    return !!download.callback_error && download.status === 'completed';
  }

  // Action handlers
  let actionLoading = $state<string | null>(null);

  async function handleCancel(e: Event, download: EnrichedDownload) {
    e.preventDefault();
    e.stopPropagation();
    if (actionLoading) return;
    actionLoading = download.id;
    try {
      await downloadsStore.cancelDownload(download.id);
    } catch (err) {
      console.error('Failed to cancel download:', err);
    } finally {
      actionLoading = null;
    }
  }

  async function handleRetry(e: Event, download: EnrichedDownload) {
    e.preventDefault();
    e.stopPropagation();
    if (actionLoading) return;
    actionLoading = download.id;
    try {
      await downloadsStore.retryDownload(download.id);
    } catch (err) {
      console.error('Failed to retry download:', err);
    } finally {
      actionLoading = null;
    }
  }

  async function handleRetryCallback(e: Event, download: EnrichedDownload) {
    e.preventDefault();
    e.stopPropagation();
    if (actionLoading) return;
    actionLoading = download.id;
    try {
      await downloadsStore.retryCallback(download.id);
    } catch (err) {
      console.error('Failed to retry callback:', err);
    } finally {
      actionLoading = null;
    }
  }

  function getMediaLink(download: EnrichedDownload): string | null {
    if (!download.media) return null;
    const meta = download.metadata as Record<string, unknown> | undefined;
    const orig = meta?.original_metadata as Record<string, unknown> | undefined;
    const sourceType = meta?.source_type ?? orig?.source_type;
    if (sourceType === 'movie') return `/movies/${download.media.id}`;
    // For both episodes and seasons, media.id is the show ID
    if (sourceType === 'tv_season' || sourceType === 'tv_episode') {
      return `/tv/${download.media.id}`;
    }
    return `/library/books/${download.media.id}`;
  }

  // Get ETA from client_progress
  function getEta(download: EnrichedDownload): string | null {
    const etaTime = download.client_progress?.eta_time;
    if (!etaTime) return null;

    const etaDate = new Date(etaTime);
    const now = new Date();
    const diffMs = etaDate.getTime() - now.getTime();
    if (diffMs <= 0) return null;

    const diffSecs = Math.floor(diffMs / 1000);
    if (diffSecs < 60) return `${diffSecs}s`;
    if (diffSecs < 3600) return `${Math.floor(diffSecs / 60)}m`;
    return `${Math.floor(diffSecs / 3600)}h ${Math.floor((diffSecs % 3600) / 60)}m`;
  }

  // Get size from client_progress or release_payload
  function getSize(download: EnrichedDownload): number | null {
    if (download.client_progress?.total_size) {
      return download.client_progress.total_size;
    }
    const releasePayload = download.release_payload as Record<string, unknown> | null;
    if (releasePayload?.size) return releasePayload.size as number;
    return null;
  }

  // Get download speed
  function getSpeed(download: EnrichedDownload): string | null {
    if (download.client_progress?.download_rate) {
      return formatSize(download.client_progress.download_rate) + '/s';
    }
    return null;
  }
</script>

<div class="downloads-page page-container">
  <header class="page-header">
    <div class="page-title">
      {#if filter !== 'all'}
        <button class="back-btn" type="button" onclick={handleBackToAll}>
          <span class="material-icons-outlined">arrow_back</span>
        </button>
      {/if}
      Downloads
      {#if filter === 'active'}
        <span class="filter-label">- Active</span>
      {:else if filter === 'completed'}
        <span class="filter-label">- Completed</span>
      {:else if filter === 'failed'}
        <span class="filter-label">- Failed</span>
      {/if}
    </div>
  </header>

  {#if loading}
    <div class="empty-state">
      <p class="muted">Loading downloads...</p>
    </div>
  {:else}
    <!-- Active Downloads Section -->
    {#if filter === 'all' || filter === 'active'}
      <section class="section">
        <div class="downloads-section-header">
          <span>Active</span>
          <span class="section-count"
            >({filter === 'active' ? allActiveDownloads.length : activeDownloads.length})</span
          >
          {#if showActiveViewAll}
            <button class="view-all" type="button" onclick={() => handleViewAll('active')}>
              View all &rarr;
            </button>
          {/if}
        </div>

        {#if activeDownloads.length === 0}
          <div class="empty-state small">
            <span class="material-icons-outlined empty-icon">download</span>
            <p class="muted">No active downloads</p>
          </div>
        {:else}
          <div class="download-list">
            {#each activeDownloads as download (download.id)}
              {@const mediaLink = getMediaLink(download)}
              {@const size = getSize(download)}
              {@const eta = getEta(download)}
              {@const speed = getSpeed(download)}
              <div class="download-item">
                <a
                  href={mediaLink || '#'}
                  class="cover"
                  class:placeholder={!download.media?.cover_url}
                >
                  {#if download.media?.cover_url}
                    <img src={download.media.cover_url} alt={download.media.title} />
                  {/if}
                </a>
                <div class="download-info">
                  <a href={mediaLink || '#'} class="download-title">
                    {download.media?.title || download.name}
                  </a>
                  <div class="download-meta">
                    {#if download.media?.subtitle}
                      <span class="subtitle">{download.media.subtitle}</span>
                    {/if}
                    {#if download.media?.author}
                      {#if download.media?.subtitle}
                        <span class="separator">·</span>
                      {/if}
                      <a
                        href={download.media.author_id
                          ? `/authors/${download.media.author_id}`
                          : '#'}
                        class="byline-link"
                      >
                        {download.media.author}
                      </a>
                    {/if}
                    {#if download.media?.series}
                      {#if download.media?.author || download.media?.subtitle}
                        <span class="separator">·</span>
                      {/if}
                      <a
                        href={download.media.series_id
                          ? `/series/${download.media.series_id}`
                          : '#'}
                        class="byline-link"
                      >
                        {download.media
                          .series}{#if download.media.series_index}{` #${download.media.series_index}`}{/if}
                      </a>
                    {/if}
                    {#if download.protocol}
                      <span class="protocol-badge">{download.protocol}</span>
                    {/if}
                  </div>
                  {#if download.status === 'downloading' && download.progress !== null}
                    <div class="progress-track">
                      <div class="progress-fill" style:width="{download.progress}%"></div>
                    </div>
                    <div class="download-status">
                      {Math.round(download.progress)}%
                      {#if size}
                        · {formatSize((download.progress / 100) * size)} of {formatSize(size)}
                      {/if}
                      {#if speed}
                        · {speed}
                      {/if}
                      {#if eta}
                        · ~{eta}
                      {/if}
                    </div>
                  {:else}
                    <div class="download-status pending">
                      <span class="material-icons-outlined spin">sync</span>
                      {download.status === 'searching'
                        ? 'Searching...'
                        : download.status === 'queued'
                          ? 'Queued'
                          : download.status === 'processing'
                            ? 'Processing...'
                            : 'Pending...'}
                    </div>
                  {/if}
                </div>
                <div class="download-actions">
                  {#if canCancel(download)}
                    <button
                      class="icon-btn"
                      title="Cancel"
                      onclick={(e) => handleCancel(e, download)}
                      disabled={actionLoading === download.id}
                    >
                      <span class="material-icons-outlined">close</span>
                    </button>
                  {/if}
                </div>
              </div>
            {/each}
          </div>
        {/if}
      </section>
    {/if}

    <!-- Recently Completed Section -->
    {#if filter === 'all' || filter === 'completed'}
      <section class="section">
        <div class="downloads-section-header">
          <span>{filter === 'completed' ? 'Completed' : 'Recently Completed'}</span>
          <span class="section-count"
            >({filter === 'completed'
              ? allCompletedDownloads.length
              : completedDownloads.length})</span
          >
          {#if showCompletedViewAll}
            <button class="view-all" type="button" onclick={() => handleViewAll('completed')}>
              View all &rarr;
            </button>
          {/if}
        </div>

        {#if (filter === 'completed' ? allCompletedDownloads : completedDownloads).length === 0}
          <div class="empty-state small">
            <p class="muted">No {filter === 'completed' ? 'completed' : 'recent'} downloads</p>
          </div>
        {:else}
          <div class="download-list">
            {#each filter === 'completed' ? allCompletedDownloads : completedDownloads as download (download.id)}
              {@const mediaLink = getMediaLink(download)}
              {@const callbackFailed = hasCallbackError(download)}
              <div class="download-item compact" class:callback-error={callbackFailed}>
                <a
                  href={mediaLink || '#'}
                  class="cover"
                  class:placeholder={!download.media?.cover_url}
                >
                  {#if download.media?.cover_url}
                    <img src={download.media.cover_url} alt={download.media.title} />
                  {/if}
                </a>
                <div class="download-info">
                  <a href={mediaLink || '#'} class="download-title">
                    {download.media?.title || download.name}
                  </a>
                  {#if download.media?.subtitle}
                    <div class="download-meta">
                      <span class="subtitle">{download.media.subtitle}</span>
                    </div>
                  {/if}
                  {#if callbackFailed}
                    <div class="download-status failed">
                      <span class="material-icons-outlined">error_outline</span>
                      Ingest failed: {download.callback_error}
                      · {formatRelativeTime(download.updated_at)}
                    </div>
                  {:else}
                    <div class="download-status complete">
                      <span class="material-icons-outlined"
                        >{download.status === 'needs_review' ? 'rate_review' : 'check'}</span
                      >
                      {download.status === 'needs_review'
                        ? 'Queued for ingest'
                        : 'Added to library'}
                      · {formatRelativeTime(download.updated_at)}
                    </div>
                    {#if download.status === 'needs_review'}
                      <span class="review-note">Sent to ingest queue</span>
                    {/if}
                  {/if}
                </div>
                {#if callbackFailed}
                  <div class="download-actions">
                    <button
                      class="icon-btn"
                      title="Retry ingest"
                      onclick={(e) => handleRetryCallback(e, download)}
                      disabled={actionLoading === download.id}
                    >
                      <span class="material-icons-outlined">refresh</span>
                    </button>
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        {/if}
      </section>
    {/if}

    <!-- Failed Section -->
    {#if (filter === 'all' && failedDownloads.length > 0) || filter === 'failed'}
      <section class="section">
        <div class="downloads-section-header">
          <span>Failed</span>
          <span class="section-count"
            >({filter === 'failed' ? allFailedDownloads.length : failedDownloads.length})</span
          >
          {#if showFailedViewAll}
            <button class="view-all" type="button" onclick={() => handleViewAll('failed')}>
              View all &rarr;
            </button>
          {/if}
        </div>

        {#if (filter === 'failed' ? allFailedDownloads : failedDownloads).length === 0}
          <div class="empty-state small">
            <p class="muted">No failed downloads</p>
          </div>
        {:else}
          <div class="download-list">
            {#each filter === 'failed' ? allFailedDownloads : failedDownloads as download (download.id)}
              {@const mediaLink = getMediaLink(download)}
              <div class="download-item compact failed">
                <a
                  href={mediaLink || '#'}
                  class="cover"
                  class:placeholder={!download.media?.cover_url}
                >
                  {#if download.media?.cover_url}
                    <img src={download.media.cover_url} alt={download.media.title} />
                  {/if}
                </a>
                <div class="download-info">
                  <a href={mediaLink || '#'} class="download-title">
                    {download.media?.title || download.name}
                  </a>
                  {#if download.media?.subtitle}
                    <div class="download-meta">
                      <span class="subtitle">{download.media.subtitle}</span>
                    </div>
                  {/if}
                  <div class="download-status failed">
                    <span class="material-icons-outlined">error_outline</span>
                    {download.error || 'Download failed'}
                    · {formatRelativeTime(download.updated_at)}
                  </div>
                </div>
                <div class="download-actions">
                  {#if canRetry(download)}
                    <button
                      class="icon-btn"
                      title="Retry"
                      onclick={(e) => handleRetry(e, download)}
                      disabled={actionLoading === download.id}
                    >
                      <span class="material-icons-outlined">refresh</span>
                    </button>
                  {/if}
                </div>
              </div>
            {/each}
          </div>
        {/if}
      </section>
    {/if}
  {/if}
</div>

<style>
  /* Page-specific styles only - uses global styles from global.css */
  .downloads-page {
    flex: 1;
    overflow-y: auto;
  }

  .page-header {
    margin-bottom: 24px;
  }

  .section {
    margin-bottom: 32px;
  }

  .downloads-section-header {
    font-size: 14px;
    font-weight: 700;
    margin-bottom: 12px;
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--ink);
  }

  .section-count {
    font-size: 12px;
    color: var(--muted);
    font-weight: 500;
  }

  .view-all {
    font-size: 12px;
    color: var(--accent);
    font-weight: 500;
    margin-left: auto;
    cursor: pointer;
    background: none;
    border: none;
    padding: 0;
  }

  .view-all:hover {
    text-decoration: underline;
  }

  .page-title {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 20px;
    font-weight: 700;
  }

  .back-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 6px;
    color: var(--muted);
    cursor: pointer;
  }

  .back-btn:hover {
    color: var(--ink);
    background: var(--bg-2);
  }

  .back-btn .material-icons-outlined {
    font-size: 16px;
  }

  .filter-label {
    font-weight: 500;
    color: var(--muted);
    font-size: 16px;
  }

  /* Uses global .empty-state, adds local modifier */
  .empty-state.small {
    padding: 24px;
  }

  .empty-icon {
    font-size: 48px;
    color: var(--muted);
    opacity: 0.3;
    margin-bottom: 12px;
  }

  /* Download list and items */
  .download-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .download-item {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    padding: 16px;
    display: flex;
    gap: 16px;
    align-items: center;
  }

  .download-item.compact {
    padding: 12px;
  }

  .download-item.failed,
  .download-item.callback-error {
    background: rgba(239, 68, 68, 0.05);
  }

  .cover {
    width: 50px;
    height: 75px;
    flex-shrink: 0;
    border-radius: 6px;
    overflow: hidden;
    display: block;
  }

  .download-item.compact .cover {
    width: 40px;
    height: 60px;
  }

  .cover img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .cover.placeholder {
    background: var(--bg);
    opacity: 0.5;
  }

  .download-info {
    flex: 1;
    min-width: 0;
  }

  .download-title {
    font-weight: 600;
    margin-bottom: 4px;
    color: var(--ink);
    text-decoration: none;
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .download-title:hover {
    color: var(--accent);
  }

  .download-meta {
    font-size: 12px;
    color: var(--muted);
    margin-bottom: 8px;
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .byline-link {
    color: var(--muted);
    text-decoration: none;
  }

  .byline-link:hover {
    color: var(--accent);
    text-decoration: underline;
  }

  .subtitle {
    color: var(--muted);
  }

  .separator {
    color: var(--muted);
    opacity: 0.5;
  }

  /* Uses global .progress-track and .progress-fill */
  .download-item :global(.progress-track) {
    margin-bottom: 4px;
  }

  .download-status {
    font-size: 12px;
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--muted);
  }

  .download-status .material-icons-outlined {
    font-size: 14px;
  }

  .download-status.complete {
    color: var(--success);
  }

  .download-status.failed {
    color: var(--danger);
  }

  .download-actions {
    display: flex;
    gap: 8px;
    flex-shrink: 0;
  }

  /* Spin animation for pending status */
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

  .review-note {
    font-size: 11px;
    color: var(--muted);
    font-style: italic;
  }

  @media (max-width: 768px) {
    .download-item {
      flex-wrap: wrap;
      padding: 12px;
    }

    .download-actions {
      width: 100%;
      justify-content: flex-end;
    }
  }
</style>
