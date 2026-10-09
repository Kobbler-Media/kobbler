<script lang="ts">
  /**
   * SeriesPage - Dumb component that displays a grid of series.
   * All sidebar/navigation logic is handled by the route page.
   */
  import { untrack } from 'svelte';
  import type { SeriesSummary, LoadState } from '$lib/types';
  import BidirectionalScroll from '$lib/components/BidirectionalScroll.svelte';

  interface Props {
    seriesList: SeriesSummary[];
    seriesNextCursor: string | null;
    seriesPrevCursor: string | null;
    seriesTotal: number;
    loadStateSeries: LoadState;
    searchQuery?: string;
    selectedId?: string | null;
    onSelectSeries: (series: SeriesSummary) => void;
    onLoadMore: () => void;
    onLoadPrevious: () => void;
    onSearch?: (query: string) => void;
    onAdd?: () => void;
  }

  let {
    seriesList,
    seriesNextCursor,
    seriesPrevCursor,
    seriesTotal,
    loadStateSeries,
    searchQuery = '',
    selectedId,
    onSelectSeries,
    onLoadMore,
    onLoadPrevious,
    onSearch,
    onAdd
  }: Props = $props();

  let localSearchQuery = $state(untrack(() => searchQuery) || '');

  // Sorted series list
  const seriesGroups = () => [...seriesList].sort((a, b) => a.name.localeCompare(b.name));

  const isLoading = () => loadStateSeries.status === 'loading';
</script>

