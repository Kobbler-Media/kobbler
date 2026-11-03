<script lang="ts">
  import type { Book, AuthorSummary, LoadState } from '#lib/types.js';
  import IdentifierLinks from '#lib/components/IdentifierLinks.svelte';
  import MetaLine from '#lib/components/shared/MetaLine.svelte';
  import { SearchSelect } from '@koblibs/ui';
  import * as api from '#lib/api/client.js';
  import { updateWantStatus } from '#lib/stores/wantStatus.svelte.js';
  import EntityHero from './shared/EntityHero.svelte';
  import BookListItem from './shared/BookListItem.svelte';
  import CompletionBar from './shared/CompletionBar.svelte';
  import { hasOutstandingBookWants } from '#lib/utils/want-status.js';

  // Want/skip state
  let wantingBookId = $state<string | null>(null);

  let unwantingAll = $state(false);

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

  interface Props {
    author: AuthorSummary;
    books: Book[];
    loadState: LoadState;
    loading?: boolean;
    onSeriesClick?: (seriesId: string) => void;
    onBookClick?: (book: Book) => void;
    onSearchMetadata?: () => void;
    onSearchInLibrary?: () => void;
    onRefresh?: () => void;
  }

  let {
    author,
    books,
    loadState,
    loading = false,
    onSeriesClick,
    onBookClick,
    onSearchMetadata,
    onSearchInLibrary,
    onRefresh
  }: Props = $props();

  // Tracking state
  let updatingTracking = $state(false);
  let optimisticTracked = $state<boolean | null>(null);
  const isTracked = $derived(optimisticTracked ?? author.is_monitored ?? false);

  // Bio expand/collapse
  let bioExpanded = $state(false);

  // Has hardcover ID
  const hasHardcoverId = $derived(!!author?.identifiers?.hardcover);

  // Group all books by series for display
  type SeriesGroup = {
    name: string;
    id: string | null;
    books: Book[];
    ownedCount: number;
    totalCount: number;
  };

  // Track collapsed series sections
  let collapsedSeries = $state<Set<string>>(new Set());

  const toggleSeriesCollapsed = (seriesKey: string) => {
    const newSet = new Set(collapsedSeries);
    if (newSet.has(seriesKey)) {
      newSet.delete(seriesKey);
    } else {
      newSet.add(seriesKey);
    }
    collapsedSeries = newSet;
  };

  // Group books by series
  const booksBySeries = $derived.by(() => {
    const groups: SeriesGroup[] = [];
    const standaloneKey = '__standalone__';
    let currentKey: string | null = null;
    let currentGroup: SeriesGroup | null = null;

    for (const book of books) {
      const key = book.series_name || standaloneKey;

      if (key !== currentKey) {
        if (currentGroup) {
          groups.push(currentGroup);
        }
        currentGroup = {
          name: book.series_name || 'Standalone Books',
          id: book.series_id || null,
          books: [],
          ownedCount: 0,
          totalCount: 0
        };
        currentKey = key;
      }

      if (currentGroup) {
        currentGroup.books.push(book);
        currentGroup.totalCount++;
        if (book.is_in_library !== false) {
          currentGroup.ownedCount++;
        }
      }
    }

    if (currentGroup) {
      groups.push(currentGroup);
    }

    return groups;
  });

  // Auto-want mode options
  const autoWantModes: { value: string; label: string }[] = [
    { value: 'none', label: 'None' },
    { value: 'new_only', label: 'New Releases Only' },
    { value: 'all', label: 'All Books' }
  ];

  // Local state for auto-want
  let localAutoWantMode = $state<string | undefined>(undefined);
  const effectiveAutoWantMode = $derived(localAutoWantMode ?? author.auto_want_mode ?? 'none');

  const handleToggleTracking = async () => {
    optimisticTracked = !isTracked;
    updatingTracking = true;
    try {
      await api.updateAuthorMonitoring(author.id, {
        is_monitored: !isTracked
      });
      onRefresh?.();
    } catch (e) {
      console.error('Failed to update monitoring:', e);
      optimisticTracked = null;
    } finally {
      updatingTracking = false;
    }
  };

  const handleAutoWantModeChange = async (value: string) => {
    if (value !== 'none' && !isTracked) {
      optimisticTracked = true;
      updatingTracking = true;
      try {
        await api.updateAuthorMonitoring(author.id, {
          is_monitored: true,
          auto_want_mode: value
        });
        localAutoWantMode = value;
        onRefresh?.();
      } catch (e) {
        console.error('Failed to enable monitoring:', e);
        optimisticTracked = null;
      } finally {
        updatingTracking = false;
      }
    } else {
      localAutoWantMode = value;
      try {
        await api.updateAuthorMonitoring(author.id, {
          auto_want_mode: value
        });
      } catch (e) {
        console.error('Failed to update auto-want mode:', e);
        localAutoWantMode = undefined;
      }
    }
  };

  // Collection stats
  const ownedBooks = $derived(books.filter((b) => b.is_in_library !== false).length);
  const totalBooks = $derived(author.known_book_count || books.length);

  const anyWanted = $derived(hasOutstandingBookWants(books));

  const handleUnwantAll = async () => {
    if (!anyWanted) return;
    if (!confirm('Stop looking for every book still being chased for this author?')) return;
    unwantingAll = true;
    try {
      await api.unwantAllForAuthor(author.id);
      onRefresh?.();
    } catch (e) {
      console.error('Failed to unwant books:', e);
    } finally {
      unwantingAll = false;
    }
  };
</script>

