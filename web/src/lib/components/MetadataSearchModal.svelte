<script lang="ts">
  /**
   * MetadataSearchModal - Unified metadata search modal for books, authors, and series.
   * Supports dynamic provider selection, multi-select results, and merge preview.
   */
  import type { MetadataResult } from '#lib/api/client.js';
  import type { AuthorSearchResult, SeriesSearchResult } from '#lib/types.js';
  import {
    getMetadataProviders,
    searchAuthors,
    searchSeries,
    mergeMetadata
  } from '#lib/api/client.js';
  import { getProviderCode, getProviderColor } from '#lib/utils/format.js';
  import MetaLine from '#lib/components/shared/MetaLine.svelte';

  type SearchMode = 'book' | 'author' | 'series';

  // Provider info with priority from backend
  export interface ProviderInfo {
    id: string;
    name: string;
    code: string;
    color: string;
    priority: number;
  }

  // Saved state for restoring when returning from edit
  export interface SearchSavedState {
    searchTitle: string;
    searchAuthor: string;
    searchIsbn: string;
    bookResults: MetadataResult[];
    authorResults: AuthorSearchResult[];
    seriesResults: SeriesSearchResult[];
    selectedIndices: number[];
    hasSearched: boolean;
    providers: ProviderInfo[];
  }

  interface Props {
    mode: SearchMode;
    initialValues?: { title?: string; author?: string; isbn?: string; name?: string };
    savedState?: SearchSavedState;
    /** When true, skips rendering modal wrapper - content only for embedding in another modal */
    embedded?: boolean;
    /** When true, hides the "Manual" button - useful for link mode where manual entry isn't applicable */
    hideManual?: boolean;
    onSelect: (
      results: MetadataResult[] | AuthorSearchResult[] | SeriesSearchResult[],
      providers: ProviderInfo[],
      merged?: MetadataResult
    ) => void;
    onSkip: () => void;
    onCancel: () => void;
  }

  let {
    mode,
    initialValues = {},
    savedState,
    embedded = false,
    hideManual = false,
    onSelect,
    onSkip,
    onCancel
  }: Props = $props();

  // Provider state (includes priority from backend for sorting)
  let providers = $state<
    {
      id: string;
      name: string;
      code: string;
      color: string;
      priority: number;
      enabled: boolean;
      supportsMode: boolean;
    }[]
  >([]);
  let loadingProviders = $state(true);

  // Search state - restore from savedState if available
  let searchTitle = $state('');
  let searchAuthor = $state('');
  let searchIsbn = $state('');
  let loading = $state(false);

  // Sync state with props
  $effect(() => {
    // If we have saved state, it takes precedence
    if (savedState) {
      searchTitle = savedState.searchTitle;
      searchAuthor = savedState.searchAuthor;
      searchIsbn = savedState.searchIsbn;
    } else {
      // Otherwise sync with initialValues if they change
      searchTitle = initialValues.title || initialValues.name || '';
      searchAuthor = initialValues.author || '';
      searchIsbn = initialValues.isbn || '';
    }
  });

  // Results state - restore from savedState if available
  let bookResults = $state<MetadataResult[]>([]);
  let authorResults = $state<AuthorSearchResult[]>([]);
  let seriesResults = $state<SeriesSearchResult[]>([]);
  let selectedIndices = $state<Set<number>>(new Set());
  let hasSearched = $state(false);

  $effect(() => {
    if (savedState) {
      bookResults = savedState.bookResults;
      authorResults = savedState.authorResults;
      seriesResults = savedState.seriesResults;
      selectedIndices = new Set(savedState.selectedIndices);
      hasSearched = savedState.hasSearched;
    }
  });

  // Function to get current state for saving
  export function getState(): SearchSavedState {
    return {
      searchTitle,
      searchAuthor,
      searchIsbn,
      bookResults,
      authorResults,
      seriesResults,
      selectedIndices: Array.from(selectedIndices),
      hasSearched,
      providers: providers.map(({ id, name, code, color, priority }) => ({
        id,
        name,
        code,
        color,
        priority
      }))
    };
  }

  const apiBase = import.meta.env.VITE_API_BASE_URL || '';

  // Load providers on mount
  $effect(() => {
    loadProviders();
  });

  async function loadProviders() {
    loadingProviders = true;
    try {
      const data = await getMetadataProviders();
      providers = data.map((p) => {
        // Determine if provider supports current mode
        const supportsMode =
          mode === 'book' ||
          (mode === 'author' && p.capabilities.includes('author_search' as any)) ||
          (mode === 'series' && p.capabilities.includes('series_info' as any));

        return {
          id: p.provider_type,
          name: p.display_name,
          code: getProviderCode(p.provider_type),
          color: getProviderColor(p.provider_type),
          priority: p.priority,
          enabled: p.enabled && supportsMode,
          supportsMode
        };
      });
    } catch (e) {
      console.error('Failed to load providers:', e);
      // Fallback providers (priority matches migration 0027)
      const fallbackIds = ['hardcover', 'googlebooks', 'openlibrary'] as const;
      const fallbackPriorities: Record<string, number> = {
        hardcover: 10,
        googlebooks: 20,
        openlibrary: 30
      };
      providers = fallbackIds.map((id) => ({
        id,
        name:
          id === 'hardcover' ? 'Hardcover' : id === 'googlebooks' ? 'Google Books' : 'Open Library',
        code: getProviderCode(id),
        color: getProviderColor(id),
        priority: fallbackPriorities[id],
        enabled: id === 'hardcover' || (mode === 'book' && id === 'googlebooks'),
        supportsMode: id === 'hardcover' || mode === 'book'
      }));
    } finally {
      loadingProviders = false;
    }
  }

  async function search() {
    if (!searchTitle.trim() && !searchIsbn.trim()) return;

    loading = true;
    hasSearched = true;
    bookResults = [];
    authorResults = [];
    seriesResults = [];
    selectedIndices = new Set();

    try {
      const enabledProviders = providers.filter((p) => p.enabled && p.supportsMode);

      if (mode === 'book') {
        await searchBooks(enabledProviders);
      } else if (mode === 'author') {
        await searchAuthorsMode(enabledProviders);
      } else if (mode === 'series') {
        await searchSeriesMode(enabledProviders);
      }
    } finally {
      loading = false;
    }
  }

  async function searchBooks(enabledProviders: typeof providers) {
    const allResults: MetadataResult[] = [];

    for (const provider of enabledProviders) {
      const params = new URLSearchParams();
      if (searchTitle.trim()) params.set('title', searchTitle.trim());
      if (searchAuthor.trim()) params.set('author', searchAuthor.trim());
      if (searchIsbn.trim()) params.set('isbn', searchIsbn.trim());
      params.set('provider', provider.id);

      try {
        const resp = await fetch(`${apiBase}/api/v1/admin/metadata/search?${params}`, {
          credentials: 'include'
        });

        if (resp.ok) {
          const data = await resp.json();
          allResults.push(...(data.results || []));
        }
      } catch {
        // Continue with other providers
      }
    }

    bookResults = allResults;
  }

  async function searchAuthorsMode(_enabledProviders: typeof providers) {
    // Only Hardcover supports author search currently
    try {
      const resp = await searchAuthors(searchTitle.trim());
      authorResults = resp.results || [];
    } catch (e) {
      console.error('Author search failed:', e);
    }
  }

  async function searchSeriesMode(_enabledProviders: typeof providers) {
    // Only Hardcover supports series search currently
    try {
      const resp = await searchSeries(searchTitle.trim());
      seriesResults = resp.results || [];
    } catch (e) {
      console.error('Series search failed:', e);
    }
  }

  function toggleResult(index: number) {
    const newSelected = new Set(selectedIndices);
    if (newSelected.has(index)) {
      newSelected.delete(index);
    } else {
      newSelected.add(index);
    }
    selectedIndices = newSelected;
  }

  // Alias for non-embedded mode
  const toggleSelection = toggleResult;

  function toggleProvider(providerId: string) {
    providers = providers.map((p) => (p.id === providerId ? { ...p, enabled: !p.enabled } : p));
  }

  async function handleUseSelected() {
    // Extract provider info for passing to form
    const providerInfo = providers.map(({ id, name, code, color, priority }) => ({
      id,
      name,
      code,
      color,
      priority
    }));

    if (mode === 'book' && bookResults.length > 0) {
      const selected = Array.from(selectedIndices).map((i) => bookResults[i]);

      // Merge if multiple selected
      if (selected.length > 1) {
        try {
          const merged = await mergeMetadata(selected);
          onSelect(selected, providerInfo, merged.merged);
        } catch {
          onSelect(selected, providerInfo);
        }
      } else {
        onSelect(selected, providerInfo);
      }
    } else if (mode === 'author' && authorResults.length > 0) {
      const selected = Array.from(selectedIndices).map((i) => authorResults[i]);
      onSelect(selected as any, providerInfo);
    } else if (mode === 'series' && seriesResults.length > 0) {
      const selected = Array.from(selectedIndices).map((i) => seriesResults[i]);
      onSelect(selected as any, providerInfo);
    }
  }

  function getSearchLabel(): string {
    switch (mode) {
      case 'book':
        return 'Search for book';
      case 'author':
        return 'Search for author';
      case 'series':
        return 'Search for series';
    }
  }

  function getPlaceholder(): string {
    switch (mode) {
      case 'book':
        return 'Enter book title...';
      case 'author':
        return 'Enter author name...';
      case 'series':
        return 'Enter series name...';
    }
  }

  function getModalTitle(): string {
    switch (mode) {
      case 'book':
        return 'Search Metadata';
      case 'author':
        return 'Search Authors';
      case 'series':
        return 'Search Series';
    }
  }

  $effect(() => {
    // Trigger search on Enter key
  });
