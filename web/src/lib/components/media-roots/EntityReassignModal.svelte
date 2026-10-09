<script lang="ts">
  import type { MovieSearchResult, TvSearchResult } from '$lib/types';
  import type { MetadataResult } from '$lib/api/client';
  import {
    bulkManualResolveMediaRootFiles,
    searchTvShows,
    searchMovies,
    searchBooks,
    searchSportsLeagues
  } from '$lib/api/client';
  import { showMessage } from '$lib/context/app.svelte';
  import { getJobsState } from '$lib/stores/jobs.svelte';

  interface Props {
    mediaType: 'tv' | 'movie' | 'book' | 'audiobook' | 'sport';
    fileIds: string[];
    groupLabel: string;
    open: boolean;
    onClose: () => void;
    onReassigned: () => void;
  }

  let { mediaType, fileIds, groupLabel, open, onClose, onReassigned }: Props = $props();

  let searchQuery = $state('');
  let searching = $state(false);
  let searchResults = $state<(MovieSearchResult | TvSearchResult | MetadataResult)[]>([]);
  let hasSearched = $state(false);
  let resolving = $state(false);
  let pendingJobId = $state<string | null>(null);
  const jobStore = getJobsState();

  const providerSearchLabel = $derived(
    mediaType === 'tv'
      ? 'TV shows'
      : mediaType === 'sport'
        ? 'leagues'
        : mediaType === 'book' || mediaType === 'audiobook'
          ? 'books'
          : 'movies'
  );

  const mediaIcon = $derived(
    mediaType === 'tv'
      ? 'tv'
      : mediaType === 'sport'
        ? 'sports_motorsports'
        : mediaType === 'book' || mediaType === 'audiobook'
          ? 'menu_book'
          : 'movie'
  );

  async function handleSearch() {
    const q = searchQuery.trim();
    if (!q) return;
    searching = true;
    hasSearched = true;
    searchResults = [];
    try {
      if (mediaType === 'tv') {
        searchResults = await searchTvShows(q);
      } else if (mediaType === 'sport') {
        searchResults = await searchSportsLeagues(q);
      } else if (mediaType === 'book' || mediaType === 'audiobook') {
        searchResults = await searchBooks(q);
      } else {
        searchResults = await searchMovies(q);
      }
    } catch {
      showMessage('Search failed');
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

  function resultPoster(
    result: MovieSearchResult | TvSearchResult | MetadataResult
  ): string | null {
    if ('poster_url' in result) return (result as MovieSearchResult).poster_url;
    return (result as MetadataResult).cover_url ?? null;
  }

  function resultYear(result: MovieSearchResult | TvSearchResult | MetadataResult): string | null {
    if ('year' in result && (result as MovieSearchResult).year != null) {
      return String((result as MovieSearchResult).year);
    }
    const pd = (result as MetadataResult).publish_date;
    return pd ? pd.slice(0, 4) : null;
  }

  async function handleSelect(result: MovieSearchResult | TvSearchResult | MetadataResult) {
    resolving = true;
    try {
      const isBook = 'authors' in result && !('genres' in result);
      const year = isBook
        ? ((result as MetadataResult).publish_date?.slice(0, 4) ?? null)
        : (result as MovieSearchResult | TvSearchResult).year;
      const candidate = {
        provider: result.provider,
        provider_id: result.external_id ?? '',
        title: result.title,
        year
      };
      const { job_id } = await bulkManualResolveMediaRootFiles(fileIds, candidate);
      pendingJobId = job_id;
      showMessage(`Reassigning ${fileIds.length} files to "${result.title}"...`);
    } catch {
      showMessage('Reassign failed');
      resolving = false;
    }
  }

  // Watch for job completion
  $effect(() => {
    if (!pendingJobId) return;
    const job = jobStore.jobs.find((j) => j.id === pendingJobId);
    if (!job) return;

    if (job.status === 'completed') {
      const r = (job.result ?? {}) as Record<string, unknown>;
      const resolved = typeof r.resolved === 'number' ? r.resolved : 0;
      const failed = typeof r.failed === 'number' ? r.failed : 0;
      if (resolved > 0) showMessage(`Reassigned ${resolved} file${resolved === 1 ? '' : 's'}`);
      if (failed > 0) showMessage(`${failed} failed to reassign`);
      pendingJobId = null;
      resolving = false;
      onReassigned();
      onClose();
    } else if (job.status === 'failed') {
      showMessage(job.error ?? 'Reassign failed');
      pendingJobId = null;
      resolving = false;
    }
  });

  function handleBackdropClick() {
    if (!resolving) onClose();
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="backdrop"
    onclick={handleBackdropClick}
    onkeydown={(e) => e.key === 'Escape' && !resolving && onClose()}
  >
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="dialog" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
      <div class="dialog-header">
        <h3>Reassign Entity</h3>
        <button class="close-btn" type="button" onclick={onClose} disabled={resolving}>
          <span class="material-icons-outlined">close</span>
        </button>
      </div>

      <div class="file-info">
        <div class="file-name">{groupLabel}</div>
        <div class="parsed">
          {fileIds.length} file{fileIds.length === 1 ? '' : 's'} will be reassigned
        </div>
      </div>

      <div class="search-row">
        <input
          type="text"
          class="search-input"
          placeholder="Search {providerSearchLabel}..."
          bind:value={searchQuery}
          onkeydown={handleKeydown}
        />
        <button
          class="btn-search"
          type="button"
          onclick={handleSearch}
          disabled={searching || !searchQuery.trim()}
        >
          {searching ? 'Searching...' : 'Search'}
        </button>
      </div>

      {#if searching}
        <div class="status-msg">Searching...</div>
      {:else if hasSearched && searchResults.length === 0}
        <div class="status-msg">No results found</div>
      {:else if searchResults.length > 0}
        <div class="candidates">
          {#each searchResults as result, i (result.provider + ':' + (result.external_id ?? i))}
            <button
              class="candidate-card"
              type="button"
              onclick={() => handleSelect(result)}
              disabled={resolving}
            >
              {#if resultPoster(result)}
                <img class="poster" src={resultPoster(result)} alt="" />
              {:else}
                <div class="poster placeholder">
                  <span class="material-icons-outlined">{mediaIcon}</span>
                </div>
              {/if}
              <div class="candidate-info">
                <div class="candidate-title">{result.title}</div>
                {#if 'authors' in result && result.authors?.length}
                  <div class="candidate-authors">{result.authors.join(', ')}</div>
                {/if}
                <div class="candidate-meta">
                  <span class="provider-tag">{result.provider}</span>
                  {#if resultYear(result)}
                    <span>{resultYear(result)}</span>
                  {/if}
                </div>
                {#if result.description}
                  <div class="candidate-desc">
                    {result.description.slice(0, 100)}{result.description.length > 100 ? '...' : ''}
                  </div>
                {/if}
              </div>
            </button>
          {/each}
        </div>
      {/if}

      {#if resolving}
        <div class="resolving-overlay">
          <span class="material-icons-outlined spin">sync</span>
          Reassigning {fileIds.length} file{fileIds.length === 1 ? '' : 's'}...
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    z-index: 200;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .dialog {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    max-width: 500px;
    width: 90vw;
    max-height: 85vh;
    overflow-y: auto;
    padding: 20px;
    position: relative;
  }

  .dialog-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 16px;
  }

  .dialog-header h3 {
    margin: 0;
    font-size: 16px;
    font-weight: 700;
    color: var(--ink);
  }

  .close-btn {
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

  .close-btn:hover {
    background: var(--bg);
    color: var(--ink);
  }

  .file-info {
    padding: 10px 12px;
    background: var(--bg);
    border: 1px solid var(--ring);
    border-radius: 10px;
    margin-bottom: 16px;
  }

  .file-name {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
  }

  .parsed {
    font-size: 11px;
    color: var(--muted);
    margin-top: 4px;
  }

  .candidates {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-height: 340px;
    overflow-y: auto;
  }

  .candidate-card {
    display: flex;
    gap: 10px;
    padding: 10px;
    border: 1px solid var(--ring);
    border-radius: 10px;
    background: var(--bg);
    cursor: pointer;
    text-align: left;
    color: inherit;
    font: inherit;
    transition:
      border-color 0.15s,
      background 0.15s;
    appearance: none;
  }

  .candidate-card:hover:not(:disabled) {
    border-color: var(--accent);
    background: var(--card);
  }

  .candidate-card:disabled {
    opacity: 0.5;
    cursor: wait;
  }

  .poster {
    width: 44px;
    height: 64px;
    border-radius: 6px;
    object-fit: cover;
    flex-shrink: 0;
  }

  .poster.placeholder {
    background: linear-gradient(140deg, #2e323a, #1a1d23);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--muted);
    font-size: 20px;
    opacity: 0.4;
  }

  .candidate-info {
    flex: 1;
    min-width: 0;
  }

  .candidate-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
  }

  .candidate-authors {
    font-size: 11px;
    color: var(--muted);
    font-style: italic;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .candidate-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--muted);
    margin-top: 2px;
  }

  .provider-tag {
    font-size: 9px;
    font-weight: 700;
    padding: 1px 5px;
    border-radius: 3px;
    background: var(--accent);
    color: #1d130c;
    text-transform: uppercase;
  }

  .candidate-desc {
    font-size: 11px;
    color: var(--muted);
    margin-top: 4px;
    line-height: 1.3;
  }

  .search-row {
    display: flex;
    gap: 8px;
    margin-bottom: 12px;
  }

  .search-input {
    flex: 1;
    padding: 10px 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg);
    color: var(--ink);
    font-size: 14px;
    outline: none;
  }

  .search-input:focus {
    border-color: var(--accent);
  }

  .btn-search {
    padding: 10px 16px;
    border-radius: 8px;
    border: none;
    background: var(--accent);
    color: #1d130c;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }

  .btn-search:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
  }

  .btn-search:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .status-msg {
    text-align: center;
    color: var(--muted);
    padding: 20px;
    font-size: 13px;
  }

  .resolving-overlay {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    border-radius: 16px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    color: var(--ink);
    font-size: 14px;
    font-weight: 600;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  .spin {
    animation: spin 1s linear infinite;
  }
</style>