<!-- Unified Hero Section -->
<EntityHero title={author.name} coverUrl={author.image_url} avatar={true} embedded={true}>
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
      class:inactive={!isTracked}
      disabled={updatingTracking}
      onclick={handleToggleTracking}
    >
      <span class="material-icons-outlined">{isTracked ? 'star' : 'star_outline'}</span>
      {isTracked ? 'Monitored' : 'Monitor'}
    </button>
  {/snippet}
</EntityHero>

<!-- Identifiers Section -->
<div class="section">
  <h4 class="section-title">Identifiers</h4>
  <div class="id-cloud">
    {#if author.identifiers}
      <IdentifierLinks identifiers={author.identifiers} entityType="author" />
    {/if}
    {#if !hasHardcoverId}
      <button
        class="id-chip warning"
        type="button"
        title="Click to link Hardcover"
        onclick={onSearchMetadata}
      >
        HC: ?
      </button>
    {/if}
  </div>
</div>

<!-- Biography Section -->
{#if author.bio}
  <div class="section">
    <h4 class="section-title">Biography</h4>
    <p class="description-text" class:expanded={bioExpanded}>{author.bio}</p>
    <button type="button" class="expand-link" onclick={() => (bioExpanded = !bioExpanded)}>
      {bioExpanded ? 'Show less' : 'Show more'}
    </button>
  </div>
{/if}

<!-- Collection Progress Row -->
<CompletionBar
  label="Collection"
  owned={ownedBooks}
  total={totalBooks}
  {loading}
  wantingAll={unwantingAll}
  onUnwantAll={anyWanted ? handleUnwantAll : undefined}
  onSearch={onSearchInLibrary}
/>

<!-- Auto-want Section -->
<div class="section">
  <h4 class="section-title">Auto-want</h4>
  <SearchSelect
    value={effectiveAutoWantMode}
    items={autoWantModes}
    disabled={updatingTracking}
    onchange={handleAutoWantModeChange}
  />
</div>

<!-- Series Section -->
<div class="section">
  <h4 class="section-title">Series</h4>

  {#if loadState.status === 'loading'}
    <div class="muted">Loading...</div>
  {:else if booksBySeries.length === 0}
    <div class="muted">No books by this author.</div>
  {:else}
    <div class="series-list">
      {#each booksBySeries as series (series.name)}
        {@const isCollapsed = collapsedSeries.has(series.name)}
        <div class="series-section" class:collapsed={isCollapsed}>
          <div class="series-section-header">
            <button
              type="button"
              class="series-section-info"
              onclick={() => series.id && onSeriesClick?.(series.id)}
              disabled={!series.id}
            >
              <div class="series-section-title">{series.name}</div>
              <div class="series-section-meta">
                {series.ownedCount} of {series.totalCount} books
              </div>
            </button>
            <button
              type="button"
              class="series-section-toggle"
              onclick={() => toggleSeriesCollapsed(series.name)}
              title={isCollapsed ? 'Expand' : 'Collapse'}
            >
              <span class="material-icons-outlined"
                >{isCollapsed ? 'expand_more' : 'expand_less'}</span
              >
            </button>
          </div>

          {#if !isCollapsed}
            <div class="book-list">
              {#each series.books as book (book.id)}
                <BookListItem
                  {book}
                  {onBookClick}
                  onWant={handleWantBook}
                  onSkip={handleSkipBook}
                  {wantingBookId}
                  showSeriesIndex={true}
                />
              {/each}
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .hero-meta {
    font-size: 13px;
    color: var(--muted);
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
    line-height: 1.6;
    color: var(--ink);
    margin: 0;
    white-space: pre-wrap;
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 4;
    line-clamp: 4;
    -webkit-box-orient: vertical;
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

  /* ID cloud / chips */
  .id-cloud {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .id-chip {
    display: flex;
    align-items: center;
    gap: 4px;
    background: var(--bg);
    border: 1px solid var(--ring);
    padding: 3px 6px;
    border-radius: 4px;
    font-size: 10px;
    color: var(--ink);
    font-family: monospace;
  }

  .id-chip.warning {
    background: rgba(234, 179, 8, 0.1);
    border-color: rgba(234, 179, 8, 0.3);
    color: var(--wanted);
    cursor: pointer;
  }

  .id-chip.warning:hover {
    background: rgba(234, 179, 8, 0.2);
  }

  /* Series list */
  .series-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .series-section {
    background: var(--bg-2);
    border-radius: 12px;
    overflow: hidden;
  }

  .series-section-header {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 12px;
  }

  .series-section-info {
    flex: 1;
    min-width: 0;
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    color: inherit;
    text-align: left;
    cursor: pointer;
  }

  .series-section-info:disabled {
    cursor: default;
  }

  .series-section-info:not(:disabled):hover .series-section-title {
    color: var(--accent);
  }

  .series-section-title {
    font-size: 13px;
    font-weight: 600;
    margin-bottom: 2px;
  }

  .series-section-meta {
    font-size: 11px;
    color: var(--muted);
  }

  .series-section-toggle {
    color: var(--muted);
    display: flex;
    align-items: center;
    background: none;
    border: none;
    padding: 4px;
    cursor: pointer;
    border-radius: 4px;
  }

  .series-section-toggle:hover {
    background: var(--ring);
    color: var(--ink);
  }

  .series-section-toggle .material-icons-outlined {
    font-size: 20px;
  }

  /* Book list */
  .book-list {
    display: flex;
    flex-direction: column;
  }

  .muted {
    color: var(--muted);
    font-size: 13px;
  }
</style>
