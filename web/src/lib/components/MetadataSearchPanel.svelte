<script lang="ts">
  import { untrack } from 'svelte';
  import { type MetadataResult } from '$lib/api/client';
  import {
    MetadataBookSearch,
    MetadataSeriesSearch,
    MetadataAuthorSearch,
    type SeriesResult,
    type AuthorResult
  } from './metadata';

  interface Props {
    initialTitle?: string;
    initialAuthor?: string;
    initialIsbn?: string;
    initialSeries?: string;
    /** Restrict to specific search mode (if provided) */
    mode?: 'book' | 'series' | 'author';
    onApply: (result: MetadataResult) => Promise<void>;
    onApplySeries?: (seriesName: string, seriesIndex: number | null) => Promise<void>;
    /** Callback for linking a series to a metadata provider (sets hardcover ID, etc.) */
    onApplySeriesLink?: (result: SeriesResult) => Promise<void>;
    /** Callback for applying author metadata (provider_id, image_url, bio) */
    onApplyAuthor?: (result: AuthorResult) => Promise<void>;
    onClose: () => void;
  }

  let {
    initialTitle = '',
    initialAuthor = '',
    initialIsbn = '',
    initialSeries = '',
    mode: restrictedMode,
    onApply,
    onApplySeries,
    onApplySeriesLink,
    onApplyAuthor,
    onClose
  }: Props = $props();

  type SearchMode = 'book' | 'series' | 'author';
  let searchMode = $state<SearchMode>(untrack(() => restrictedMode) || 'book');

  const switchMode = (mode: SearchMode) => {
    searchMode = mode;
  };
</script>

<div class="search-panel">
  <div class="panel-header">
    <h3>Search Metadata</h3>
    <button class="icon-btn" type="button" aria-label="Close" onclick={onClose}>
      <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor">
        <path d="M6.4 5.6 5 7l5 5-5 5 1.4 1.4 5-5 5 5L17.8 17l-5-5 5-5-1.4-1.4-5 5-5-5z"></path>
      </svg>
    </button>
  </div>

  <!-- Mode toggle (hidden if mode is restricted) -->
  {#if !restrictedMode}
    <div class="mode-toggle">
      <button
        class="mode-btn"
        class:active={searchMode === 'book'}
        type="button"
        onclick={() => switchMode('book')}
      >
        Book
      </button>
      <button
        class="mode-btn"
        class:active={searchMode === 'series'}
        type="button"
        onclick={() => switchMode('series')}
      >
        Series
      </button>
      <button
        class="mode-btn"
        class:active={searchMode === 'author'}
        type="button"
        onclick={() => switchMode('author')}
      >
        Author
      </button>
    </div>
  {/if}

  {#if searchMode === 'book'}
    <MetadataBookSearch {initialTitle} {initialAuthor} {initialIsbn} {onApply} />
  {:else if searchMode === 'series'}
    <MetadataSeriesSearch {initialSeries} {onApplySeries} {onApplySeriesLink} />
  {:else if searchMode === 'author'}
    <MetadataAuthorSearch {initialAuthor} {onApplyAuthor} />
  {/if}
</div>

<style>
  .search-panel {
    display: grid;
    gap: 16px;
    max-height: 70vh;
    overflow-y: auto;
    padding-inline: 2px;
  }

  .panel-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }

  .panel-header h3 {
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
  }

  .icon-btn:hover {
    background: var(--ring);
    color: var(--ink);
  }

  /* Mode toggle */
  .mode-toggle {
    display: flex;
    gap: 4px;
    padding: 4px;
    background: var(--ring);
    border-radius: 10px;
  }

  .mode-btn {
    flex: 1;
    padding: 6px 12px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--muted);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition:
      background 0.15s,
      color 0.15s;
  }

  .mode-btn:hover {
    color: var(--ink);
  }

  .mode-btn.active {
    background: var(--bg);
    color: var(--ink);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.1);
  }
</style>