<div class="page-content page-container">
  {#if loadStateSeries.status === 'loading' && seriesList.length === 0}
    <div class="loading-card">Loading series…</div>
  {:else if loadStateSeries.status === 'error'}
    <div class="error">{loadStateSeries.error}</div>
  {:else}
    <div class="section-header">
      <div class="header-left">
        <h2>Series</h2>
        {#if onSearch}
          <div class="search-box">
            <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor" class="search-icon">
              <path
                d="M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z"
              ></path>
            </svg>
            <input
              type="text"
              placeholder="Search series..."
              value={localSearchQuery}
              oninput={(e) => {
                localSearchQuery = (e.target as HTMLInputElement).value;
                onSearch?.(localSearchQuery);
              }}
            />
            {#if localSearchQuery}
              <button
                type="button"
                class="clear-btn"
                aria-label="Clear search"
                onclick={() => {
                  localSearchQuery = '';
                  onSearch?.('');
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
        {/if}
      </div>
      <div class="section-row">
        <span>{seriesTotal} series{searchQuery ? ` matching "${searchQuery}"` : ''}</span>
        {#if onAdd}
          <button
            class="icon-btn add-btn"
            type="button"
            title="Add series"
            aria-label="Add series"
            onclick={onAdd}
          >
            <span class="material-icons-outlined">add</span>
          </button>
        {/if}
      </div>
    </div>
    {#if seriesGroups().length === 0}
      <div class="empty-state">
        <h3>No series yet</h3>
        <p class="muted">Series will appear once books are tagged with a series.</p>
      </div>
    {:else}
      <BidirectionalScroll
        hasMore={seriesNextCursor !== null}
        hasPrevious={seriesPrevCursor !== null}
        loading={isLoading()}
        {onLoadMore}
        {onLoadPrevious}
      >
        <section class="series-grid">
          {#each seriesGroups() as group (group.id)}
            {@const totalBooks = Math.max(group.total_book_count || 0, group.book_count)}
            {@const ownedBooks = group.book_count}
            {@const progressPercent =
              totalBooks > 0 ? Math.round((ownedBooks / totalBooks) * 100) : 0}
            <button
              class:selected={selectedId === group.id}
              class="series-card"
              type="button"
              data-series-id={group.id}
              onclick={() => onSelectSeries(group)}
            >
              {#if group.is_monitored}
                <span class="monitored-indicator">
                  <span class="material-icons-outlined">star</span>
                </span>
              {/if}
              <div
                class="series-cover"
                style={group.cover_urls.length > 0
                  ? `background-image: url(${group.cover_urls[0]});`
                  : ''}
              ></div>
              <div class="series-info">
                <div class="meta-primary">{group.name}</div>
                {#if group.authors.length > 0}
                  {@const authorDisplay =
                    group.authors.length > 1
                      ? `by ${group.authors[0].name} + ${group.authors.length - 1} other${group.authors.length > 2 ? 's' : ''}`
                      : `by ${group.authors[0].name}`}
                  <div class="series-authors">{authorDisplay}</div>
                {/if}
                <div class="meta-secondary">
                  {ownedBooks} of {totalBooks} books owned
                </div>
                <div class="progress-track">
                  <div class="progress-fill" style="width: {progressPercent}%"></div>
                </div>
              </div>
            </button>
          {/each}
        </section>
      </BidirectionalScroll>
    {/if}
  {/if}
</div>

<style>
  .page-content {
    flex: 1;
    min-width: 0;
  }

  .section-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
    margin-bottom: 16px;
  }

  .section-header h2 {
    font-size: 18px;
    margin: 0;
  }

  .header-left {
    display: flex;
    align-items: center;
    gap: 16px;
  }

  .section-row {
    display: flex;
    align-items: center;
    gap: 12px;
    color: var(--muted);
    font-size: 14px;
  }

  .add-btn {
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    padding: 0;
    transition: all 0.15s;
  }

  .add-btn:hover {
    background: var(--bg-2);
    color: var(--ink);
  }

  .add-btn .material-icons-outlined {
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

  .search-icon {
    color: var(--muted);
    flex-shrink: 0;
  }

  .search-box input {
    flex: 1;
    border: none;
    background: transparent;
    color: var(--ink);
    font-size: 13px;
    outline: none;
    min-width: 0;
  }

  .search-box input::placeholder {
    color: var(--muted);
  }

  .clear-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    background: none;
    border: none;
    padding: 2px;
    cursor: pointer;
    color: var(--muted);
    border-radius: 4px;
  }

  .clear-btn:hover {
    background: var(--ring);
    color: var(--ink);
  }

  /* Series Grid */
  .series-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(340px, 1fr));
    gap: 16px;
  }

  .series-card {
    appearance: none;
    text-align: left;
    border-radius: 16px;
    padding: 16px;
    border: 1px solid var(--ring);
    background: var(--card);
    display: flex;
    gap: 16px;
    align-items: flex-start;
    cursor: pointer;
    color: inherit;
    font: inherit;
    position: relative;
    transition: border-color 0.15s;
  }

  .series-card:hover {
    border-color: var(--muted);
  }

  .series-card.selected {
    border-color: var(--accent);
  }

  .series-cover {
    width: 80px;
    height: 120px;
    flex-shrink: 0;
    border-radius: 8px;
    background-size: cover;
    background-position: center;
    background-color: var(--bg-2);
  }

  .series-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  .meta-primary {
    font-weight: 700;
    font-size: 16px;
    line-height: 1.3;
    word-break: break-word;
  }

  .series-authors {
    font-size: 12px;
    color: var(--muted);
  }

  .meta-secondary {
    font-size: 12px;
    color: var(--muted);
    margin-top: 8px;
  }

  .progress-track {
    height: 6px;
    background: var(--ring);
    border-radius: 3px;
    margin-top: 12px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 3px;
  }

  .monitored-indicator {
    position: absolute;
    top: 12px;
    right: 12px;
    color: var(--monitored);
  }

  .monitored-indicator .material-icons-outlined {
    font-size: 18px;
  }

  .loading-card {
    padding: 24px;
    text-align: center;
    color: var(--muted);
  }

  .error {
    background: rgba(220, 53, 69, 0.15);
    border: 1px solid rgba(220, 53, 69, 0.3);
    border-radius: 8px;
    padding: 12px;
    margin-bottom: 16px;
    font-size: 13px;
  }

  .empty-state {
    text-align: center;
    padding: 48px 24px;
    color: var(--muted);
  }

  .empty-state h3 {
    margin: 0 0 8px;
    font-size: 16px;
    color: var(--ink);
  }

  .empty-state p {
    margin: 0;
    font-size: 14px;
  }

  @media (max-width: 768px) {
    .series-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
