<script lang="ts">
  import { SearchSelect } from '@koblibs/ui';
  import { type AuthorResult, getConfidenceColor } from './types';
  import { getProviderName, getProviderColor } from '#lib/utils/format.js';

  interface Props {
    initialAuthor?: string;
    onApplyAuthor?: (result: AuthorResult) => Promise<void>;
  }

  let { initialAuthor = '', onApplyAuthor }: Props = $props();

  // Search state
  let searchAuthorName = $state('');
  let initialSyncDone = false;
  $effect(() => {
    if (!initialSyncDone && initialAuthor) {
      searchAuthorName = initialAuthor;
      initialSyncDone = true;
    }
  });
  let searchProvider = $state('hardcover');
  let authorResults = $state<AuthorResult[]>([]);
  let loading = $state(false);
  let error = $state('');
  let selectedIndex = $state<number | null>(null);

  const authorProviders = [{ value: 'hardcover', label: getProviderName('hardcover') }];

  const apiBase = import.meta.env.VITE_API_BASE_URL || '';

  const searchAuthors = async () => {
    if (!searchAuthorName) {
      error = 'Please enter an author name to search';
      return;
    }

    loading = true;
    error = '';
    authorResults = [];
    selectedIndex = null;

    try {
      const params = new URLSearchParams();
      params.set('name', searchAuthorName);
      if (searchProvider) params.set('provider', searchProvider);

      const resp = await fetch(`${apiBase}/api/v1/admin/metadata/search-authors?${params}`, {
        credentials: 'include'
      });

      if (!resp.ok) {
        const data = await resp.json().catch(() => ({}));
        error = data.error || 'Search failed';
        return;
      }

      const data = await resp.json();
      authorResults = data.results || [];

      if (authorResults.length === 0) {
        error = 'No authors found';
      }
    } catch {
      error = 'Search failed';
    } finally {
      loading = false;
    }
  };

  const handleApplyAuthor = async () => {
    if (selectedIndex === null || !onApplyAuthor) return;
    const result = authorResults[selectedIndex];
    await onApplyAuthor(result);
  };
</script>

<div class="search-form">
  <label>
    Author Name
    <input type="text" bind:value={searchAuthorName} placeholder="e.g., Brandon Sanderson" />
  </label>
  <div class="field">
    <span class="field-label">Provider</span>
    <SearchSelect
      items={authorProviders}
      bind:value={searchProvider}
      placeholder="Select provider"
    />
  </div>
  <button class="btn" type="button" onclick={searchAuthors} disabled={loading}>
    {loading ? 'Searching...' : 'Search Authors'}
  </button>
</div>

{#if error}
  <div class="error-message">{error}</div>
{/if}

{#if authorResults.length > 0}
  <div class="results-list">
    {#each authorResults as result, i (i)}
      <button
        class="result-item"
        class:selected={selectedIndex === i}
        type="button"
        onclick={() => (selectedIndex = i)}
      >
        <div class="result-main">
          {#if result.image_url}
            <img src={result.image_url} alt="" class="author-image" />
          {:else}
            <div class="author-image-placeholder"></div>
          {/if}
          <div class="result-info">
            <div class="result-title">{result.name}</div>
            {#if result.book_count}
              <div class="result-books muted">{result.book_count} books</div>
            {/if}
            {#if result.series_names && result.series_names.length > 0}
              <div class="result-series muted">
                {result.series_names.slice(0, 3).join(', ')}{result.series_names.length > 3
                  ? '...'
                  : ''}
              </div>
            {/if}
            <div class="result-meta muted">
              <span class="provider-badge" style="background: {getProviderColor(result.provider)}"
                >{getProviderName(result.provider)}</span
              >
              <span class="confidence-badge {getConfidenceColor(result.confidence)}">
                {Math.round(result.confidence * 100)}%
              </span>
            </div>
          </div>
        </div>
      </button>
    {/each}
  </div>

  {#if onApplyAuthor}
    <div class="panel-actions">
      <button
        class="btn"
        type="button"
        onclick={handleApplyAuthor}
        disabled={selectedIndex === null}
      >
        Apply Selected Author
      </button>
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

  .result-item {
    display: grid;
    gap: 12px;
    padding: 12px;
    background: transparent;
    border: 1px solid var(--ring);
    border-radius: 10px;
    text-align: left;
    cursor: pointer;
    transition:
      border-color 0.15s,
      background 0.15s;
    color: inherit;
    font: inherit;
  }

  .result-item:hover {
    border-color: var(--accent);
    background: rgba(211, 140, 90, 0.04);
  }

  .result-item.selected {
    border-color: var(--accent);
    background: rgba(211, 140, 90, 0.08);
  }

  .result-main {
    display: flex;
    gap: 12px;
  }

  .author-image {
    width: 50px;
    height: 50px;
    object-fit: cover;
    border-radius: 25px;
    flex-shrink: 0;
  }

  .author-image-placeholder {
    width: 50px;
    height: 50px;
    background: var(--ring);
    border-radius: 25px;
    flex-shrink: 0;
  }

  .result-info {
    flex: 1;
    min-width: 0;
  }

  .result-title {
    font-weight: 600;
    color: var(--ink);
  }

  .result-books {
    font-size: 12px;
    margin-top: 2px;
  }

  .result-series {
    font-size: 11px;
    margin-top: 2px;
    font-style: italic;
  }

  .result-meta {
    display: flex;
    gap: 8px;
    margin-top: 6px;
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

  .panel-actions {
    padding-top: 8px;
    border-top: 1px solid var(--ring);
  }

  .btn {
    padding: 8px 16px;
    border-radius: 10px;
    border: none;
    background: var(--accent);
    color: white;
    font-weight: 600;
    font-size: 13px;
    cursor: pointer;
    width: 100%;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .muted {
    color: var(--muted);
  }
</style>
