<script lang="ts">
  import { untrack } from 'svelte';
  import type { Book, SeriesSummary } from '$lib/types';
  import type { MetadataResult } from '$lib/api/client';
  import { SearchSelect, type SearchSelectItem } from '@koblibs/ui';
  import CoverSelector from './CoverSelector.svelte';
  import { getProviderName, getProviderColor } from '$lib/utils/format';

  type Mode = 'add' | 'edit';

  interface Props {
    book?: Book | null;
    mode: Mode;
    seriesList: SeriesSummary[];
    loading?: boolean;
    onSave: (data: EditData, coverFile?: File) => Promise<void>;
    onClose: () => void;
    onShowMessage?: (message: string) => void;
  }

  export interface EditData {
    title: string;
    authors: string[];
    series_name: string | null;
    series_id: string | null;
    series_index: number | null;
    genres: string[];
    language: string | null;
    publisher: string | null;
    pubdate: string | null;
    page_count: number | null;
    isbn: string | null;
    rating: number | null;
    description: string | null;
    cover_url: string | null;
    identifiers: Record<string, string>;
    series_memberships?: import('$lib/types').SeriesMembership[];
  }

  let {
    book,
    mode,
    seriesList,
    loading = false,
    onSave,
    onClose,
    onShowMessage: _onShowMessage
  }: Props = $props();

  // Helper functions need to be defined before use
  function formatDateInput(value: string | null): string {
    if (!value) return '';
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return '';
    return date.toISOString().slice(0, 10);
  }

  function getIsbn(identifiers: Record<string, string>): string {
    return identifiers['isbn13'] || identifiers['isbn10'] || identifiers['isbn'] || '';
  }

  // Use untrack to capture initial values without reactive tracking
  // (component remounts when modal reopens, so initial values are always fresh)
  let title = $state(untrack(() => book?.title ?? ''));
  let authors = $state(untrack(() => book?.authors.join(', ') ?? ''));
  let seriesName = $state(untrack(() => book?.series_name ?? ''));
  let seriesIndex = $state(
    untrack(() => {
      const idx = book?.series_index;
      return idx ? String(idx) : '';
    })
  );
  let genres = $state(untrack(() => book?.genres?.join(', ') ?? ''));
  let language = $state(untrack(() => book?.language ?? ''));
  let publisher = $state(untrack(() => book?.publisher ?? ''));
  let pubdate = $state(untrack(() => formatDateInput(book?.pubdate ?? null)));
  let pageCount = $state(untrack(() => (book?.page_count ? String(book.page_count) : '')));
  let isbn = $state(untrack(() => getIsbn(book?.identifiers ?? {})));
  let rating = $state(untrack(() => (book?.rating ? String(book.rating) : '')));
  let description = $state(untrack(() => book?.description ?? ''));
  let coverUrl = $state<string | null>(untrack(() => book?.cover_url ?? null));
  let coverFile = $state<File | undefined>(undefined);

  // Search state
  let searchExpanded = $state(untrack(() => mode === 'add'));
  let searchIsbn = $state('');
  let searchTitle = $state(untrack(() => book?.title ?? ''));
  let searchAuthor = $state(untrack(() => book?.authors[0]?.name ?? ''));
  let searchProvider = $state('hardcover');
  let searchResults = $state<MetadataResult[]>([]);
  let searchLoading = $state(false);
  let searchError = $state('');
  let selectedResultIndex = $state<number | null>(null);

  // Multi-select for merge
  let multiSelectMode = $state(false);
  let selectedForMerge = $state<number[]>([]);

  // Search covers from results
  let searchCovers = $derived(
    searchResults
      .filter((r) => r.cover_url)
      .map((r) => ({ url: r.cover_url!, provider: r.provider }))
  );

  const apiBase = import.meta.env.VITE_API_BASE_URL || '';

  const providers = [
    { value: '', label: 'All Providers' },
    { value: 'hardcover', label: getProviderName('hardcover') },
    { value: 'googlebooks', label: getProviderName('googlebooks') },
    { value: 'openlibrary', label: getProviderName('openlibrary') }
  ];

  const seriesOptions = (): SearchSelectItem[] =>
    seriesList.map((s) => ({ value: s.name, label: s.name }));

  const handleSearch = async () => {
    if (!searchIsbn && !searchTitle) {
      searchError = 'Please enter an ISBN or title to search';
      return;
    }

    searchLoading = true;
    searchError = '';
    searchResults = [];
    selectedResultIndex = null;
    selectedForMerge = [];

    try {
      const params = new URLSearchParams();
      if (searchIsbn) params.set('isbn', searchIsbn);
      if (searchTitle) params.set('title', searchTitle);
      if (searchAuthor) params.set('author', searchAuthor);
      if (searchProvider) params.set('provider', searchProvider);

      const resp = await fetch(`${apiBase}/api/v1/admin/metadata/search?${params}`, {
        credentials: 'include'
      });

      if (!resp.ok) {
        searchError = 'Search failed';
        return;
      }

      const data = await resp.json();
      searchResults = data.results || [];

      if (searchResults.length === 0) {
        searchError = 'No results found';
      }
    } catch {
      searchError = 'Search failed';
    } finally {
      searchLoading = false;
    }
  };

  const applyResult = (result: MetadataResult) => {
    title = result.title;
    authors = result.authors.join(', ');
    if (result.series) seriesName = result.series;
    if (result.series_index) seriesIndex = String(result.series_index);
    if (result.publisher) publisher = result.publisher;
    if (result.publish_date) pubdate = formatDateInput(result.publish_date);
    if (result.description) description = result.description;
    if (result.page_count) pageCount = String(result.page_count);
    if (result.isbn) isbn = result.isbn;
    if (result.rating) rating = String(result.rating);
    if (result.subjects && result.subjects.length > 0) {
      genres = result.subjects.join(', ');
    }
    if (result.cover_url) coverUrl = result.cover_url;
  };

  const handleApplySelected = () => {
    if (selectedResultIndex !== null) {
      applyResult(searchResults[selectedResultIndex]);
      searchExpanded = false;
    }
  };

  const toggleMergeSelection = (index: number) => {
    const pos = selectedForMerge.indexOf(index);
    if (pos >= 0) {
      selectedForMerge = selectedForMerge.filter((i) => i !== index);
    } else {
      selectedForMerge = [...selectedForMerge, index];
    }
  };

  const handleApplyMerged = async () => {
    if (selectedForMerge.length < 2) return;

    // Build merged result (first selected = highest priority)
    const merged: Partial<MetadataResult> = {
      title: '',
      authors: [],
      subjects: [],
      identifiers: {}
    };

    // For each field, take the first non-empty value from selected results
    for (const idx of selectedForMerge) {
      const result = searchResults[idx];
      if (!merged.title && result.title) merged.title = result.title;
      if (!merged.authors?.length && result.authors.length) merged.authors = result.authors;
      if (!merged.series && result.series) merged.series = result.series;
      if (!merged.series_index && result.series_index) merged.series_index = result.series_index;
      if (!merged.publisher && result.publisher) merged.publisher = result.publisher;
      if (!merged.publish_date && result.publish_date) merged.publish_date = result.publish_date;
      if (!merged.description && result.description) merged.description = result.description;
      if (!merged.page_count && result.page_count) merged.page_count = result.page_count;
      if (!merged.isbn && result.isbn) merged.isbn = result.isbn;
      if (!merged.rating && result.rating) merged.rating = result.rating;
      if (!merged.cover_url && result.cover_url) merged.cover_url = result.cover_url;

      // Merge subjects (union)
      for (const subject of result.subjects || []) {
        if (!merged.subjects!.includes(subject)) {
          merged.subjects!.push(subject);
        }
      }
    }

    applyResult(merged as MetadataResult);
    searchExpanded = false;
    multiSelectMode = false;
    selectedForMerge = [];
  };

  const handleCoverSelect = (url: string | null, file?: File) => {
    coverUrl = url;
    coverFile = file;
  };

  const handleSave = async () => {
    const authorList = authors
      .split(',')
      .map((a) => a.trim())
      .filter(Boolean);
    const genreList = genres
      .split(',')
      .map((g) => g.trim())
      .filter(Boolean);
    const indexValue = seriesIndex.trim() ? Number(seriesIndex) : null;

    // Find matching series
    const seriesMatch = seriesList.find((s) => s.name === seriesName.trim());

    const identifiers: Record<string, string> = { ...(book?.identifiers || {}) };
    if (isbn) {
      const key = isbn.replace(/[^0-9X]/gi, '').length === 13 ? 'isbn13' : 'isbn10';
      identifiers[key] = isbn;
    }

    const data: EditData = {
      title,
      authors: authorList,
      series_name: seriesName.trim() || null,
      series_id: seriesMatch?.id || null,
      series_index: indexValue,
      genres: genreList,
      language: language.trim() || null,
      publisher: publisher.trim() || null,
      pubdate: pubdate ? new Date(pubdate).toISOString() : null,
      page_count: pageCount ? Number(pageCount) : null,
      isbn: isbn.trim() || null,
      rating: rating ? Number(rating) : null,
      description: description.trim() || null,
      cover_url: coverUrl,
      identifiers
    };

    await onSave(data, coverFile);
  };

  const getConfidenceColor = (confidence: number) => {
    if (confidence >= 0.8) return 'high';
    if (confidence >= 0.5) return 'medium';
    return 'low';
  };
