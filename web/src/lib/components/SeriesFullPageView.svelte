<script lang="ts">
  /**
   * SeriesFullPageView - Full page view for series details.
   * Two-column layout matching mockup: cover sidebar + main content.
   */
  import * as api from '$lib/api/client';
  import type { SeriesSummary, Book } from '$lib/types';
  import { updateWantStatus } from '$lib/stores/wantStatus.svelte';
  import { getBookListStatusBadge } from '$lib/utils/book-status';
  import EntityHero from './shared/EntityHero.svelte';
  import ContentPageView from './ContentPageView.svelte';
  import IdentifierLinks from './IdentifierLinks.svelte';

  interface Props {
    series: SeriesSummary;
    books: Book[];
    canGoBack?: boolean; // Show back arrow when there's navigation history
    onBack?: () => void; // Go back in history
    onClose: () => void; // Close/exit to parent list
    onEdit: () => void;
    onTrack?: () => void;
    onUntrack?: () => void;
    onRefresh?: () => void;
    onBookClick?: (book: Book) => void;
    onAuthorClick?: (name: string, id: string) => void;
    onWantAllMissing?: () => void;
    onUnwantAll?: () => void;
    onSearchSeries?: () => void;
    onSearchReleases?: () => void;
  }

  let {
    series,
    books,
    canGoBack = false,
    onBack,
    onClose,
    onEdit,
    onTrack,
    onUntrack,
    onRefresh,
    onBookClick,
    onAuthorClick,
    onWantAllMissing,
    onUnwantAll,
    onSearchSeries,
    onSearchReleases
  }: Props = $props();

  const ownedCount = $derived(books.filter((b) => b.is_in_library !== false).length);
  const totalCount = $derived(series.total_book_count || books.length);
  const progressPercent = $derived(
    totalCount > 0 ? Math.round((ownedCount / totalCount) * 100) : 0
  );

  const getCoverUrl = () => {
    // Fallback order: series image → first book cover → placeholder
    if (series.image_url) return series.image_url;
    if (series.cover_urls && series.cover_urls.length > 0) return series.cover_urls[0];
    return null;
  };

  const getStatusBadge = (book: Book) => getBookListStatusBadge(book);

  const canWant = (status: string) =>
    status === 'missing' || status === 'failed' || status === 'skipped';
  const canSkip = (status: string) =>
    status === 'missing' || status === 'failed' || status === 'wanted';
  const isActive = (status: string) =>
    status === 'searching' ||
    status === 'downloading' ||
    status === 'ingesting' ||
    status === 'watching';

  const autoWantLabel = $derived(() => {
    switch (series.auto_want_mode) {
      case 'none':
        return 'None';
      case 'new_only':
        return 'New Only';
      case 'all':
        return 'All';
      case 'fill_gaps':
        return 'Fill Gaps';
      default:
        return 'Inherit';
    }
  });

  let wantingBookId = $state<string | null>(null);

  const handleWantBook = async (book: Book) => {
    wantingBookId = book.id;
    updateWantStatus(book.id, 'wanted');
    try {
      await api.wantBook(book.id);
    } catch (e) {
      console.error('Failed to want book:', e);
      updateWantStatus(book.id, book.want_status ?? 'none');
    } finally {
      wantingBookId = null;
    }
  };

  const handleSkipBook = async (book: Book) => {
    wantingBookId = book.id;
    updateWantStatus(book.id, 'skipped');
    try {
      await api.skipBook(book.id);
    } catch (e) {
      console.error('Failed to skip book:', e);
      updateWantStatus(book.id, book.want_status ?? 'none');
    } finally {
      wantingBookId = null;
    }
  };

  // Sort books by series index
  const sortedBooks = $derived(
    [...books].sort((a, b) => {
      const indexA = a.series_index ?? 999;
      const indexB = b.series_index ?? 999;
      return indexA - indexB;
    })
  );
</script>

<ContentPageView
  noHeader={true}
  noPadding={true}
  pageBackdropUrl={getCoverUrl()}
  onBack={canGoBack ? onBack : undefined}
  {onClose}
  {onEdit}
  {onRefresh}
