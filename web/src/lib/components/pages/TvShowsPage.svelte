<script lang="ts">
  import { untrack } from 'svelte';
  import type { TvShow } from '#lib/types.js';
  import TvShowCard from '#lib/components/tv/TvShowCard.svelte';
  import TvBatchActions from '#lib/components/tv/TvBatchActions.svelte';
  import BidirectionalScroll from '#lib/components/BidirectionalScroll.svelte';

  export type FilterValue = 'all' | 'monitored' | 'returning' | 'ended';
  export type SortValue = 'title' | 'next_airing' | 'created_at';

  interface Props {
    shows: TvShow[];
    loading: boolean;
    filter: FilterValue;
    sort: SortValue;
    searchQuery: string;
    onFilterChange: (filter: FilterValue) => void;
    onSortChange: (sort: SortValue) => void;
    onSearch: (query: string) => void;
    onSelectShow: (show: TvShow) => void;
    onAdd: () => void;
    onBulkUpdate?: (
      ids: string[],
      updates: { auto_want_mode?: string; quality_profile_id?: string | null }
    ) => void;
    availableLetters?: string[];
    onJumpToLetter?: (letter: string | null) => void;
    onLoadMore?: () => void;
    onLoadPrevious?: () => void;
    hasMore?: boolean;
    hasPrevious?: boolean;
    totalCount?: number | null;
  }

  let {
    shows,
    loading,
    filter,
    sort,
    searchQuery,
    onFilterChange,
    onSortChange,
    onSearch,
    onSelectShow,
    onAdd,
    onBulkUpdate,
    availableLetters = [],
    onJumpToLetter,
    onLoadMore,
    onLoadPrevious,
    hasMore = false,
    hasPrevious = false,
    totalCount = null
  }: Props = $props();

  let localSearchQuery = $state(untrack(() => searchQuery) || '');
  let selectionMode = $state(false);
  let selectedIds = $state(new Set<string>());

  const sectionTitle = $derived.by(() => {
    switch (filter) {
      case 'monitored':
        return 'Monitored Shows';
      case 'returning':
        return 'Returning Shows';
      case 'ended':
        return 'Ended Shows';
      default:
        return 'All Shows';
    }
  });

  const countDisplay = $derived(
    totalCount != null ? `${totalCount} shows` : `${shows.length} shows`
  );

  const showAlphaJump = $derived(sort === 'title' && !selectionMode && onJumpToLetter);

  const handleLetterClick = (letter: string | null) => {
    onJumpToLetter?.(letter);
  };

  function toggleSelectionMode() {
    selectionMode = !selectionMode;
    if (!selectionMode) {
      selectedIds = new Set();
    }
  }

  function handleToggleSelect(show: TvShow) {
    const next = new Set(selectedIds);
    if (next.has(show.id)) {
      next.delete(show.id);
    } else {
      next.add(show.id);
    }
    selectedIds = next;
  }

  function handleSelectAll() {
    selectedIds = new Set(shows.map((s) => s.id));
  }

  function handleClearSelection() {
    selectedIds = new Set();
  }

  function handleBulkApply(updates: {
    auto_want_mode?: string;
    quality_profile_id?: string | null;
  }) {
    if (selectedIds.size === 0) return;
    onBulkUpdate?.([...selectedIds], updates);
    selectionMode = false;
    selectedIds = new Set();
  }

  const ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ#'.split('');
</script>

