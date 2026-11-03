<script lang="ts">
  /**
   * VideoMetadataSearchModal - Search modal for movies/TV shows that returns
   * results to the caller (doesn't create entities). For use in edit forms
   * to populate SourceField values from multiple providers.
   */
  import { onMount, untrack } from 'svelte';
  import { Modal } from '@koblibs/ui';
  import { SearchSelect } from '@koblibs/ui';
  import { searchMovies, searchTvShows, getVideoMetadataProviders } from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import type { MovieSearchResult, TvSearchResult } from '#lib/types.js';

  type Result = MovieSearchResult | TvSearchResult;

  interface Props {
    mode: 'movie' | 'tv';
    initialTitle?: string;
    onSelect: (results: Result[]) => void;
    onCancel: () => void;
  }

  let { mode, initialTitle = '', onSelect, onCancel }: Props = $props();

  let query = $state(untrack(() => initialTitle));
  let yearFilter = $state('');
  let results = $state<Result[]>([]);
  let searching = $state(false);
  let providerFilter = $state('');
  let mergeMode = $state(true);
  let selectedForMerge = $state<number[]>([]);
  let providerOptions = $state<{ value: string; label: string }[]>([
    { value: '', label: 'All Providers' }
  ]);

  onMount(async () => {
    try {
      const providers = await getVideoMetadataProviders();
      const enabled = providers.filter((p) => p.enabled);
      providerOptions = [
        { value: '', label: 'All Providers' },
        ...enabled.map((p) => ({ value: p.provider_type, label: p.display_name }))
      ];
    } catch {
      // Fall back to just "All Providers"
    }
  });

  async function handleSearch() {
    const trimmed = query.trim();
    if (!trimmed) return;

    searching = true;
    results = [];
    selectedForMerge = [];
    try {
      if (mode === 'movie') {
        const yearNum = yearFilter ? parseInt(yearFilter, 10) : undefined;
        results = await searchMovies(trimmed, {
          year: yearNum,
          provider: providerFilter || undefined
        });
      } else {
        results = await searchTvShows(trimmed, {
          provider: providerFilter || undefined
        });
      }
      if (results.length === 0) {
        showMessage('No results found');
      }
    } catch (e) {
      showMessage('Search failed');
      console.error('Video metadata search failed:', e);
    } finally {
      searching = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      handleSearch();
    }
  }

  function toggleMergeSelection(index: number) {
    if (selectedForMerge.includes(index)) {
      selectedForMerge = selectedForMerge.filter((i) => i !== index);
    } else {
      selectedForMerge = [...selectedForMerge, index];
    }
  }

  function handleSelectSingle(result: Result) {
    onSelect([result]);
  }

  function handleUseSelected() {
    const selected = selectedForMerge.map((i) => results[i]);
    onSelect(selected);
  }

  const entityLabel = $derived(mode === 'movie' ? 'Movie' : 'TV Show');
</script>

