<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import AuthorsPage from '$lib/components/pages/AuthorsPage.svelte';
  import BookDetailsPanel from '$lib/components/BookDetailsPanel.svelte';
  import BookForm from '$lib/components/BookForm.svelte';
  import SeriesSidebarContent from '$lib/components/SeriesSidebarContent.svelte';
  import SeriesEditForm from '$lib/components/SeriesEditForm.svelte';
  import AuthorSidebarContent from '$lib/components/AuthorSidebarContent.svelte';
  import AuthorEditForm from '$lib/components/AuthorEditForm.svelte';
  import { ExpandableSidebar } from '@koblibs/ui';
  import AddEntityModal from '$lib/components/AddEntityModal.svelte';
  import ErrorBanner from '$lib/components/shared/ErrorBanner.svelte';
  import ReleaseSearchModal from '$lib/components/ReleaseSearchModal.svelte';
  import { getLibraryState } from '$lib/stores/library.svelte';
  import { getDevicesState } from '$lib/stores/devices.svelte';
  import { setSearchQuery } from '$lib/stores/ui.svelte';
  import {
    loadAuthors,
    loadMoreAuthors,
    loadPreviousAuthors,
    loadDevices,
    showMessage
  } from '$lib/context/app.svelte';
  import { updateUrlParams } from '$lib/utils/url-state';
  import { scrollContainerToTop, scrollItemToTop } from '$lib/utils/scroll-anchor';
  import {
    getNavigationState,
    pushEntity,
    popEntity,
    clearStack,
    setLoadingEntity,
    updateCurrentEntityData
  } from '$lib/stores/navigation.svelte';
  import { getStatsState } from '$lib/stores/stats.svelte';
  import * as api from '$lib/api/client';
  import { searchBookReleases, queueBookRelease } from '$lib/api/client';
  import type {
    AuthorSummary,
    Book,
    SeriesSummary,
    AuthorSearchResult,
    SeriesSearchResult,
    BookFormData,
    AuthorFormData,
    SeriesFormData
  } from '$lib/types';

  // Store state
  const library = getLibraryState();
  const devicesStore = getDevicesState();
  const nav = getNavigationState();
  const stats = getStatsState();

  // Derived state
  const authors = $derived(library.authors);
  const devices = $derived(devicesStore.devices);
  const authorsNextCursor = $derived(library.authorsNextCursor);
  const authorsPrevCursor = $derived(library.authorsPrevCursor);
  const authorsTotal = $derived(library.authorsTotal);
  const loadState = $derived(library.loadState);

  // Navigation sidebar state
  const currentNavEntity = $derived(nav.current);
  const currentNavAuthor = $derived(
    currentNavEntity?.type === 'author'
      ? (currentNavEntity.data as AuthorSummary | undefined)
      : undefined
  );
  const currentNavBook = $derived(
    currentNavEntity?.type === 'book' ? (currentNavEntity.data as Book | undefined) : undefined
  );
  const currentNavSeries = $derived(
    currentNavEntity?.type === 'series'
      ? (currentNavEntity.data as SeriesSummary | undefined)
      : undefined
  );
  const showNavSidebar = $derived(nav.isOpen && currentNavEntity !== undefined);

  // Books for author/series sidebar
  let navAuthorBooks = $state<Book[]>([]);
  let navSeriesBooks = $state<Book[]>([]);

  // Local state
  let loading = $state(false);
  let error = $state('');
  let showAddAuthorModal = $state(false);
  let showMetadataSearchModal = $state(false);
  let showReleaseSearchModal = $state(false);

  // Metadata search results for edit forms
  let authorMetadataResult = $state<AuthorSearchResult | null>(null);
  let seriesMetadataResult = $state<SeriesSearchResult | null>(null);

  // Sidebar edit mode
  let editMode = $state(false);
  let savingEdit = $state(false);
  let deletingEdit = $state(false);

  // BookForm external footer bindings
  let formRequestSave: (() => void) | undefined = $state();
  let formCanSave: boolean | undefined = $state();

  // URL state
  const urlSelected = $derived($page.url.searchParams.get('selected'));
  const urlAround = $derived($page.url.searchParams.get('around'));
  const urlSearch = $derived($page.url.searchParams.get('q') || '');

  // Track last selected/around to prevent duplicate loads
  let lastUrlSelected = $state<string | null>(null);
  let lastSearch = $state('');
  let lastAround = $state<string | null>(null);

  // Handle scroll-to-top event - reload from beginning of list
  const handleScrollToTop = () => {
    loadAuthors(undefined, undefined, urlSearch);
  };

  // Handle global format filter changes - reload from beginning
  const handleFormatFilterChange = () => {
    loadAuthors(undefined, undefined, urlSearch);
  };

  onMount(async () => {
    window.addEventListener('scrolltotop', handleScrollToTop);
    window.addEventListener('formatfilterchange', handleFormatFilterChange);
    lastSearch = urlSearch;
    lastAround = urlAround;
    // Load authors with around parameter for jump-to navigation
    await loadAuthors(undefined, urlAround || undefined, urlSearch || undefined);

    // Load devices for book details
    await loadDevices();

    // Restore selection from URL - push to nav stack
    if (urlSelected) {
      lastUrlSelected = urlSelected;
      pushEntity('author', urlSelected);
    }

    // Clear the around param from URL after using it
    if (urlAround) {
      // The window is centred on the target, and main.content keeps its old scrollTop
      // across the navigation, so the target has to be scrolled to explicitly.
      scrollItemToTop(urlAround);
      updateUrlParams({ around: null });
    }
  });

  // Load data when navigation stack changes
  $effect(() => {
    const entity = nav.current;
    if (!entity) return;

    // Always load books for author/series even if we have entity data
    // (grid selection provides entity data but not books)
    if (entity.type === 'author') {
      if (!entity.data) {
        loadNavAuthorData(entity.id);
      } else {
        // Have entity data, just load books
        api.getAuthorBooks(entity.id, true).then((books) => {
          navAuthorBooks = books;
        });
      }
    } else if (entity.type === 'book') {
      if (!entity.data) {
        loadNavBookData(entity.id);
      }
    } else if (entity.type === 'series') {
      if (!entity.data) {
        loadNavSeriesData(entity.id);
      } else {
        // Have entity data, just load books
        api.getSeriesBooks(entity.id, true).then((books) => {
          navSeriesBooks = books;
        });
      }
    }
  });

  async function loadNavAuthorData(authorId: string) {
    setLoadingEntity(true);
    try {
      const [author, books] = await Promise.all([
        api.getAuthor(authorId),
        api.getAuthorBooks(authorId, true)
      ]);
      updateCurrentEntityData(author);
      navAuthorBooks = books;
    } catch (e) {
      console.error('Failed to load author:', e);
    } finally {
      setLoadingEntity(false);
    }
  }

  async function loadNavBookData(bookId: string) {
    setLoadingEntity(true);
    try {
      const book = await api.getBook(bookId);
      updateCurrentEntityData(book);
    } catch (e) {
      console.error('Failed to load book:', e);
    } finally {
      setLoadingEntity(false);
    }
  }

  async function loadNavSeriesData(seriesId: string) {
    setLoadingEntity(true);
    try {
      const [series, books] = await Promise.all([
        api.getSeriesById(seriesId),
        api.getSeriesBooks(seriesId, true)
      ]);
      updateCurrentEntityData(series);
      navSeriesBooks = books;
    } catch (e) {
      console.error('Failed to load series:', e);
    } finally {
      setLoadingEntity(false);
    }
  }

  onDestroy(() => {
    window.removeEventListener('scrolltotop', handleScrollToTop);
    window.removeEventListener('formatfilterchange', handleFormatFilterChange);
  });

  // Reflect store load errors into local error state
  $effect(() => {
    if (loadState.authors.status === 'error') {
      error = 'Failed to load data. Please check your connection and try again.';
    } else if (loadState.authors.status === 'success') {
      error = '';
    }
  });

  // Handle around parameter changes
  $effect(() => {
    if (urlAround && urlAround !== lastAround) {
      lastAround = urlAround;
      lastUrlSelected = null;
      const target = urlAround;
      loadAuthors(undefined, target, urlSearch).then(() => {
        if (urlSelected) {
          lastUrlSelected = urlSelected;
          pushEntity('author', urlSelected);
        }
        scrollItemToTop(target);
        // Clear the around param after using it
        updateUrlParams({ around: null });
      });
    }
  });

  // Reactive selection from URL changes
  $effect(() => {
    if (urlSelected && urlSelected !== lastUrlSelected && authors.length > 0) {
      lastUrlSelected = urlSelected;
      pushEntity('author', urlSelected);
    }
  });

  // Reactive search from URL changes
  $effect(() => {
    if (urlSearch !== lastSearch) {
      lastSearch = urlSearch;
      // A new search is a new list: start it at the top, since the scroll container
      // keeps its offset across the swap.
      loadAuthors(undefined, undefined, urlSearch).then(scrollContainerToTop);
    }
  });

  const handleSearch = (query: string) => {
    updateUrlParams({ q: query || null, selected: null });
  };

  // Handlers
  const handleSelectAuthor = async (author: AuthorSummary) => {
    if (window.matchMedia('(max-width: 768px)').matches) {
      goto(`/authors/${author.id}`);
      return;
    }
    updateUrlParams({ selected: author.id });
    // Push to navigation stack with the data we already have
    pushEntity('author', author.id, author);
  };

  const handleLoadMore = () => {
    loadMoreAuthors();
  };

  const handleLoadPrevious = () => {
    loadPreviousAuthors();
  };

  // Navigation sidebar handlers
  const handleNavClose = () => {
    if (editMode) {
      editMode = false;
    } else {
      clearStack();
      navAuthorBooks = [];
      navSeriesBooks = [];
      updateUrlParams({ selected: null });
    }
  };

  const handleNavBack = () => {
    editMode = false;
    popEntity();
    navAuthorBooks = [];
    navSeriesBooks = [];
  };

  const handleNavExpand = () => {
    const entity = nav.current;
    if (!entity) return;

    if (entity.type === 'author') {
      goto(`/authors/${entity.id}`);
    } else if (entity.type === 'book') {
      goto(`/library/books/${entity.id}`);
    } else if (entity.type === 'series') {
      goto(`/series/${entity.id}`);
    }
    clearStack();
  };

  // Handle author click from book details - just close if we're on the same author
  const handleAuthorClick = (_name: string, authorId: string) => {
    // If it's the same author at the root, just close the sidebar
    if (nav.stack.length > 0 && nav.stack[0].type === 'author' && nav.stack[0].id === authorId) {
      popEntity();
    } else {
      pushEntity('author', authorId);
    }
  };

  // Handle series click from author/book sidebar - push to stack
  const handleSeriesClick = (seriesId: string) => {
    pushEntity('series', seriesId);
  };

  // Handle book click from author/series sidebar - push to stack
  const handleNavBookClick = (book: Book) => {
    pushEntity('book', book.id, book);
  };

  const handleSearchInLibrary = () => {
    if (currentNavAuthor) {
      setSearchQuery(currentNavAuthor.name);
      goto(`/library?q=${encodeURIComponent(currentNavAuthor.name)}`);
    }
  };

  // Sidebar edit handlers
  const handleToggleEdit = () => {
    editMode = !editMode;
    // Clear metadata results when toggling edit mode off
    if (!editMode) {
      authorMetadataResult = null;
      seriesMetadataResult = null;
    }
  };

  const handleEditCancel = () => {
    editMode = false;
    authorMetadataResult = null;
    seriesMetadataResult = null;
  };

  // Author save handler
  const handleAuthorSave = async (data: AuthorFormData) => {
    if (!currentNavAuthor) return;

    savingEdit = true;
    try {
      await api.updateAuthor(currentNavAuthor.id, {
        name: data.name,
        identifiers: data.identifiers,
        image_url: data.image_url,
        bio: data.bio,
        is_monitored: data.is_monitored,
        auto_want_mode: data.auto_want_mode
      });
      showMessage('Author updated');
      editMode = false;
      await loadNavAuthorData(currentNavAuthor.id);
      // Refresh the list
      await loadAuthors(undefined, undefined, urlSearch);
    } catch {
      showMessage('Update failed');
    } finally {
      savingEdit = false;
    }
  };

  const handleAuthorMerge = async (targetId: string) => {
    if (!currentNavAuthor) return;

    savingEdit = true;
    try {
      await api.mergeAuthors(currentNavAuthor.id, targetId);
      showMessage('Authors merged');
      // Navigate to merged author
      clearStack();
      pushEntity('author', targetId);
      updateUrlParams({ selected: targetId });
      await loadAuthors(undefined, undefined, urlSearch);
    } catch {
      showMessage('Merge failed');
    } finally {
      savingEdit = false;
    }
  };

  const handleAuthorDelete = async () => {
    if (!currentNavAuthor) return;

    deletingEdit = true;
    try {
      await api.deleteAuthor(currentNavAuthor.id);
      showMessage('Author deleted');
      clearStack();
      updateUrlParams({ selected: null });
      await loadAuthors(undefined, undefined, urlSearch);
    } catch {
      showMessage('Delete failed');
    } finally {
      deletingEdit = false;
    }
  };

  // Book save handler
  const handleBookSave = async (data: BookFormData, coverFile?: File) => {
    if (!currentNavBook) return;

    savingEdit = true;
    try {
      await api.updateBook(currentNavBook.id, {
        title: data.title,
        authors: data.authors,
        series_id: data.series_id ?? undefined,
        series_name: data.series_name ?? undefined,
        series_index: data.series_index ?? undefined,
        publisher: data.publisher ?? undefined,
        pubdate: data.pubdate ?? undefined,
        genres: data.genres,
        description: data.description ?? undefined,
        identifiers: data.identifiers,
        series_memberships: data.series_memberships
      });

      if (coverFile) {
        await api.uploadBookCover(currentNavBook.id, coverFile);
      }

      showMessage('Book updated');
      editMode = false;
      await loadNavBookData(currentNavBook.id);
    } catch {
      showMessage('Update failed');
    } finally {
      savingEdit = false;
    }
  };

  // Series save handler
  const handleSeriesSave = async (data: SeriesFormData) => {
    if (!currentNavSeries) return;

    savingEdit = true;
    try {
      await api.updateSeries(currentNavSeries.id, {
        name: data.name,
        description: data.description ?? undefined,
        identifiers: data.identifiers,
        is_monitored: data.is_monitored,
        auto_want_mode: data.auto_want_mode,
        image_url: data.image_url ?? undefined
      });

      showMessage('Series updated');
      editMode = false;
      await loadNavSeriesData(currentNavSeries.id);
    } catch {
      showMessage('Update failed');
    } finally {
      savingEdit = false;
    }
  };

  const handleSeriesMerge = async (targetId: string) => {
    if (!currentNavSeries) return;

    savingEdit = true;
    try {
      await api.mergeSeries(currentNavSeries.id, targetId);
      showMessage('Series merged');
      popEntity();
    } catch {
      showMessage('Merge failed');
    } finally {
      savingEdit = false;
    }
  };

  const handleSeriesDelete = async () => {
    if (!currentNavSeries) return;

    deletingEdit = true;
    try {
      await api.deleteSeries(currentNavSeries.id);
      showMessage('Series deleted');
      popEntity();
    } catch {
      showMessage('Delete failed');
    } finally {
      deletingEdit = false;
    }
  };

  // Sidebar title based on mode and entity type
  const sidebarTitle = $derived.by(() => {
    if (editMode) {
      if (currentNavEntity?.type === 'author') return 'Edit Author';
      if (currentNavEntity?.type === 'book') return 'Edit Book';
      if (currentNavEntity?.type === 'series') return 'Edit Series';
    }
    if (currentNavEntity?.type === 'author') return 'Author Details';
    if (currentNavEntity?.type === 'book') return 'Book Details';
    if (currentNavEntity?.type === 'series') return 'Series Details';
    return 'Details';
  });

  // Show edit button for all entity types
  const showEditButton = $derived(
    currentNavEntity?.type === 'author' ||
      currentNavEntity?.type === 'book' ||
      currentNavEntity?.type === 'series'
  );
