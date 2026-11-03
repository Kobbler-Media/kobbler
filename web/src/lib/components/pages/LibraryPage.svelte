<script lang="ts">
  import { untrack } from 'svelte';
  import type { Book, LoadState, BookSelectHandler } from '#lib/types.js';
  import BookCard from '#lib/components/BookCard.svelte';
  import BidirectionalScroll from '#lib/components/BidirectionalScroll.svelte';
  import { createDelayedLoading } from '#lib/utils/delayed-loading.svelte.js';

  type BrowseFilter = 'all' | 'recent' | 'pending' | 'synced';
  type OwnershipFilter = 'all' | 'owned' | 'wanted' | 'orphaned';
  type ViewSize = 'normal' | 'large';

  interface Props {
    books: Book[];
    booksNextCursor: string | null;
    booksPrevCursor: string | null;
    booksTotal: number;
    selected: Set<string>;
    loading: boolean;
    loadState: LoadState;
    multiSelect: boolean;
    showRefresh: boolean;
    // URL-driven state
    filter?: BrowseFilter;
    ownership?: OwnershipFilter;
    searchQuery?: string;
    // Callbacks
    onToggleBook: BookSelectHandler;
    onLoadMore: () => void;
    onLoadPrevious: () => void;
    onRefresh: () => void;
    onToggleMultiSelect: () => void;
    onFilterChange?: (filter: BrowseFilter) => void;
    onOwnershipChange?: (ownership: OwnershipFilter) => void;
    /** Whether orphaned books exist (controls visibility of Orphaned filter) */
    hasOrphanedBooks?: boolean;
    onSearch?: (query: string) => void;
    // Jump to letter navigation (calls backend first-by-letter endpoint)
    onJumpToLetter?: (letter: string | null) => void;
    // Combined handler to avoid multiple goto() calls racing
    onNavigate?: (params: { filter?: BrowseFilter; ownership?: OwnershipFilter }) => void;
    // Available letters for the alphabet bar (from search backend)
    availableLetters?: string[];
    // Add new book callback
    onAdd?: () => void;
  }

  let {
    books,
    booksNextCursor,
    booksPrevCursor,
    booksTotal,
    selected,
    loading,
    loadState,
    multiSelect,
    showRefresh,
    filter = 'recent',
    ownership = 'all',
    searchQuery = '',
    onToggleBook,
    onLoadMore,
    onLoadPrevious,
    onRefresh,
    onToggleMultiSelect,
    onFilterChange,
    onOwnershipChange,
    onSearch,
    onJumpToLetter,
    onNavigate,
    availableLetters = [],
    onAdd,
    hasOrphanedBooks = false
  }: Props = $props();

  // Local search state for input binding
  let localSearchQuery = $state(untrack(() => searchQuery) || '');

  // Local UI state
  let viewSize: ViewSize = $state('normal');

  // Delayed loading for initial load (prevents flash)
  const initialLoadingState = createDelayedLoading(false);
  $effect(() => {
    initialLoadingState.loading = loadState.status === 'loading';
  });

  // Use props for view state (URL-driven)
  const browseFilter = $derived(filter);
  const ownershipFilter = $derived(ownership);

  const setFilter = (f: BrowseFilter) => {
    if (onNavigate) {
      onNavigate({ filter: f });
    } else {
      onFilterChange?.(f);
    }
  };

  const setOwnership = (o: OwnershipFilter) => {
    if (onNavigate) {
      onNavigate({ ownership: o });
    } else {
      onOwnershipChange?.(o);
    }
  };

  const handleLetterClick = (letter: string | null) => {
    onJumpToLetter?.(letter);
  };

  const ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ#'.split('');

  const isLoading = () => loadState.status === 'loading';
</script>