<Modal title="Search {entityLabel} Metadata" onClose={onCancel} width="700px">
  <div class="search-form">
    <div class="search-inputs">
      <input
        type="text"
        class="search-input"
        placeholder="Search by title..."
        bind:value={query}
        onkeydown={handleKeydown}
      />
      {#if mode === 'movie'}
        <input
          type="text"
          class="year-input"
          placeholder="Year"
          bind:value={yearFilter}
          onkeydown={handleKeydown}
          maxlength="4"
        />
      {/if}
      <button
        class="btn-search"
        type="button"
        onclick={handleSearch}
        disabled={searching || !query.trim()}
      >
        {searching ? 'Searching...' : 'Search'}
      </button>
    </div>
    <div class="search-options">
      <div class="provider-select-wrapper">
        <SearchSelect
          items={providerOptions}
          value={providerFilter}
          onchange={(val) => (providerFilter = val)}
          placeholder="All Providers"
        />
      </div>
      <label class="merge-toggle">
        <input type="checkbox" bind:checked={mergeMode} />
        <span>Multi-select</span>
      </label>
      {#if mergeMode && selectedForMerge.length >= 1}
        <button class="btn-use-selected" type="button" onclick={handleUseSelected}>
          Use Selected ({selectedForMerge.length})
        </button>
      {/if}
    </div>
  </div>

  <div class="results-layout">
    <div class="results-area">
      {#if searching}
        <div class="results-message">Searching...</div>
      {:else if results.length === 0 && query.trim()}
        <div class="results-message">No results. Try a different search.</div>
      {:else}
        <div class="results-grid">
          {#each results as result, index (`${result.provider}:${result.external_id}`)}
            <button
              class="result-card"
              class:selected={mergeMode && selectedForMerge.includes(index)}
              type="button"
              onclick={() => (mergeMode ? toggleMergeSelection(index) : handleSelectSingle(result))}
            >
              {#if mergeMode}
                <div class="merge-checkbox" class:checked={selectedForMerge.includes(index)}>
                  {#if selectedForMerge.includes(index)}
                    {@const priority = selectedForMerge.indexOf(index) + 1}
                    <span class="priority-badge">{priority}</span>
                  {/if}
                </div>
              {/if}
              <div
                class="result-poster"
                style={result.poster_url ? `background-image: url(${result.poster_url})` : ''}
              >
                {#if !result.poster_url}
                  <span class="material-icons-outlined no-poster"
                    >{mode === 'movie' ? 'movie' : 'tv'}</span
                  >
                {/if}
              </div>
              <div class="result-info">
                <strong class="result-title">{result.title}</strong>
                {#if result.year}
                  <span class="result-year">{result.year}</span>
                {/if}
                {#if result.description}
                  <p class="result-desc">
                    {result.description.slice(0, 120)}{result.description.length > 120 ? '...' : ''}
                  </p>
                {/if}
                <span class="result-provider">{result.provider}</span>
              </div>
            </button>
          {/each}
        </div>
      {/if}
    </div>
  </div>
</Modal>

<style>
  .search-form {
    margin-bottom: 16px;
  }

  .search-inputs {
    display: flex;
    gap: 8px;
  }

  .search-options {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 8px;
  }

  .provider-select-wrapper {
    min-width: 150px;
  }

  .merge-toggle {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    color: var(--muted);
    cursor: pointer;
    user-select: none;
  }

  .merge-toggle input {
    accent-color: var(--accent);
  }

  .btn-use-selected {
    margin-left: auto;
    padding: 6px 14px;
    border-radius: 8px;
    border: none;
    background: var(--accent);
    color: #1d130c;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-use-selected:hover {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
  }

  .search-input {
    flex: 1;
    padding: 10px 14px;
    border: 1px solid var(--ring);
    border-radius: 10px;
    background: var(--bg);
    color: var(--ink);
    font-size: 14px;
    outline: none;
  }

  .search-input:focus {
    border-color: var(--accent);
  }

  .year-input {
    width: 80px;
    padding: 10px 14px;
    border: 1px solid var(--ring);
    border-radius: 10px;
    background: var(--bg);
    color: var(--ink);
    font-size: 14px;
    outline: none;
    text-align: center;
  }

  .year-input:focus {
    border-color: var(--accent);
  }

  .btn-search {
    padding: 10px 20px;
    border-radius: 10px;
    border: none;
    background: var(--accent);
    color: #1d130c;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
    transition: all 0.15s;
  }

  .btn-search:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
  }

  .btn-search:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .results-layout {
    display: flex;
    gap: 16px;
  }

  .results-area {
    max-height: 400px;
    overflow-y: auto;
    flex: 1;
  }

  .results-message {
    text-align: center;
    color: var(--muted);
    padding: 32px 16px;
    font-size: 13px;
  }

  .results-grid {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .result-card {
    display: flex;
    gap: 12px;
    padding: 10px;
    border: 1px solid var(--ring);
    border-radius: 12px;
    background: var(--bg-2);
    cursor: pointer;
    text-align: left;
    color: inherit;
    font: inherit;
    transition: all 0.15s;
    appearance: none;
  }

  .result-card:hover {
    border-color: var(--accent);
    background: var(--card);
  }

  .result-card.selected {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-dim);
  }

  .merge-checkbox {
    width: 24px;
    height: 24px;
    border-radius: 6px;
    border: 2px solid var(--ring);
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    align-self: center;
    transition: all 0.15s;
  }

  .merge-checkbox.checked {
    border-color: var(--accent);
    background: var(--accent);
  }

  .priority-badge {
    font-size: 11px;
    font-weight: 800;
    color: #1d130c;
  }

  .result-poster {
    width: 60px;
    height: 90px;
    border-radius: 8px;
    background: linear-gradient(140deg, #2e323a, #1a1d23);
    background-size: cover;
    background-position: center;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
  }

  .no-poster {
    font-size: 24px;
    color: var(--muted);
    opacity: 0.3;
  }

  .result-info {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
    flex: 1;
  }

  .result-title {
    font-size: 14px;
    font-weight: 700;
    line-height: 1.3;
  }

  .result-year {
    font-size: 12px;
    color: var(--muted);
  }

  .result-desc {
    font-size: 11px;
    color: var(--muted);
    line-height: 1.4;
    margin: 2px 0 0;
  }

  .result-provider {
    font-size: 10px;
    color: var(--accent);
    text-transform: uppercase;
    font-weight: 700;
    letter-spacing: 0.5px;
    margin-top: auto;
  }
</style>
