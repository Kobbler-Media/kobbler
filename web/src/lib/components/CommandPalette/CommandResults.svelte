<script lang="ts">
  import type { GroupedResults, SearchResult } from './types';
  import ResultItem from './ResultItem.svelte';
  import { flattenResults } from './search';

  interface Props {
    results: GroupedResults;
    selectedIndex: number;
    onSelect: (result: SearchResult) => void;
    isEmptyQuery?: boolean;
  }

  let { results, selectedIndex, onSelect, isEmptyQuery = false }: Props = $props();

  const flatResults = $derived(flattenResults(results));

  function isSelectedByIndex(idx: number): boolean {
    return idx === selectedIndex;
  }

  function getGroupLabel(key: keyof Omit<GroupedResults, 'topHits'>): string {
    switch (key) {
      case 'pages':
        return 'Pages';
      case 'commands':
        return 'Commands';
      case 'books':
        return 'Books';
      case 'ingest':
        return 'Ingest Queue';
      case 'authors':
        return 'Authors';
      case 'series':
        return 'Series';
      case 'movies':
        return 'Movies';
      case 'tv_shows':
        return 'TV Shows';
      case 'tv_seasons':
        return 'Seasons';
      case 'tv_episodes':
        return 'Episodes';
      case 'sections':
        return 'Sections';
      case 'externalSearch':
        return 'Search Providers';
      case 'transcripts':
        return 'Transcripts';
      case 'iptv_channels':
        return 'IPTV';
      case 'livetv_channels':
        return 'Live TV';
      case 'settings':
        return 'Settings';
    }
  }

  const hasResults = $derived(flatResults.length > 0);
  const hasTopHits = $derived(results.topHits.length > 0);

  // Groups for remaining results (after topHits)
  const remainingGroupOrder: (keyof Omit<GroupedResults, 'topHits'>)[] = [
    'pages',
    'commands',
    'settings',
    'authors',
    'series',
    'books',
    'ingest',
    'movies',
    'tv_shows',
    'tv_seasons',
    'tv_episodes',
    'sections',
    'externalSearch',
    'transcripts',
    'iptv_channels',
    'livetv_channels'
  ];

  // Check if any remaining groups have content
  const hasRemainingResults = $derived(remainingGroupOrder.some((key) => results[key].length > 0));

  // Calculate the starting index for each section
  function getStartingIndex(groupKey: keyof Omit<GroupedResults, 'topHits'>): number {
    let idx = results.topHits.length;
    for (const key of remainingGroupOrder) {
      if (key === groupKey) return idx;
      idx += results[key].length;
    }
    return idx;
  }
</script>

<div class="results-container">
  {#if hasResults}
    <!-- Top Hits section: best match per type, ordered by relevance (or Recent for empty query) -->
    {#if hasTopHits}
      <div class="result-group top-hits">
        <div class="group-header">{isEmptyQuery ? 'Recent' : 'Best Matches'}</div>
        {#each results.topHits as result, idx (result.type + ':' + result.id)}
          <ResultItem
            {result}
            selected={isSelectedByIndex(idx)}
            onSelect={() => onSelect(result)}
          />
        {/each}
      </div>
    {/if}

    <!-- Remaining results by type -->
    {#if hasRemainingResults}
      {#if hasTopHits}
        <div class="section-divider"></div>
      {/if}
      {#each remainingGroupOrder as groupKey (groupKey)}
        {#if results[groupKey].length > 0}
          <div class="result-group">
            <div class="group-header">{getGroupLabel(groupKey)}</div>
            {#each results[groupKey] as result, idx (result.type + ':' + result.id)}
              <ResultItem
                {result}
                selected={isSelectedByIndex(getStartingIndex(groupKey) + idx)}
                onSelect={() => onSelect(result)}
              />
            {/each}
          </div>
        {/if}
      {/each}
    {/if}
  {:else}
    <div class="no-results">
      {#if isEmptyQuery}
        <span>Type to search your library</span>
        <span class="hint">Use prefixes to narrow your search:</span>
      {:else}
        <span>No results found</span>
        <span class="hint">Try a different search term or use prefixes:</span>
      {/if}
      <div class="prefix-hints">
        <span><code>&gt;</code> Commands</span>
        <span><code>@</code> Authors</span>
        <span><code>:</code> Series</span>
        <span><code>/</code> Pages</span>
      </div>
    </div>
  {/if}
</div>

<style>
  .results-container {
    max-height: 400px;
    overflow-y: auto;
    padding: 8px;
  }

  .result-group {
    margin-bottom: 8px;
  }

  .result-group:last-child {
    margin-bottom: 0;
  }

  .result-group.top-hits .group-header {
    color: var(--accent);
  }

  .group-header {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--muted);
    padding: 8px 16px 4px;
  }

  .section-divider {
    height: 1px;
    background: var(--ring);
    margin: 8px 16px;
    opacity: 0.5;
  }

  .no-results {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 32px 16px;
    color: var(--muted);
    text-align: center;
  }

  .no-results span:first-child {
    font-size: 14px;
    color: var(--ink);
  }

  .hint {
    font-size: 12px;
  }

  .prefix-hints {
    display: flex;
    gap: 16px;
    margin-top: 8px;
    font-size: 12px;
  }

  .prefix-hints code {
    background: rgba(255, 255, 255, 0.1);
    padding: 2px 6px;
    border-radius: 4px;
    font-family: monospace;
    margin-right: 4px;
  }

  :global([data-theme='light']) .prefix-hints code {
    background: rgba(0, 0, 0, 0.08);
  }
</style>