>
  <EntityHero title={series.name} coverUrl={getCoverUrl()} description={series.description}>
    {#snippet subtitle()}
      {#if series.authors && series.authors.length > 0}
        <div class="hero-authors">
          {#each series.authors as author, i (author.id || author.name)}
            {#if i > 0},
            {/if}
            <button
              type="button"
              class="author-link"
              onclick={() => onAuthorClick?.(author.name, author.id!)}
            >
              {author.name}
            </button>
          {/each}
        </div>
      {/if}
    {/snippet}

    {#snippet actions()}
      <button
        type="button"
        class="monitored-pill"
        class:inactive={!series.is_monitored}
        onclick={series.is_monitored ? onUntrack : onTrack}
      >
        <span class="material-icons-outlined">{series.is_monitored ? 'star' : 'star_outline'}</span>
        {series.is_monitored ? 'Monitored' : 'Monitor'}
      </button>
    {/snippet}
  </EntityHero>

  <div class="full-page-main-grid page-container">
    <div class="books-column">
      <!-- Books List -->
      <div class="section card-section">
        <div class="label">Books in Series</div>
        <div class="book-list">
          {#each sortedBooks as book (book.id)}
            {@const status = getStatusBadge(book)}
            {@const isOwned = book.is_in_library !== false}
            <div
              class="book-list-item"
              class:not-owned={!isOwned}
              role="button"
              tabindex="0"
              onclick={() => onBookClick?.(book)}
              onkeydown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  onBookClick?.(book);
                }
              }}
            >
              <div
                class="book-list-thumb"
                style="background-image: url('{book.cover_url || '/placeholder-cover.png'}')"
              ></div>
              <div class="book-list-info">
                <div class="book-list-title">{book.title}</div>
                <div class="book-list-meta">
                  {book.series_index != null ? `#${book.series_index}` : ''}
                  {#if book.pubdate}
                    · {new Date(book.pubdate).getFullYear()}
                  {/if}
                </div>
              </div>
              <span class="status-pill {status.class}" class:pulse={isActive(status.class)}
                >{status.text}</span
              >
              {#if !isOwned && (canWant(status.class) || canSkip(status.class))}
                <div class="action-btns">
                  {#if canWant(status.class)}
                    <button
                      type="button"
                      class="want-btn"
                      disabled={wantingBookId === book.id}
                      onclick={(e) => {
                        e.stopPropagation();
                        handleWantBook(book);
                      }}
                    >
                      Want
                    </button>
                  {/if}
                  {#if canSkip(status.class)}
                    <button
                      type="button"
                      class="skip-btn"
                      disabled={wantingBookId === book.id}
                      onclick={(e) => {
                        e.stopPropagation();
                        handleSkipBook(book);
                      }}
                    >
                      Skip
                    </button>
                  {/if}
                </div>
              {/if}
            </div>
          {/each}
        </div>
      </div>
    </div>

    <div class="sidebar-column">
      <!-- Collection Progress -->
      <div class="section card-section accent-border">
        <div class="collection-header">
          <div class="label">Collection</div>
          <div class="section-actions">
            {#if onWantAllMissing}
              <button
                type="button"
                class="icon-btn-small"
                title="Want all missing"
                onclick={onWantAllMissing}
              >
                <span class="material-icons-outlined">playlist_add</span>
              </button>
            {/if}
            {#if onUnwantAll}
              <button
                type="button"
                class="icon-btn-small"
                title="Unwant every book still being chased"
                onclick={onUnwantAll}
              >
                <span class="material-icons-outlined">playlist_remove</span>
              </button>
            {/if}
            {#if onSearchSeries}
              <button
                type="button"
                class="icon-btn-small"
                title="Browse library"
                onclick={onSearchSeries}
              >
                <span class="material-icons-outlined">search</span>
              </button>
            {/if}
            {#if onSearchReleases}
              <button
                type="button"
                class="icon-btn-small"
                title="Search releases"
                onclick={onSearchReleases}
              >
                <span class="material-icons-outlined">download</span>
              </button>
            {/if}
          </div>
        </div>
        <div class="progress-track">
          <div class="progress-fill" style="width: {progressPercent}%"></div>
        </div>
        <div class="completion-text">
          {ownedCount} of {totalCount} books owned ({progressPercent}%)
        </div>

        <!-- Auto-want Mode -->
        <div class="auto-want-row">
          <span class="auto-want-label">Auto-want</span>
          <div class="auto-want-badge">
            <span class="material-icons-outlined">settings_suggest</span>
            {autoWantLabel()}
          </div>
        </div>
      </div>

      <!-- Identifiers -->
      <div class="section card-section">
        <div class="label">Identifiers</div>
        <div class="id-cloud">
          {#if series.identifiers}
            <IdentifierLinks identifiers={series.identifiers} entityType="series" />
          {/if}
          {#if !series.identifiers?.hardcover}
            <button
              class="id-chip warning"
              type="button"
              title="Click to link"
              onclick={onSearchSeries}
            >
              HC: ?
            </button>
          {/if}
        </div>
      </div>
    </div>
  </div>
</ContentPageView>

<style>
  .full-page-main-grid {
    display: grid;
    grid-template-columns: 1fr 400px;
    gap: 32px;
    align-items: start;
  }

  .books-column,
  .sidebar-column {
    display: flex;
    flex-direction: column;
    gap: 24px;
  }

  .card-section {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 24px;
  }

  .card-section.accent-border {
    border-color: var(--accent);
    background: linear-gradient(to bottom right, var(--card), var(--bg-2));
  }

  .hero-authors {
    font-size: 16px;
    color: var(--muted);
  }

  .author-link {
    color: var(--accent);
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    font-size: inherit;
  }

  .author-link:hover {
    text-decoration: underline;
  }

  .monitored-pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: rgba(139, 92, 246, 0.15);
    border: 1px solid var(--monitored);
    color: var(--monitored);
    padding: 6px 14px;
    border-radius: 100px;
    font-size: 12px;
    font-weight: 700;
    cursor: pointer;
    transition: all 0.2s;
  }

  .monitored-pill:hover {
    background: rgba(139, 92, 246, 0.25);
    transform: translateY(-1px);
  }

  .monitored-pill.inactive {
    background: var(--bg-2);
    border-color: var(--ring);
    color: var(--muted);
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .label {
    font-size: 11px;
    font-weight: 800;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.1em;
  }

  .collection-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .progress-track {
    height: 12px;
    background: var(--ring);
    border-radius: 6px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 6px;
    box-shadow: 0 0 12px var(--accent-dim);
  }

  .completion-text {
    font-size: 12px;
    color: var(--muted);
    font-weight: 500;
  }

  .section-actions {
    display: flex;
    gap: 8px;
  }

  .icon-btn-small {
    width: 36px;
    height: 36px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 10px;
    background: var(--bg-2);
    color: var(--muted);
    cursor: pointer;
    transition: all 0.2s;
  }

  .icon-btn-small:hover:not(:disabled) {
    background: var(--ring);
    color: var(--ink);
  }

  .icon-btn-small:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .id-cloud {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .id-chip.warning {
    background: rgba(234, 179, 8, 0.1);
    border: 1px solid var(--wanted);
    color: var(--wanted);
    padding: 4px 10px;
    border-radius: 6px;
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
  }

  .auto-want-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding-top: 12px;
    border-top: 1px solid var(--ring);
  }

  .auto-want-label {
    font-size: 12px;
    font-weight: 700;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    white-space: nowrap;
  }

  .auto-want-badge {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 6px 14px;
    background: var(--bg-2);
    border: 1px solid var(--ring);
    border-radius: 8px;
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
  }

  .auto-want-badge .material-icons-outlined {
    font-size: 18px;
    color: var(--muted);
  }

  .book-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .book-list-item {
    display: grid;
    grid-template-columns: 40px 1fr 80px auto;
    gap: 16px;
    padding: 12px;
    border-radius: 12px;
    align-items: center;
    background: var(--bg-2);
    border: 1px solid transparent;
    transition: all 0.2s;
    cursor: pointer;
  }

  .book-list-item:hover {
    background: var(--card);
    border-color: var(--ring);
    transform: translateX(4px);
  }

  .book-list-item.not-owned {
    opacity: 0.8;
  }

  .book-list-thumb {
    width: 40px;
    aspect-ratio: 2 / 3;
    border-radius: 6px;
    background-size: cover;
    background-position: center;
    background-color: var(--bg);
    border: 1px solid var(--ring);
    box-shadow: 0 4px 8px rgba(0, 0, 0, 0.2);
  }

  .book-list-info {
    min-width: 0;
  }

  .book-list-title {
    font-weight: 600;
    font-size: 14px;
    margin-bottom: 2px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--ink);
  }

  .book-list-meta {
    font-size: 12px;
    color: var(--muted);
  }

  .status-pill {
    font-size: 10px;
    font-weight: 800;
    padding: 3px 8px;
    border-radius: 6px;
    border: 1px solid var(--ring);
    color: var(--muted);
    text-align: center;
    text-transform: uppercase;
  }

  .status-pill.synced,
  .status-pill.owned {
    border-color: rgba(34, 197, 94, 0.3);
    color: #4ade80;
    background: rgba(34, 197, 94, 0.1);
  }

  .status-pill.wanted {
    border-color: rgba(234, 179, 8, 0.3);
    color: var(--wanted);
    background: rgba(234, 179, 8, 0.1);
  }

  .status-pill.pulse {
    animation: pulse-badge 2s ease-in-out infinite;
  }

  @keyframes pulse-badge {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.5;
    }
  }

  .action-btns {
    display: flex;
    gap: 8px;
  }

  .want-btn,
  .skip-btn {
    font-size: 11px;
    font-weight: 700;
    padding: 6px 12px;
    border-radius: 100px;
    cursor: pointer;
    transition: all 0.2s;
  }

  .want-btn {
    border: 1px solid var(--accent);
    background: transparent;
    color: var(--accent);
  }

  .want-btn:hover:not(:disabled) {
    background: var(--accent);
    color: #1d130c;
  }

  .skip-btn {
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
  }

  .skip-btn:hover:not(:disabled) {
    background: var(--ring);
    color: var(--ink);
  }

  @media (max-width: 1100px) {
    .full-page-main-grid {
      grid-template-columns: 1fr;
    }

    .sidebar-column {
      order: -1;
    }
  }
</style>
