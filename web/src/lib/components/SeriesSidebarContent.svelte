<script lang="ts">
  import type { Book, SeriesSummary, LoadState } from '#lib/types.js';
  import IdentifierLinks from '#lib/components/IdentifierLinks.svelte';
  import { SearchSelect } from '@koblibs/ui';
  import * as api from '#lib/api/client.js';
  import { updateWantStatus } from '#lib/stores/wantStatus.svelte.js';
  import EntityHero from './shared/EntityHero.svelte';
  import BookListItem from './shared/BookListItem.svelte';
  import CompletionBar from './shared/CompletionBar.svelte';
  import { hasOutstandingBookWants } from '#lib/utils/want-status.js';

  interface Props {
    series: SeriesSummary;
    books: Book[];
    loadState: LoadState;
    onAuthorClick?: (authorName: string, authorId: string) => void;
    onBookClick?: (book: Book) => void;
    onSearchMetadata?: () => void;
    onRefresh?: () => void;
  }

  let { series, books, loadState, onAuthorClick, onBookClick, onSearchMetadata, onRefresh }: Props =
    $props();

  // Monitoring toggle state
  let updatingMonitoring = $state(false);

  const handleToggleMonitoring = async () => {
    updatingMonitoring = true;
    try {
      await api.updateSeriesMonitoring(series.id, { is_monitored: !series.is_monitored });
      onRefresh?.();
    } catch (e) {
      console.error('Failed to update series monitoring:', e);
    } finally {
      updatingMonitoring = false;
    }
  };

  // Want/skip state
  let wantingBookId = $state<string | null>(null);
  let wantingAll = $state(false);

  // Books split into owned and not owned
  const ownedBooks = $derived(books.filter((b) => b.is_in_library !== false));
  const notOwnedBooks = $derived(books.filter((b) => b.is_in_library === false));

  // Combined books sorted by series index
  const allBooksSorted = $derived(
    [...books].sort((a, b) => {
      const indexA = a.series_index ?? 999;
      const indexB = b.series_index ?? 999;
      return indexA - indexB;
    })
  );

  // Series completion stats
  const seriesStats = $derived.by(() => {
    const total = allBooksSorted.length;
    const owned = ownedBooks.length;
    const wanted = notOwnedBooks.filter((b) => b.want_status === 'wanted').length;
    const skipped = notOwnedBooks.filter((b) => b.want_status === 'skipped').length;
    const missing = notOwnedBooks.length - wanted - skipped;
    return { total, owned, wanted, skipped, missing };
  });

  // Has hardcover ID = can show not-owned books
  const hasHardcoverId = $derived(!!series?.identifiers?.hardcover);

  // Description expand/collapse
  let descriptionExpanded = $state(false);

  // Want all missing books
  const handleWantAllMissing = async () => {
    const missingBooks = notOwnedBooks.filter(
      (b) => b.want_status !== 'wanted' && b.want_status !== 'skipped'
    );
    if (missingBooks.length === 0) return;

    wantingAll = true;
    try {
      for (const book of missingBooks) {
        await api.wantBook(book.id);
      }
      onRefresh?.();
    } catch (e) {
      console.error('Failed to want all missing books:', e);
    } finally {
      wantingAll = false;
    }
  };

  const anyWanted = $derived(hasOutstandingBookWants(books));

  const handleUnwantAll = async () => {
    if (!anyWanted) return;
    if (!confirm('Stop looking for every book still being chased in this series?')) return;
    wantingAll = true;
    try {
      await api.unwantAllForSeries(series.id);
      onRefresh?.();
    } catch (e) {
      console.error('Failed to unwant books:', e);
    } finally {
      wantingAll = false;
    }
  };

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

  // Local state for auto-want updates
  let localAutoWantMode = $state<string | undefined>(undefined);
  const effectiveAutoWantMode = $derived(localAutoWantMode ?? series.auto_want_mode ?? 'inherit');
</script>

<!-- Unified Hero Section -->
<EntityHero
  title={series.name}
  coverUrl={series.cover_urls && series.cover_urls.length > 0 ? series.cover_urls[0] : null}
  embedded={true}
