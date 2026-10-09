<script lang="ts">
  import type { Book, DeviceSummary, ReadingData, ReadingStatus } from '$lib/types';
  import { getWantStatusOverride } from '$lib/stores/wantStatus.svelte';
  import GenreChips from './GenreChips.svelte';
  import IdentifierLinks from './IdentifierLinks.svelte';
  import BookFilesPanel from './BookFilesPanel.svelte';
  import DualRatingStars from './DualRatingStars.svelte';
  import * as api from '$lib/api/client';
  import { getBookStatusBadge } from '$lib/utils/book-status';
  import ReadingSection from './ReadingSection.svelte';
  import EntityHero from './shared/EntityHero.svelte';
  import { formatSize } from '$lib/utils/format';
  import SelectDevicesModal from './SelectDevicesModal.svelte';

  type Context = 'library' | 'monitored' | 'ingest';

  interface Props {
    book: Book | null;
    context: Context;
    devices?: DeviceSummary[];
    selectedDeviceIds?: string[];
    loading?: boolean;
    embedded?: boolean; // When true, render without outer container styling (for use inside ExpandableSidebar)
    onClose: () => void;
    // Library actions
    onForceResync?: () => void;
    onSendToDevice?: () => void;
    onAddToShelf?: () => void;
    onSearchReleases?: () => void;
    onLinkSource?: () => void;
    // Monitored actions
    onWant?: () => Promise<void>;
    onSkip?: () => Promise<void>;
    onViewInLibrary?: () => void;
    onViewInMonitored?: () => void;
    // Ingest actions
    onApprove?: () => void;
    onReject?: () => void;
    onRestore?: () => void;
    onSearchMetadata?: () => void;
    onDeviceToggle?: (deviceId: string) => void;
    // Device sync (library context)
    onAddDevice?: () => void;
    onRemoveDevice?: (deviceId: string) => void;
    // Navigation
    onAuthorClick?: (authorName: string, authorId: string) => void;
    onSeriesClick?: (seriesId: string) => void;
    onGenreClick?: (genre: string) => void;
    // Refresh callback for file operations
    onRefresh?: () => void;
    // Audiobook chapters
    onEditChapters?: () => void;
  }

  let {
    book,
    context,
    devices = [],
    selectedDeviceIds = [],
    loading = false,
    embedded = false,
    onClose,
    onForceResync,
    onSendToDevice: _onSendToDevice,
    onAddToShelf,
    onSearchReleases,
    onLinkSource,
    onWant,
    onSkip,
    onViewInLibrary,
    onViewInMonitored: _onViewInMonitored,
    onApprove,
    onReject,
    onRestore,
    onSearchMetadata,
    onDeviceToggle,
    onAddDevice: _onAddDevice,
    onRemoveDevice: _onRemoveDevice,
    onAuthorClick,
    onSeriesClick,
    onGenreClick,
    onRefresh,
    onEditChapters
  }: Props = $props();

  let descriptionExpanded = $state(false);
  let wantLoading = $state(false);
  let skipLoading = $state(false);
  let readingData = $state<ReadingData | null>(null);
  let readingDataLoading = $state(false);
  let bookDeviceStatus = $state<api.BookDevicesResponse | null>(null);
  let deviceStatusLoading = $state(false);
  let showDevicePicker = $state(false);
  let kindleDeliveries = $state<api.KindleDelivery[]>([]);
  let kindleSendingTargets = $state<Set<string>>(new Set());

  // Fetch reading data when book changes (library context only)
  $effect(() => {
    if (context === 'library' && book?.id) {
      fetchReadingData(book.id);
    } else {
      readingData = null;
    }
  });

  // Fetch book device status when book changes (library context only)
  $effect(() => {
    if (context === 'library' && book?.id) {
      fetchBookDeviceStatus(book.id);
      fetchKindleDeliveries(book.id);
    } else {
      bookDeviceStatus = null;
      kindleDeliveries = [];
    }
  });

  async function fetchReadingData(bookId: string) {
    readingDataLoading = true;
    try {
      readingData = await api.getReadingData(bookId);
    } catch (e) {
      console.error('Failed to fetch reading data:', e);
      readingData = null;
    } finally {
      readingDataLoading = false;
    }
  }

  async function fetchBookDeviceStatus(bookId: string) {
    deviceStatusLoading = true;
    try {
      bookDeviceStatus = await api.getBookDevices(bookId);
    } catch (e) {
      console.error('Failed to fetch book device status:', e);
      bookDeviceStatus = null;
    } finally {
      deviceStatusLoading = false;
    }
  }

  async function fetchKindleDeliveries(bookId: string) {
    try {
      const resp = await api.getBookKindleDeliveries(bookId);
      kindleDeliveries = resp.deliveries;
    } catch (e) {
      console.error('Failed to fetch Kindle deliveries:', e);
      kindleDeliveries = [];
    }
  }

  function isKindleDelivered(targetId: string): boolean {
    return kindleDeliveries.some((d) => d.target_id === targetId);
  }

  async function handleKindleSend(targetId: string, force: boolean = false) {
    if (!book?.id) return;
    kindleSendingTargets = new Set([...kindleSendingTargets, targetId]);
    try {
      await api.sendToKindle(book.id, targetId, force);
      await fetchKindleDeliveries(book.id);
    } catch (e) {
      console.error('Failed to send to Kindle:', e);
    } finally {
      kindleSendingTargets = new Set([...kindleSendingTargets].filter((id) => id !== targetId));
    }
  }

  async function handleAddDeviceToBook(deviceId: string) {
    if (!book?.id) return;
    deviceStatusLoading = true;
    try {
      await api.addBookDevice(book.id, deviceId);
      await fetchBookDeviceStatus(book.id);
    } catch (e) {
      console.error('Failed to add device to book:', e);
    } finally {
      deviceStatusLoading = false;
    }
  }

  async function handleRemoveDeviceFromBook(deviceId: string) {
    if (!book?.id) return;
    deviceStatusLoading = true;
    try {
      await api.removeBookDevice(book.id, deviceId);
      await fetchBookDeviceStatus(book.id);
    } catch (e) {
      console.error('Failed to remove device from book:', e);
    } finally {
      deviceStatusLoading = false;
    }
  }

  async function handleStatusChange(status: ReadingStatus) {
    if (!book?.id) return;
    try {
      await api.updateReadingData(book.id, { status: status ?? '' });
      await fetchReadingData(book.id);
    } catch (e) {
      console.error('Failed to update reading status:', e);
    }
  }

  async function handleProgressChange(percent: number) {
    if (!book?.id) return;
    try {
      await api.updateReadingData(book.id, { progress_percent: percent });
      await fetchReadingData(book.id);
    } catch (e) {
      console.error('Failed to update progress:', e);
    }
  }

  async function handleRatingChange(rating: number | null) {
    if (!book?.id) return;
    try {
      await api.updateReadingData(book.id, { rating: rating ?? 0 });
      await fetchReadingData(book.id);
    } catch (e) {
      console.error('Failed to update rating:', e);
    }
  }

  async function handleClearProgress() {
    if (!book?.id) return;
    try {
      await api.updateReadingData(book.id, { status: '', progress_percent: 0 });
      await fetchReadingData(book.id);
    } catch (e) {
      console.error('Failed to clear reading progress:', e);
    }
  }

  async function handleLogTime(minutes: number) {
    if (!book?.id) return;
    try {
      await api.logReadingTime(book.id, { minutes });
      await fetchReadingData(book.id);
    } catch (e) {
      console.error('Failed to log reading time:', e);
    }
  }

  // Normalize data from book (works for both library books and ingest items since
  // ingest items are now Book-shaped with an optional ingest_info sidecar)
  const normalizedData = $derived.by(() => {
    if (!book) return null;
    return {
      title: book.title,
      authors: book.authors,
      seriesId: book.series_id,
      seriesName: book.series_name,
      seriesIndex: book.series_index,
      seriesMemberships: book.series_memberships || [],
      pubdate: book.pubdate,
      publisher: book.publisher,
      language: book.language,
      description: book.description,
      identifiers: book.identifiers || {},
      formats: book.formats,
      coverUrl: book.cover_url
        ? `${book.cover_url}?t=${encodeURIComponent(book.updated_at || '')}`
        : null,
      genres: book.genres || [],
      rating: book.rating,
      ratingCount: book.rating_count,
      pageCount: book.page_count,
      isInLibrary: book.is_in_library ?? true,
      wantStatus: getWantStatusOverride(book.id) ?? book.want_status,
      synced: book.synced,
      pendingSync: book.pending_sync,
      shelves: book.shelves || []
    };
  });

  // Strip HTML for truncated preview
  const plainDescription = $derived.by(() => {
    if (!normalizedData?.description) return '';
    const stripped = normalizedData.description
      .replace(/<[^>]*>/g, ' ')
      .replace(/\s+/g, ' ')
      .trim();
    return stripped;
  });

  // Format date
  const formatDate = (dateStr: string | null): string => {
    if (!dateStr) return 'Unknown';
    try {
      const date = new Date(dateStr);
      return date.toLocaleDateString('en-US', { year: 'numeric', month: 'short', day: 'numeric' });
    } catch {
      return dateStr;
    }
  };

  // Whether any devices exist in the system (for hiding sync UI)
  const hasDevices = $derived(bookDeviceStatus !== null && bookDeviceStatus.devices.length > 0);

  // Status badge info
  const statusBadge = $derived.by(() => {
    if (context === 'library' && book) {
      return getBookStatusBadge(book);
    }
    if (context === 'monitored') {
      if (normalizedData?.isInLibrary) return { label: 'OWNED', class: 'synced' };
      if (normalizedData?.wantStatus === 'wanted') return { label: 'WANTED', class: 'wanted' };
      if (normalizedData?.wantStatus === 'searching')
        return { label: 'SEARCHING', class: 'searching' };
      if (normalizedData?.wantStatus === 'downloading')
        return { label: 'DOWNLOADING', class: 'downloading' };
      if (normalizedData?.wantStatus === 'ingesting')
        return { label: 'INGESTING', class: 'downloading' };
      if (normalizedData?.wantStatus === 'watching')
        return { label: 'WATCHING', class: 'watching' };
      if (normalizedData?.wantStatus === 'search_failed')
        return { label: 'FAILED', class: 'failed' };
      if (normalizedData?.wantStatus === 'skipped') return { label: 'SKIPPED', class: 'skipped' };
      return null;
    }
    if (context === 'ingest' && book?.ingest_status) {
      if (book.ingest_status === 'pending') return { label: 'REVIEW', class: 'pending' };
      if (book.ingest_status === 'approved') return { label: 'MATCHED', class: 'synced' };
      if (book.ingest_status === 'extracting') return { label: 'EXTRACTING', class: 'extracting' };
    }
    return null;
  });

  // Check if Hardcover identifier is missing
  const missingHardcover = $derived(normalizedData && !normalizedData.identifiers?.hardcover);

  // Handle want action
  const handleWant = async () => {
    if (!onWant || wantLoading) return;
    wantLoading = true;
    try {
      await onWant();
    } finally {
      wantLoading = false;
    }
  };

  // Handle skip action
  const handleSkip = async () => {
    if (!onSkip || skipLoading) return;
    skipLoading = true;
    try {
      await onSkip();
    } finally {
      skipLoading = false;
    }
  };