</script>

<svelte:head>
  <title>Authors — Kobbler</title>
</svelte:head>

{#if error}
  <div class="error-wrapper">
    <ErrorBanner
      message={error}
      onRetry={() => loadAuthors(undefined, undefined, urlSearch)}
      onDismiss={() => (error = '')}
    />
  </div>
{/if}

<AuthorsPage
  {authors}
  {authorsNextCursor}
  {authorsPrevCursor}
  {authorsTotal}
  loadStateAuthors={loadState.authors}
  searchQuery={urlSearch}
  selectedId={urlSelected}
  onSelectAuthor={handleSelectAuthor}
  onLoadMore={handleLoadMore}
  onLoadPrevious={handleLoadPrevious}
  onSearch={handleSearch}
  onAdd={() => (showAddAuthorModal = true)}
/>

<!-- Navigation Sidebar for Author/Book/Series details -->
<ExpandableSidebar
  open={showNavSidebar}
  expanded={editMode}
  title={sidebarTitle}
  showRefresh={!editMode}
  showEdit={showEditButton}
  {editMode}
  onClose={handleNavClose}
  onExpand={handleNavExpand}
  onToggleEdit={handleToggleEdit}
  onSearch={currentNavEntity?.type === 'author' || currentNavEntity?.type === 'series'
    ? () => (showMetadataSearchModal = true)
    : undefined}
  onRefresh={async () => {
    if (currentNavEntity?.type === 'author') {
      api.refreshAuthorMetadata(currentNavEntity.id).catch(() => {});
      loadNavAuthorData(currentNavEntity.id);
    } else if (currentNavEntity?.type === 'book') {
      loadNavBookData(currentNavEntity.id);
    } else if (currentNavEntity?.type === 'series') {
      api.refreshSeriesMetadata(currentNavEntity.id).catch(() => {});
      loadNavSeriesData(currentNavEntity.id);
    }
  }}
>
  {#snippet headerLeft()}
    {#if nav.canGoBack && !editMode}
      <button class="icon-btn small" type="button" title="Back" onclick={handleNavBack}>
        <span class="material-icons-outlined">arrow_back</span>
      </button>
    {/if}
  {/snippet}

  {#snippet footer()}
    {#if editMode && currentNavEntity?.type === 'book' && currentNavBook}
      <div class="sidebar-footer-actions">
        <button class="btn-sec" type="button" onclick={handleEditCancel} disabled={savingEdit}>
          Cancel
        </button>
        <button
          class="btn-primary"
          type="button"
          onclick={() => formRequestSave?.()}
          disabled={!formCanSave}
        >
          {savingEdit ? 'Saving...' : 'Save Changes'}
        </button>
      </div>
    {/if}
  {/snippet}

  {#if nav.loadingEntity}
    <div class="loading-state">Loading...</div>
  {:else if currentNavEntity?.type === 'author' && currentNavAuthor}
    {#if editMode}
      <AuthorEditForm
        author={currentNavAuthor}
        searchResult={authorMetadataResult}
        onSave={handleAuthorSave}
        onMerge={handleAuthorMerge}
        onDelete={handleAuthorDelete}
        onCancel={handleEditCancel}
        saving={savingEdit}
        deleting={deletingEdit}
      />
    {:else}
      <AuthorSidebarContent
        author={currentNavAuthor}
        books={navAuthorBooks}
        loadState={{ status: nav.loadingEntity ? 'loading' : 'success', error: '' }}
        loading={nav.loadingEntity || loading}
        onSeriesClick={handleSeriesClick}
        onBookClick={handleNavBookClick}
        onSearchMetadata={() => (showMetadataSearchModal = true)}
        onSearchInLibrary={handleSearchInLibrary}
        onRefresh={() => loadNavAuthorData(currentNavAuthor.id)}
      />
    {/if}
  {:else if currentNavEntity?.type === 'book' && currentNavBook}
    {#if editMode}
      <BookForm
        book={currentNavBook}
        onSave={handleBookSave}
        onCancel={handleEditCancel}
        saving={savingEdit}
        deleting={deletingEdit}
        hideFooter
        bind:requestSave={formRequestSave}
        bind:canSave={formCanSave}
      />
    {:else}
      <BookDetailsPanel
        book={currentNavBook}
        context="library"
        {devices}
        {loading}
        embedded
        onClose={handleNavClose}
        onForceResync={() =>
          api.touchBooks([currentNavBook.id]).then(() => loadNavBookData(currentNavBook.id))}
        onAuthorClick={handleAuthorClick}
        onSeriesClick={handleSeriesClick}
        onGenreClick={(genre: string) => goto(`/library?q=${encodeURIComponent(genre)}`)}
        onSearchReleases={stats.stats.kobload_configured
          ? () => (showReleaseSearchModal = true)
          : undefined}
        onRefresh={() => loadNavBookData(currentNavBook.id)}
        onWant={currentNavBook?.is_in_library
          ? undefined
          : async () => {
              await api.wantBook(currentNavBook.id);
              showMessage(`Added "${currentNavBook.title}" to wanted list`);
              await loadNavBookData(currentNavBook.id);
            }}
        onSkip={currentNavBook?.is_in_library
          ? undefined
          : async () => {
              await api.skipBook(currentNavBook.id);
              showMessage(`Skipped "${currentNavBook.title}"`);
              await loadNavBookData(currentNavBook.id);
            }}
      />
    {/if}
  {:else if currentNavEntity?.type === 'series' && currentNavSeries}
    {#if editMode}
      <SeriesEditForm
        series={currentNavSeries}
        searchResult={seriesMetadataResult}
        onSave={handleSeriesSave}
        onMerge={handleSeriesMerge}
        onDelete={handleSeriesDelete}
        onCancel={handleEditCancel}
        saving={savingEdit}
        deleting={deletingEdit}
      />
    {:else}
      <SeriesSidebarContent
        series={currentNavSeries}
        books={navSeriesBooks}
        loadState={{ status: nav.loadingEntity ? 'loading' : 'success', error: '' }}
        onAuthorClick={handleAuthorClick}
        onBookClick={handleNavBookClick}
        onSearchMetadata={() => (showMetadataSearchModal = true)}
        onRefresh={async () => {
          if (currentNavSeries) {
            const [series, books] = await Promise.all([
              api.getSeriesById(currentNavSeries.id),
              api.getSeriesBooks(currentNavSeries.id, true)
            ]);
            updateCurrentEntityData(series);
            navSeriesBooks = books;
          }
        }}
      />
    {/if}
  {/if}
</ExpandableSidebar>

<!-- Add Author Modal -->
{#if showAddAuthorModal}
  <AddEntityModal
    mode="author"
    onComplete={(id) => {
      showAddAuthorModal = false;
      // Refresh the authors list to show the new author
      loadAuthors();
      // Navigate to the new author
      goto(`/authors/${id}`);
    }}
    onCancel={() => (showAddAuthorModal = false)}
  />
{/if}

<!-- Metadata Search Modal -->
{#if showMetadataSearchModal && currentNavEntity}
  <AddEntityModal
    mode={currentNavEntity.type === 'author' ? 'author' : 'series'}
    linkMode={true}
    existingId={currentNavEntity.id}
    initialSearchValues={currentNavEntity.type === 'author' && currentNavAuthor
      ? { title: currentNavAuthor.name }
      : currentNavEntity.type === 'series' && currentNavSeries
        ? { title: currentNavSeries.name }
        : undefined}
    onResultsSelected={(results) => {
      showMetadataSearchModal = false;
      // Store the search result and enter edit mode
      if (currentNavEntity.type === 'author') {
        authorMetadataResult = (results[0] as AuthorSearchResult) || null;
      } else if (currentNavEntity.type === 'series') {
        seriesMetadataResult = (results[0] as SeriesSearchResult) || null;
      }
      editMode = true;
    }}
    onComplete={() => {
      showMetadataSearchModal = false;
    }}
    onCancel={() => (showMetadataSearchModal = false)}
  />
{/if}

<!-- Release Search Modal -->
{#if showReleaseSearchModal && currentNavBook}
  <ReleaseSearchModal
    entityTitle={currentNavBook.title}
    searchFn={(query, scope) => searchBookReleases(currentNavBook.id, query, scope)}
    scopeLabel="book"
    smartSearchFn={() => searchBookReleases(currentNavBook.id)}
    queueFn={(release, source) =>
      queueBookRelease(currentNavBook.id, release as Record<string, unknown>, source)}
    wantFn={async () => {
      await api.wantBook(currentNavBook.id);
      showMessage(`Searching for best match for "${currentNavBook.title}"`);
      await loadNavBookData(currentNavBook.id);
    }}
    initialQuery={`${currentNavBook.title} ${currentNavBook.authors?.[0]?.name || ''}`}
    onClose={() => (showReleaseSearchModal = false)}
  />
{/if}

<style>
  .error-wrapper {
    padding: 12px 24px 0;
  }

  .sidebar-footer-actions {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
  }

  .sidebar-footer-actions :global(button) {
    padding: 8px 16px;
    font-size: 12px;
  }

  .loading-state {
    padding: 24px;
    text-align: center;
    color: var(--muted);
  }

  .icon-btn.small {
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

  .icon-btn.small:hover {
    background: var(--bg);
    color: var(--ink);
  }
</style>
