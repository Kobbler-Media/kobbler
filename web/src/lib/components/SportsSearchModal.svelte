<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { Modal } from '@koblibs/ui';
  import { searchSportsLeagues, createSportsLeague } from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import type { TvSearchResult } from '#lib/types.js';

  interface Props {
    initialQuery?: string;
    onComplete: (id: string) => void;
    onClose: () => void;
  }

  let { initialQuery, onComplete, onClose }: Props = $props();

  let query = $state(untrack(() => initialQuery) ?? '');
  let results = $state<TvSearchResult[]>([]);
  let searching = $state(false);
  let hasSearched = $state(false);
  let creating = $state(false);

  let showManualForm = $state(false);
  let manualTitle = $state('');
  let manualDescription = $state('');
  let manualNetwork = $state('');

  onMount(() => {
    if (initialQuery?.trim()) {
      handleSearch();
    }
  });

  async function handleSearch() {
    const trimmed = query.trim();
    if (!trimmed) return;

    searching = true;
    hasSearched = true;
    results = [];
    try {
      results = await searchSportsLeagues(trimmed);
      if (results.length === 0) {
        showMessage('No results found');
      }
    } catch (e) {
      showMessage('Search failed');
      console.error('Search failed:', e);
    } finally {
      searching = false;
    }
  }

  async function handleCreate(result: TvSearchResult) {
    creating = true;
    try {
      const identifiers: Record<string, unknown> = {};
      if (result.identifiers) {
        Object.assign(identifiers, result.identifiers);
      }
      if (result.provider && result.external_id) {
        identifiers[result.provider] = result.external_id;
      }
      const { id } = await createSportsLeague({
        title: result.title,
        year: result.year,
        description: result.description,
        poster_url: result.poster_url,
        identifiers,
        genres: result.genres || [],
        network: result.network
      });
      showMessage(`Added "${result.title}"`);
      onComplete(id);
    } catch (e) {
      showMessage('Failed to create league');
      console.error('Create failed:', e);
    } finally {
      creating = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      handleSearch();
    }
  }

  async function handleManualCreate() {
    const trimmedTitle = manualTitle.trim();
    if (!trimmedTitle) {
      showMessage('Title is required');
      return;
    }

    creating = true;
    try {
      const { id } = await createSportsLeague({
        title: trimmedTitle,
        description: manualDescription.trim() || undefined,
        network: manualNetwork.trim() || undefined,
        genres: []
      });
      showMessage(`Created "${trimmedTitle}"`);
      onComplete(id);
    } catch (e) {
      showMessage('Failed to create league');
      console.error('Create failed:', e);
    } finally {
      creating = false;
    }
  }

  function toggleManualForm() {
    showManualForm = !showManualForm;
    if (showManualForm) {
      manualTitle = '';
      manualDescription = '';
      manualNetwork = '';
      results = [];
      query = '';
    }
  }
</script>

<Modal title="Add League" {onClose} width="700px">
  {#if !showManualForm}
    <div class="search-form">
      <div class="search-inputs">
        <input
          type="text"
          class="search-input"
          placeholder="Search leagues..."
          bind:value={query}
          onkeydown={handleKeydown}
        />
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
        <button class="btn-manual" type="button" onclick={toggleManualForm}>
          Create Manually
        </button>
      </div>
    </div>

    <div class="results-area">
      {#if searching}
        <div class="results-message">Searching...</div>
      {:else if hasSearched && results.length === 0 && query.trim()}
        <div class="results-message">No results. Try a different search.</div>
      {:else}
        <div class="results-grid">
          {#each results as result, i ((result.external_id ?? i) + ':' + result.provider)}
            <button
              class="result-card"
              type="button"
              onclick={() => handleCreate(result)}
              disabled={creating}
            >
              <div
                class="result-poster"
                style={result.poster_url ? `background-image: url(${result.poster_url})` : ''}
              >
                {#if !result.poster_url}
                  <span class="material-icons-outlined no-poster">emoji_events</span>
                {/if}
              </div>
              <div class="result-info">
                <strong class="result-title">{result.title}</strong>
                {#if result.year}
                  <span class="result-year">{result.year}</span>
                {/if}
                {#if result.network}
                  <span class="result-network">{result.network}</span>
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
  {:else}
    <div class="manual-form">
      <button class="btn-back" type="button" onclick={toggleManualForm} disabled={creating}>
        &larr; Back to Search
      </button>

      <div class="form-group">
        <label for="manual-title">Title *</label>
        <input
          id="manual-title"
          type="text"
          class="search-input"
          placeholder="Enter league name"
          bind:value={manualTitle}
        />
      </div>

      <div class="form-group">
        <label for="manual-network">Network / Broadcaster</label>
        <input
          id="manual-network"
          type="text"
          class="search-input"
          placeholder="e.g. ESPN, Sky Sports"
          bind:value={manualNetwork}
        />
      </div>

      <div class="form-group">
        <label for="manual-description">Description</label>
        <textarea
          id="manual-description"
          class="search-input manual-textarea"
          placeholder="Enter description (optional)"
          bind:value={manualDescription}
          rows="4"></textarea>
      </div>

      <button
        class="btn-search btn-create"
        type="button"
        onclick={handleManualCreate}
        disabled={creating || !manualTitle.trim()}
      >
        {creating ? 'Creating...' : 'Create'}
      </button>
    </div>
  {/if}
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

  .btn-manual {
    margin-left: auto;
    padding: 6px 12px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: var(--bg-2);
    color: var(--ink);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-manual:hover {
    border-color: var(--accent);
    background: var(--card);
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

  .results-area {
    max-height: 400px;
    overflow-y: auto;
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

  .result-card:hover:not(:disabled) {
    border-color: var(--accent);
    background: var(--card);
  }

  .result-card:disabled {
    opacity: 0.6;
    cursor: wait;
  }

  .result-poster {
    width: 60px;
    height: 60px;
    border-radius: 10px;
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

  .result-network {
    font-size: 11px;
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

  .manual-form {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .btn-back {
    align-self: flex-start;
    padding: 8px 16px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: var(--bg-2);
    color: var(--muted);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-back:hover:not(:disabled) {
    color: var(--ink);
    border-color: var(--accent);
  }

  .btn-back:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .form-group label {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
  }

  .manual-textarea {
    resize: vertical;
    font-family: inherit;
    line-height: 1.5;
  }

  .btn-create {
    margin-top: 8px;
  }
</style>