</script>

{#if !embedded}
  <button class="panel-backdrop" type="button" aria-label="Close details" onclick={onClose}
  ></button>
{/if}

<aside class="details-panel" class:embedded>
  {#if normalizedData}
    <!-- Unified Hero Section -->
    <EntityHero
      title={normalizedData.title}
      coverUrl={normalizedData.coverUrl}
      {statusBadge}
      rating={context !== 'library' ? normalizedData.rating : undefined}
      ratingCount={context !== 'library' ? normalizedData.ratingCount : undefined}
      embedded={true}
      description={plainDescription}
    >
      {#snippet subtitle()}
        {#if normalizedData.authors.length > 0}
          <div class="hero-author">
            {#each normalizedData.authors.slice(0, 3) as author, i (author.id || i)}
              {#if i > 0},
              {/if}
              {#if onAuthorClick && author.id}
                <button
                  type="button"
                  class="author-link"
                  onclick={() => onAuthorClick?.(author.name, author.id!)}
                >
                  {author.name}
                </button>
              {:else}
                <span class="author-name">{author.name}</span>
              {/if}
            {/each}
            {#if normalizedData.authors.length > 3}
              <span class="muted">+{normalizedData.authors.length - 3}</span>
            {/if}
          </div>
        {/if}
        {#if normalizedData.genres && normalizedData.genres.length > 0}
          <div class="hero-genres">
            <GenreChips
              genres={normalizedData.genres}
              clickable={!!onGenreClick}
              {onGenreClick}
              size="small"
            />
          </div>
        {/if}
      {/snippet}

      {#snippet meta()}
        {#if normalizedData.seriesMemberships.length > 1}
          {#each normalizedData.seriesMemberships as membership (membership.series_id)}
            <div class="hero-series" class:series-secondary={!membership.is_primary}>
              {#if onSeriesClick}
                <button
                  type="button"
                  class="series-link"
                  onclick={() => onSeriesClick?.(membership.series_id)}
                >
                  {membership.series_name}
                  {#if membership.series_index != null}
                    <span class="series-index">#{membership.series_index}</span>
                  {/if}
                </button>
              {:else}
                {membership.series_name}
                {#if membership.series_index != null}
                  <span class="series-index">#{membership.series_index}</span>
                {/if}
              {/if}
            </div>
          {/each}
        {:else if normalizedData.seriesName}
          <div class="hero-series">
            {#if normalizedData.seriesId && onSeriesClick}
              <button
                type="button"
                class="series-link"
                onclick={() => onSeriesClick?.(normalizedData.seriesId!)}
              >
                {normalizedData.seriesName}
                {#if normalizedData.seriesIndex != null}
                  <span class="series-index">#{normalizedData.seriesIndex}</span>
                {/if}
              </button>
            {:else}
              {normalizedData.seriesName}
              {#if normalizedData.seriesIndex != null}
                <span class="series-index">#{normalizedData.seriesIndex}</span>
              {/if}
            {/if}
          </div>
        {/if}
      {/snippet}

      {#snippet ratingContent()}
        {#if context === 'library' && (readingData || normalizedData.rating)}
          <DualRatingStars
            communityRating={readingData?.community_rating ?? normalizedData.rating}
            communityRatingCount={readingData?.community_rating_count ?? normalizedData.ratingCount}
            userRating={readingData?.rating ?? null}
            size="small"
            editable={true}
            onRatingChange={handleRatingChange}
          />
        {/if}
      {/snippet}
    </EntityHero>

    <div class="panel-body">
      <!-- Description -->
      {#if plainDescription}
        <div class="section">
          <h4 class="section-title">Description</h4>
          <div class="description-box">
            <p class="description-text" class:expanded={descriptionExpanded}>
              {plainDescription}
            </p>
            <button
              type="button"
              class="expand-link"
              onclick={() => (descriptionExpanded = !descriptionExpanded)}
            >
              {descriptionExpanded ? 'Show less' : 'Show more'}
            </button>
          </div>
        </div>
      {/if}

      <!-- Meta grid -->
      <div class="meta-grid">
        <div>
          <div class="label">Publisher</div>
          <div class="value">{normalizedData.publisher || 'Unknown'}</div>
        </div>
        <div>
          <div class="label">Published</div>
          <div class="value">{formatDate(normalizedData.pubdate)}</div>
        </div>
        {#if context !== 'ingest' || normalizedData.pageCount}
          <div>
            <div class="label">Pages</div>
            <div class="value">
              {normalizedData.pageCount ? normalizedData.pageCount.toLocaleString() : 'Unknown'}
            </div>
          </div>
        {/if}
        {#if context !== 'ingest' || normalizedData.language}
          <div>
            <div class="label">Language</div>
            <div class="value">{normalizedData.language || 'Unknown'}</div>
          </div>
        {/if}
        {#if book?.narrator}
          <div>
            <div class="label">Narrator</div>
            <div class="value">{book.narrator}</div>
          </div>
        {/if}
        {#if book?.duration_seconds}
          <div>
            <div class="label">Duration</div>
            <div class="value">
              {Math.floor(book.duration_seconds / 3600)}h {String(
                Math.floor((book.duration_seconds % 3600) / 60)
              ).padStart(2, '0')}m
            </div>
          </div>
        {/if}
      </div>
      {#if book?.is_abridged || book?.has_audiobook}
        <div class="audiobook-badges">
          {#if book.is_abridged}
            <span class="audiobook-badge abridged">Abridged</span>
          {/if}
          {#if book.has_audiobook}
            <a href="/library/books/{book.id}/listen" class="btn-listen">
              <span class="material-icons-outlined">headphones</span>
              Listen
            </a>
            {#if onEditChapters}
              <button type="button" class="btn-listen" onclick={onEditChapters}>
                <span class="material-icons-outlined">queue_music</span>
                Chapters
              </button>
            {/if}
          {/if}
        </div>
      {/if}

      <!-- Identifiers -->
      {#if Object.keys(normalizedData.identifiers).length > 0 || missingHardcover}
        <div class="section">
          <h4 class="section-title">Identifiers</h4>
          <div class="identifiers-row">
            <IdentifierLinks identifiers={normalizedData.identifiers} />
            {#if missingHardcover}
              {#if onLinkSource}
                <button
                  type="button"
                  class="warning-chip clickable"
                  onclick={onLinkSource}
                  title="Link to Hardcover"
                >
                  HC: ?
                </button>
              {:else}
                <span class="warning-chip">HC: ?</span>
              {/if}
            {/if}
          </div>
        </div>
      {/if}

      <!-- Shelves -->
      {#if context === 'library'}
        <div class="section">
          <div class="label-row">
            <h4 class="section-title">Shelves</h4>
            {#if onAddToShelf}
              <button type="button" class="add-btn" onclick={onAddToShelf}>
                <span class="material-icons-outlined" style="font-size: 14px;">add</span>
              </button>
            {/if}
          </div>
          <div class="shelf-chips">
            {#if normalizedData.shelves && normalizedData.shelves.length > 0}
              {#each normalizedData.shelves as shelf, i (shelf.id || shelf.name + ':' + i)}
                <span class="shelf-chip">{shelf.name}</span>
              {/each}
            {:else}
              <span class="shelf-empty">No shelves</span>
            {/if}
          </div>
        </div>
      {/if}

      <!-- Files panel for library books -->
      {#if context === 'library' && book}
        <BookFilesPanel bookId={book.id} onFileDeleted={onRefresh} {onSearchReleases} />
      {/if}

      <!-- Ingest file info -->
      {#if context === 'ingest' && book?.ingest_info}
        <div class="section">
          <h4 class="section-title">Source File</h4>
          <div class="file-box">
            <div class="file-box-main">
              <div class="file-box-name">{book.ingest_info.file_name}</div>
              <div class="file-box-size">{formatSize(book.ingest_info.file_size, 'Unknown')}</div>
            </div>
          </div>
        </div>

        <!-- Ingest To (device selection for approval) -->
        {#if devices.length > 0}
          {@const selectedDevices = devices.filter((d) => selectedDeviceIds.includes(d.id))}
          <div class="section">
            <div class="section-title-row">
              <h4 class="section-title">Ingest To</h4>
              <div class="section-actions">
                <button
                  type="button"
                  class="icon-btn-small"
                  title="Select devices"
                  onclick={() => (showDevicePicker = true)}
                >
                  <span class="material-icons-outlined">devices</span>
                </button>
              </div>
            </div>
            {#if selectedDevices.length === 0}
              <div class="empty-devices">No devices selected</div>
            {:else}
              <div class="device-list">
                {#each selectedDevices as device (device.id)}
                  <div class="device-row">
                    <span class="device-name">{device.name || 'Unknown device'}</span>
                    <div class="device-row-right">
                      <button
                        type="button"
                        class="device-remove-btn"
                        onclick={() => onDeviceToggle?.(device.id)}
                        aria-label="Remove {device.name}"
                        title="Remove"
                      >
                        <span class="material-icons-outlined">close</span>
                      </button>
                    </div>
                  </div>
                {/each}
              </div>
            {/if}
          </div>
        {/if}
      {/if}

      <!-- Reading section (library context only) -->
      {#if context === 'library' && !readingDataLoading}
        <ReadingSection
          status={readingData?.status ?? null}
          progressPercent={readingData?.progress_percent ?? null}
          pageCount={readingData?.page_count ?? normalizedData.pageCount}
          spentReadingMinutes={readingData?.spent_reading_minutes ?? null}
          editable={true}
          onStatusChange={handleStatusChange}
          onProgressChange={handleProgressChange}
          onLogTime={handleLogTime}
          onClear={handleClearProgress}
        />
      {/if}

      <!-- Sync Status per device (hidden when no devices exist) -->
      {#if context === 'library' && bookDeviceStatus && hasDevices}
        {@const entitledDevices = bookDeviceStatus.devices.filter(
          (d: any) => d.entitled && !d.is_removed
        )}
        {@const koboDevices = bookDeviceStatus.devices.filter((d: any) => d.device_type === 'kobo')}
        {@const kindleDevices = bookDeviceStatus.devices.filter(
          (d: any) => d.device_type === 'kindle'
        )}
        <div class="section">
          <div class="section-title-row">
            <h4 class="section-title">Device Status</h4>
            <div class="section-actions">
              {#if onForceResync && koboDevices.length > 0}
                <button
                  type="button"
                  class="icon-btn-small"
                  title="Sync now"
                  onclick={onForceResync}
                  disabled={loading}
                >
                  <span class="material-icons-outlined">sync</span>
                </button>
              {/if}
              <button
                type="button"
                class="icon-btn-small"
                title="Select devices"
                onclick={() => (showDevicePicker = true)}
                disabled={deviceStatusLoading}
              >
                <span class="material-icons-outlined">devices</span>
              </button>
            </div>
          </div>

          {#if bookDeviceStatus.all_users_entitled && entitledDevices.length === 0}
            <!-- ALL_USERS scope: show all devices -->
            <div class="device-list">
              {#each bookDeviceStatus.devices as device (device.device_id)}
                {@const isKindle = device.device_type === 'kindle'}
                {@const delivered = isKindle && isKindleDelivered(device.device_id)}
                {@const sending = kindleSendingTargets.has(device.device_id)}
                <div class="device-row">
                  <span class="device-name">{device.device_name || 'Unnamed device'}</span>
                  <div class="device-row-right">
                    {#if isKindle}
                      {#if sending}
                        <span class="kindle-status sending">Sending...</span>
                      {:else if delivered}
                        <div class="sync-status">
                          <div class="sync-dot synced"></div>
                          <span>Sent</span>
                        </div>
                        <button
                          type="button"
                          class="kindle-send-btn"
                          onclick={() => handleKindleSend(device.device_id, true)}
                          title="Re-send to Kindle"
                        >
                          <span class="material-icons-outlined">refresh</span>
                        </button>
                      {:else}
                        <button
                          type="button"
                          class="kindle-send-btn"
                          onclick={() => handleKindleSend(device.device_id)}
                          title="Send to Kindle"
                        >
                          <span class="material-icons-outlined">send</span>
                        </button>
                      {/if}
                    {:else}
                      <div class="sync-status">
                        <div
                          class="sync-dot"
                          class:synced={normalizedData?.synced && !normalizedData?.pendingSync}
                          class:pending={!normalizedData?.synced || normalizedData?.pendingSync}
                        ></div>
                        <span
                          >{normalizedData?.synced && !normalizedData?.pendingSync
                            ? 'Synced'
                            : 'Pending'}</span
                        >
                      </div>
                    {/if}
                    <button
                      type="button"
                      class="device-remove-btn"
                      onclick={() => handleRemoveDeviceFromBook(device.device_id)}
                      aria-label="Remove {device.device_name}"
                      title="Remove"
                      disabled={deviceStatusLoading}
                    >
                      <span class="material-icons-outlined">close</span>
                    </button>
                  </div>
                </div>
              {/each}
            </div>
          {:else if entitledDevices.length === 0 && kindleDevices.length === 0}
            <div class="empty-devices">No devices syncing this book</div>
          {:else}
            <div class="device-list">
              {#each entitledDevices as device (device.device_id)}
                {@const isKindle = device.device_type === 'kindle'}
                {@const delivered = isKindle && isKindleDelivered(device.device_id)}
                {@const sending = kindleSendingTargets.has(device.device_id)}
                <div class="device-row">
                  <span class="device-name">{device.device_name || 'Unnamed device'}</span>
                  <div class="device-row-right">
                    {#if isKindle}
                      {#if sending}
                        <span class="kindle-status sending">Sending...</span>
                      {:else if delivered}
                        <div class="sync-status">
                          <div class="sync-dot synced"></div>
                          <span>Sent</span>
                        </div>
                        <button
                          type="button"
                          class="kindle-send-btn"
                          onclick={() => handleKindleSend(device.device_id, true)}
                          title="Re-send to Kindle"
                        >
                          <span class="material-icons-outlined">refresh</span>
                        </button>
                      {:else}
                        <button
                          type="button"
                          class="kindle-send-btn"
                          onclick={() => handleKindleSend(device.device_id)}
                          title="Send to Kindle"
                        >
                          <span class="material-icons-outlined">send</span>
                        </button>
                      {/if}
                    {:else}
                      <div class="sync-status">
                        <div
                          class="sync-dot"
                          class:synced={normalizedData.synced && !normalizedData.pendingSync}
                          class:pending={!normalizedData.synced || normalizedData.pendingSync}
                        ></div>
                        <span
                          >{normalizedData.synced && !normalizedData.pendingSync
                            ? 'Synced'
                            : 'Pending'}</span
                        >
                      </div>
                    {/if}
                    <button
                      type="button"
                      class="device-remove-btn"
                      onclick={() => handleRemoveDeviceFromBook(device.device_id)}
                      aria-label="Remove {device.device_name}"
                      title="Remove"
                      disabled={deviceStatusLoading}
                    >
                      <span class="material-icons-outlined">close</span>
                    </button>
                  </div>
                </div>
              {/each}
              <!-- Show un-entitled Kindle devices with send buttons -->
              {#each kindleDevices.filter((d: any) => !d.entitled || d.is_removed) as device (device.device_id)}
                {@const delivered = isKindleDelivered(device.device_id)}
                {@const sending = kindleSendingTargets.has(device.device_id)}
                <div class="device-row not-entitled">
                  <span class="device-name">{device.device_name || 'Unnamed device'}</span>
                  <div class="device-row-right">
                    {#if sending}
                      <span class="kindle-status sending">Sending...</span>
                    {:else if delivered}
                      <div class="sync-status">
                        <div class="sync-dot synced"></div>
                        <span>Sent</span>
                      </div>
                      <button
                        type="button"
                        class="kindle-send-btn"
                        onclick={() => handleKindleSend(device.device_id, true)}
                        title="Re-send to Kindle"
                      >
                        <span class="material-icons-outlined">refresh</span>
                      </button>
                    {:else}
                      <button
                        type="button"
                        class="kindle-send-btn"
                        onclick={() => handleKindleSend(device.device_id)}
                        title="Send to Kindle"
                      >
                        <span class="material-icons-outlined">send</span>
                      </button>
                    {/if}
                  </div>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      {/if}

      <!-- Actions (only when not embedded - embedded uses ExpandableSidebar footer) -->
      <!-- Show for monitored/ingest contexts, OR for library context with non-owned books -->
      {#if !embedded && (context === 'monitored' || context === 'ingest' || (context === 'library' && !normalizedData.isInLibrary))}
        <div class="actions-section">
          {#if context === 'monitored' || (context === 'library' && !normalizedData.isInLibrary)}
            {#if normalizedData.isInLibrary}
              {#if onViewInLibrary}
                <button
                  class="btn-primary"
                  type="button"
                  onclick={onViewInLibrary}
                  disabled={loading}
                >
                  View in Library
                </button>
              {/if}
            {:else if normalizedData.wantStatus && ['wanted', 'searching', 'downloading', 'ingesting', 'watching', 'search_failed'].includes(normalizedData.wantStatus)}
              <div class="action-info">
                {#if normalizedData.wantStatus === 'wanted'}
                  <span class="info-badge wanted">On wanted list</span>
                {:else if normalizedData.wantStatus === 'searching'}
                  <span class="info-badge searching">Searching...</span>
                {:else if normalizedData.wantStatus === 'downloading'}
                  <span class="info-badge downloading">Downloading</span>
                {:else if normalizedData.wantStatus === 'ingesting'}
                  <span class="info-badge downloading">Ingesting</span>
                {:else if normalizedData.wantStatus === 'watching'}
                  <span class="info-badge watching">Watching</span>
                {:else if normalizedData.wantStatus === 'search_failed'}
                  <span class="info-badge failed">Search failed</span>
                {/if}
              </div>
              {#if onSearchReleases}
                <button
                  class="btn-primary"
                  type="button"
                  onclick={onSearchReleases}
                  disabled={loading}
                >
                  Search
                </button>
              {/if}
              {#if onSkip}
                <button
                  class="btn-sec"
                  type="button"
                  onclick={handleSkip}
                  disabled={loading || skipLoading}
                >
                  {skipLoading ? 'Removing...' : 'Remove'}
                </button>
              {/if}
            {:else if normalizedData.wantStatus === 'skipped'}
              <div class="action-info">
                <span class="info-badge skipped">Skipped</span>
              </div>
              {#if onWant}
                <button
                  class="btn-primary"
                  type="button"
                  onclick={handleWant}
                  disabled={loading || wantLoading}
                >
                  {wantLoading ? 'Adding...' : 'Want Instead'}
                </button>
              {/if}
            {:else}
              {#if onWant}
                <button
                  class="btn-primary"
                  type="button"
                  onclick={handleWant}
                  disabled={loading || wantLoading}
                >
                  {wantLoading ? 'Adding...' : 'Want This Book'}
                </button>
              {/if}
              {#if onSkip}
                <button
                  class="btn-sec"
                  type="button"
                  onclick={handleSkip}
                  disabled={loading || skipLoading}
                >
                  {skipLoading ? 'Skipping...' : 'Skip'}
                </button>
              {/if}
            {/if}
          {:else if context === 'ingest'}
            <div class="ingest-actions">
              {#if onRestore}
                <button class="btn-sec" type="button" onclick={onRestore} disabled={loading}>
                  Restore
                </button>
              {:else if onReject}
                <button
                  class="btn-danger-outline"
                  type="button"
                  onclick={onReject}
                  disabled={loading}
                >
                  Reject
                </button>
              {/if}
              {#if onApprove}
                <button class="btn-primary" type="button" onclick={onApprove} disabled={loading}>
                  Approve
                </button>
              {/if}
            </div>
            {#if onSearchMetadata}
              <button
                class="btn-sec full-width"
                type="button"
                onclick={onSearchMetadata}
                disabled={loading}
              >
                <span class="material-icons-outlined" style="font-size: 14px;">search</span>
                Search Metadata
              </button>
            {/if}
          {/if}
        </div>
      {/if}
    </div>
  {:else}
    <div class="panel-header">
      <button class="icon-btn" type="button" aria-label="Close" onclick={onClose}>
        <span class="material-icons-outlined">close</span>
      </button>
    </div>
    <div class="empty-state">
      <p class="muted">Select a book to view details.</p>
    </div>
  {/if}
</aside>

{#if showDevicePicker}
  {#if context === 'ingest'}
    <SelectDevicesModal
      devices={devices.map((d) => ({
        id: d.id,
        name: d.name || 'Unknown device',
        selected: selectedDeviceIds.includes(d.id)
      }))}
      onToggle={(deviceId) => onDeviceToggle?.(deviceId)}
      onClose={() => (showDevicePicker = false)}
    />
  {:else if bookDeviceStatus}
    <SelectDevicesModal
      devices={bookDeviceStatus.devices.map((d) => ({
        id: d.device_id,
        name: d.device_name || 'Unnamed device',
        selected: d.entitled && !d.is_removed
      }))}
      disabled={deviceStatusLoading}
      onToggle={(deviceId) => {
        const device = bookDeviceStatus?.devices.find((d) => d.device_id === deviceId);
        if (!device) return;
        if (device.entitled && !device.is_removed) {
          handleRemoveDeviceFromBook(deviceId);
        } else {
          handleAddDeviceToBook(deviceId);
        }
      }}
      onClose={() => (showDevicePicker = false)}
    />
  {/if}
{/if}

<style>
  .panel-backdrop {
    display: none;
  }

  .details-panel {
    position: sticky;
    top: 88px;
    width: 380px;
    max-height: calc(100vh - 100px);
    overflow-y: auto;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: var(--radius, 18px);
    padding: 16px;
    box-shadow: var(--shadow, 0 18px 40px rgba(5, 7, 10, 0.5));
    z-index: 6;
  }

  /* When embedded in ExpandableSidebar, remove outer container styling */
  .details-panel.embedded {
    position: static;
    width: auto;
    max-height: none;
    overflow-y: visible;
    background: transparent;
    border: none;
    border-radius: 0;
    padding: 0;
    box-shadow: none;
    display: flex;
    flex-direction: column;
    min-height: 100%;
  }

  .panel-header {
    display: flex;
    justify-content: flex-end;
    margin-bottom: 12px;
  }

  .icon-btn {
    flex-shrink: 0;
    border-radius: 8px;
    padding: 6px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transition: all 0.15s ease;
  }

  .icon-btn:hover {
    background: var(--ring);
    color: var(--ink);
  }

  .author-link,
  .series-link {
    background: none;
    border: none;
    color: var(--accent);
    cursor: pointer;
    font: inherit;
    padding: 0;
  }

  .author-link:hover,
  .series-link:hover {
    text-decoration: underline;
  }

  .author-name {
    color: var(--ink);
  }

  .series-index {
    margin-left: 4px;
  }

  .series-secondary {
    opacity: 0.7;
    font-size: 0.9em;
  }

  .hero-genres {
    margin-top: 6px;
  }

  /* Sections */
  .section {
    padding: 16px 0;
  }

  .section-title {
    margin: 0 0 10px;
    font-size: 12px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--muted);
  }

  .description-text {
    font-size: 13px;
    color: var(--ink);
    line-height: 1.6;
    margin: 0;
    display: -webkit-box;
    -webkit-line-clamp: 4;
    line-clamp: 4;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .description-text.expanded {
    display: block;
    -webkit-line-clamp: unset;
    line-clamp: unset;
    overflow: visible;
  }

  .expand-link {
    background: none;
    border: none;
    color: var(--accent);
    cursor: pointer;
    font-size: 12px;
    padding: 4px 0 0;
    font-weight: 600;
  }

  .expand-link:hover {
    text-decoration: underline;
  }

  .label {
    font-size: 10px;
    text-transform: uppercase;
    color: var(--muted);
    font-weight: 700;
    letter-spacing: 0.5px;
    margin-bottom: 6px;
  }

  .label-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 6px;
  }

  .label-row .section-title {
    margin-bottom: 0;
  }

  .section-title-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
  }

  .section-title-row .section-title {
    margin-bottom: 0;
  }

  .section-actions {
    display: flex;
    gap: 4px;
  }

  .icon-btn-small {
    width: 28px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
  }

  .icon-btn-small:hover:not(:disabled) {
    background: var(--bg);
    color: var(--ink);
  }

  .icon-btn-small:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .icon-btn-small .material-icons-outlined {
    font-size: 16px;
  }

  .value {
    font-size: 13px;
    color: var(--ink);
    line-height: 1.5;
  }

  .muted {
    color: var(--muted);
  }

  /* Meta grid */
  .meta-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px 20px;
    margin-bottom: 16px;
  }

  .audiobook-badges {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 16px;
    flex-wrap: wrap;
  }

  .audiobook-badge {
    font-size: 10px;
    font-weight: 800;
    padding: 3px 10px;
    border-radius: 6px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .audiobook-badge.abridged {
    background: var(--accent-dim);
    color: var(--accent);
  }

  .btn-listen {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    font-weight: 700;
    padding: 5px 14px;
    border-radius: 8px;
    background: var(--accent);
    color: #000;
    text-decoration: none;
    transition: opacity 0.15s ease;
  }

  .btn-listen:hover {
    opacity: 0.85;
  }

  .btn-listen .material-icons-outlined {
    font-size: 16px;
  }

  /* Identifiers */
  .identifiers-row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
  }

  .warning-chip {
    font-size: 10px;
    padding: 2px 6px;
    border-radius: 4px;
    background: rgba(249, 115, 22, 0.15);
    color: var(--warning);
    font-weight: 600;
    border: none;
  }

  .warning-chip.clickable {
    cursor: pointer;
    transition: background 0.15s;
  }

  .warning-chip.clickable:hover {
    background: rgba(249, 115, 22, 0.3);
  }

  /* Shelves */
  .add-btn {
    width: 24px;
    height: 24px;
    border-radius: 6px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transition: all 0.15s;
  }

  .add-btn:hover {
    background: var(--ring);
    color: var(--ink);
  }

  .shelf-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .shelf-chip {
    font-size: 11px;
    padding: 4px 10px;
    border-radius: 999px;
    background: var(--tag-bg);
    color: var(--tag-text);
  }

  .shelf-empty {
    font-size: 12px;
    color: var(--muted);
  }

  /* Device list (shared for sync status and ingest to) */
  .device-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .device-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 10px;
    background: var(--bg-2);
    border-radius: 6px;
    font-size: 12px;
  }

  .device-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .device-row-right {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .sync-status {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .sync-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--muted);
  }

  .sync-dot.synced {
    background: var(--success);
  }

  .sync-dot.pending {
    background: var(--wanted);
  }

  .device-remove-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    border-radius: 4px;
    transition: all 0.15s;
  }

  .device-remove-btn:hover {
    background: var(--ring);
    color: var(--danger);
  }

  .device-remove-btn .material-icons-outlined {
    font-size: 14px;
  }

  .device-row.not-entitled {
    opacity: 0.6;
  }

  .kindle-send-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--accent);
    cursor: pointer;
    border-radius: 4px;
    transition: all 0.15s;
  }

  .kindle-send-btn:hover {
    background: var(--accent-dim);
  }

  .kindle-send-btn .material-icons-outlined {
    font-size: 14px;
  }

  .kindle-status.sending {
    font-size: 11px;
    color: var(--muted);
  }

  .empty-devices {
    font-size: 12px;
    color: var(--muted);
    padding: 8px 0;
  }

  /* Actions */
  /* File box (for ingest source file) */
  .file-box {
    background: var(--bg-2);
    border: 1px solid var(--ring);
    border-radius: 8px;
    padding: 12px;
  }

  .file-box-main {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .file-box-name {
    font-size: 13px;
    font-family: monospace;
    word-break: break-all;
  }

  .file-box-size {
    font-size: 11px;
    color: var(--muted);
  }

  .actions-section {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-top: 12px;
    border-top: 1px solid var(--ring);
    margin-top: auto;
  }

  .ingest-actions {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
  }

  .full-width {
    width: 100%;
  }

  .action-info {
    display: flex;
    align-items: center;
    flex: 1;
    min-width: 100%;
    margin-bottom: 4px;
  }

  .info-badge {
    font-size: 12px;
    padding: 4px 10px;
    border-radius: 6px;
  }

  .info-badge.wanted {
    background: rgba(234, 179, 8, 0.15);
    color: var(--wanted);
  }

  .info-badge.skipped {
    background: rgba(107, 114, 128, 0.15);
    color: var(--skipped);
  }

  .info-badge.searching {
    background: rgba(59, 130, 246, 0.15);
    color: var(--info, #3b82f6);
  }

  .info-badge.downloading {
    background: rgba(34, 197, 94, 0.15);
    color: var(--success, #22c55e);
  }

  .info-badge.watching {
    background: rgba(168, 85, 247, 0.15);
    color: var(--purple, #a855f7);
  }

  .info-badge.failed {
    background: rgba(239, 68, 68, 0.15);
    color: var(--error, #ef4444);
  }

  .btn-primary {
    padding: 8px 16px;
    border-radius: 8px;
    border: 1px solid transparent;
    background: var(--accent);
    color: #1d130c;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s ease;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .btn-primary:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .btn-primary:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-sec {
    padding: 8px 16px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .btn-sec:hover:not(:disabled) {
    background: var(--ring);
  }

  .btn-sec:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  /* Mobile responsive */
  @media (max-width: 768px) {
    .ingest-actions {
      width: 100%;
    }
  }

  @media (max-width: 900px) {
    .details-panel {
      position: fixed;
      inset: 12px;
      width: auto;
      max-height: calc(100vh - 24px);
      overflow: auto;
      margin-top: 0;
    }

    .panel-backdrop {
      display: block;
      position: fixed;
      inset: 0;
      background: rgba(6, 6, 6, 0.45);
      z-index: 5;
      border: none;
      padding: 0;
      cursor: pointer;
    }
  }
</style>