</script>

<button class="modal-backdrop" type="button" aria-label="Close" onclick={onClose}></button>

<div class="modal edit-modal">
  <div class="modal-header">
    <h3>{mode === 'add' ? 'Add Book' : 'Edit Book Metadata'}</h3>
    <button class="icon-btn" type="button" aria-label="Close" onclick={onClose}>
      <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor">
        <path d="M6.4 5.6 5 7l5 5-5 5 1.4 1.4 5-5 5 5L17.8 17l-5-5 5-5-1.4-1.4-5 5-5-5z"></path>
      </svg>
    </button>
  </div>

  <div class="modal-content">
    <!-- Search Section (collapsible) -->
    <div class="search-section">
      <button
        type="button"
        class="section-toggle"
        onclick={() => (searchExpanded = !searchExpanded)}
      >
        <svg
          viewBox="0 0 24 24"
          width="12"
          height="12"
          fill="currentColor"
          class:rotated={searchExpanded}
        >
          <path d="M10 6L8.59 7.41 13.17 12l-4.58 4.59L10 18l6-6z"></path>
        </svg>
        Search Metadata
      </button>

      {#if searchExpanded}
        <div class="search-form">
          <div class="search-fields-grid">
            <label class="search-field">
              <span>ISBN</span>
              <input type="text" placeholder="978-0-123456-78-9" bind:value={searchIsbn} />
            </label>
            <label class="search-field">
              <span>Title</span>
              <input type="text" placeholder="Book title" bind:value={searchTitle} />
            </label>
            <label class="search-field">
              <span>Author <span class="optional">(optional)</span></span>
              <input type="text" placeholder="Author name" bind:value={searchAuthor} />
            </label>
          </div>
          <div class="search-actions-row">
            <div class="provider-field">
              <SearchSelect items={providers} bind:value={searchProvider} placeholder="Provider" />
            </div>
            <button
              class="btn search-btn"
              type="button"
              onclick={handleSearch}
              disabled={searchLoading}
            >
              {searchLoading ? 'Searching...' : 'Search'}
            </button>
          </div>

          {#if searchError}
            <div class="search-error">{searchError}</div>
          {/if}

          {#if searchResults.length > 0}
            <div class="search-results">
              <div class="results-header">
                <span class="results-count">{searchResults.length} results</span>
                <button
                  type="button"
                  class="merge-toggle"
                  class:active={multiSelectMode}
                  onclick={() => {
                    multiSelectMode = !multiSelectMode;
                    selectedForMerge = [];
                  }}
                >
                  {multiSelectMode ? 'Cancel Merge' : 'Merge Multiple'}
                </button>
              </div>

              {#each searchResults as result, i (i)}
                <button
                  type="button"
                  class="result-item"
                  class:selected={multiSelectMode
                    ? selectedForMerge.includes(i)
                    : selectedResultIndex === i}
                  onclick={() => {
                    if (multiSelectMode) {
                      toggleMergeSelection(i);
                    } else {
                      selectedResultIndex = i;
                    }
                  }}
                >
                  {#if multiSelectMode}
                    <div class="merge-checkbox" class:checked={selectedForMerge.includes(i)}>
                      {#if selectedForMerge.includes(i)}
                        <span class="priority">{selectedForMerge.indexOf(i) + 1}</span>
                      {/if}
                    </div>
                  {/if}
                  {#if result.cover_url}
                    <img src={result.cover_url} alt="" class="result-cover" />
                  {:else}
                    <div class="result-cover-placeholder"></div>
                  {/if}
                  <div class="result-info">
                    <div class="result-title">{result.title}</div>
                    <div class="result-authors">{result.authors.join(', ')}</div>
                    <div class="result-meta">
                      <span
                        class="provider-badge"
                        style="background: {getProviderColor(result.provider)}"
                        >{getProviderName(result.provider)}</span
                      >
                      <span class="confidence-badge {getConfidenceColor(result.confidence)}">
                        {Math.round(result.confidence * 100)}%
                      </span>
                    </div>
                  </div>
                </button>
              {/each}

              <div class="results-actions">
                {#if multiSelectMode}
                  <button
                    class="btn"
                    type="button"
                    onclick={handleApplyMerged}
                    disabled={selectedForMerge.length < 2}
                  >
                    Apply Merged ({selectedForMerge.length})
                  </button>
                {:else}
                  <button
                    class="btn"
                    type="button"
                    onclick={handleApplySelected}
                    disabled={selectedResultIndex === null}
                  >
                    Apply Selected
                  </button>
                {/if}
              </div>
            </div>
          {/if}
        </div>
      {/if}
    </div>

    <!-- Cover Selection -->
    <div class="form-section">
      <span class="section-label">Cover</span>
      <CoverSelector
        currentUrl={book?.cover_url}
        {searchCovers}
        selectedUrl={coverUrl}
        onSelect={handleCoverSelect}
      />
    </div>

    <!-- Editable Fields -->
    <div class="form-fields">
      <label class="form-field">
        <span>Title</span>
        <input type="text" bind:value={title} placeholder="Book title" required />
      </label>

      <label class="form-field">
        <span>Authors</span>
        <input type="text" bind:value={authors} placeholder="Comma separated" />
      </label>

      <div class="form-row">
        <label class="form-field flex-2">
          <span>Series</span>
          <SearchSelect
            items={seriesOptions()}
            bind:value={seriesName}
            placeholder="Series name"
            allowCustom
          />
        </label>
        <label class="form-field flex-1">
          <span>Index</span>
          <input type="text" bind:value={seriesIndex} placeholder="1, 1.5..." />
        </label>
      </div>

      <label class="form-field">
        <span>Genres</span>
        <input type="text" bind:value={genres} placeholder="Comma separated" />
      </label>

      <div class="form-row">
        <label class="form-field">
          <span>Language</span>
          <input type="text" bind:value={language} placeholder="en" />
        </label>
        <label class="form-field">
          <span>Page Count</span>
          <input type="number" bind:value={pageCount} placeholder="350" />
        </label>
      </div>

      <label class="form-field">
        <span>Publisher</span>
        <input type="text" bind:value={publisher} placeholder="Publisher name" />
      </label>

      <label class="form-field">
        <span>Publication Date</span>
        <input type="date" bind:value={pubdate} />
      </label>

      <label class="form-field">
        <span>ISBN</span>
        <input type="text" bind:value={isbn} placeholder="978-0-123456-78-9" />
      </label>

      <label class="form-field">
        <span>Rating (0-5)</span>
        <input type="number" min="0" max="5" step="0.1" bind:value={rating} placeholder="4.2" />
      </label>

      <label class="form-field">
        <span>Description</span>
        <textarea rows="4" bind:value={description} placeholder="Book description..."></textarea>
      </label>
    </div>
  </div>

  <div class="modal-footer">
    <button class="btn alt" type="button" onclick={onClose} disabled={loading}> Cancel </button>
    <button class="btn" type="button" onclick={handleSave} disabled={loading}>
      {loading ? 'Saving...' : 'Save Changes'}
    </button>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    z-index: 100;
    border: none;
    cursor: pointer;
  }

  .modal {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    z-index: 101;
    display: flex;
    flex-direction: column;
    max-height: 90vh;
  }

  .edit-modal {
    width: min(90vw, 560px);
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--ring);
    flex-shrink: 0;
  }

  .modal-header h3 {
    margin: 0;
    font-size: 16px;
  }

  .icon-btn {
    background: transparent;
    border: none;
    padding: 6px;
    border-radius: 6px;
    cursor: pointer;
    color: var(--muted);
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }

  .icon-btn:hover {
    background: var(--ring);
    color: var(--ink);
  }

  .modal-content {
    padding: 20px;
    overflow-y: auto;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 20px;
  }

  .modal-footer {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    padding: 16px 20px;
    border-top: 1px solid var(--ring);
    flex-shrink: 0;
  }

  /* Search section */
  .search-section {
    background: var(--bg);
    border-radius: 12px;
    padding: 12px;
  }

  .section-toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: none;
    border: none;
    color: var(--accent);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    padding: 0;
  }

  .section-toggle:hover {
    text-decoration: underline;
  }

  .section-toggle svg {
    transition: transform 0.2s ease;
  }

  .section-toggle svg.rotated {
    transform: rotate(90deg);
  }

  .search-form {
    margin-top: 12px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .search-fields-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 10px;
  }

  .search-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .search-field span {
    font-size: 12px;
    color: var(--muted);
  }

  .search-field .optional {
    opacity: 0.6;
    font-size: 10px;
  }

  .search-field input {
    padding: 8px 10px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .search-actions-row {
    display: flex;
    gap: 8px;
    align-items: flex-end;
  }

  .provider-field {
    flex: 1;
    max-width: 180px;
  }

  .search-btn {
    flex-shrink: 0;
  }

  .search-error {
    color: var(--danger, #dc3545);
    font-size: 12px;
    padding: 8px;
    background: rgba(220, 53, 69, 0.1);
    border-radius: 6px;
  }

  .search-results {
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-height: 300px;
    overflow-y: auto;
  }

  .results-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 4px 0;
  }

  .results-count {
    font-size: 12px;
    color: var(--muted);
  }

  .merge-toggle {
    background: none;
    border: 1px solid var(--ring);
    padding: 4px 10px;
    border-radius: 6px;
    font-size: 11px;
    color: var(--muted);
    cursor: pointer;
  }

  .merge-toggle:hover {
    border-color: var(--accent);
    color: var(--accent);
  }

  .merge-toggle.active {
    background: var(--accent);
    border-color: var(--accent);
    color: white;
  }

  .result-item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    cursor: pointer;
    text-align: left;
    color: inherit;
    font: inherit;
    transition: all 0.15s ease;
  }

  .result-item:hover {
    border-color: var(--accent);
    background: rgba(211, 140, 90, 0.04);
  }

  .result-item.selected {
    border-color: var(--accent);
    background: rgba(211, 140, 90, 0.08);
  }

  .merge-checkbox {
    width: 22px;
    height: 22px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 2px solid var(--ring);
    border-radius: 6px;
    flex-shrink: 0;
  }

  .merge-checkbox.checked {
    background: var(--accent);
    border-color: var(--accent);
    color: white;
  }

  .priority {
    font-size: 12px;
    font-weight: 700;
  }

  .result-cover {
    width: 40px;
    height: 56px;
    object-fit: cover;
    border-radius: 4px;
    flex-shrink: 0;
  }

  .result-cover-placeholder {
    width: 40px;
    height: 56px;
    background: var(--ring);
    border-radius: 4px;
    flex-shrink: 0;
  }

  .result-info {
    flex: 1;
    min-width: 0;
  }

  .result-title {
    font-weight: 500;
    font-size: 13px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .result-authors {
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .result-meta {
    display: flex;
    gap: 6px;
    margin-top: 4px;
  }

  .provider-badge {
    padding: 2px 6px;
    border-radius: 4px;
    font-size: 10px;
    color: #fff;
  }

  .confidence-badge {
    padding: 2px 6px;
    border-radius: 4px;
    font-size: 10px;
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

  .results-actions {
    padding-top: 8px;
    border-top: 1px solid var(--ring);
  }

  /* Form section */
  .form-section {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .section-label {
    font-size: 12px;
    font-weight: 500;
    color: var(--muted);
  }

  /* Form fields */
  .form-fields {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .form-field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .form-field span {
    font-size: 12px;
    color: var(--muted);
  }

  .form-field input,
  .form-field textarea {
    padding: 10px 12px;
    border: 1px solid var(--ring);
    border-radius: 10px;
    background: transparent;
    color: var(--ink);
    font-size: 13px;
    resize: vertical;
  }

  .form-row {
    display: flex;
    gap: 12px;
  }

  .form-row .form-field {
    flex: 1;
  }

  .form-row .form-field.flex-2 {
    flex: 2;
  }

  .form-row .form-field.flex-1 {
    flex: 1;
  }

  .btn {
    padding: 10px 20px;
    border-radius: 10px;
    border: none;
    background: var(--accent);
    color: #1d130c;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .btn:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
  }

  .btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn.alt {
    background: var(--card);
    border: 1px solid var(--ring);
    color: var(--ink);
  }

  .btn.alt:hover:not(:disabled) {
    background: var(--ring);
  }

  /* Mobile responsive */
  @media (max-width: 600px) {
    .edit-modal {
      width: calc(100vw - 24px);
      max-height: calc(100vh - 24px);
    }

    .search-fields-grid {
      grid-template-columns: 1fr 1fr;
    }

    .search-fields-grid .search-field:last-child {
      grid-column: span 2;
    }

    .form-row {
      flex-direction: column;
      gap: 14px;
    }
  }

  @media (max-width: 400px) {
    .search-fields-grid {
      grid-template-columns: 1fr;
    }

    .search-fields-grid .search-field:last-child {
      grid-column: span 1;
    }
  }
</style>
