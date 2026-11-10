<script lang="ts">
  /**
   * BookFullPageView - Full page view for book details.
   * Renders within the content area using ContentPageView.
   */
  import type { Book, ReadingData, ReadingStatus } from '#lib/types.js';
  import ContentPageView from './ContentPageView.svelte';
  import IdentifierLinks from './IdentifierLinks.svelte';
  import GenreChips from './GenreChips.svelte';
  import BookFilesPanel from './BookFilesPanel.svelte';
  import * as api from '#lib/api/client.js';
  import ReadingSection from './ReadingSection.svelte';
  import DualRatingStars from './DualRatingStars.svelte';
  import EntityHero from './shared/EntityHero.svelte';
  import MetaLine from './shared/MetaLine.svelte';
  import UpgradeStatusBadge from './shared/UpgradeStatusBadge.svelte';
  import { formatCalendarDate, formatSize } from '#lib/utils/format.js';
  import WantedStatusPanel from './WantedStatusPanel.svelte';
  import MoveToMediaRootModal from './MoveToMediaRootModal.svelte';
  import SelectDevicesModal from './SelectDevicesModal.svelte';
  import { getAuthState } from '#lib/stores/auth.svelte.js';

  type Context = 'library' | 'ingest';

  interface Props {
    book: Book;
    context?: Context;
    canGoBack?: boolean;
    onBack?: () => void;
    onClose: () => void;
    onEdit: () => void;
    onForceResync?: () => void;
    onRefresh?: () => void;
    onSearchReleases?: () => void;
    onAuthorClick?: (name: string, id: string) => void;
    onSeriesClick?: (id: string) => void;
    onGenreClick?: (genre: string) => void;
    onEditChapters?: () => void;
    footer?: import('svelte').Snippet;
  }

  let {
    book,
    context = 'library',
    canGoBack = false,
    onBack,
    onClose,
    onEdit,
    onForceResync,
    onRefresh,
    onSearchReleases,
    onAuthorClick,
    onSeriesClick,
    onGenreClick,
    onEditChapters,
    footer
  }: Props = $props();

  const auth = getAuthState();

  // Transcript / text extraction state
  let hasAudioTranscript = $state(false);
  let hasEbookSegments = $state(false);
  let transcribing = $state(false);
  let canTranscribe = $state(false);
  let fullCastAvailable = $state(false);
  let extracting = $state(false);

  // Move to media root state
  let showMoveModal = $state(false);

  // Device sync state
  let bookDeviceStatus = $state<api.BookDevicesResponse | null>(null);
  let deviceStatusLoading = $state(false);
  let showDevicePicker = $state(false);

  // Reading data state
  let readingData = $state<ReadingData | null>(null);
  let readingDataLoading = $state(false);

  // Fetch transcript status when book changes
  $effect(() => {
    if (book?.id && (book.has_audiobook || (book.formats && book.formats.length > 0))) {
      fetchTranscriptStatus(book.id);
    }
  });

  async function fetchTranscriptStatus(bookId: string) {
    try {
      const status = await api.hasTranscript(bookId);
      hasAudioTranscript = status.has_audio_transcript;
      hasEbookSegments = status.has_ebook_segments;
      canTranscribe = status.transcript_mode !== 'off';
      fullCastAvailable = status.full_cast_available;
    } catch {
      // Not critical
    }
  }

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
    } else {
      bookDeviceStatus = null;
    }
  });

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

  async function handleLogTime(minutes: number) {
    if (!book?.id) return;
    try {
      await api.logReadingTime(book.id, { minutes });
      await fetchReadingData(book.id);
    } catch (e) {
      console.error('Failed to log reading time:', e);
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

  const hasDevices = $derived(bookDeviceStatus !== null && bookDeviceStatus.devices.length > 0);
  const isOwned = $derived(context === 'library' && book.is_in_library !== false);
  const coverUrl = $derived(
    book.cover_url
      ? `${book.cover_url}?t=${encodeURIComponent(book.updated_at || '')}`
      : '/placeholder-cover.png'
  );
</script>

<ContentPageView
  noHeader={true}
  noPadding={true}
  pageBackdropUrl={book.cover_url}
  onBack={canGoBack ? onBack : undefined}
  {onClose}
  {onEdit}
  {onRefresh}
  {footer}
>
  <EntityHero
    title={book.title}
    {coverUrl}
    description={book.description}
    rating={context !== 'library' ? book.rating : undefined}
    ratingCount={context !== 'library' ? book.rating_count : undefined}
  >
    {#snippet subtitle()}
      <div class="hero-authors">
        {#if book.authors && book.authors.length > 0}
          {#each book.authors as author, i (author.id || author.name)}
            {#if onAuthorClick && author.id}
              <button
                type="button"
                class="author-link"
                class:separated={i < book.authors.length - 1}
                onclick={() => onAuthorClick?.(author.name, author.id!)}>{author.name}</button
              >
            {:else}
              <span class:separated={i < book.authors.length - 1}>{author.name}</span>
            {/if}
          {/each}
        {/if}
      </div>
    {/snippet}

    {#snippet meta()}
      <MetaLine>
        {#if book.series_name}
          <div class="hero-series">
            {#if book.series_id && onSeriesClick}
              <button
                type="button"
                class="series-link"
                onclick={() => onSeriesClick?.(book.series_id!)}
              >
                {book.series_name}
                {#if book.series_index != null}<span class="series-index">#{book.series_index}</span
                  >{/if}
              </button>
            {:else}
              {book.series_name}
              {#if book.series_index != null}<span class="series-index">#{book.series_index}</span
                >{/if}
            {/if}
          </div>
        {/if}
        {#if book.publisher}<span>{book.publisher}</span>{/if}
        {#if book.pubdate}<span>{new Date(book.pubdate).getFullYear()}</span>{/if}
      </MetaLine>
    {/snippet}

    {#snippet ratingContent()}
      {#if context === 'library' && (readingData || book.rating)}
        <DualRatingStars
          communityRating={readingData?.community_rating ?? book.rating}
          communityRatingCount={readingData?.community_rating_count ?? book.rating_count}
          userRating={readingData?.rating ?? null}
          size="small"
          editable={true}
          onRatingChange={handleRatingChange}
        />
      {/if}
    {/snippet}
  </EntityHero>

  <div class="full-page-main-grid page-container">
    <!-- Left Column: Details & Meta -->
    <div class="details-column">
      <!-- Meta grid -->
      <div class="section card-section">
        <div class="label">Book Information</div>
        <div class="meta-grid">
          {#if book.publisher}
            <div>
              <div class="meta-label">Publisher</div>
              <div class="value">{book.publisher}</div>
            </div>
          {/if}
          {#if book.pubdate}
            <div>
              <div class="meta-label">Published</div>
              <div class="value">{formatCalendarDate(book.pubdate) ?? 'Unknown'}</div>
            </div>
          {/if}
          {#if book.page_count}
            <div>
              <div class="meta-label">Pages</div>
              <div class="value">{book.page_count.toLocaleString()}</div>
            </div>
          {/if}
          {#if book.language}
            <div>
              <div class="meta-label">Language</div>
              <div class="value">{book.language}</div>
            </div>
          {/if}
          {#if book.narrator}
            <div>
              <div class="meta-label">Narrator</div>
              <div class="value">{book.narrator}</div>
            </div>
          {/if}
          {#if book.duration_seconds}
            <div>
              <div class="meta-label">Duration</div>
              <div class="value">
                {Math.floor(book.duration_seconds / 3600)}h {String(
                  Math.floor((book.duration_seconds % 3600) / 60)
                ).padStart(2, '0')}m
              </div>
            </div>
          {/if}
          <div>
            <div class="meta-label">Added</div>
            <div class="value">{formatCalendarDate(book.created_at) ?? 'Unknown'}</div>
          </div>
        </div>
        {#if book.is_abridged || book.has_audiobook}
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
        {#if context === 'library' && auth.isAdmin && hasEbookSegments && fullCastAvailable}
          <div class="transcript-actions">
            <a href="/library/books/{book.id}/cast" class="btn-action-outline">
              <span class="material-icons-outlined">record_voice_over</span>
              Full cast
            </a>
          </div>
        {/if}
        {#if (book.has_audiobook && !hasAudioTranscript && canTranscribe) || (book.formats && book.formats.length > 0 && !hasEbookSegments)}
          <div class="transcript-actions">
            {#if book.has_audiobook && !hasAudioTranscript && canTranscribe}
              <button
                type="button"
                class="btn-action-outline"
                disabled={transcribing}
                onclick={async () => {
                  transcribing = true;
                  try {
                    await api.transcribeAudiobook(book.id);
                  } catch {
                    // Job status shows progress
                  }
                }}
              >
                <span class="material-icons-outlined">mic</span>
                {transcribing ? 'Transcription started' : 'Transcribe'}
              </button>
            {/if}
            {#if book.formats && book.formats.length > 0 && !hasEbookSegments}
              <button
                type="button"
                class="btn-action-outline"
                disabled={extracting}
                onclick={async () => {
                  extracting = true;
                  try {
                    await api.extractEbookText(book.id);
                  } catch {
                    // Job status shows progress
                  }
                }}
              >
                <span class="material-icons-outlined">text_snippet</span>
                {extracting ? 'Extraction started' : 'Extract Text'}
              </button>
            {/if}
          </div>
        {/if}
      </div>

      <!-- Genres -->
      {#if book.genres && book.genres.length > 0}
        <div class="section compact-section">
          <div class="label">Genres</div>
          <GenreChips
            genres={book.genres}
            clickable={!!onGenreClick}
            {onGenreClick}
            maxDisplay={20}
            size="small"
          />
        </div>
      {/if}

      <!-- Identifiers -->
      {#if book.identifiers && Object.keys(book.identifiers).length > 0}
        <div class="section card-section">
          <div class="label">Identifiers</div>
          <IdentifierLinks identifiers={book.identifiers} />
        </div>
      {/if}
    </div>

    <!-- Right Column: Reading & Files -->
    <div class="actions-column side-first">
      <!-- Reading section (library context only) -->
      {#if context === 'library' && !readingDataLoading}
        <div class="section card-section accent-border">
          <div class="label">Your Progress</div>
          <ReadingSection
            status={readingData?.status ?? null}
            progressPercent={readingData?.progress_percent ?? null}
            pageCount={readingData?.page_count ?? book.page_count}
            spentReadingMinutes={readingData?.spent_reading_minutes ?? null}
            editable={true}
            onStatusChange={handleStatusChange}
            onProgressChange={handleProgressChange}
            onLogTime={handleLogTime}
          />
        </div>
      {/if}

      <!-- Download Status (library context, book in wanted/download pipeline) -->
      {#if context === 'library' && book.want_status && ['wanted', 'searching', 'downloading', 'watching'].includes(book.want_status)}
        <div class="section card-section">
          <div class="label">Download Status</div>
          <WantedStatusPanel wantedId={book.id} onStatusChange={onRefresh} {onSearchReleases} />
        </div>
      {/if}

      <!-- Files (library only) -->
      {#if context === 'library' && isOwned}
        <div class="section card-section">
          <div class="label">Files & Releases</div>
          <!-- TODO: Pass upgradeUntil from entity's watch pattern status once available -->
          <UpgradeStatusBadge upgradeUntil={null} />
          <BookFilesPanel bookId={book.id} onFileDeleted={onRefresh} {onSearchReleases} />
        </div>
      {/if}

      <!-- Move to Media Root (orphaned books only) -->
      {#if context === 'library' && isOwned && !book.media_root_id}
        <div class="section card-section">
          <div class="label">Storage</div>
          <p class="orphaned-notice">This book is not in a media root.</p>
          <button type="button" class="btn" onclick={() => (showMoveModal = true)}>
            Move to Media Root
          </button>
        </div>
      {/if}

      <!-- Source file info (ingest only) -->
      {#if context === 'ingest' && book.ingest_info}
        <div class="section card-section">
          <div class="label">Source File</div>
          <div class="source-file-box">
            <div class="source-file-name">{book.ingest_info.file_name}</div>
            <div class="source-file-meta">
              {#if book.ingest_info.source_format}
                <span class="format-badge">{book.ingest_info.source_format.toUpperCase()}</span>
              {/if}
              <span class="source-file-size"
                >{formatSize(book.ingest_info.file_size, 'Unknown')}</span
              >
            </div>
          </div>
        </div>
      {/if}

      <!-- Sync Status per device (library only) -->
      {#if context === 'library' && bookDeviceStatus && hasDevices}
        {@const entitledDevices = bookDeviceStatus.devices.filter(
          (d: any) => d.entitled && !d.is_removed
        )}
        <div class="section card-section">
          <div class="section-title-row">
            <div class="label">Sync Status</div>
            <div class="section-actions">
              {#if onForceResync}
                <button
                  type="button"
                  class="icon-btn-small"
                  title="Sync now"
                  onclick={onForceResync}
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
            <div class="device-list">
              {#each bookDeviceStatus.devices as device (device.device_id)}
                <div class="device-row">
                  <span class="device-name">{device.device_name || 'Unnamed device'}</span>
                  <div class="device-row-right">
                    <div class="sync-status">
                      <div
                        class="sync-dot"
                        class:synced={book.synced && !book.pending_sync}
                        class:pending={!book.synced || book.pending_sync}
                      ></div>
                      <span>{book.synced && !book.pending_sync ? 'Synced' : 'Pending'}</span>
                    </div>
                    <button
                      type="button"
                      class="device-remove-btn"
                      onclick={() => handleRemoveDeviceFromBook(device.device_id)}
                      aria-label="Remove {device.device_name} from sync"
                      title="Remove from sync"
                      disabled={deviceStatusLoading}
                    >
                      <span class="material-icons-outlined">close</span>
                    </button>
                  </div>
                </div>
              {/each}
            </div>
          {:else if entitledDevices.length === 0}
            <div class="empty-devices">No devices syncing this book</div>
          {:else}
            <div class="device-list">
              {#each entitledDevices as device (device.device_id)}
                <div class="device-row">
                  <span class="device-name">{device.device_name || 'Unnamed device'}</span>
                  <div class="device-row-right">
                    <div class="sync-status">
                      <div
                        class="sync-dot"
                        class:synced={book.synced && !book.pending_sync}
                        class:pending={!book.synced || book.pending_sync}
                      ></div>
                      <span>{book.synced && !book.pending_sync ? 'Synced' : 'Pending'}</span>
                    </div>
                    <button
                      type="button"
                      class="device-remove-btn"
                      onclick={() => handleRemoveDeviceFromBook(device.device_id)}
                      aria-label="Remove {device.device_name} from sync"
                      title="Remove from sync"
                      disabled={deviceStatusLoading}
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
    </div>
  </div>
</ContentPageView>

{#if context === 'library' && showDevicePicker && bookDeviceStatus}
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

{#if showMoveModal}
  <MoveToMediaRootModal
    bookId={book.id}
    bookTitle={book.title}
    onClose={() => (showMoveModal = false)}
    onMoved={() => {
      showMoveModal = false;
      onRefresh?.();
    }}
  />
{/if}

<style>
  .card-section {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 24px;
  }

  .compact-section {
    padding: 8px 0;
  }

  .card-section.accent-border {
    border-color: var(--accent);
    background: linear-gradient(to bottom right, var(--card), var(--bg-2));
  }

  .hero-authors {
    font-size: 16px;
    color: var(--muted);
    font-weight: 500;
  }

  /* Inside the button, so a wrapped line never starts with an orphaned comma. */
  .hero-authors .separated::after {
    content: ',';
    margin-right: 0.3em;
    color: var(--muted);
  }

  .hero-series {
    font-size: 14px;
  }

  .author-link,
  .series-link {
    font-size: inherit;
    text-align: inherit;
    color: var(--accent);
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
  }

  .author-link:hover,
  .series-link:hover {
    text-decoration: underline;
  }

  .series-index {
    font-weight: 700;
    margin-left: 4px;
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .label {
    font-size: 12px;
    text-transform: uppercase;
    color: var(--muted);
    font-weight: 800;
    letter-spacing: 0.1em;
  }

  .meta-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 20px;
  }

  .meta-label {
    font-size: 11px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin-bottom: 4px;
  }

  .value {
    font-size: 14px;
    font-weight: 500;
  }

  .audiobook-badges {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 16px;
    flex-wrap: wrap;
  }

  .audiobook-badge {
    font-size: 10px;
    font-weight: 800;
    padding: 4px 12px;
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
    font-size: 13px;
    font-weight: 700;
    padding: 6px 16px;
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
    font-size: 18px;
  }

  .transcript-actions {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 16px;
    flex-wrap: wrap;
  }

  .btn-action-outline {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    font-weight: 600;
    padding: 6px 16px;
    border-radius: 8px;
    background: transparent;
    border: 1px solid var(--ring);
    color: var(--ink);
    cursor: pointer;
    text-decoration: none;
    transition: all 0.15s ease;
  }

  .btn-action-outline:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }

  .btn-action-outline:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .btn-action-outline .material-icons-outlined {
    font-size: 18px;
  }

  .source-file-box {
    background: var(--bg-2);
    border: 1px solid var(--ring);
    border-radius: 12px;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .source-file-name {
    font-size: 13px;
    font-family: 'JetBrains Mono', monospace;
    word-break: break-all;
    color: var(--ink);
  }

  .source-file-meta {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .format-badge {
    font-size: 10px;
    font-weight: 800;
    padding: 4px 10px;
    background: var(--accent-dim);
    color: var(--accent);
    border-radius: 6px;
    text-transform: uppercase;
  }

  .source-file-size {
    font-size: 12px;
    color: var(--muted);
  }

  .section-title-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .section-actions {
    display: flex;
    gap: 8px;
  }

  .icon-btn-small {
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg-2);
    color: var(--muted);
    cursor: pointer;
    transition: all 0.2s;
  }

  .icon-btn-small:hover {
    background: var(--ring);
    color: var(--ink);
    border-color: var(--muted);
  }

  .device-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 4px;
  }

  .device-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    background: var(--bg-2);
    border: 1px solid var(--ring);
    border-radius: 12px;
    font-size: 13px;
  }

  .device-name {
    font-weight: 600;
  }

  .device-row-right {
    display: flex;
    align-items: center;
    gap: 16px;
  }

  .sync-status {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--muted);
    font-size: 12px;
    font-weight: 500;
  }

  .sync-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--muted);
  }

  .sync-dot.synced {
    background: var(--success);
    box-shadow: 0 0 8px var(--success);
  }

  .sync-dot.pending {
    background: var(--wanted);
    box-shadow: 0 0 8px var(--wanted);
  }

  .empty-devices {
    color: var(--muted);
    font-size: 13px;
    font-weight: 400;
  }

  .device-remove-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border: none;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    border-radius: 6px;
    transition: all 0.2s;
  }

  .device-remove-btn:hover {
    background: rgba(239, 68, 68, 0.1);
    color: var(--danger);
  }

  @media (max-width: 768px) {
    .card-section {
      padding: 16px;
    }
  }

  .orphaned-notice {
    font-size: 13px;
    color: var(--muted);
    margin: 0 0 12px;
  }
</style>