<div class="toolbar">
  <div class="toolbar-left">
    <div class="view-controls">
      <button
        class="view-tab"
        class:active={browseFilter === 'recent'}
        type="button"
        onclick={() => setFilter('recent')}
      >
        Recent
      </button>
      <button
        class="view-tab"
        class:active={browseFilter === 'all'}
        type="button"
        onclick={() => setFilter('all')}
      >
        All
      </button>
      <button
        class="view-tab"
        class:active={browseFilter === 'pending'}
        type="button"
        onclick={() => setFilter('pending')}
      >
        Pending
      </button>
      <button
        class="view-tab"
        class:active={browseFilter === 'synced'}
        type="button"
        onclick={() => setFilter('synced')}
      >
        Synced
      </button>
    </div>

    <div class="ownership-toggle">
      <button
        class="ownership-btn"
        class:active={ownershipFilter === 'all'}
        type="button"
        onclick={() => setOwnership('all')}
      >
        All
      </button>
      <button
        class="ownership-btn"
        class:active={ownershipFilter === 'owned'}
        type="button"
        onclick={() => setOwnership('owned')}
      >
        Owned
      </button>
      <button
        class="ownership-btn"
        class:active={ownershipFilter === 'wanted'}
        type="button"
        onclick={() => setOwnership('wanted')}
      >
        Wanted
      </button>
      {#if hasOrphanedBooks}
        <button
          class="ownership-btn"
          class:active={ownershipFilter === 'orphaned'}
          type="button"
          onclick={() => setOwnership('orphaned')}
        >
          Orphaned
        </button>
      {/if}
    </div>
  </div>

  <div class="toolbar-right">
    {#if onSearch}
      <div class="search-box">
        <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor" class="search-icon">
          <path
            d="M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z"
          ></path>
        </svg>
        <input
          type="text"
          placeholder="Search books..."
          value={localSearchQuery}
          oninput={(e) => {
            localSearchQuery = (e.target as HTMLInputElement).value;
            onSearch?.(localSearchQuery);
          }}
        />
        {#if localSearchQuery}
          <button
            type="button"
            class="clear-btn"
            aria-label="Clear search"
            onclick={() => {
              localSearchQuery = '';
              onSearch?.('');
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
    {/if}
    <button
      class="icon-btn"
      type="button"
      title={viewSize === 'normal' ? 'Switch to large view' : 'Switch to normal view'}
      aria-label="Toggle view size"
      onclick={() => (viewSize = viewSize === 'normal' ? 'large' : 'normal')}
    >
      <span class="material-icons-outlined" style="font-size: 16px;">
        {viewSize === 'normal' ? 'view_module' : 'grid_view'}
      </span>
    </button>

    <button
      class:active={multiSelect}
      class="icon-btn"
      type="button"
      aria-label="Toggle multi-select"
      title="Toggle multi-select"
      onclick={onToggleMultiSelect}
    >
      <span class="material-icons-outlined" style="font-size: 16px;">checklist</span>
    </button>

    {#if onAdd}
      <button
        class="icon-btn add-btn"
        type="button"
        title="Add book"
        aria-label="Add book"
        onclick={onAdd}
      >
        <span class="material-icons-outlined" style="font-size: 18px;">add</span>
      </button>
    {/if}

    {#if showRefresh}
      <button class="icon-btn" type="button" onclick={onRefresh} disabled={loading} title="Refresh">
        <span class="material-icons-outlined" style="font-size: 16px;">refresh</span>
      </button>
    {/if}
  </div>
</div>

{#if loadState.status === 'error'}
  <div class="error">{loadState.error}</div>
{/if}

{#if initialLoadingState.showLoading && books.length === 0}
  <div class="loading-card">Loading library…</div>
{:else if books.length === 0}
  <div class="empty-state">
    <h3>No books yet</h3>
    <p class="muted">Seed data or send a book from Calibre to get started.</p>
  </div>
{:else}
  <div class="section-header">
    <h2>
      {browseFilter === 'all'
        ? 'All Books'
        : browseFilter === 'recent'
          ? 'Recently Added'
          : browseFilter === 'pending'
            ? 'Pending Sync'
            : browseFilter === 'synced'
              ? 'Synced'
              : 'Wanted Books'}
    </h2>
    <div class="section-row">
      <span>{booksTotal} books</span>
    </div>
  </div>

  <div class="grid-container">
    <BidirectionalScroll
      hasMore={booksNextCursor !== null}
      hasPrevious={booksPrevCursor !== null}
      loading={isLoading()}
      {onLoadMore}
      {onLoadPrevious}
    >
      <section class="library-grid" class:large={viewSize === 'large'}>
        {#each books as book (book.id)}
          <BookCard {book} selected={selected.has(book.id)} onSelect={onToggleBook} />
        {/each}
      </section>
    </BidirectionalScroll>
  </div>

  <!-- Alpha Jump Bar (fixed right edge) -->
  {#if browseFilter === 'all'}
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

  .ownership-toggle {
    display: flex;
    gap: 2px;
    background: var(--card);
    padding: 4px;
    border: 1px solid var(--ring);
    border-radius: 8px;
  }

  .ownership-btn {
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

  .ownership-btn:hover {
    background: rgba(255, 255, 255, 0.05);
    color: var(--ink);
  }

  .ownership-btn.active {
    background: var(--accent-dim);
    color: var(--accent);
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

  /* Grid container - main scrolling content area */
  .grid-container {
    flex: 1;
    min-width: 0;
    /* Add right padding when alpha-jump is visible to prevent overlap */
    padding-right: 30px;
  }

  .library-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    gap: 12px;
    padding-bottom: 120px;
  }

  .library-grid.large {
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 14px;
  }

  /* Alpha Jump Bar - fixed to right edge of viewport */
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

  /* Component-specific empty/loading/error overrides */
  .empty-state {
    border: none;
    opacity: 0.7;
    margin-top: 24px;
  }

  .loading-card {
    border: none;
    background: none;
  }

  .error {
    background: rgba(220, 53, 69, 0.15);
    border: 1px solid rgba(220, 53, 69, 0.3);
    padding: 12px;
    margin-bottom: 16px;
    font-size: 13px;
    border-radius: 8px;
  }

  /* Icon button styling */
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

  .icon-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .icon-btn.active {
    background: var(--accent-dim);
    color: var(--accent);
    border-color: var(--accent);
  }

  @media (max-width: 768px) {
    .toolbar {
      flex-wrap: wrap;
      gap: 8px;
    }

    .toolbar-left {
      flex: 1 1 100%;
      justify-content: space-between;
    }

    .toolbar-right {
      flex: 1 1 100%;
    }

    .search-box {
      flex: 1;
      max-width: none;
    }

    .alpha-jump {
      display: none;
    }

    .library-grid {
      grid-template-columns: repeat(auto-fill, minmax(115px, 1fr));
      gap: 12px;
    }

    .library-grid.large {
      grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
      gap: 16px;
    }

    .grid-container {
      padding-right: 0;
    }
  }
</style>
