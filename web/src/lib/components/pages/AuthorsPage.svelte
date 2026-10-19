<script lang="ts">
  /**
   * AuthorsPage - Dumb component that displays a grid of authors.
   * All sidebar/navigation logic is handled by the route page.
   */
  import { untrack } from 'svelte';
  import type { AuthorSummary, LoadState } from '$lib/types';
  import BidirectionalScroll from '$lib/components/BidirectionalScroll.svelte';
  import MetaLine from '$lib/components/shared/MetaLine.svelte';

  interface Props {
    authors: AuthorSummary[];
    authorsNextCursor: string | null;
    authorsPrevCursor: string | null;
    authorsTotal: number;
    loadStateAuthors: LoadState;
    searchQuery?: string;
    selectedId?: string | null;
    onSelectAuthor: (author: AuthorSummary) => void;
    onLoadMore: () => void;
    onLoadPrevious: () => void;
    onSearch?: (query: string) => void;
    onAdd?: () => void;
  }

  let {
    authors,
    authorsNextCursor,
    authorsPrevCursor,
    authorsTotal,
    loadStateAuthors,
    searchQuery = '',
    selectedId,
    onSelectAuthor,
    onLoadMore,
    onLoadPrevious,
    onSearch,
    onAdd
  }: Props = $props();

  let localSearchQuery = $state(untrack(() => searchQuery) || '');

  const isLoading = () => loadStateAuthors.status === 'loading';
</script>

<div class="page-content page-container">
  {#if loadStateAuthors.status === 'loading' && authors.length === 0}
    <div class="loading-card">Loading authors…</div>
  {:else if loadStateAuthors.status === 'error'}
    <div class="error">{loadStateAuthors.error}</div>
  {:else}
    <div class="section-header">
      <div class="header-left">
        <h2>Authors</h2>
        {#if onSearch}
          <div class="search-box">
            <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor" class="search-icon">
              <path
                d="M15.5 14h-.79l-.28-.27A6.471 6.471 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z"
              ></path>
            </svg>
            <input
              type="text"
              placeholder="Search authors..."
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
      </div>
      <div class="section-row">
        <span>{authorsTotal} authors{searchQuery ? ` matching "${searchQuery}"` : ''}</span>
        {#if onAdd}
          <button
            class="icon-btn add-btn"
            type="button"
            title="Add author"
            aria-label="Add author"
            onclick={onAdd}
          >
            <span class="material-icons-outlined">add</span>
          </button>
        {/if}
      </div>
    </div>
    {#if authors.length === 0}
      <div class="empty-state">
        <h3>No authors yet</h3>
        <p class="muted">Authors will appear as books are added.</p>
      </div>
    {:else}
      <BidirectionalScroll
        hasMore={authorsNextCursor !== null}
        hasPrevious={authorsPrevCursor !== null}
        loading={isLoading()}
        {onLoadMore}
        {onLoadPrevious}
      >
        <section class="author-grid">
          {#each authors as author (author.id)}
            <button
              class:selected={selectedId === author.id}
              class="author-card"
              type="button"
              data-author-id={author.id}
              onclick={() => onSelectAuthor(author)}
            >
              {#if author.is_monitored}
                <span class="monitored-indicator">
                  <span class="material-icons-outlined">star</span>
                </span>
              {/if}
              <div class="author-avatar">
                {#if author.image_url}
                  <div
                    class="author-image"
                    style="background-image: url({author.image_url});"
                  ></div>
                {:else}
                  <div class="author-placeholder">
                    <span class="material-icons-outlined">person</span>
                  </div>
                {/if}
              </div>
              <div class="author-info">
                <div class="meta-primary">{author.name}</div>
                <div class="meta-secondary">
                  <MetaLine
                    items={[
                      author.total_book_count > author.book_count
                        ? `${author.book_count} of ${author.total_book_count} books`
                        : `${author.book_count} books`,
                      author.total_series_count ? `${author.total_series_count} series` : null
                    ]}
                  />
                </div>
              </div>
            </button>
          {/each}
        </section>
      </BidirectionalScroll>
    {/if}
  {/if}
</div>

<style>
  .page-content {
    flex: 1;
    min-width: 0;
  }

  .section-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
    margin-bottom: 16px;
  }

  .section-header h2 {
    font-size: 18px;
    margin: 0;
  }

  .header-left {
    display: flex;
    align-items: center;
    gap: 16px;
  }

  .section-row {
    display: flex;
    align-items: center;
    gap: 12px;
    color: var(--muted);
    font-size: 14px;
  }

  .add-btn {
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    padding: 0;
    transition: all 0.15s;
  }

  .add-btn:hover {
    background: var(--bg-2);
    color: var(--ink);
  }

  .add-btn .material-icons-outlined {
    font-size: 18px;
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

  .search-icon {
    color: var(--muted);
    flex-shrink: 0;
  }

  .search-box input {
    flex: 1;
    border: none;
    background: transparent;
    color: var(--ink);
    font-size: 13px;
    outline: none;
    min-width: 0;
  }

  .search-box input::placeholder {
    color: var(--muted);
  }

  .clear-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    background: none;
    border: none;
    padding: 2px;
    cursor: pointer;
    color: var(--muted);
    border-radius: 4px;
  }

  .clear-btn:hover {
    background: var(--ring);
    color: var(--ink);
  }

  /* Author Grid - vertical centered cards */
  .author-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 16px;
  }

  .author-card {
    appearance: none;
    text-align: center;
    border-radius: 16px;
    padding: 20px 16px;
    border: 1px solid var(--ring);
    background: var(--card);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    cursor: pointer;
    color: inherit;
    font: inherit;
    position: relative;
    transition: border-color 0.15s;
  }

  .author-card:hover {
    border-color: var(--muted);
  }

  .author-card.selected {
    border-color: var(--accent);
  }

  .author-avatar {
    width: 100px;
    height: 100px;
    border-radius: 50%;
    overflow: hidden;
    flex-shrink: 0;
  }

  .author-image {
    width: 100%;
    height: 100%;
    background-size: cover;
    background-position: center;
  }

  .author-placeholder {
    width: 100%;
    height: 100%;
    background: var(--bg-2);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--muted);
  }

  .author-placeholder .material-icons-outlined {
    font-size: 36px;
  }

  .author-info {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  .meta-primary {
    font-weight: 700;
    font-size: 14px;
    line-height: 1.3;
    word-break: break-word;
  }

  .meta-secondary {
    font-size: 12px;
    color: var(--muted);
    --meta-line-justify: center;
  }

  .monitored-indicator {
    position: absolute;
    top: 12px;
    right: 12px;
    color: var(--monitored);
  }

  .monitored-indicator .material-icons-outlined {
    font-size: 18px;
  }

  .loading-card {
    padding: 24px;
    text-align: center;
    color: var(--muted);
  }

  .error {
    background: rgba(220, 53, 69, 0.15);
    border: 1px solid rgba(220, 53, 69, 0.3);
    border-radius: 8px;
    padding: 12px;
    margin-bottom: 16px;
    font-size: 13px;
  }

  .empty-state {
    text-align: center;
    padding: 48px 24px;
    color: var(--muted);
  }

  .empty-state h3 {
    margin: 0 0 8px;
    font-size: 16px;
    color: var(--ink);
  }

  .empty-state p {
    margin: 0;
    font-size: 14px;
  }

  @media (max-width: 768px) {
    .author-grid {
      grid-template-columns: 1fr 1fr;
    }
  }
</style>
