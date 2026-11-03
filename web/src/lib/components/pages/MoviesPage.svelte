<script lang="ts">
  import { untrack } from 'svelte';
  import type { Movie } from '#lib/types.js';
  import MovieCard from '#lib/components/movies/MovieCard.svelte';
  import BidirectionalScroll from '#lib/components/BidirectionalScroll.svelte';

  type MovieFilter = 'all' | 'wanted' | 'in_library';
  export type MovieSortValue = 'title' | 'created_at';

  interface Props {
    movies: Movie[];
    loading: boolean;
    filter: MovieFilter;
    searchQuery: string;
    onFilterChange: (filter: MovieFilter) => void;
    sort: MovieSortValue;
    onSortChange: (sort: MovieSortValue) => void;
    onSearch: (query: string) => void;
    onSelectMovie: (movie: Movie) => void;
    onAdd: () => void;
    selectedMovieId?: string | null;
    availableLetters?: string[];
    onJumpToLetter?: (letter: string | null) => void;
    onLoadMore?: () => void;
    onLoadPrevious?: () => void;
    hasMore?: boolean;
    hasPrevious?: boolean;
    totalCount?: number | null;
  }

  let {
    movies,
    loading,
    filter,
    searchQuery,
    onFilterChange,
    sort,
    onSortChange,
    onSearch,
    onSelectMovie,
    onAdd,
    selectedMovieId = null,
    availableLetters = [],
    onJumpToLetter,
    onLoadMore,
    onLoadPrevious,
    hasMore = false,
    hasPrevious = false,
    totalCount = null
  }: Props = $props();

  let localSearchQuery = $state(untrack(() => searchQuery) || '');

  const sectionTitle = $derived(
    filter === 'wanted' ? 'Wanted Movies' : filter === 'in_library' ? 'In Library' : 'All Movies'
  );

  const countDisplay = $derived(
    totalCount != null ? `${totalCount} movies` : `${movies.length} movies`
  );

  const handleLetterClick = (letter: string | null) => {
    onJumpToLetter?.(letter);
  };

  const ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ#'.split('');
</script>

<div class="movies-page page-container">
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
          class:active={filter === 'wanted'}
          type="button"
          onclick={() => onFilterChange('wanted')}
        >
          Wanted
        </button>
        <button
          class="view-tab"
          class:active={filter === 'in_library'}
          type="button"
          onclick={() => onFilterChange('in_library')}
        >
          In Library
        </button>
      </div>

      <div class="sort-controls">
        <select
          class="sort-select"
          value={sort}
          onchange={(e) => onSortChange((e.target as HTMLSelectElement).value as MovieSortValue)}
        >
          <option value="title">A-Z</option>
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
          placeholder="Search movies..."
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
        class="icon-btn add-btn"
        type="button"
        title="Add movie"
        aria-label="Add movie"
        onclick={onAdd}
      >
        <span class="material-icons-outlined" style="font-size: 18px;">add</span>
      </button>
    </div>
  </div>

  {#if loading && movies.length === 0}
    <div class="loading-card">Loading movies...</div>
  {:else if movies.length === 0}
    <div class="empty-state">
      {#if localSearchQuery.trim()}
        <h3>No results</h3>
        <p class="muted">No movies match your search.</p>
      {:else}
        <h3>No movies yet</h3>
        <p class="muted">Add a movie to get started.</p>
      {/if}
    </div>
  {:else}
    <div class="section-header">
      <h2>{sectionTitle}</h2>
      <span class="count">{countDisplay}</span>
    </div>

    <div class="grid-container">
      <BidirectionalScroll
        {hasMore}
        {hasPrevious}
        {loading}
        onLoadMore={onLoadMore ?? (() => {})}
        onLoadPrevious={onLoadPrevious ?? (() => {})}
      >
        <section class="movie-grid">
          {#each movies as movie (movie.id)}
            <MovieCard {movie} selected={selectedMovieId === movie.id} onSelect={onSelectMovie} />
          {/each}
        </section>
      </BidirectionalScroll>
    </div>

    <!-- Alpha Jump Bar (fixed right edge) -->
    {#if filter === 'all' && sort === 'title' && onJumpToLetter}
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

    .icon-btn:hover {
      background: var(--bg-2);
      color: var(--ink);
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
      color: var(--muted);
      font-size: 13px;
    }

    .grid-container {
      flex: 1;
      min-width: 0;
      padding-right: 30px;
    }

    .movie-grid {
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

      .movie-grid {
        grid-template-columns: repeat(auto-fill, minmax(115px, 1fr));
        gap: 12px;
      }

      .alpha-jump {
        display: none;
      }

      .grid-container {
        padding-right: 0;
      }
    }
  </style>
</div>
