<script lang="ts">
  import { SearchSelect } from '@koblibs/ui';
  import { type SeriesResult, getConfidenceColor } from './types';
  import { getProviderName, getProviderColor } from '#lib/utils/format.js';

  interface Props {
    initialSeries?: string;
    onApplySeries?: (seriesName: string, seriesIndex: number | null) => Promise<void>;
    onApplySeriesLink?: (result: SeriesResult) => Promise<void>;
  }

  let { initialSeries = '', onApplySeries, onApplySeriesLink }: Props = $props();

  // Search state
  let searchSeriesName = $state('');
  let initialSyncDone = false;
  $effect(() => {
    if (!initialSyncDone && initialSeries) {
      searchSeriesName = initialSeries;
      initialSyncDone = true;
    }
  });
  let searchProvider = $state('hardcover');
  let seriesResults = $state<SeriesResult[]>([]);
  let loading = $state(false);
  let error = $state('');
  let selectedIndex = $state<number | null>(null);
  let selectedBookIndex = $state<number | null>(null);
  let expandedSeriesIndex = $state<number | null>(null);

  const seriesProviders = [{ value: 'hardcover', label: getProviderName('hardcover') }];

  const apiBase = import.meta.env.VITE_API_BASE_URL || '';

  const searchSeries = async () => {
    if (!searchSeriesName) {
      error = 'Please enter a series name to search';
      return;
    }

    loading = true;
    error = '';
    seriesResults = [];
    selectedIndex = null;
    selectedBookIndex = null;
    expandedSeriesIndex = null;

    try {
      const params = new URLSearchParams();
      params.set('name', searchSeriesName);
      if (searchProvider) params.set('provider', searchProvider);

      const resp = await fetch(`${apiBase}/api/v1/admin/metadata/search-series?${params}`, {
        credentials: 'include'
      });

      if (!resp.ok) {
        const data = await resp.json().catch(() => ({}));
        error = data.error || 'Search failed';
        return;
      }

      const data = await resp.json();
      seriesResults = data.results || [];

      if (seriesResults.length === 0) {
        error = 'No series found';
      }
    } catch {
      error = 'Search failed';
    } finally {
      loading = false;
    }
  };

  const handleApplySeriesBook = async () => {
    if (selectedIndex === null || selectedBookIndex === null) return;
    const series = seriesResults[selectedIndex];
    const book = series.books[selectedBookIndex];

    if (onApplySeries) {
      await onApplySeries(series.name, book.series_index);
    }
  };

  const handleApplySeriesLink = async () => {
    if (selectedIndex === null || !onApplySeriesLink) return;
    const series = seriesResults[selectedIndex];
    await onApplySeriesLink(series);
  };
</script>

<div class="search-form">
  <label>
    Series Name
    <input type="text" bind:value={searchSeriesName} placeholder="e.g., The Stormlight Archive" />
  </label>
  <div class="field">
    <span class="field-label">Provider</span>
    <SearchSelect
      items={seriesProviders}
      bind:value={searchProvider}
      placeholder="Select provider"
    />
  </div>
  <button class="btn" type="button" onclick={searchSeries} disabled={loading}>
    {loading ? 'Searching...' : 'Search Series'}
  </button>
</div>

