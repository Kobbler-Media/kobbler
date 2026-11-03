<script lang="ts">
  /**
   * AuthorFullPageView - Full page view for author details.
   * Two-column layout: wide books column + 400px sidebar with combined collection card.
   */
  import * as api from '#lib/api/client.js';
  import type { AuthorSummary, Book, SeriesSummary } from '#lib/types.js';
  import { updateWantStatus } from '#lib/stores/wantStatus.svelte.js';
  import { getBookListStatusBadge } from '#lib/utils/book-status.js';
  import EntityHero from './shared/EntityHero.svelte';
  import ContentPageView from './ContentPageView.svelte';
  import MetaLine from './shared/MetaLine.svelte';
  import IdentifierLinks from './IdentifierLinks.svelte';

  interface Props {
    author: AuthorSummary;
    books: Book[];
    series?: SeriesSummary[];
    canGoBack?: boolean;
    onBack?: () => void;
    onClose: () => void;
    onEdit: () => void;
    onTrack?: () => void;
    onUntrack?: () => void;
    onRefresh?: () => void;
    onBookClick?: (book: Book) => void;
    onSeriesClick?: (seriesId: string) => void;
    onWantAllMissing?: () => void;
    onUnwantAll?: () => void;
    onSearch?: () => void;
    onSearchReleases?: () => void;
  }

  let {
    author,
    books,
    series = [],
    canGoBack = false,
    onBack,
    onClose,
    onEdit,
    onTrack,
    onUntrack,
    onRefresh,
    onBookClick,
    onSeriesClick,
    onWantAllMissing,
    onUnwantAll,
    onSearch,
    onSearchReleases
  }: Props = $props();

  const ownedCount = $derived(books.filter((b) => b.is_in_library !== false).length);
  const totalCount = $derived(author.known_book_count || books.length);
  const progressPercent = $derived(
    totalCount > 0 ? Math.round((ownedCount / totalCount) * 100) : 0
  );

  const getAvatarUrl = () => {
    if (author.image_url) return author.image_url;
    if (author.cover_urls && author.cover_urls.length > 0) return author.cover_urls[0];
    return null;
  };

  const getStatusBadge = (book: Book) => getBookListStatusBadge(book);

  /** Statuses where Want/Skip actions make sense */
  const canWant = (status: string) =>
    status === 'missing' || status === 'failed' || status === 'skipped';
  const canSkip = (status: string) =>
    status === 'missing' || status === 'failed' || status === 'wanted';
  const isActive = (status: string) =>
    status === 'searching' ||
    status === 'downloading' ||
    status === 'ingesting' ||
    status === 'watching';

  // Auto-want state (interactive segmented toggle)
  const autoWantModes = [
    { value: 'none', label: 'None' },
    { value: 'new_only', label: 'New' },
    { value: 'all', label: 'All' }
  ] as const;

  let localAutoWantMode = $state<string | undefined>(undefined);
  let updatingAutoWant = $state(false);
  const effectiveAutoWantMode = $derived(localAutoWantMode ?? author.auto_want_mode ?? 'none');

  const handleAutoWantModeChange = async (value: string) => {
    if (value === effectiveAutoWantMode) return;

    if (value !== 'none' && !author.is_monitored) {
      // Auto-enable monitoring when setting a non-none mode
      updatingAutoWant = true;
      try {
        await api.updateAuthorMonitoring(author.id, {
          is_monitored: true,
          auto_want_mode: value
        });
        localAutoWantMode = value;
        onRefresh?.();
      } catch (e) {
        console.error('Failed to enable monitoring:', e);
      } finally {
        updatingAutoWant = false;
      }
    } else {
      localAutoWantMode = value;
      updatingAutoWant = true;
      try {
        await api.updateAuthorMonitoring(author.id, {
          auto_want_mode: value
        });
      } catch (e) {
        console.error('Failed to update auto-want mode:', e);
        localAutoWantMode = undefined;
      } finally {
        updatingAutoWant = false;
      }
    }
  };

  // Group books by series for display
  interface BookGroup {
    series: SeriesSummary | null;
    seriesId: string | null;
    seriesName: string;
    books: Book[];
    ownedCount: number;
    totalCount: number;
  }

  const groupedBooks = $derived(() => {
    const groups = new Map<string | null, BookGroup>();

    for (const book of books) {
      const seriesId = book.series_id;
      const seriesName = book.series_name || 'Standalone';

      if (!groups.has(seriesId)) {
        const seriesData = series.find((s) => s.id === seriesId) || null;
        groups.set(seriesId, {
          series: seriesData,
          seriesId,
          seriesName,
          books: [],
          ownedCount: 0,
          totalCount: seriesData?.total_book_count || 0
        });
      }

      const group = groups.get(seriesId)!;
      group.books.push(book);
      if (book.is_in_library !== false) {
        group.ownedCount++;
      }
      if (!group.series) {
        group.totalCount = group.books.length;
      }
    }

    // Sort: series first, standalone last
    return [...groups.values()].sort((a, b) => {
      if (a.seriesId && !b.seriesId) return -1;
      if (!a.seriesId && b.seriesId) return 1;
      return a.seriesName.localeCompare(b.seriesName);
    });
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

  // Collapsed state for series sections
  let collapsedSeries = $state<Set<string | null>>(new Set());

  const toggleSeriesCollapse = (seriesId: string | null) => {
    if (collapsedSeries.has(seriesId)) {
      collapsedSeries.delete(seriesId);
    } else {
      collapsedSeries.add(seriesId);
    }
    collapsedSeries = new Set(collapsedSeries);
  };
</script>

<ContentPageView
  noHeader={true}
  noPadding={true}
  pageBackdropUrl={author.image_url}
  onBack={canGoBack ? onBack : undefined}
  {onClose}
  {onEdit}
  {onRefresh}
>
  <EntityHero title={author.name} coverUrl={getAvatarUrl()} avatar={true} description={author.bio}>
    {#snippet subtitle()}
      <div class="hero-meta">
        <MetaLine
          items={[
            author.total_book_count > author.book_count
              ? `${author.book_count} of ${author.total_book_count} books`
              : `${author.book_count} books`,
            author.total_series_count && `${author.total_series_count} series`
          ]}
        />
      </div>
    {/snippet}

    {#snippet actions()}
      <button
        type="button"
        class="monitored-pill"
        class:inactive={!author.is_monitored}
        onclick={author.is_monitored ? onUntrack : onTrack}
      >
        <span class="material-icons-outlined">{author.is_monitored ? 'star' : 'star_outline'}</span>
        {author.is_monitored ? 'Monitored' : 'Monitor'}
      </button>
    {/snippet}

    {#snippet extraContent()}
      <div class="hero-identifiers">
        <div class="id-cloud">
          {#if author.identifiers}
            <IdentifierLinks identifiers={author.identifiers} entityType="author" />
          {/if}
          {#if !author.identifiers?.hardcover}
            <button class="id-chip warning" type="button" title="Click to link" onclick={onSearch}>
              HC: ?
            </button>
          {/if}
        </div>
      </div>
    {/snippet}
  </EntityHero>

  <div class="full-page-main-grid page-container">
    <div class="books-column">
      <!-- Books List -->
      <div class="section card-section">
        <div class="label">Series & Books</div>
        <div class="series-sections">
          {#each groupedBooks() as group (group.seriesId || 'standalone')}
            {@const isCollapsed = collapsedSeries.has(group.seriesId)}
            {@const groupProgressPercent =
              group.totalCount > 0 ? Math.round((group.ownedCount / group.totalCount) * 100) : 0}
            <div class="series-section" class:collapsed={isCollapsed}>
              <div
                class="series-section-header"
                role="button"
                tabindex="0"
                onclick={() => toggleSeriesCollapse(group.seriesId)}
                onkeydown={(e) => {
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault();
                    toggleSeriesCollapse(group.seriesId);
                  }
                }}
              >
                <div class="series-section-info">
                  <div class="series-section-title">
                    {group.seriesName}
                  </div>
                  <div class="series-section-meta">
                    <MetaLine
                      items={[
                        `${group.ownedCount} of ${group.totalCount} books`,
                        `${groupProgressPercent}%`
                      ]}
                    />
                  </div>
                </div>
                <span class="series-section-actions">
                  {#if group.seriesId && onSeriesClick}
                    <button
                      class="series-link-btn"
                      type="button"
                      title="Open series details"
                      onclick={(e) => {
                        e.stopPropagation();
                        onSeriesClick(group.seriesId!);
                      }}
                    >
                      <span class="material-icons-outlined" style="font-size: 16px;"
                        >open_in_new</span
                      >
                    </button>
                  {/if}
                  <span class="series-section-toggle">
                    <span class="material-icons-outlined"
                      >{isCollapsed ? 'expand_more' : 'expand_less'}</span
                    >
                  </span>
                </span>
              </div>
              {#if !isCollapsed}
                <div class="book-list">
                  {#each group.books as book (book.id)}
                    {@const status = getStatusBadge(book)}
                    <div
                      class="book-list-item"
                      class:not-owned={book.is_in_library === false}
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
                        style="background-image: url('{book.cover_url ||
                          '/placeholder-cover.png'}')"
                      ></div>
                      <div class="book-list-info">
                        <div class="book-list-title">{book.title}</div>
                        <div class="book-list-meta">
                          <MetaLine
                            items={[
                              book.series_index && `#${book.series_index}`,
                              book.pubdate && new Date(book.pubdate).getFullYear()
                            ]}
                          />
                        </div>
                      </div>
                      <span class="status-pill {status.class}" class:pulse={isActive(status.class)}
                        >{status.text}</span
                      >
                      {#if book.is_in_library === false && (canWant(status.class) || canSkip(status.class))}
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
              {/if}
            </div>
          {/each}
        </div>
      </div>
    </div>

    <div class="sidebar-column side-first">
      <!-- Combined Collection + Auto-want -->
      <div class="section card-section accent-border">
        <div class="collection-header">
          <div class="label">Collection</div>
          <div class="section-actions">
            {#if onWantAllMissing}
              <button
                type="button"
                class="icon-btn-small"
                onclick={onWantAllMissing}
                title="Want all missing"
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
            {#if onSearch}
              <button
                type="button"
                class="icon-btn-small"
                onclick={onSearch}
                title="Browse library"
              >
                <span class="material-icons-outlined">search</span>
              </button>
            {/if}
            {#if onSearchReleases}
              <button
                type="button"
                class="icon-btn-small"
                onclick={onSearchReleases}
                title="Search releases"
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

        <div class="auto-want-row">
          <span class="auto-want-label">Auto-want</span>
          <div class="segmented-toggle" class:disabled={updatingAutoWant}>
            {#each autoWantModes as mode (mode.value)}
              <button
                type="button"
                class="segment"
                class:active={effectiveAutoWantMode === mode.value}
                disabled={updatingAutoWant}
                onclick={() => handleAutoWantModeChange(mode.value)}
              >
                {mode.label}
              </button>
            {/each}
          </div>
        </div>
      </div>
    </div>
  </div>
</ContentPageView>

<style>
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

  /* Collection header with actions inline */
  .collection-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
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

  /* Auto-want row */
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

  .segmented-toggle {
    display: flex;
    background: var(--bg-2);
    border: 1px solid var(--ring);
    border-radius: 8px;
    overflow: hidden;
  }

  .segmented-toggle.disabled {
    opacity: 0.6;
    pointer-events: none;
  }

  .segment {
    padding: 6px 14px;
    font-size: 12px;
    font-weight: 600;
    background: transparent;
    border: none;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s;
    white-space: nowrap;
  }

  .segment:not(:last-child) {
    border-right: 1px solid var(--ring);
  }

  .segment:hover:not(:disabled):not(.active) {
    background: var(--ring);
    color: var(--ink);
  }

  .segment.active {
    background: var(--accent);
    color: #1d130c;
  }

  /* Identifier chips in hero */
  .hero-identifiers {
    display: flex;
    align-items: center;
    gap: 8px;
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

  .series-sections {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .series-section {
    background: var(--bg-2);
    border: 1px solid var(--ring);
    border-radius: 12px;
    overflow: hidden;
  }

  .series-section-header {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
    color: var(--ink);
    transition: background 0.2s;
  }

  .series-section-header:hover {
    background: var(--card);
  }

  .series-section-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .series-section-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .series-link-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    transition: all 0.15s ease;
    flex-shrink: 0;
  }

  .series-link-btn:hover {
    background: var(--bg-2);
    color: var(--ink);
    border-color: var(--ink);
  }

  .series-section-title {
    font-weight: 700;
    font-size: 15px;
  }

  .series-section-meta {
    font-size: 12px;
    color: var(--muted);
  }

  /* Subgrid keeps the status and action columns aligned across rows that have no actions. */
  .book-list {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) 80px auto;
    row-gap: 4px;
    background: var(--bg);
    padding: 8px;
    border-top: 1px solid var(--ring);
  }

  .book-list-item {
    grid-column: 1 / -1;
    display: grid;
    grid-template-columns: subgrid;
    gap: 16px;
    padding: 10px;
    border-radius: 10px;
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
    white-space: nowrap;
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

  @media (max-width: 600px) {
    .card-section {
      padding: 16px 10px;
    }

    .book-list-item {
      gap: 10px;
    }
  }
</style>
