<script lang="ts">
  import type { Book, ShelfSummary, LoadState } from '#lib/types.js';
  import { InfiniteScroll } from '@koblibs/ui';
  import { ExpandableSidebar } from '@koblibs/ui';
  import * as api from '#lib/api/client.js';

  type SelectShelfHandler = (shelfId: string | null) => Promise<void>;
  type CreateShelfHandler = (name: string) => Promise<void>;
  type RenameShelfHandler = (shelfId: string, newName: string) => Promise<void>;
  type DeleteShelfHandler = (shelfId: string) => Promise<void>;
  type OpenInLibraryHandler = (book: Book) => void;
  type AddBookHandler = (shelfId: string, bookId: string) => Promise<void>;
  type RemoveBookHandler = (shelfId: string, bookId: string) => Promise<void>;

  interface Props {
    shelves: ShelfSummary[];
    shelvesNextCursor: string | null;
    shelvesTotal: number;
    shelfItems: Book[];
    loading: boolean;
    loadStateShelves: LoadState;
    loadStateShelfItems: LoadState;
    onSelectShelf: SelectShelfHandler;
    onLoadMore: () => void;
    onCreateShelf: CreateShelfHandler;
    onRenameShelf: RenameShelfHandler;
    onDeleteShelf: DeleteShelfHandler;
    onOpenInLibrary: OpenInLibraryHandler;
    onAddBook?: AddBookHandler;
    onRemoveBook?: RemoveBookHandler;
  }

  let {
    shelves,
    shelvesNextCursor,
    shelvesTotal,
    shelfItems,
    loading,
    loadStateShelves,
    loadStateShelfItems,
    onSelectShelf,
    onLoadMore,
    onCreateShelf,
    onRenameShelf,
    onDeleteShelf,
    onOpenInLibrary,
    onAddBook,
    onRemoveBook
  }: Props = $props();

  // Local UI state
  let selectedShelfId = $state<string | null>(null);
  let showNewShelfForm = $state(false);
  let newShelfName = $state('');
  let editingShelfName = $state('');
  let editingDescription = $state('');
  let sidebarExpanded = $state(false);

  // Book search state
  let bookSearchQuery = $state('');
  let bookSearchResults = $state<Book[]>([]);
  let bookSearchLoading = $state(false);
  let showBookSearch = $state(false);

  // Search for books to add
  const searchBooks = async (query: string) => {
    if (!query.trim()) {
      bookSearchResults = [];
      return;
    }
    bookSearchLoading = true;
    try {
      const response = await api.getBooks({ limit: 10, search: query });
      // Filter out books already in the shelf
      const existingIds = new Set(shelfItems.map((b) => b.id));
      bookSearchResults = response.items.filter((b) => !existingIds.has(b.id));
    } catch {
      bookSearchResults = [];
    } finally {
      bookSearchLoading = false;
    }
  };

  const handleAddBook = async (bookId: string) => {
    if (!selectedShelfId || !onAddBook) return;
    await onAddBook(selectedShelfId, bookId);
    bookSearchQuery = '';
    bookSearchResults = [];
    showBookSearch = false;
  };

  const handleRemoveBook = async (e: Event, bookId: string) => {
    e.stopPropagation();
    if (!selectedShelfId || !onRemoveBook) return;
    await onRemoveBook(selectedShelfId, bookId);
  };

  const selectedShelf = $derived(
    selectedShelfId ? shelves.find((s) => s.id === selectedShelfId) : null
  );

  const selectShelf = async (shelfId: string | null) => {
    // Toggle selection if clicking the same shelf
    if (shelfId && selectedShelfId === shelfId) {
      closeSidebar();
      return;
    }
    selectedShelfId = shelfId;
    if (shelfId) {
      const shelf = shelves.find((s) => s.id === shelfId);
      if (shelf) {
        editingShelfName = shelf.name;
        editingDescription = '';
      }
      await onSelectShelf(shelfId);
    }
  };

  const closeSidebar = () => {
    selectedShelfId = null;
    sidebarExpanded = false;
  };

  const handleCreateShelf = async () => {
    if (!newShelfName.trim()) return;
    await onCreateShelf(newShelfName.trim());
    newShelfName = '';
    showNewShelfForm = false;
  };

  const handleRenameShelf = async () => {
    if (!selectedShelfId || !editingShelfName.trim()) return;
    await onRenameShelf(selectedShelfId, editingShelfName.trim());
  };

  const handleDeleteShelf = async () => {
    if (!selectedShelfId) return;
    if (!confirm('Are you sure you want to delete this shelf?')) return;
    await onDeleteShelf(selectedShelfId);
    closeSidebar();
  };

  const isLoading = () => loadStateShelves.status === 'loading';

  const formatBookMeta = (book: Book): string => {
    const parts: string[] = [];
    if (book.authors.length > 0) {
      parts.push(book.authors.map((a) => a.name).join(', '));
    }
    if (book.series_name) {
      const idx = book.series_index;
      parts.push(idx ? `${book.series_name} #${idx}` : book.series_name);
    }
    return parts.join(' · ');
  };
