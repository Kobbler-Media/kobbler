<script lang="ts">
  /**
   * VideoMetadataFetchModal - Fetches metadata for a season from all providers
   * where the parent show has identifiers. Unlike VideoMetadataSearchModal,
   * this doesn't search by title - it fetches details using known provider IDs.
   */
  import { Modal } from '@koblibs/ui';
  import { getVideoSeasonDetails } from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { formatCalendarDate, getProviderColor } from '#lib/utils/format.js';
  import type { SeasonDetailResult } from '#lib/types.js';

  interface Props {
    showIdentifiers: Record<string, unknown>;
    seasonNumber: number;
    onSelect: (results: SeasonDetailResult[]) => void;
    onCancel: () => void;
  }

  let { showIdentifiers, seasonNumber, onSelect, onCancel }: Props = $props();

  let fetching = $state(false);
  let results = $state<SeasonDetailResult[]>([]);
  let errors = $state<string[]>([]);
  let selectedIndices = $state<number[]>([]);

  // Map identifier keys to provider names
  const providerMap: Record<string, string> = {
    tmdb_id: 'tmdb',
    tvmaze_id: 'tvmaze',
    tvdb_id: 'tvdb'
  };

  const availableProviders = $derived(() => {
    const providers: { provider: string; id: string }[] = [];
    for (const [key, providerName] of Object.entries(providerMap)) {
      const id = showIdentifiers[key];
      if (id && String(id)) {
        providers.push({ provider: providerName, id: String(id) });
      }
    }
    return providers;
  });

  async function handleFetch() {
    const providers = availableProviders();
    if (providers.length === 0) {
      showMessage('No provider identifiers available');
      return;
    }

    fetching = true;
    results = [];
    errors = [];
    selectedIndices = [];

    const fetchResults = await Promise.allSettled(
      providers.map(({ provider, id }) => getVideoSeasonDetails(id, seasonNumber, provider))
    );

    const successResults: SeasonDetailResult[] = [];
    const fetchErrors: string[] = [];

    fetchResults.forEach((result, i) => {
      if (result.status === 'fulfilled') {
        successResults.push(result.value);
      } else {
        fetchErrors.push(`${providers[i].provider}: ${result.reason?.message || 'Failed'}`);
      }
    });

    results = successResults;
    errors = fetchErrors;
    // Auto-select all successful results
    selectedIndices = successResults.map((_, i) => i);
    fetching = false;

    if (successResults.length === 0) {
      showMessage('No providers returned data for this season');
    }
  }

  function toggleSelection(index: number) {
    if (selectedIndices.includes(index)) {
      selectedIndices = selectedIndices.filter((i) => i !== index);
    } else {
      selectedIndices = [...selectedIndices, index];
    }
  }

  function handleUseSelected() {
    const selected = selectedIndices.map((i) => results[i]);
    onSelect(selected);
  }

  // Auto-fetch on mount if providers are available
  $effect(() => {
    if (availableProviders().length > 0 && results.length === 0 && !fetching) {
      handleFetch();
    }
  });
</script>

<Modal title="Fetch Season {seasonNumber} Metadata" onClose={onCancel} width="600px">
  {#if fetching}
    <div class="fetch-status">
      <p>Fetching metadata from {availableProviders().length} provider(s)...</p>
    </div>
  {:else if results.length === 0 && availableProviders().length === 0}
    <div class="fetch-status">
      <p>No provider identifiers found on the parent show.</p>
      <p class="hint">Add TMDB, TVMaze, or TVDB identifiers to the show first.</p>
    </div>
  {:else if results.length === 0}
    <div class="fetch-status">
      <p>No providers returned data for this season.</p>
      {#each errors as err, i (i)}
        <p class="error-line">{err}</p>
      {/each}
    </div>
  {:else}
    <div class="results-list">
      {#each results as result, index (result.provider)}
        <button
          class="provider-result"
          class:selected={selectedIndices.includes(index)}
          type="button"
          onclick={() => toggleSelection(index)}
        >
          <div class="provider-header">
            <span class="provider-badge" style="background: {getProviderColor(result.provider)}"
              >{result.provider.toUpperCase()}</span
            >
            <span class="provider-title">{result.title || `Season ${result.season_number}`}</span>
            {#if selectedIndices.includes(index)}
              <span class="check-icon material-icons-outlined">check_circle</span>
            {/if}
          </div>
          <div class="provider-details">
            {#if result.air_date}
              <span class="detail">Aired: {formatCalendarDate(result.air_date, 'short')}</span>
            {/if}
            {#if result.episodes.length > 0}
              <span class="detail">{result.episodes.length} episodes</span>
            {/if}
            {#if result.poster_url}
              <span class="detail has-poster">Has poster</span>
            {/if}
          </div>
          {#if result.description}
            <p class="provider-desc">
              {result.description.slice(0, 150)}{result.description.length > 150 ? '...' : ''}
            </p>
          {/if}
        </button>
      {/each}
    </div>

    {#if errors.length > 0}
      <div class="errors-section">
        {#each errors as err, i (i)}
          <p class="error-line">{err}</p>
        {/each}
      </div>
    {/if}

    <div class="actions">
      <button class="btn-cancel" type="button" onclick={onCancel}>Cancel</button>
      <button
        class="btn-use"
        type="button"
        onclick={handleUseSelected}
        disabled={selectedIndices.length === 0}
      >
        Use Selected ({selectedIndices.length})
      </button>
    </div>
  {/if}
</Modal>

<style>
  .fetch-status {
    text-align: center;
    padding: 32px 16px;
    color: var(--muted);
    font-size: 14px;
  }

  .hint {
    font-size: 12px;
    margin-top: 8px;
  }

  .results-list {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .provider-result {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 14px;
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

  .provider-result:hover {
    border-color: var(--accent);
    background: var(--card);
  }

  .provider-result.selected {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-dim);
  }

  .provider-header {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .provider-badge {
    font-size: 10px;
    font-weight: 700;
    padding: 2px 6px;
    border-radius: 4px;
    color: #fff;
    letter-spacing: 0.5px;
  }

  .provider-title {
    font-size: 14px;
    font-weight: 600;
    flex: 1;
  }

  .check-icon {
    font-size: 18px;
    color: var(--accent);
  }

  .provider-details {
    display: flex;
    gap: 12px;
  }

  .detail {
    font-size: 12px;
    color: var(--muted);
  }

  .has-poster {
    color: var(--accent);
  }

  .provider-desc {
    font-size: 12px;
    color: var(--muted);
    line-height: 1.4;
    margin: 0;
  }

  .errors-section {
    margin-top: 12px;
    padding: 8px 12px;
    background: rgba(239, 68, 68, 0.1);
    border-radius: 8px;
  }

  .error-line {
    font-size: 12px;
    color: var(--error, #ef4444);
    margin: 2px 0;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
    padding-top: 16px;
    border-top: 1px solid var(--ring);
  }

  .btn-cancel {
    padding: 10px 20px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: var(--bg-2);
    color: var(--ink);
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
  }

  .btn-cancel:hover {
    border-color: var(--accent);
  }

  .btn-use {
    padding: 10px 20px;
    border-radius: 10px;
    border: none;
    background: var(--accent);
    color: #1d130c;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-use:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
  }

  .btn-use:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
</style>