<div class="tv-shows-page page-container">
  <div class="toolbar">
    <div class="toolbar-left">
      <div class="view-controls">
        <button
          class="view-tab"
          class:active={filter === 'all'}
          type="button"
          onclick={() => onFilterChange('all')}
        >
          All
        </button>
        <button
          class="view-tab"
          class:active={filter === 'monitored'}
          type="button"
          onclick={() => onFilterChange('monitored')}
        >
          Monitored
        </button>
        <button
          class="view-tab"
          class:active={filter === 'returning'}
          type="button"
          onclick={() => onFilterChange('returning')}
        >
          Returning
        </button>
        <button
          class="view-tab"
          class:active={filter === 'ended'}
          type="button"
          onclick={() => onFilterChange('ended')}
        >
          Ended
        </button>
      </div>
      <div class="sort-controls">
        <select
          class="sort-select"
          value={sort}
          onchange={(e) => onSortChange((e.target as HTMLSelectElement).value as SortValue)}
        >
          <option value="title">A-Z</option>
          <option value="next_airing">Next Airing</option>
          <option value="created_at">Recently Added</option>
        </select>
      </div>
    </div>

    <div class="toolbar-right">
      <div class="search-box">
        <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor" class="search-icon">
          <path
            d="M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z"
          ></path>
        </svg>
        <input
          type="text"
          placeholder="Search shows..."
          value={localSearchQuery}
          oninput={(e) => {
            localSearchQuery = (e.target as HTMLInputElement).value;
            onSearch(localSearchQuery);
          }}
        />
        {#if localSearchQuery}
          <button
            type="button"
            class="clear-btn"
            aria-label="Clear search"
            onclick={() => {
              localSearchQuery = '';
              onSearch('');
            }}
          >
            <svg viewBox="0 0 24 24" width="12" height="12" fill="currentColor">
              <path
                d="M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"
              ></path>
            </svg>
          </button>
        {/if}
      </div>

      <button
        class="icon-btn"
        class:active={selectionMode}
        type="button"
        title={selectionMode ? 'Exit selection' : 'Select shows'}
        aria-label={selectionMode ? 'Exit selection' : 'Select shows'}
        onclick={toggleSelectionMode}
      >
        <span class="material-icons-outlined" style="font-size: 18px;">
          {selectionMode ? 'close' : 'checklist'}
        </span>
      </button>

      <button
        class="icon-btn add-btn"
        type="button"
        title="Add TV show"
        aria-label="Add TV show"
        onclick={onAdd}
      >
        <span class="material-icons-outlined" style="font-size: 18px;">add</span>
      </button>
    </div>
  </div>

  {#if selectionMode}
    <TvBatchActions
      selectedCount={selectedIds.size}
      {loading}
      onSelectAll={handleSelectAll}
      onClearSelection={handleClearSelection}
      onApply={handleBulkApply}
    />
  {/if}

  {#if loading && shows.length === 0}
    <div class="loading-card">Loading shows...</div>
  {:else if shows.length === 0}
    <div class="empty-state">
      <h3>No TV shows</h3>
      <p class="muted">Add a TV show to get started.</p>
    </div>
  {:else}
    <div class="section-header">
      <h2>{sectionTitle}</h2>
      <span class="count">{countDisplay}</span>
    </div>

    <div class="grid-container" class:has-alpha={showAlphaJump}>
      <BidirectionalScroll
        {hasMore}
        {hasPrevious}
        {loading}
        onLoadMore={onLoadMore ?? (() => {})}
        onLoadPrevious={onLoadPrevious ?? (() => {})}
      >
        <section class="shows-grid">
          {#each shows as show (show.id)}
            <TvShowCard
              {show}
              {selectionMode}
              selected={selectedIds.has(show.id)}
              onSelect={onSelectShow}
              onToggleSelect={handleToggleSelect}
            />
          {/each}
        </section>
      </BidirectionalScroll>
    </div>

    <!-- Alpha Jump Bar (fixed right edge) -->
    {#if showAlphaJump}
      {@const availableSet = new Set(availableLetters)}
      <aside class="alpha-jump">
        <button type="button" class="alpha-btn" onclick={() => handleLetterClick(null)}>*</button>
        {#each ALPHABET as letter (letter)}
          <button
            type="button"
            class="alpha-btn"
            onclick={() => handleLetterClick(letter)}
            disabled={availableLetters.length > 0 && !availableSet.has(letter)}
          >
            {letter}
          </button>
        {/each}
      </aside>
    {/if}
  {/if}

  <style>
    .toolbar {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 12px;
      flex-wrap: wrap;
      margin-bottom: 16px;
    }

    .toolbar-left {
      display: flex;
      align-items: center;
      gap: 12px;
    }

    .toolbar-right {
      display: flex;
      align-items: center;
      gap: 8px;
    }

    .view-controls {
      display: flex;
      gap: 2px;
      background: var(--card);
      padding: 4px;
      border-radius: 8px;
      border: 1px solid var(--ring);
    }

    .view-tab {
      padding: 6px 12px;
      border-radius: 6px;
      font-size: 12px;
      font-weight: 600;
      cursor: pointer;
      color: var(--muted);
      border: none;
      background: none;
      transition: background 0.15s;
    }

    .view-tab:hover {
      background: rgba(255, 255, 255, 0.05);
      color: var(--ink);
    }

    .view-tab.active {
      background: var(--bg-2);
      color: var(--ink);
    }

    .sort-controls {
      display: flex;
      align-items: center;
    }

    .sort-select {
      appearance: none;
      background: var(--card);
      border: 1px solid var(--ring);
      border-radius: 6px;
      padding: 6px 28px 6px 10px;
      font-size: 12px;
      font-weight: 600;
      color: var(--muted);
      cursor: pointer;
      background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='6' viewBox='0 0 10 6'%3E%3Cpath d='M0 0l5 6 5-6z' fill='%23888'/%3E%3C/svg%3E");
      background-repeat: no-repeat;
      background-position: right 8px center;
    }

    .sort-select:hover {
      color: var(--ink);
      border-color: var(--accent);
    }

    .search-box {
      display: flex;
      align-items: center;
      gap: 8px;
      background: var(--bg);
      border: 1px solid var(--ring);
      border-radius: 10px;
      padding: 6px 10px;
      flex: 1;
      max-width: 280px;
    }

    .search-box:focus-within {
      border-color: var(--accent);
    }

    .search-box input {
      border: none;
      background: transparent;
      outline: none;
      font-size: 13px;
      color: var(--ink);
      flex: 1;
      min-width: 0;
    }

    .search-box input::placeholder {
      color: var(--muted);
    }

    .search-icon {
      color: var(--muted);
      flex-shrink: 0;
    }

    .clear-btn {
      display: flex;
      align-items: center;
      justify-content: center;
      background: transparent;
      border: none;
      color: var(--muted);
      cursor: pointer;
      padding: 2px;
      border-radius: 4px;
    }

    .clear-btn:hover {
      color: var(--ink);
      background: var(--bg-2);
    }

    .icon-btn {
      width: 32px;
      height: 32px;
      display: flex;
      align-items: center;
      justify-content: center;
      border: 1px solid var(--ring);
      border-radius: 6px;
      background: var(--card);
      color: var(--muted);
      cursor: pointer;
      transition: all 0.15s;
    }

    .icon-btn:hover:not(:disabled) {
      background: var(--bg-2);
      color: var(--ink);
    }

    .icon-btn.active {
      background: var(--accent-dim);
      color: var(--accent);
      border-color: var(--accent);
    }

    .section-header {
      display: flex;
      justify-content: space-between;
      align-items: center;
      margin-bottom: 16px;
      gap: 16px;
    }

    .section-header h2 {
      margin: 0;
      font-size: 18px;
    }

    .count {
      font-size: 13px;
      color: var(--muted);
    }

    .grid-container {
      flex: 1;
      min-width: 0;
    }

    .grid-container.has-alpha {
      padding-right: 30px;
    }

    .shows-grid {
      display: grid;
      grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
      gap: 12px;
      padding-bottom: 120px;
    }

    .alpha-jump {
      position: fixed;
      right: 8px;
      top: 50%;
      transform: translateY(-50%);
      display: flex;
      flex-direction: column;
      gap: 2px;
      z-index: 30;
      background: var(--card);
      padding: 4px;
      border-radius: 8px;
      border: 1px solid var(--ring);
    }

    .alpha-btn {
      width: 20px;
      height: 18px;
      display: flex;
      align-items: center;
      justify-content: center;
      border-radius: 4px;
      border: none;
      background: transparent;
      color: var(--muted);
      cursor: pointer;
      font-size: 10px;
      font-weight: 600;
      text-align: center;
      transition: background 0.15s;
    }

    .alpha-btn:hover:not(:disabled) {
      background: var(--accent-dim);
      color: var(--accent);
    }

    .alpha-btn:disabled {
      opacity: 0.3;
      cursor: not-allowed;
    }

    .empty-state {
      border: none;
      opacity: 0.7;
      margin-top: 24px;
    }

    .empty-state h3 {
      margin: 0 0 8px;
    }

    .muted {
      color: var(--muted);
      font-size: 13px;
    }

    .loading-card {
      border: none;
      background: none;
      color: var(--muted);
      padding: 24px;
    }

    @media (max-width: 768px) {
      .toolbar {
        flex-wrap: wrap;
        gap: 8px;
      }

      .toolbar-left {
        flex: 1 1 100%;
      }

      .toolbar-right {
        flex: 1 1 100%;
      }

      .search-box {
        flex: 1;
        max-width: none;
      }

      .shows-grid {
        grid-template-columns: repeat(auto-fill, minmax(115px, 1fr));
        gap: 12px;
      }

      .alpha-jump {
        display: none;
      }

      .grid-container.has-alpha {
        padding-right: 0;
      }
    }
  </style>
</div>
