<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import SeriesPage from '$lib/components/pages/SeriesPage.svelte';
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
  import {
    loadSeries,
    loadMoreSeries,
    loadPreviousSeries,
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
    SeriesSummary,
    Book,
    AuthorSummary,
    SeriesSearchResult,
    AuthorSearchResult,
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
  const seriesList = $derived(library.seriesList);
  const seriesNextCursor = $derived(library.seriesNextCursor);
  const seriesPrevCursor = $derived(library.seriesPrevCursor);
  const seriesTotal = $derived(library.seriesTotal);
  const loadState = $derived(library.loadState);
  const devices = $derived(devicesStore.devices);

  // Navigation sidebar state
  const currentNavEntity = $derived(nav.current);
  const currentNavSeries = $derived(
    currentNavEntity?.type === 'series'
      ? (currentNavEntity.data as SeriesSummary | undefined)
      : undefined
  );
  const currentNavBook = $derived(
    currentNavEntity?.type === 'book' ? (currentNavEntity.data as Book | undefined) : undefined
  );
  const currentNavAuthor = $derived(
    currentNavEntity?.type === 'author'
      ? (currentNavEntity.data as AuthorSummary | undefined)
      : undefined
  );
  const showNavSidebar = $derived(nav.isOpen && currentNavEntity !== undefined);

  // Books for series/author sidebar
  let navSeriesBooks = $state<Book[]>([]);
  let navAuthorBooks = $state<Book[]>([]);

  // Local state
  let loading = $state(false);
  let error = $state('');
  let showAddSeriesModal = $state(false);
  let showMetadataSearchModal = $state(false);
  let showReleaseSearchModal = $state(false);

  // Metadata search results for edit forms
  let seriesMetadataResult = $state<SeriesSearchResult | null>(null);
  let authorMetadataResult = $state<AuthorSearchResult | null>(null);

  // Sidebar edit mode
  let editMode = $state(false);
  let savingEdit = $state(false);
  let deletingEdit = $state(false);

  // BookForm external footer bindings
  let formRequestSave: (() => void) | undefined = $state();
  let formCanSave: boolean | undefined = $state();

  // URL state
  const urlSelected = $derived($page.url.searchParams.get('selected'));
  const urlSearch = $derived($page.url.searchParams.get('q') || '');
  const urlAround = $derived($page.url.searchParams.get('around'));

  // Track last selected to prevent duplicate loads
  let lastUrlSelected = $state<string | null>(null);
  let lastSearch = $state('');
  let lastAround = $state<string | null>(null);

  // Handle scroll-to-top event - reload from beginning of list
  const handleScrollToTop = () => {
    loadSeries(undefined, urlSearch);
  };

  // Handle global format filter changes - reload from beginning
  const handleFormatFilterChange = () => {
    loadSeries(undefined, urlSearch);
  };

  onMount(async () => {
    window.addEventListener('scrolltotop', handleScrollToTop);
    window.addEventListener('formatfilterchange', handleFormatFilterChange);
    lastSearch = urlSearch;
    lastAround = urlAround;
    // Load series with around parameter for jump-to navigation
    await loadSeries(undefined, urlSearch, urlAround || undefined);

    // Load devices for book details
    await loadDevices();

    // Restore selection from URL - push to nav stack
    if (urlSelected) {
      lastUrlSelected = urlSelected;
      pushEntity('series', urlSelected);
    }

    // Clear the around param from URL after using it (it's a one-time navigation aid)
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

    // Always load books for series/author even if we have entity data
    // (grid selection provides entity data but not books)
    if (entity.type === 'series') {
      if (!entity.data) {
        loadNavSeriesData(entity.id);
      } else {
        // Have entity data, just load books
        api.getSeriesBooks(entity.id, true).then((books) => {
          navSeriesBooks = books;
        });
      }
    } else if (entity.type === 'book') {
      if (!entity.data) {
        loadNavBookData(entity.id);
      }
    } else if (entity.type === 'author') {
      if (!entity.data) {
        loadNavAuthorData(entity.id);
      } else {
        // Have entity data, just load books
        api.getAuthorBooks(entity.id, true).then((books) => {
          navAuthorBooks = books;
        });
      }
    }
  });

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

  onDestroy(() => {
    window.removeEventListener('scrolltotop', handleScrollToTop);
    window.removeEventListener('formatfilterchange', handleFormatFilterChange);
  });

  // Reflect store load errors into local error state
  $effect(() => {
    if (loadState.series.status === 'error') {
      error = 'Failed to load data. Please check your connection and try again.';
    } else if (loadState.series.status === 'success') {
      error = '';
    }
  });

  // Handle around parameter changes (e.g., navigating from command palette while already on /series)
  $effect(() => {
    if (urlAround && urlAround !== lastAround) {
      lastAround = urlAround;
      lastUrlSelected = null; // Reset so selection effect will trigger
      // Reload with the new around parameter
      const target = urlAround;
      loadSeries(undefined, urlSearch, target).then(() => {
        // Push series to nav stack if selected
        if (urlSelected) {
          lastUrlSelected = urlSelected;
          pushEntity('series', urlSelected);
        }
        scrollItemToTop(target);
        // Clear the around param after using it
        updateUrlParams({ around: null });
      });
    }
  });

  // Reactive selection from URL changes (e.g., navigating from library)
  $effect(() => {
    if (urlSelected && urlSelected !== lastUrlSelected && seriesList.length > 0) {
      lastUrlSelected = urlSelected;
      pushEntity('series', urlSelected);
    }
  });

  // Reactive search from URL changes
  $effect(() => {
    if (urlSearch !== lastSearch) {
      lastSearch = urlSearch;
      // A new search is a new list: start it at the top, since the scroll container
      // keeps its offset across the swap.
      loadSeries(undefined, urlSearch).then(scrollContainerToTop);
    }
  });

  const handleSearch = (query: string) => {
    updateUrlParams({ q: query || null, selected: null });
  };

  // Handlers
  const handleSelectSeries = async (series: SeriesSummary) => {
    if (window.matchMedia('(max-width: 768px)').matches) {
      goto(`/series/${series.id}`);
      return;
    }
    updateUrlParams({ selected: series.id });
    // Push to navigation stack with the data we already have
    pushEntity('series', series.id, series);
  };

  const handleLoadMore = () => {
    loadMoreSeries();
  };

  const handleLoadPrevious = () => {
    loadPreviousSeries();
  };

  // Navigation sidebar handlers
  const handleNavClose = () => {
    if (editMode) {
      editMode = false;
    } else {
      clearStack();
      navSeriesBooks = [];
      navAuthorBooks = [];
      updateUrlParams({ selected: null });
    }
  };

  const handleNavBack = () => {
    editMode = false;
    popEntity();
    navSeriesBooks = [];
    navAuthorBooks = [];
  };

  const handleNavExpand = () => {
    const entity = nav.current;
    if (!entity) return;

    if (entity.type === 'series') {
      goto(`/series/${entity.id}`);
    } else if (entity.type === 'book') {
      goto(`/library/books/${entity.id}`);
    } else if (entity.type === 'author') {
      goto(`/authors/${entity.id}`);
    }
    clearStack();
  };

  // Handle author click from series sidebar - push to stack
  const handleAuthorClick = (_name: string, authorId: string) => {
    pushEntity('author', authorId);
  };

  // Handle series click from author sidebar - push to stack
  const handleSeriesClick = (seriesId: string) => {
    pushEntity('series', seriesId);
  };

  // Handle book click from series/author sidebar - push to stack
  const handleNavBookClick = (book: Book) => {
    pushEntity('book', book.id, book);
  };

  // Sidebar edit handlers
  const handleToggleEdit = () => {
    editMode = !editMode;
    // Clear metadata results when toggling edit mode off
    if (!editMode) {
      seriesMetadataResult = null;
      authorMetadataResult = null;
    }
  };

  const handleEditCancel = () => {
    editMode = false;
    seriesMetadataResult = null;
    authorMetadataResult = null;
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
      // Refresh the list
      await loadSeries();
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
      clearStack();
      await loadSeries();
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
      clearStack();
      updateUrlParams({ selected: null });
      await loadSeries();
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
        series_hardcover_id: data.series_hardcover_id ?? undefined,
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
      popEntity();
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
      if (currentNavEntity?.type === 'series') return 'Edit Series';
      if (currentNavEntity?.type === 'book') return 'Edit Book';
      if (currentNavEntity?.type === 'author') return 'Edit Author';
    }
    if (currentNavEntity?.type === 'series') return 'Series Details';
    if (currentNavEntity?.type === 'book') return 'Book Details';
    if (currentNavEntity?.type === 'author') return 'Author Details';
    return 'Details';
  });

  // Show edit button for all entity types
  const showEditButton = $derived(
    currentNavEntity?.type === 'series' ||
      currentNavEntity?.type === 'book' ||
      currentNavEntity?.type === 'author'
  );
