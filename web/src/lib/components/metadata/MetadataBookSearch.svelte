<script lang="ts">
  import type { MetadataResult } from '#lib/api/client.js';
  import { SearchSelect } from '@koblibs/ui';
  import MetadataResultCard from './MetadataResultCard.svelte';
  import MetadataMergePanel from './MetadataMergePanel.svelte';
  import { type OverrideableField, buildMergedResult } from './types';
  import { getProviderName } from '#lib/utils/format.js';

  interface Props {
    initialTitle?: string;
    initialAuthor?: string;
    initialIsbn?: string;
    onApply: (result: MetadataResult) => Promise<void>;
  }

  let { initialTitle = '', initialAuthor = '', initialIsbn = '', onApply }: Props = $props();

  // Search state
  let searchTitle = $state('');
  let searchAuthor = $state('');
  let searchIsbn = $state('');

  let initialSyncDone = false;
  $effect(() => {
    if (!initialSyncDone) {
      if (initialTitle) searchTitle = initialTitle;
      if (initialAuthor) searchAuthor = initialAuthor;
      if (initialIsbn) searchIsbn = initialIsbn;
      initialSyncDone = true;
    }
  });

  $effect(() => {
    if (initialTitle !== undefined) searchTitle = initialTitle;
  });
  $effect(() => {
    if (initialAuthor !== undefined) searchAuthor = initialAuthor;
  });
  $effect(() => {
    if (initialIsbn !== undefined) searchIsbn = initialIsbn;
  });

  let searchProvider = $state('hardcover');
  let results = $state<MetadataResult[]>([]);
  let loading = $state(false);
  let error = $state('');
  let selectedIndex = $state<number | null>(null);

  // Multi-select merge state
  let multiSelectMode = $state(false);
  let selectedForMerge = $state<number[]>([]);
  let mergePreview = $state<MetadataResult | null>(null);
  let fieldOverrides = $state<Partial<Record<OverrideableField, number>>>({});

  const providers = [
    { value: '', label: 'All Providers' },
    { value: 'hardcover', label: getProviderName('hardcover') },
    { value: 'googlebooks', label: getProviderName('googlebooks') },
    { value: 'openlibrary', label: getProviderName('openlibrary') }
  ];

  const apiBase = import.meta.env.VITE_API_BASE_URL || '';

  const search = async () => {
    if (!searchIsbn && !searchTitle) {
      error = 'Please enter an ISBN or title to search';
      return;
    }

    loading = true;
    error = '';
    results = [];
    selectedIndex = null;
    selectedForMerge = [];
    mergePreview = null;

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
        error = 'Search failed';
        return;
      }

      const data = await resp.json();
      results = data.results || [];

      if (results.length === 0) {
        error = 'No results found';
      }
    } catch {
      error = 'Search failed';
    } finally {
      loading = false;
    }
  };

  const handleApply = async () => {
    if (selectedIndex === null) return;
    const result = results[selectedIndex];
    await onApply(result);
  };

  const handleApplyMerged = async () => {
    if (!mergePreview) return;
    await onApply(mergePreview);
  };

  const toggleMultiSelect = () => {
    multiSelectMode = !multiSelectMode;
    if (!multiSelectMode) {
      selectedForMerge = [];
      mergePreview = null;
      fieldOverrides = {};
    }
  };

  const toggleMergeSelection = (index: number) => {
    const existingPos = selectedForMerge.indexOf(index);
    if (existingPos >= 0) {
      selectedForMerge = selectedForMerge.filter((i) => i !== index);
    } else {
      selectedForMerge = [...selectedForMerge, index];
    }
    updateMergePreview();
  };

  const getMergePriority = (index: number): number | null => {
    const pos = selectedForMerge.indexOf(index);
    return pos >= 0 ? pos + 1 : null;
  };

  const updateMergePreview = () => {
    fieldOverrides = {};
    mergePreview = buildMergedResult(selectedForMerge, results, fieldOverrides);
  };

  const handleSetFieldOverride = (field: OverrideableField, index: number) => {
    fieldOverrides = { ...fieldOverrides, [field]: index };
    mergePreview = buildMergedResult(selectedForMerge, results, fieldOverrides);
  };
</script>

<div class="search-form">
  <label>
    ISBN
    <input type="text" bind:value={searchIsbn} placeholder="978-0-123456-78-9" />
  </label>
  <label>
    Title
    <input type="text" bind:value={searchTitle} placeholder="Book title" />
  </label>
  <label>
    Author
    <input type="text" bind:value={searchAuthor} placeholder="Author name" />
  </label>
  <div class="field">
    <span class="field-label">Provider</span>
    <SearchSelect items={providers} bind:value={searchProvider} placeholder="Select provider" />
  </div>
  <button class="btn" type="button" onclick={search} disabled={loading}>
    {loading ? 'Searching...' : 'Search'}
  </button>
</div>

{#if error}
  <div class="error-message">{error}</div>
{/if}

{#if results.length > 0}
  <div class="multi-select-toggle">
    <button
      class="toggle-btn"
      class:active={multiSelectMode}
      type="button"
      onclick={toggleMultiSelect}
    >
      <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor">
        <path
          d="M19 3H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm-9 14l-5-5 1.4-1.4 3.6 3.6 7.6-7.6L19 8l-9 9z"
        ></path>
      </svg>
      {multiSelectMode ? 'Exit Merge Mode' : 'Merge Multiple'}
    </button>
    {#if multiSelectMode && selectedForMerge.length > 0}
      <span class="selection-count">{selectedForMerge.length} selected</span>
    {/if}
  </div>

  <div class="results-list">
    {#each results as result, i (i)}
      <MetadataResultCard
        {result}
        index={i}
        selected={multiSelectMode ? selectedForMerge.includes(i) : selectedIndex === i}
        {multiSelectMode}
        mergePriority={getMergePriority(i)}
        onSelect={() => {
          if (multiSelectMode) {
            toggleMergeSelection(i);
          } else {
            selectedIndex = i;
          }
        }}
      />
    {/each}
  </div>

  {#if multiSelectMode && selectedForMerge.length >= 2}
    <MetadataMergePanel
      {results}
      {selectedForMerge}
      {mergePreview}
      {fieldOverrides}
      onSetFieldOverride={handleSetFieldOverride}
    />
  {/if}

  <div class="panel-actions">
    {#if multiSelectMode}
      <button
        class="btn"
        type="button"
        onclick={handleApplyMerged}
        disabled={selectedForMerge.length < 2 || !mergePreview}
      >
        Apply Merged ({selectedForMerge.length} sources)
      </button>
    {:else}
      <button class="btn" type="button" onclick={handleApply} disabled={selectedIndex === null}>
        Apply Selected
      </button>
    {/if}
  </div>
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

  .multi-select-toggle {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 12px;
  }

  .toggle-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    color: var(--muted);
    font-size: 12px;
    cursor: pointer;
    transition:
      background 0.15s,
      border-color 0.15s,
      color 0.15s;
  }

  .toggle-btn:hover {
    border-color: var(--accent);
    color: var(--ink);
  }

  .toggle-btn.active {
    background: var(--accent);
    border-color: var(--accent);
    color: white;
  }

  .selection-count {
    font-size: 12px;
    color: var(--accent);
    font-weight: 500;
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
</style>