</script>

{#if embedded}
  <!-- Embedded mode: content only, no modal wrapper -->
  <div class="embedded-content">
    <div class="embedded-body">
      <!-- Search fields -->
      <div class="search-section">
        <div class="label">{getSearchLabel()}</div>
        <div class="search-fields">
          <div class="search-row">
            <input
              type="text"
              class="search-input"
              bind:value={searchTitle}
              placeholder={getPlaceholder()}
              onkeydown={(e) => e.key === 'Enter' && search()}
            />
          </div>

          {#if mode === 'book'}
            <div class="search-row secondary">
              <input
                type="text"
                class="search-input"
                bind:value={searchAuthor}
                placeholder="Author (optional)..."
                onkeydown={(e) => e.key === 'Enter' && search()}
              />
              <input
                type="text"
                class="search-input isbn"
                bind:value={searchIsbn}
                placeholder="ISBN..."
                onkeydown={(e) => e.key === 'Enter' && search()}
              />
            </div>
          {/if}

          <button
            type="button"
            class="btn-primary search-btn"
            onclick={search}
            disabled={loading || loadingProviders}
          >
            {loading ? 'Searching...' : 'Search'}
          </button>
        </div>
      </div>

      <!-- Provider checkboxes (only for book mode with multiple providers) -->
      {#if mode === 'book' && providers.filter((p) => p.supportsMode).length > 1}
        <div class="providers-section">
          <div class="label">Sources</div>
          <div class="providers-row">
            {#each providers.filter((p) => p.supportsMode) as provider (provider.id)}
              <label class="provider-checkbox">
                <input type="checkbox" bind:checked={provider.enabled} />
                <span class="source-tag" style="background: {provider.color}">{provider.code}</span>
                {provider.name}
              </label>
            {/each}
          </div>
        </div>
      {/if}

      <!-- Results -->
      {#if mode === 'book' && bookResults.length > 0}
        <div class="results-section">
          <div class="label">Results ({bookResults.length})</div>
          <div class="results-list">
            {#each bookResults as result, i (i)}
              <button
                type="button"
                class="search-result"
                class:selected={selectedIndices.has(i)}
                onclick={() => toggleResult(i)}
              >
                <div
                  class="thumb"
                  style="background-image: url('{result.cover_url || '/placeholder-cover.png'}')"
                ></div>
                <div class="search-result-info">
                  <div class="search-result-title">{result.title}</div>
                  <div class="search-result-meta">
                    <span class="source-tag" style="background: {getProviderColor(result.provider)}"
                      >{getProviderCode(result.provider)}</span
                    >
                    <MetaLine
                      items={[
                        result.authors?.join(', ') || 'Unknown Author',
                        result.publish_date?.substring(0, 4),
                        result.series && result.series_index
                          ? `${result.series} #${result.series_index}`
                          : result.series
                      ]}
                    />
                  </div>
                  {#if result.external_id || result.isbn}
                    <div class="search-result-meta secondary">
                      <MetaLine
                        items={[
                          result.external_id && `ID: ${result.external_id}`,
                          result.isbn && `ISBN: ${result.isbn}`,
                          result.rating
                            ? `${result.rating.toFixed(2)} ★${result.rating_count ? ` (${(result.rating_count / 1000).toFixed(0)}k)` : ''}`
                            : null
                        ]}
                      />
                    </div>
                  {/if}
                </div>
                {#if selectedIndices.has(i)}
                  <span class="check-mark material-icons-outlined">check_circle</span>
                {/if}
              </button>
            {/each}
          </div>
        </div>
      {:else if mode === 'author' && authorResults.length > 0}
        <div class="results-section">
          <div class="label">Results ({authorResults.length})</div>
          <div class="results-list">
            {#each authorResults as result, i (i)}
              <button
                type="button"
                class="search-result"
                class:selected={selectedIndices.has(i)}
                onclick={() => toggleResult(i)}
              >
                <div
                  class="thumb author"
                  style="background-image: url('{result.image_url || '/placeholder-author.png'}')"
                ></div>
                <div class="search-result-info">
                  <div class="search-result-title">{result.name}</div>
                  <div class="search-result-meta">
                    <span class="source-tag" style="background: {getProviderColor(result.provider)}"
                      >{getProviderCode(result.provider)}</span
                    >
                    <MetaLine
                      items={[
                        result.book_count ? `${result.book_count} books` : null,
                        result.confidence ? `${Math.round(result.confidence * 100)}% match` : null
                      ]}
                    />
                  </div>
                  {#if result.series_names && result.series_names.length > 0}
                    <div class="search-result-series">
                      {result.series_names.slice(0, 3).join(', ')}{result.series_names.length > 3
                        ? '...'
                        : ''}
                    </div>
                  {/if}
                </div>
                {#if selectedIndices.has(i)}
                  <span class="check-mark material-icons-outlined">check_circle</span>
                {/if}
              </button>
            {/each}
          </div>
        </div>
      {:else if mode === 'series' && seriesResults.length > 0}
        <div class="results-section">
          <div class="label">Results ({seriesResults.length})</div>
          <div class="results-list">
            {#each seriesResults as result, i (i)}
              <button
                type="button"
                class="search-result"
                class:selected={selectedIndices.has(i)}
                onclick={() => toggleResult(i)}
              >
                <div
                  class="thumb"
                  style="background-image: url('{result.image_url || '/placeholder-cover.png'}')"
                ></div>
                <div class="search-result-info">
                  <div class="search-result-title">{result.name}</div>
                  <div class="search-result-meta">
                    <span class="source-tag" style="background: {getProviderColor(result.provider)}"
                      >{getProviderCode(result.provider)}</span
                    >
                    <MetaLine
                      items={[
                        result.authors?.join(', ') || 'Unknown Author',
                        result.book_count ? `${result.book_count} books` : null
                      ]}
                    />
                  </div>
                  {#if result.description}
                    <div class="search-result-bio">
                      {result.description.substring(0, 150)}{result.description.length > 150
                        ? '...'
                        : ''}
                    </div>
                  {/if}
                </div>
                {#if selectedIndices.has(i)}
                  <span class="check-mark material-icons-outlined">check_circle</span>
                {/if}
              </button>
            {/each}
          </div>
        </div>
      {:else if loading}
        <div class="loading-state">Searching...</div>
      {:else if !loading && hasSearched && bookResults.length === 0 && authorResults.length === 0 && seriesResults.length === 0}
        <div class="empty-state">
          <span class="material-icons-outlined">search_off</span>
          <p>No results found</p>
        </div>
      {/if}
    </div>

    <div class="modal-footer">
      {#if !hideManual}
        <button type="button" class="btn-sec" onclick={onSkip}> Manual </button>
      {/if}
      <div class="spacer"></div>
      <button type="button" class="btn-sec" onclick={onCancel}>Cancel</button>
      <button
        type="button"
        class="btn-primary"
        onclick={handleUseSelected}
        disabled={selectedIndices.size === 0}
      >
        Use Selected ({selectedIndices.size})
      </button>
    </div>
  </div>
{:else}
  <!-- Standalone mode: full modal with backdrop -->
  <div
    class="modal-backdrop open"
    role="presentation"
    onclick={onCancel}
    onkeydown={(e) => e.key === 'Escape' && onCancel()}
  >
    <div
      class="modal"
      role="presentation"
      onclick={(e) => e.stopPropagation()}
      onkeydown={() => {}}
    >
      <div class="modal-header">
        <div class="modal-title">{getModalTitle()}</div>
        <button class="icon-btn small" type="button" onclick={onCancel}>
          <span class="material-icons-outlined">close</span>
        </button>
      </div>

      <div class="modal-body">
        <!-- Search fields -->
        <div class="search-section">
          <div class="label">{getSearchLabel()}</div>
          <div class="search-fields">
            <div class="search-row">
              <input
                type="text"
                class="search-input"
                bind:value={searchTitle}
                placeholder={getPlaceholder()}
                onkeydown={(e) => e.key === 'Enter' && search()}
              />
            </div>

            {#if mode === 'book'}
              <div class="search-row secondary">
                <input
                  type="text"
                  class="search-input"
                  bind:value={searchAuthor}
                  placeholder="Author (optional)..."
                  onkeydown={(e) => e.key === 'Enter' && search()}
                />
                <input
                  type="text"
                  class="search-input isbn"
                  bind:value={searchIsbn}
                  placeholder="ISBN (optional)..."
                  onkeydown={(e) => e.key === 'Enter' && search()}
                />
              </div>
            {/if}

            <div class="search-row">
              <div class="provider-toggles">
                {#each providers.filter((p) => p.supportsMode) as provider (provider.id)}
                  <label class="provider-toggle" class:disabled={!provider.enabled}>
                    <input
                      type="checkbox"
                      checked={provider.enabled}
                      onchange={() => toggleProvider(provider.id)}
                    />
                    <span class="provider-tag" style="background: {provider.color}"
                      >{provider.code}</span
                    >
                  </label>
                {/each}
              </div>
              <button
                type="button"
                class="btn-primary search-btn"
                onclick={search}
                disabled={loading}
              >
                {loading ? 'Searching...' : 'Search'}
              </button>
            </div>
          </div>
        </div>

        <!-- Results -->
        {#if bookResults.length > 0 || authorResults.length > 0 || seriesResults.length > 0}
          <div class="results-section">
            <div class="results-header">
              <div class="results-count">
                {#if mode === 'book'}
                  {bookResults.length} result{bookResults.length === 1 ? '' : 's'}
                {:else if mode === 'author'}
                  {authorResults.length} result{authorResults.length === 1 ? '' : 's'}
                {:else}
                  {seriesResults.length} result{seriesResults.length === 1 ? '' : 's'}
                {/if}
              </div>
              {#if selectedIndices.size > 0}
                <div class="selected-count">{selectedIndices.size} selected</div>
              {/if}
            </div>
            <div class="results-list">
              {#if mode === 'book'}
                {#each bookResults as result, i (i)}
                  <button
                    type="button"
                    class="search-result"
                    class:selected={selectedIndices.has(i)}
                    onclick={() => toggleSelection(i)}
                  >
                    <div class="result-cover">
                      {#if result.cover_url}
                        <img src={result.cover_url} alt="" />
                      {:else}
                        <div class="no-cover">
                          <span class="material-icons-outlined">menu_book</span>
                        </div>
                      {/if}
                    </div>
                    <div class="result-info">
                      <div class="result-title-row">
                        <span
                          class="provider-badge"
                          style="background: {getProviderColor(result.provider)}"
                          >{getProviderCode(result.provider)}</span
                        >
                        <span class="search-result-title">{result.title}</span>
                      </div>
                      {#if result.authors?.length}
                        <div class="result-authors">{result.authors.join(', ')}</div>
                      {/if}
                      {#if result.description}
                        <div class="result-description">
                          {result.description.substring(0, 150)}{result.description.length > 150
                            ? '...'
                            : ''}
                        </div>
                      {/if}
                    </div>
                    {#if selectedIndices.has(i)}
                      <span class="check-mark material-icons-outlined">check_circle</span>
                    {/if}
                  </button>
                {/each}
              {:else if mode === 'author'}
                {#each authorResults as result, i (i)}
                  <button
                    type="button"
                    class="search-result"
                    class:selected={selectedIndices.has(i)}
                    onclick={() => toggleSelection(i)}
                  >
                    <div class="result-cover">
                      {#if result.image_url}
                        <img src={result.image_url} alt="" />
                      {:else}
                        <div class="no-cover">
                          <span class="material-icons-outlined">person</span>
                        </div>
                      {/if}
                    </div>
                    <div class="result-info">
                      <div class="result-title-row">
                        <span
                          class="provider-badge"
                          style="background: {getProviderColor(result.provider)}"
                          >{getProviderCode(result.provider)}</span
                        >
                        <span class="search-result-title">{result.name}</span>
                      </div>
                      {#if result.series_names && result.series_names.length > 0}
                        <div class="result-description">
                          {result.series_names.slice(0, 3).join(', ')}{result.series_names.length >
                          3
                            ? '...'
                            : ''}
                        </div>
                      {/if}
                    </div>
                    {#if selectedIndices.has(i)}
                      <span class="check-mark material-icons-outlined">check_circle</span>
                    {/if}
                  </button>
                {/each}
              {:else if mode === 'series'}
                {#each seriesResults as result, i (i)}
                  <button
                    type="button"
                    class="search-result"
                    class:selected={selectedIndices.has(i)}
                    onclick={() => toggleSelection(i)}
                  >
                    <div class="result-cover">
                      {#if result.image_url}
                        <img src={result.image_url} alt="" />
                      {:else}
                        <div class="no-cover">
                          <span class="material-icons-outlined">collections_bookmark</span>
                        </div>
                      {/if}
                    </div>
                    <div class="result-info">
                      <div class="result-title-row">
                        <span
                          class="provider-badge"
                          style="background: {getProviderColor(result.provider)}"
                          >{getProviderCode(result.provider)}</span
                        >
                        <span class="search-result-title">{result.name}</span>
                      </div>
                      {#if result.book_count}
                        <div class="result-authors">{result.book_count} books</div>
                      {/if}
                      {#if result.description}
                        <div class="result-description">
                          {result.description.substring(0, 150)}{result.description.length > 150
                            ? '...'
                            : ''}
                        </div>
                      {/if}
                    </div>
                    {#if selectedIndices.has(i)}
                      <span class="check-mark material-icons-outlined">check_circle</span>
                    {/if}
                  </button>
                {/each}
              {/if}
            </div>
          </div>
        {:else if loading}
          <div class="loading-state">Searching...</div>
        {:else if !loading && hasSearched && bookResults.length === 0 && authorResults.length === 0 && seriesResults.length === 0}
          <div class="empty-state">
            <span class="material-icons-outlined">search_off</span>
            <p>No results found</p>
          </div>
        {/if}
      </div>

      <div class="modal-footer">
        {#if !hideManual}
          <button type="button" class="btn-sec" onclick={onSkip}> Manual </button>
        {/if}
        <div class="spacer"></div>
        <button type="button" class="btn-sec" onclick={onCancel}>Cancel</button>
        <button
          type="button"
          class="btn-primary"
          onclick={handleUseSelected}
          disabled={selectedIndices.size === 0}
        >
          Use Selected ({selectedIndices.size})
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  /* Embedded mode - fills parent container */
  .embedded-content {
    display: flex;
    flex-direction: column;
    flex: 1;
    overflow: hidden;
  }

  .embedded-body {
    padding: 20px;
    overflow-y: auto;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 20px;
  }

  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.7);
    z-index: 200;
    display: none;
    align-items: center;
    justify-content: center;
  }

  .modal-backdrop.open {
    display: flex;
  }

  .modal {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    width: 650px;
    max-width: 90vw;
    max-height: 85vh;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .modal-header {
    padding: 16px 20px;
    border-bottom: 1px solid var(--ring);
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .modal-title {
    font-size: 16px;
    font-weight: 700;
  }

  .modal-body {
    padding: 20px;
    overflow-y: auto;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 20px;
  }

  .modal-footer {
    padding: 10px 16px;
    border-top: 1px solid var(--ring);
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .spacer {
    flex: 1;
  }

  .icon-btn {
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
  }

  .icon-btn:hover {
    background: var(--bg);
    color: var(--ink);
  }

  .icon-btn.small {
    width: 28px;
    height: 28px;
  }

  .label {
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    margin-bottom: 8px;
  }

  .search-section,
  .providers-section,
  .results-section {
    display: flex;
    flex-direction: column;
  }

  .search-fields {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .search-row {
    display: flex;
    gap: 8px;
  }

  .search-row.secondary {
    gap: 8px;
  }

  .search-input {
    flex: 1;
    padding: 10px 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg);
    color: var(--ink);
    font-size: 14px;
  }

  .search-input:focus {
    outline: none;
    border-color: var(--accent);
  }

  .search-input.isbn {
    max-width: 140px;
  }

  .search-btn {
    align-self: flex-end;
    min-width: 100px;
  }

  .providers-row {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
  }

  .provider-checkbox {
    display: flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
    font-size: 13px;
  }

  .provider-checkbox input {
    width: 16px;
    height: 16px;
  }

  .source-tag {
    display: inline-block;
    font-size: 9px;
    font-weight: 700;
    padding: 2px 5px;
    border-radius: 3px;
    color: #000;
    text-transform: uppercase;
  }

  .results-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-height: 350px;
    overflow-y: auto;
  }

  .search-result {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 12px;
    background: var(--bg);
    border: 1px solid var(--ring);
    border-radius: 8px;
    cursor: pointer;
    text-align: left;
    width: 100%;
    transition:
      border-color 0.15s,
      background 0.15s;
  }

  .search-result:hover {
    border-color: var(--accent);
  }

  .search-result.selected {
    border-color: var(--accent);
    background: rgba(211, 140, 90, 0.1);
  }

  .thumb {
    width: 48px;
    height: 72px;
    border-radius: 4px;
    background-size: cover;
    background-position: center;
    background-color: var(--bg-2);
    flex-shrink: 0;
  }

  .thumb.author {
    width: 56px;
    height: 56px;
    border-radius: 50%;
  }

  .search-result-info {
    flex: 1;
    min-width: 0;
  }

  .search-result-title {
    font-weight: 600;
    font-size: 14px;
    margin-bottom: 4px;
    color: var(--ink);
  }

  .search-result-meta {
    font-size: 12px;
    color: var(--muted);
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }

  .search-result-meta.secondary {
    margin-top: 4px;
  }

  .search-result-series {
    font-size: 12px;
    color: var(--muted);
    font-style: italic;
    margin-top: 6px;
    line-height: 1.4;
  }

  .check-mark {
    color: var(--accent);
    font-size: 20px;
    flex-shrink: 0;
  }

  .loading-state,
  .empty-state {
    text-align: center;
    color: var(--muted);
    padding: 40px 20px;
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
  }

  .empty-state .material-icons-outlined {
    font-size: 48px;
    opacity: 0.5;
  }
</style>