</script>

<svelte:head>
  <title>Series — Kobbler</title>
</svelte:head>

{#if error}
  <div class="error-wrapper">
    <ErrorBanner
      message={error}
      onRetry={() => loadSeries(undefined, urlSearch)}
      onDismiss={() => (error = '')}
    />
  </div>
{/if}

<SeriesPage
  {seriesList}
  {seriesNextCursor}
  {seriesPrevCursor}
  {seriesTotal}
  loadStateSeries={loadState.series}
  searchQuery={urlSearch}
  selectedId={urlSelected}
  onSelectSeries={handleSelectSeries}
  onLoadMore={handleLoadMore}
  onLoadPrevious={handleLoadPrevious}
  onSearch={handleSearch}
  onAdd={() => (showAddSeriesModal = true)}
/>

<!-- Navigation Sidebar for Series/Book/Author details -->
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
  onSearch={currentNavEntity?.type === 'series' || currentNavEntity?.type === 'author'
    ? () => (showMetadataSearchModal = true)
    : undefined}
  onRefresh={async () => {
    if (currentNavEntity?.type === 'series') {
      api.refreshSeriesMetadata(currentNavEntity.id).catch(() => {});
      loadNavSeriesData(currentNavEntity.id);
    } else if (currentNavEntity?.type === 'book') {
      loadNavBookData(currentNavEntity.id);
    } else if (currentNavEntity?.type === 'author') {
      api.refreshAuthorMetadata(currentNavEntity.id).catch(() => {});
      loadNavAuthorData(currentNavEntity.id);
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
          await loadNavSeriesData(currentNavSeries.id);
          await loadSeries();
        }}
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
        loading={nav.loadingEntity}
        onSeriesClick={handleSeriesClick}
        onBookClick={handleNavBookClick}
        onSearchMetadata={() => (showMetadataSearchModal = true)}
        onRefresh={async () => {
          if (currentNavAuthor) {
            const [author, books] = await Promise.all([
              api.getAuthor(currentNavAuthor.id),
              api.getAuthorBooks(currentNavAuthor.id, true)
            ]);
            updateCurrentEntityData(author);
            navAuthorBooks = books;
          }
        }}
      />
    {/if}
  {/if}
</ExpandableSidebar>

<!-- Add Series Modal -->
{#if showAddSeriesModal}
  <AddEntityModal
    mode="series"
    onComplete={(id) => {
      showAddSeriesModal = false;
      // Refresh the series list to show the new series
      loadSeries();
      // Navigate to the new series
      goto(`/series/${id}`);
    }}
    onCancel={() => (showAddSeriesModal = false)}
  />
{/if}

<!-- Metadata Search Modal -->
{#if showMetadataSearchModal && currentNavEntity}
  <AddEntityModal
    mode={currentNavEntity.type === 'series' ? 'series' : 'author'}
    linkMode={true}
    existingId={currentNavEntity.id}
    initialSearchValues={currentNavEntity.type === 'series' && currentNavSeries
      ? { title: currentNavSeries.name }
      : currentNavEntity.type === 'author' && currentNavAuthor
        ? { title: currentNavAuthor.name }
        : undefined}
    onResultsSelected={(results) => {
      showMetadataSearchModal = false;
      // Store the search result and enter edit mode
      if (currentNavEntity.type === 'series') {
        seriesMetadataResult = (results[0] as SeriesSearchResult) || null;
      } else if (currentNavEntity.type === 'author') {
        authorMetadataResult = (results[0] as AuthorSearchResult) || null;
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
