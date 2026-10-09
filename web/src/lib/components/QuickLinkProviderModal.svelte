<script lang="ts">
  /**
   * QuickLinkProviderModal - Modal for linking a book to an external provider.
   * Searches the specified provider and updates only that provider's identifier.
   * Does not modify any other book fields.
   */
  import type { MetadataResult } from '$lib/api/client';
  import { getProviderCode, getProviderColor } from '$lib/utils/format';

  interface Props {
    bookTitle: string;
    bookAuthor?: string;
    provider: string;
    providerName: string;
    onLink: (externalId: string) => Promise<void>;
    onClose: () => void;
  }

  let { bookTitle, bookAuthor = '', provider, providerName, onLink, onClose }: Props = $props();

  const apiBase = import.meta.env.VITE_API_BASE_URL || '';
  const badgeText = $derived(providerName.substring(0, 2).toUpperCase());

  let searchQuery = $state('');
  let searchAuthor = $state('');
  let initialSyncDone = false;
  $effect(() => {
    if (!initialSyncDone) {
      if (bookTitle) searchQuery = bookTitle;
      if (bookAuthor) searchAuthor = bookAuthor;
      initialSyncDone = true;
    }
  });
  let results = $state<MetadataResult[]>([]);
  let loading = $state(false);
  let linking = $state(false);
  let hasSearched = $state(false);
  let error = $state<string | null>(null);

  async function search() {
    if (!searchQuery.trim()) return;

    loading = true;
    hasSearched = true;
    error = null;
    results = [];

    try {
      const params = new URLSearchParams();
      params.set('title', searchQuery.trim());
      if (searchAuthor.trim()) {
        params.set('author', searchAuthor.trim());
      }
      params.set('provider', provider);

      const resp = await fetch(`${apiBase}/api/v1/admin/metadata/search?${params}`, {
        credentials: 'include'
      });

      if (resp.ok) {
        const data = await resp.json();
        results = data.results || [];
      } else {
        error = 'Search failed';
      }
    } catch (e) {
      console.error(`${providerName} search failed:`, e);
      error = 'Search failed';
    } finally {
      loading = false;
    }
  }

  async function handleSelect(result: MetadataResult) {
    if (!result.external_id) return;

    linking = true;
    error = null;

    try {
      await onLink(result.external_id);
      onClose();
    } catch (e) {
      console.error(`Failed to link ${providerName}:`, e);
      error = e instanceof Error ? e.message : 'Failed to link';
      linking = false;
    }
  }
</script>

<div
  class="modal-backdrop"
  role="presentation"
  onclick={onClose}
  onkeydown={(e) => e.key === 'Escape' && onClose()}
>
  <div class="modal" role="presentation" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
    <div class="modal-header">
      <div class="modal-title">
        <span class="provider-badge">{badgeText}</span>
        Link {providerName}
      </div>
      <button class="icon-btn" type="button" aria-label="Close" onclick={onClose}>
        <span class="material-icons-outlined">close</span>
      </button>
    </div>

    <div class="modal-body">
      {#if error}
        <div class="error-banner">
          <span class="material-icons-outlined">error</span>
          <span>{error}</span>
          <button aria-label="Close error" onclick={() => (error = null)}>
            <span class="material-icons-outlined">close</span>
          </button>
        </div>
      {/if}

      <div class="search-section">
        <div class="search-fields">
          <input
            type="text"
            class="search-input"
            bind:value={searchQuery}
            placeholder="Book title..."
            onkeydown={(e) => e.key === 'Enter' && search()}
          />
          <input
            type="text"
            class="search-input author"
            bind:value={searchAuthor}
            placeholder="Author (optional)..."
            onkeydown={(e) => e.key === 'Enter' && search()}
          />
          <button
            type="button"
            class="btn-primary search-btn"
            onclick={search}
            disabled={loading || linking}
          >
            {loading ? 'Searching...' : 'Search'}
          </button>
        </div>
      </div>

      {#if results.length > 0}
        <div class="results-section">
          <div class="label">Select a match ({results.length} results)</div>
          <div class="results-list">
            {#each results as result, i (result.external_id || i)}
              <button
                type="button"
                class="search-result"
                onclick={() => handleSelect(result)}
                disabled={linking}
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
                    {result.authors?.join(', ') || 'Unknown Author'}
                    {#if result.publish_date}
                      &middot; {result.publish_date.substring(0, 4)}
                    {/if}
                    {#if result.series}
                      &middot; {result.series}
                      {#if result.series_index}{` #${result.series_index}`}{/if}
                    {/if}
                  </div>
                  {#if result.external_id || result.rating}
                    <div class="search-result-meta secondary">
                      {#if result.external_id}ID: {result.external_id}{/if}
                      {#if result.rating}
                        &middot; {result.rating.toFixed(2)} ★
                        {#if result.rating_count}({(result.rating_count / 1000).toFixed(0)}k){/if}
                      {/if}
                    </div>
                  {/if}
                </div>
                <span class="link-icon material-icons-outlined">link</span>
              </button>
            {/each}
          </div>
        </div>
      {:else if loading}
        <div class="loading-state">Searching {providerName}...</div>
      {:else if hasSearched}
        <div class="empty-state">
          <span class="material-icons-outlined">search_off</span>
          <p>No results found on {providerName}</p>
        </div>
      {:else}
        <div class="empty-state">
          <span class="material-icons-outlined">search</span>
          <p>Search {providerName} to find a match</p>
        </div>
      {/if}
    </div>

    <div class="modal-footer">
      <button type="button" class="btn-sec" onclick={onClose}>Cancel</button>
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.7);
    z-index: 200;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .modal {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    width: 550px;
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
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .provider-badge {
    display: inline-block;
    font-size: 10px;
    font-weight: 700;
    padding: 3px 6px;
    border-radius: 4px;
    background: var(--monitored);
    color: #000;
    text-transform: uppercase;
  }

  .modal-body {
    padding: 20px;
    overflow-y: auto;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .modal-footer {
    padding: 16px 20px;
    border-top: 1px solid var(--ring);
    display: flex;
    justify-content: flex-end;
    gap: 8px;
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

  .error-banner {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 16px;
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
    font-size: 13px;
    border-radius: 8px;
  }

  .error-banner button {
    margin-left: auto;
    border: none;
    background: transparent;
    color: var(--danger);
    cursor: pointer;
    padding: 0;
    display: flex;
  }

  .search-section {
    display: flex;
    flex-direction: column;
  }

  .search-fields {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }

  .search-input {
    flex: 1;
    min-width: 150px;
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

  .search-input.author {
    flex: 0.6;
    min-width: 120px;
  }

  .search-btn {
    flex-shrink: 0;
  }

  .label {
    font-size: 11px;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    margin-bottom: 8px;
  }

  .results-section {
    display: flex;
    flex-direction: column;
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

  .search-result:hover:not(:disabled) {
    border-color: var(--monitored);
    background: rgba(var(--monitored-rgb, 46, 204, 113), 0.1);
  }

  .search-result:disabled {
    opacity: 0.6;
    cursor: not-allowed;
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

  .source-tag {
    display: inline-block;
    font-size: 9px;
    font-weight: 700;
    padding: 2px 5px;
    border-radius: 3px;
    color: #000;
    text-transform: uppercase;
  }

  .link-icon {
    color: var(--muted);
    font-size: 20px;
    flex-shrink: 0;
    opacity: 0;
    transition: opacity 0.15s;
  }

  .search-result:hover .link-icon {
    opacity: 1;
    color: var(--monitored);
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