</script>

<div class="page-layout page-container">
  <div class="library-main">
    {#if loadStateShelves.status === 'loading' && shelves.length === 0}
      <div class="loading-card">Loading shelves…</div>
    {:else if loadStateShelves.status === 'error'}
      <div class="error">{loadStateShelves.error}</div>
    {:else}
      <div class="page-header">
        <h2>Shelves</h2>
        <div class="header-actions">
          <span class="count-label">{shelvesTotal} shelves</span>
          <button
            class="btn-sec"
            type="button"
            onclick={() => (showNewShelfForm = !showNewShelfForm)}
          >
            <span class="material-icons-outlined">add</span>
            {showNewShelfForm ? 'Cancel' : 'New Shelf'}
          </button>
        </div>
      </div>

      {#if showNewShelfForm}
        <form
          class="new-shelf-form"
          onsubmit={(e) => {
            e.preventDefault();
            handleCreateShelf();
          }}
        >
          <input placeholder="Shelf name" bind:value={newShelfName} />
          <button class="btn-primary" type="submit" disabled={loading}>Create</button>
        </form>
      {/if}

      {#if shelves.length === 0}
        <div class="empty-state">
          <span class="material-icons-outlined empty-icon">collections_bookmark</span>
          <h3>No shelves yet</h3>
          <p class="muted">Create a shelf to organize your books.</p>
        </div>
      {:else}
        <section class="shelf-grid">
          {#each shelves as shelf (shelf.id)}
            <button
              class="shelf-card"
              class:selected={selectedShelfId === shelf.id}
              type="button"
              onclick={() => selectShelf(shelf.id)}
            >
              <div class="shelf-covers">
                {#if shelf.cover_urls.length === 0}
                  <div class="mini-cover placeholder">
                    <span class="material-icons-outlined">auto_stories</span>
                  </div>
                {:else}
                  {#each shelf.cover_urls.slice(0, 4) as url, i (`${i}-${url}`)}
                    <div class="mini-cover" style:background-image="url({url})"></div>
                  {/each}
                {/if}
              </div>
              <div class="shelf-name">{shelf.name}</div>
              <div class="shelf-count">{shelf.book_count} books</div>
            </button>
          {/each}
        </section>

        <InfiniteScroll hasMore={shelvesNextCursor !== null} loading={isLoading()} {onLoadMore} />
      {/if}
    {/if}
  </div>

  <!-- Sidebar -->
  {#if selectedShelf}
    <button class="detail-backdrop" type="button" aria-label="Close details" onclick={closeSidebar}
    ></button>
  {/if}

  <ExpandableSidebar
    open={selectedShelfId !== null}
    expanded={sidebarExpanded}
    onClose={closeSidebar}
    onExpand={() => (sidebarExpanded = !sidebarExpanded)}
  >
    {#if selectedShelf}
      <div class="sidebar-header">
        <div class="sidebar-title">{selectedShelf.name}</div>
        <div class="sidebar-subtitle">{selectedShelf.book_count} books</div>
      </div>

      <!-- Edit Section -->
      <div class="edit-section">
        <div class="edit-row">
          <span class="edit-label">Name</span>
          <input class="edit-input" bind:value={editingShelfName} placeholder="Shelf name" />
          <button class="btn-sec" type="button" onclick={handleRenameShelf} disabled={loading}>
            Save
          </button>
        </div>

        <div class="edit-row description-row">
          <span class="edit-label">Description</span>
          <textarea
            class="edit-input"
            rows="2"
            bind:value={editingDescription}
            placeholder="Optional description..."></textarea>
        </div>
      </div>

      <!-- Kobo Sync Info -->
      <div class="kobo-sync-info">
        <span class="material-icons-outlined">sync</span>
        <div class="kobo-sync-text">
          <strong>Kobo Sync</strong>
          <span>Shelf name syncs to devices. Book order is device-controlled.</span>
        </div>
      </div>

      <!-- Books Section -->
      <div class="books-section">
        <div class="section-header-row">
          <span class="section-label">Books ({selectedShelf.book_count})</span>
          {#if onAddBook}
            <button
              type="button"
              class="add-btn"
              onclick={() => (showBookSearch = !showBookSearch)}
            >
              <span class="material-icons-outlined">add</span>
            </button>
          {/if}
        </div>

        {#if showBookSearch && onAddBook}
          <div class="book-search-section">
            <input
              type="text"
              placeholder="Search books to add..."
              bind:value={bookSearchQuery}
              oninput={() => searchBooks(bookSearchQuery)}
            />
            {#if bookSearchLoading}
              <div class="muted search-status">Searching...</div>
            {:else if bookSearchResults.length > 0}
              <div class="search-results">
                {#each bookSearchResults as book (book.id)}
                  <button
                    type="button"
                    class="search-result-item"
                    onclick={() => handleAddBook(book.id)}
                  >
                    <div
                      class="search-result-cover"
                      style:background-image="url({book.cover_url})"
                    ></div>
                    <div class="search-result-info">
                      <div class="search-result-title">{book.title}</div>
                      <div class="muted">{book.authors.map((a) => a.name).join(', ')}</div>
                    </div>
                  </button>
                {/each}
              </div>
            {:else if bookSearchQuery.trim()}
              <div class="muted search-status">No results found</div>
            {/if}
          </div>
        {/if}

        {#if loadStateShelfItems.status === 'loading'}
          <div class="muted loading-text">Loading…</div>
        {:else if shelfItems.length === 0}
          <div class="muted empty-books">No books in this shelf.</div>
        {:else}
          <div class="book-list">
            {#each shelfItems as book (book.id)}
              <div class="shelf-book-item">
                <span class="drag-handle material-icons-outlined">drag_indicator</span>
                <button type="button" class="book-item-main" onclick={() => onOpenInLibrary(book)}>
                  <div class="book-list-thumb" style:background-image="url({book.cover_url})"></div>
                  <div class="book-list-info">
                    <div class="book-list-title">{book.title}</div>
                    <div class="book-list-meta">{formatBookMeta(book)}</div>
                  </div>
                </button>
                {#if onRemoveBook}
                  <button
                    type="button"
                    class="remove-btn"
                    title="Remove from shelf"
                    onclick={(e) => handleRemoveBook(e, book.id)}
                  >
                    <span class="material-icons-outlined">close</span>
                  </button>
                {/if}
              </div>
            {/each}
          </div>
        {/if}
      </div>

      <!-- Delete Section -->
      <div class="delete-section">
        <button class="btn-danger full-width" type="button" onclick={handleDeleteShelf}>
          <span class="material-icons-outlined">delete</span>
          Delete Shelf
        </button>
      </div>

      <div class="shelf-meta muted">
        Updated: {new Date(selectedShelf.last_modified).toLocaleString()}
      </div>
    {:else}
      <div class="sidebar-empty">
        <span class="material-icons-outlined empty-icon">collections_bookmark</span>
        <div class="sidebar-empty-title">Select a Shelf</div>
        <p class="muted">Click on a shelf to view its books and manage contents.</p>
      </div>
    {/if}
  </ExpandableSidebar>
</div>

<style>
  .page-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 16px;
    gap: 16px;
  }

  .page-header h2 {
    font-size: 20px;
    font-weight: 700;
    margin: 0;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .count-label {
    font-size: 13px;
    color: var(--muted);
  }

  .shelf-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 16px;
  }

  .shelf-card {
    text-align: left;
    border-radius: 16px;
    padding: 20px;
    border: 1px solid var(--ring);
    background: var(--card);
    cursor: pointer;
    color: var(--ink);
    font: inherit;
    transition: border-color 0.15s;
  }

  .shelf-card:hover {
    border-color: var(--muted);
  }

  .shelf-card.selected {
    border-color: var(--accent);
  }

  .shelf-covers {
    display: flex;
    margin-bottom: 16px;
  }

  .mini-cover {
    width: 60px;
    height: 90px;
    border-radius: 6px;
    background-size: cover;
    background-position: center;
    border: 2px solid var(--card);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.3);
    flex-shrink: 0;
  }

  .mini-cover:nth-child(n + 2) {
    margin-left: -20px;
  }

  .mini-cover.placeholder {
    background: var(--bg);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--muted);
  }

  .mini-cover.placeholder .material-icons-outlined {
    font-size: 24px;
  }

  .shelf-name {
    font-size: 16px;
    font-weight: 700;
    margin-bottom: 4px;
  }

  .shelf-count {
    font-size: 12px;
    color: var(--muted);
  }

  .new-shelf-form {
    display: flex;
    gap: 8px;
    margin-bottom: 16px;
    padding: 12px;
    border-radius: 12px;
    border: 1px solid var(--ring);
    background: var(--card);
  }

  .new-shelf-form input {
    flex: 1;
    padding: 8px 12px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .empty-state {
    text-align: center;
    padding: 48px 24px;
    color: var(--muted);
  }

  .empty-state .empty-icon {
    font-size: 48px;
    opacity: 0.3;
    margin-bottom: 16px;
  }

  .empty-state h3 {
    margin: 0 0 8px;
    color: var(--ink);
  }

  .empty-state p {
    margin: 0;
  }

  /* Sidebar content styles */
  .sidebar-header {
    margin-bottom: 20px;
  }

  .sidebar-title {
    font-size: 18px;
    font-weight: 700;
    margin-bottom: 4px;
  }

  .sidebar-subtitle {
    font-size: 13px;
    color: var(--muted);
  }

  .edit-section {
    display: flex;
    flex-direction: column;
    gap: 12px;
    margin-bottom: 16px;
    padding-bottom: 16px;
    border-bottom: 1px solid var(--ring);
  }

  .edit-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .edit-row.description-row {
    flex-direction: column;
    align-items: stretch;
  }

  .edit-row.description-row .edit-label {
    width: auto;
    margin-bottom: 4px;
  }

  .edit-label {
    font-size: 12px;
    color: var(--muted);
    width: 60px;
    flex-shrink: 0;
  }

  .edit-input {
    flex: 1;
    padding: 8px 10px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    color: var(--ink);
    font-size: 13px;
    font-family: inherit;
  }

  .edit-input:focus {
    border-color: var(--accent);
    outline: none;
  }

  textarea.edit-input {
    resize: vertical;
    min-height: 60px;
  }

  .kobo-sync-info {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px;
    background: var(--bg-2);
    border-radius: 8px;
    margin-bottom: 16px;
  }

  .kobo-sync-info .material-icons-outlined {
    font-size: 18px;
    color: var(--muted);
  }

  .kobo-sync-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 12px;
    color: var(--muted);
  }

  .kobo-sync-text strong {
    color: var(--ink);
  }

  .books-section {
    margin-bottom: 16px;
  }

  .section-header-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 10px;
  }

  .section-label {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink);
  }

  .add-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: 1px dashed var(--ring);
    border-radius: 4px;
    color: var(--muted);
    cursor: pointer;
    padding: 2px 6px;
  }

  .add-btn:hover {
    border-color: var(--accent);
    color: var(--accent);
  }

  .add-btn .material-icons-outlined {
    font-size: 14px;
  }

  .book-search-section {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-bottom: 12px;
    padding: 10px;
    background: var(--bg);
    border-radius: 8px;
  }

  .book-search-section input {
    width: 100%;
    padding: 8px 10px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .search-status {
    font-size: 12px;
    padding: 4px 0;
  }

  .search-results {
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 200px;
    overflow-y: auto;
  }

  .search-result-item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px;
    border: 1px solid var(--ring);
    border-radius: 6px;
    background: transparent;
    cursor: pointer;
    text-align: left;
    color: inherit;
    font: inherit;
  }

  .search-result-item:hover {
    border-color: var(--accent);
    background: rgba(211, 140, 90, 0.08);
  }

  .search-result-cover {
    width: 28px;
    height: 40px;
    border-radius: 4px;
    background-size: cover;
    background-position: center;
    flex-shrink: 0;
  }

  .search-result-info {
    flex: 1;
    min-width: 0;
  }

  .search-result-title {
    font-size: 12px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .loading-text,
  .empty-books {
    font-size: 13px;
    padding: 8px 0;
  }

  .book-list {
    display: flex;
    flex-direction: column;
  }

  .shelf-book-item {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 10px;
    border-radius: 8px;
    border-bottom: 1px solid var(--ring);
  }

  .shelf-book-item:last-child {
    border-bottom: none;
  }

  .shelf-book-item:hover {
    background: var(--bg);
  }

  .drag-handle {
    color: var(--muted);
    cursor: grab;
    font-size: 16px;
  }

  .drag-handle:active {
    cursor: grabbing;
  }

  .book-item-main {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 10px;
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    text-align: left;
    color: inherit;
    font: inherit;
    min-width: 0;
  }

  .book-list-thumb {
    width: 32px;
    height: 48px;
    border-radius: 4px;
    background-size: cover;
    background-position: center;
    flex-shrink: 0;
  }

  .book-list-info {
    flex: 1;
    min-width: 0;
  }

  .book-list-title {
    font-size: 13px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .book-list-meta {
    font-size: 11px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .remove-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border-radius: 4px;
    background: none;
    border: none;
    color: var(--muted);
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.15s;
  }

  .shelf-book-item:hover .remove-btn {
    opacity: 1;
  }

  .remove-btn:hover {
    background: rgba(239, 68, 68, 0.15);
    color: var(--danger);
  }

  .remove-btn .material-icons-outlined {
    font-size: 16px;
  }

  .delete-section {
    margin-top: 16px;
    padding-top: 16px;
    border-top: 1px solid var(--ring);
  }

  .btn-danger {
    background: var(--danger);
    border: 1px solid var(--danger);
    color: #fff;
  }

  .btn-danger:hover {
    background: #dc2626;
  }

  .btn-danger .material-icons-outlined {
    font-size: 16px;
    margin-right: 6px;
  }

  .full-width {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .shelf-meta {
    margin-top: 12px;
    font-size: 11px;
    text-align: center;
  }

  .sidebar-empty {
    text-align: center;
    padding: 24px;
    color: var(--muted);
  }

  .sidebar-empty .empty-icon {
    font-size: 48px;
    opacity: 0.3;
    margin-bottom: 12px;
  }

  .sidebar-empty-title {
    font-size: 16px;
    font-weight: 600;
    color: var(--ink);
    margin-bottom: 8px;
  }

  .sidebar-empty p {
    font-size: 13px;
    margin: 0;
    line-height: 1.5;
  }

  .loading-card {
    padding: 24px;
    text-align: center;
    color: var(--muted);
  }

  .error {
    background: rgba(239, 68, 68, 0.15);
    border: 1px solid rgba(239, 68, 68, 0.3);
    padding: 12px;
    border-radius: 8px;
    margin-bottom: 16px;
    font-size: 13px;
    color: var(--danger);
  }

  @media (max-width: 768px) {
    .page-header {
      flex-direction: column;
      align-items: flex-start;
    }

    .shelf-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