>
  {#snippet subtitle()}
    {#if series.authors && series.authors.length > 0}
      <button
        type="button"
        class="hero-author-link"
        onclick={() => {
          if (series.authors?.[0]?.id) {
            onAuthorClick?.(series.authors[0].name, series.authors[0].id);
          }
        }}
      >
        {series.authors[0].name}
      </button>
    {/if}
  {/snippet}

  {#snippet actions()}
    <button
      type="button"
      class="monitored-pill"
      class:inactive={!series.is_monitored}
      disabled={updatingMonitoring}
      onclick={handleToggleMonitoring}
    >
      <span class="material-icons-outlined">{series.is_monitored ? 'star' : 'star_outline'}</span>
      {series.is_monitored ? 'Monitored' : 'Monitor'}
    </button>
  {/snippet}
</EntityHero>

<!-- Identifiers Section -->
<div class="section">
  <h4 class="section-title">Identifiers</h4>
  <div class="id-cloud">
    {#if series.identifiers}
      <IdentifierLinks identifiers={series.identifiers} entityType="series" />
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

<!-- Description Section -->
{#if series.description}
  <div class="section">
    <h4 class="section-title">Description</h4>
    <p class="description-text" class:expanded={descriptionExpanded}>{series.description}</p>
    <button
      type="button"
      class="expand-link"
      onclick={() => (descriptionExpanded = !descriptionExpanded)}
    >
      {descriptionExpanded ? 'Show less' : 'Show more'}
    </button>
  </div>
{/if}

<!-- Completion Row -->
<CompletionBar
  label="Completion"
  owned={seriesStats.owned}
  total={seriesStats.total}
  {wantingAll}
  onWantAllMissing={seriesStats.missing > 0 ? handleWantAllMissing : undefined}
  onUnwantAll={anyWanted ? handleUnwantAll : undefined}
/>

<!-- Auto-want Section -->
<div class="section">
  <h4 class="section-title">Auto-want</h4>
  <SearchSelect
    value={effectiveAutoWantMode}
    items={[
      { value: 'inherit', label: 'Inherit (from author)' },
      { value: 'none', label: 'None' },
      { value: 'new_only', label: 'New Only' },
      { value: 'all', label: 'All' },
      { value: 'fill_gaps', label: 'Fill Gaps' }
    ]}
    placeholder="Select mode..."
    onchange={async (mode) => {
      const validMode = mode as 'inherit' | 'none' | 'new_only' | 'all' | 'fill_gaps';
      localAutoWantMode = validMode;
      try {
        await api.updateSeriesAutoWant(series.id, validMode);
      } catch (err) {
        console.error('Failed to update series auto-want mode:', err);
        localAutoWantMode = undefined;
      }
    }}
  />
</div>

<!-- Books in Series Section -->
<div class="section">
  <h4 class="section-title">Books in Series</h4>

  {#if loadState.status === 'loading'}
    <div class="muted">Loading books...</div>
  {:else if allBooksSorted.length > 0}
    <div class="book-list">
      {#each allBooksSorted as book (book.id)}
        <BookListItem
          {book}
          {onBookClick}
          onWant={handleWantBook}
          onSkip={handleSkipBook}
          {wantingBookId}
          showSeriesIndex={true}
          showPubdate={true}
        />
      {/each}
    </div>
  {:else}
    <div class="muted">No books from this series in your library.</div>
    {#if !hasHardcoverId}
      <p class="muted link-hint">Link this series to metadata to see missing books.</p>
    {/if}
  {/if}
</div>

<style>
  .hero-author-link {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font-size: 14px;
    text-align: left;
  }

  .hero-author-link:hover {
    text-decoration: underline;
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

  /* Book list */
  .book-list {
    display: flex;
    flex-direction: column;
  }

  .muted {
    color: var(--muted);
    font-size: 13px;
  }

  .link-hint {
    font-size: 12px;
    margin-top: 8px;
    padding: 8px;
    background: var(--bg-2);
    border-radius: 8px;
    text-align: center;
  }
</style>