{#if error}
  <div class="error-message">{error}</div>
{/if}

{#if seriesResults.length > 0}
  <div class="results-list">
    {#each seriesResults as series, i (i)}
      <div class="series-result" class:expanded={expandedSeriesIndex === i}>
        <button
          class="series-header"
          class:selected={selectedIndex === i}
          type="button"
          onclick={() => {
            expandedSeriesIndex = expandedSeriesIndex === i ? null : i;
            selectedIndex = i;
            selectedBookIndex = null;
          }}
        >
          <div class="series-info">
            <div class="series-name">{series.name}</div>
            {#if series.authors && series.authors.length > 0}
              <div class="series-authors muted">
                by {series.authors.slice(0, 2).join(', ')}{series.authors.length > 2
                  ? ' +' + (series.authors.length - 2)
                  : ''}
              </div>
            {/if}
            <div class="series-meta muted">
              {#if series.book_count}
                <span>{series.book_count} books</span>
              {/if}
              <span class="provider-badge" style="background: {getProviderColor(series.provider)}"
                >{getProviderName(series.provider)}</span
              >
              <span class="confidence-badge {getConfidenceColor(series.confidence)}">
                {Math.round(series.confidence * 100)}%
              </span>
            </div>
          </div>
          <svg
            class="expand-icon"
            class:rotated={expandedSeriesIndex === i}
            viewBox="0 0 24 24"
            width="16"
            height="16"
            fill="currentColor"
          >
            <path d="M7 10l5 5 5-5H7z"></path>
          </svg>
        </button>

        {#if expandedSeriesIndex === i && series.books.length > 0}
          <div class="series-books">
            {#each series.books as book, bi (bi)}
              <button
                class="series-book-item"
                class:selected={selectedIndex === i && selectedBookIndex === bi}
                type="button"
                onclick={() => {
                  selectedIndex = i;
                  selectedBookIndex = bi;
                }}
              >
                {#if book.cover_url}
                  <img src={book.cover_url} alt="" class="book-cover-small" />
                {:else}
                  <div class="book-cover-small-placeholder"></div>
                {/if}
                <div class="book-info">
                  <span class="book-index">#{book.series_index ?? '?'}</span>
                  <span class="book-title">{book.title}</span>
                  {#if book.publish_date}
                    <span class="book-year muted">({book.publish_date})</span>
                  {/if}
                </div>
              </button>
            {/each}
          </div>
        {/if}
      </div>
    {/each}
  </div>

  {#if selectedIndex !== null}
    <div class="panel-actions">
      {#if onApplySeriesLink}
        <button class="btn" type="button" onclick={handleApplySeriesLink}>
          Link This Series
        </button>
      {/if}
      {#if onApplySeries && selectedBookIndex !== null}
        <button class="btn btn-secondary" type="button" onclick={handleApplySeriesBook}>
          Apply Series Info
        </button>
      {/if}
    </div>
  {/if}
{/if}

<style>
  .search-form {
    display: grid;
    gap: 12px;
  }

  .search-form label {
    display: grid;
    gap: 4px;
    font-size: 12px;
    color: var(--muted);
  }

  .field {
    display: grid;
    gap: 4px;
  }

  .field-label {
    font-size: 12px;
    color: var(--muted);
  }

  .search-form input {
    padding: 8px 10px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .error-message {
    color: var(--danger);
    font-size: 13px;
    padding: 8px 12px;
    background: rgba(220, 53, 69, 0.1);
    border-radius: 8px;
  }

  .results-list {
    display: grid;
    gap: 8px;
  }

  .series-result {
    border: 1px solid var(--ring);
    border-radius: 10px;
    overflow: hidden;
  }

  .series-result.expanded {
    border-color: var(--accent);
  }

  .series-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    padding: 12px;
    background: transparent;
    border: none;
    text-align: left;
    cursor: pointer;
    color: inherit;
    font: inherit;
    transition: background 0.15s;
  }

  .series-header:hover {
    background: rgba(211, 140, 90, 0.05);
  }

  .series-header.selected {
    background: rgba(211, 140, 90, 0.08);
  }

  .series-info {
    flex: 1;
  }

  .series-name {
    font-weight: 600;
    color: var(--ink);
  }

  .series-authors {
    font-size: 12px;
    margin-top: 2px;
  }

  .series-meta {
    display: flex;
    gap: 8px;
    margin-top: 4px;
    font-size: 11px;
  }

  .provider-badge {
    padding: 2px 6px;
    border-radius: 4px;
    color: #fff;
  }

  .confidence-badge {
    padding: 2px 6px;
    border-radius: 4px;
  }

  .confidence-badge.high {
    background: #22c55e;
    color: white;
  }

  .confidence-badge.medium {
    background: #f59e0b;
    color: white;
  }

  .confidence-badge.low {
    background: var(--ring);
  }

  .expand-icon {
    color: var(--muted);
    transition: transform 0.2s;
  }

  .expand-icon.rotated {
    transform: rotate(180deg);
  }

  .series-books {
    border-top: 1px solid var(--ring);
    max-height: 240px;
    overflow-y: auto;
  }

  .series-book-item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 12px;
    background: transparent;
    border: none;
    border-bottom: 1px solid var(--ring);
    text-align: left;
    cursor: pointer;
    color: inherit;
    font: inherit;
    transition: background 0.15s;
  }

  .series-book-item:last-child {
    border-bottom: none;
  }

  .series-book-item:hover {
    background: rgba(211, 140, 90, 0.05);
  }

  .series-book-item.selected {
    background: rgba(211, 140, 90, 0.12);
  }

  .book-cover-small {
    width: 30px;
    height: 45px;
    object-fit: cover;
    border-radius: 3px;
    flex-shrink: 0;
  }

  .book-cover-small-placeholder {
    width: 30px;
    height: 45px;
    background: var(--ring);
    border-radius: 3px;
    flex-shrink: 0;
  }

  .book-info {
    display: flex;
    align-items: baseline;
    gap: 6px;
    flex: 1;
    min-width: 0;
    font-size: 13px;
  }

  .book-index {
    color: var(--accent);
    font-weight: 600;
    flex-shrink: 0;
  }

  .book-title {
    flex: 1;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .book-year {
    flex-shrink: 0;
    font-size: 11px;
  }

  .panel-actions {
    display: flex;
    gap: 8px;
    padding-top: 8px;
    border-top: 1px solid var(--ring);
  }

  .btn {
    flex: 1;
    padding: 8px 16px;
    border-radius: 10px;
    border: none;
    background: var(--accent);
    color: white;
    font-weight: 600;
    font-size: 13px;
    cursor: pointer;
  }

  .btn-secondary {
    background: var(--ring);
    color: var(--ink);
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .muted {
    color: var(--muted);
  }
</style>
