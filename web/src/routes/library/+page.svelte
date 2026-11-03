<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import LibraryPage from '#lib/components/pages/LibraryPage.svelte';
  import BookDetailsPanel from '#lib/components/BookDetailsPanel.svelte';
  import BookForm from '#lib/components/BookForm.svelte';
  import QuickLinkProviderModal from '#lib/components/QuickLinkProviderModal.svelte';
  import BookEditModal, { type EditData } from '#lib/components/BookEditModal.svelte';
  import DevicePickerModal from '#lib/components/DevicePickerModal.svelte';
  import { DeleteConfirmModal } from '@koblibs/ui';
  import ReleaseSearchModal from '#lib/components/ReleaseSearchModal.svelte';
  import AddEntityModal from '#lib/components/AddEntityModal.svelte';
  import { SearchSelect, type SearchSelectItem } from '@koblibs/ui';
  import { ExpandableSidebar } from '@koblibs/ui';
  import SeriesSidebarContent from '#lib/components/SeriesSidebarContent.svelte';
  import AuthorSidebarContent from '#lib/components/AuthorSidebarContent.svelte';
  import { MultiSelectBar } from '@koblibs/ui';
  import ErrorBanner from '#lib/components/shared/ErrorBanner.svelte';
  import { getLibraryState, setSelection, ensureBookInStore } from '#lib/stores/library.svelte.js';
  import { getFormatFilter } from '#lib/stores/format-filter.svelte.js';
  import { getDevicesState } from '#lib/stores/devices.svelte.js';
  import {
    getUIState,
    setShowDetails,
    setMultiSelect,
    setSearchQuery
  } from '#lib/stores/ui.svelte.js';

  import {
    getNavigationState,
    pushEntity,
    popEntity,
    clearStack,
    setLoadingEntity,
    updateCurrentEntityData
  } from '#lib/stores/navigation.svelte.js';
  import type {
    Book,
    SeriesSummary,
    AuthorSummary,
    AuthorSearchResult,
    SeriesSearchResult,
    BookFormData
  } from '#lib/types.js';

  import {
    getStreamState,
    onReconnect,
    offReconnect,
    onDebounced,
    off
  } from '#lib/services/eventstream.svelte.js';

  import {
    loadLibrary,
    reloadLibraryInPlace,
    loadMoreBooks,
    loadPreviousBooks,
    toggleBook,
    loadShelves,
    loadDevices,
    showMessage
  } from '#lib/context/app.svelte.js';
  import { updateUrlParams } from '#lib/utils/url-state.js';
  import {
    captureScrollAnchor,
    restoreScrollAnchor,
    scrollContainerToTop,
    scrollItemToTop
  } from '#lib/utils/scroll-anchor.js';
  import { getStatsState } from '#lib/stores/stats.svelte.js';
  import * as api from '#lib/api/client.js';
  import { searchBookReleases, queueBookRelease } from '#lib/api/client.js';
  import type { MetadataResult } from '#lib/api/client.js';
  import type { ProviderInfo } from '#lib/components/MetadataSearchModal.svelte';
  import { getProviderCode, getProviderColor, getProviderName } from '#lib/utils/format.js';

  // Store state
  const library = getLibraryState();
  const devicesStore = getDevicesState();
  const ui = getUIState();
  const streamState = getStreamState();
  const stats = getStatsState();
  const nav = getNavigationState();

  // Derived state
  const books = $derived(library.books);
  const booksNextCursor = $derived(library.booksNextCursor);
  const booksPrevCursor = $derived(library.booksPrevCursor);
  const booksTotal = $derived(library.booksTotal);
  const selected = $derived(library.selected);
  const loadState = $derived(library.loadState);
  const shelves = $derived(library.shelves);
  const seriesList = $derived(library.seriesList);
  const devices = $derived(devicesStore.devices);
  const searchQuery = $derived(ui.searchQuery);
  const multiSelect = $derived(ui.multiSelect);
  const showDetails = $derived(ui.showDetails);
  const showRefresh = $derived(streamState.showBanner);

  // Reconnect/SSE handlers
  let reconnectHandler: (() => void) | null = null;
  let sseHandler: ((data: unknown) => void) | null = null;

  // Local state
  let loading = $state(false);
  let error = $state('');
  let availableLetters = $state<string[]>([]);
  let editMode = $state(false);
  let showLinkMetadataModal = $state(false);
  let showLinkHardcoverModal = $state(false);
  let showEditModal = $state(false);
  let showDevicePicker = $state(false);
  let showDeleteConfirm = $state(false);
  let showShelfPicker = $state(false);
  let showReleaseSearchModal = $state(false);
  let showAddBookModal = $state(false);
  let hasOrphanedBooks = $state(false);
  let deleting = $state(false);
  let shelfTargetId = $state('');

  // BookForm external footer bindings
  let formRequestSave: (() => void) | undefined = $state();
  let formCanSave: boolean | undefined = $state();

  // Navigation sidebar state for author/series
  let navSeriesBooks = $state<Book[]>([]);
  let navAuthorBooks = $state<Book[]>([]);

  // Search results for edit mode (populated when user searches metadata)
  let editSearchResults = $state<MetadataResult[]>([]);
  let editProviders = $state<ProviderInfo[]>([]);

  // Clear search results when selected book changes
  $effect(() => {
    selectedBook; // track
    editSearchResults = [];
    editProviders = [];
  });

  // Derived navigation state
  const currentNavEntity = $derived(nav.current);

  const currentNavBook = $derived(
    currentNavEntity?.type === 'book' ? (currentNavEntity.data as Book | undefined) : undefined
  );

  const currentNavSeries = $derived(
    currentNavEntity?.type === 'series'
      ? (currentNavEntity.data as SeriesSummary | undefined)
      : undefined
  );

  const currentNavAuthor = $derived(
    currentNavEntity?.type === 'author'
      ? (currentNavEntity.data as AuthorSummary | undefined)
      : undefined
  );

  // Show nav sidebar when we have any entity in the stack (navigation mode)
  const showNavSidebar = $derived(nav.isOpen && currentNavEntity !== undefined);
  // Hide the original book details sidebar when nav sidebar is active
  const showBookDetailsSidebar = $derived(
    !showNavSidebar && showDetails && library.selected.size === 1 && !multiSelect
  );
  // Unified sidebar state
  const sidebarOpen = $derived(showBookDetailsSidebar || showNavSidebar);
  const sidebarTitle = $derived(
    editMode
      ? 'Edit Book'
      : showNavSidebar
        ? currentNavEntity?.type === 'book'
          ? 'Book Details'
          : currentNavEntity?.type === 'series'
            ? 'Series Details'
            : 'Author Details'
        : 'Book Details'
  );
  const showEditButton = $derived(showNavSidebar ? currentNavEntity?.type === 'book' : !editMode);
  const showRefreshButton = $derived(showNavSidebar || !editMode);

  // Selected book - access library state directly in derived to establish proper tracking
  const selectedBook = $derived.by(() => {
    // Access library.selected directly (a SvelteSet)
    const sel = library.selected;
    if (sel.size !== 1) return null;
    const bookId = Array.from(sel)[0];
    // Access library.books directly
    const booksList = library.books;
    return booksList.find((b) => b.id === bookId) ?? null;
  });

  const apiBase = import.meta.env.VITE_API_BASE_URL || '';

  // URL state sync
  const urlSearchQuery = $derived(page.url.searchParams.get('q') || '');

  const urlSelectedBook = $derived(page.url.searchParams.get('book'));
  const urlAround = $derived(page.url.searchParams.get('around'));
  const urlAroundBefore = $derived(page.url.searchParams.get('around_before'));
  const urlFilter = $derived(
    (page.url.searchParams.get('filter') as 'all' | 'recent' | 'pending' | 'synced') || 'recent'
  );
  const urlOwnership = $derived(
    (page.url.searchParams.get('ownership') as 'all' | 'owned' | 'wanted' | 'orphaned') || 'owned'
  );

  // Track current around value to detect changes
  let lastAround = $state<string | null>(null);
  $effect(() => {
    if (urlAround) lastAround = urlAround;
  });

  // The bar has to answer for the same shelf the grid is showing, or it advertises
  // letters with nothing under them and jumps to books the list then filters out.
  const formatParam = () => {
    const fmt = getFormatFilter();
    return fmt === 'all' ? undefined : fmt;
  };

  // Fetch available letters for alphabet bar
  const loadAvailableLetters = async () => {
    try {
      availableLetters = await api.getAvailableLetters(true, formatParam());
    } catch (e) {
      console.error('Failed to load available letters:', e);
      availableLetters = [];
    }
  };

  // Determine sort order based on filter
  // 'all' filter uses alphabetical (title) sort for proper pagination
  // 'recent' filter uses created_at sort to show newest first
  const sortForFilter = (filter: string): 'title' | 'created_at' =>
    filter === 'recent' ? 'created_at' : 'title';

  // Track the last ownership filter to detect changes
  let lastOwnership = $state<string>('owned');

  // Handle scroll-to-top event - reload from beginning of list
  const handleScrollToTop = () => {
    const sort = sortForFilter(urlFilter);
    loadLibrary(
      undefined,
      searchQuery || undefined,
      urlFilter,
      undefined,
      sort,
      undefined,
      urlOwnership
    );
  };

  /**
   * Refresh the list after a background change (SSE update, stream reconnect) without
   * moving the reader: re-request the window at its current size around the first
   * visible card, then pin that card back to the same offset.
   */
  const reloadPreservingScroll = () => {
    const anchor = captureScrollAnchor();
    reloadLibraryInPlace({
      anchorId: anchor?.id,
      search: searchQuery || undefined,
      filter: urlFilter,
      sort: sortForFilter(urlFilter),
      ownership: urlOwnership
    }).then(() => restoreScrollAnchor(anchor));
  };

  // Handle global format filter changes - reload from beginning
  const handleFormatFilterChange = () => {
    loadAvailableLetters();
    const sort = sortForFilter(urlFilter);
    loadLibrary(
      undefined,
      searchQuery || undefined,
      urlFilter,
      undefined,
      sort,
      undefined,
      urlOwnership
    );
  };

  // Sync URL to state on mount
  onMount(async () => {
    // Listen for scroll-to-top events from the global button
    window.addEventListener('scrolltotop', handleScrollToTop);

    // Reload when global format filter changes
    window.addEventListener('formatfilterchange', handleFormatFilterChange);

    // Restore search from URL
    if (urlSearchQuery && urlSearchQuery !== searchQuery) {
      setSearchQuery(urlSearchQuery);
    }

    // Load library with current search, filter, around parameter, and sort based on filter
    lastAround = urlAround;
    const sort = sortForFilter(urlFilter);
    const aroundBefore = urlAroundBefore ? parseInt(urlAroundBefore, 10) : undefined;
    await loadLibrary(
      undefined,
      urlSearchQuery || undefined,
      urlFilter,
      urlAround || undefined,
      sort,
      aroundBefore,
      urlOwnership
    );

    // main.content keeps its scrollTop across navigations, so a freshly loaded window
    // would otherwise render underneath wherever the reader was already scrolled.
    // A `book` selection does its own scroll-into-view, so leave that case alone.
    if (urlAround && !urlSelectedBook) {
      if (aroundBefore && aroundBefore > 0) scrollItemToTop(urlAround);
      else scrollContainerToTop();
    }

    // Load shelves and devices for book details
    await Promise.all([loadShelves(), loadDevices()]);

    // Check if orphaned books exist (for showing the filter button)
    try {
      const orphanedCheck = await api.getBooks({ orphaned: true, limit: 1, in_library: true });
      hasOrphanedBooks = orphanedCheck.items.length > 0;
    } catch {
      // Non-critical, silently ignore
    }

    // Not awaited: the bar is furniture beside the grid, and the grid is already on
    // screen by here. Blocking on it held up the selection restore and the SSE
    // handlers behind a request that nothing on the page was waiting for.
    void loadAvailableLetters();

    // Restore selection from URL
    if (urlSelectedBook) {
      setSelection(new Set([urlSelectedBook]));
      setShowDetails(true);
    }

    // Clear the around params from URL after using it (it's a one-time navigation aid)
    // This is deferred to allow the selection effect to run first
    if (urlAround) {
      // Use requestAnimationFrame to defer the URL cleanup until after selection is processed
      requestAnimationFrame(() => {
        updateUrlParams({ around: null, around_before: null });
      });
    }

    // Re-fetch data when SSE reconnects (e.g. PWA coming back from background)
    reconnectHandler = onReconnect(() => {
      reloadPreservingScroll();
      loadAvailableLetters();
    });

    // Re-fetch on library_updated SSE events
    sseHandler = onDebounced('library_updated', () => {
      reloadPreservingScroll();
    });
  });

  onDestroy(() => {
    window.removeEventListener('scrolltotop', handleScrollToTop);
    window.removeEventListener('formatfilterchange', handleFormatFilterChange);
    if (reconnectHandler) offReconnect(reconnectHandler);
    if (sseHandler) off('library_updated', sseHandler);
  });

  // Reflect store load errors into local error state
  $effect(() => {
    if (loadState.library.status === 'error') {
      error = 'Failed to load data. Please check your connection and try again.';
    } else if (loadState.library.status === 'success') {
      error = '';
    }
  });

  // Handle around parameter changes (e.g., navigating from command palette while already on /library)
  $effect(() => {
    const around = urlAround;
    if (around && around !== lastAround) {
      lastAround = around;
      // Reset the selection handler so it will re-trigger
      handledUrlSelectedBook = null;
      // Reload with the new around parameter
      const sort = sortForFilter(urlFilter);
      const aroundBefore = urlAroundBefore ? parseInt(urlAroundBefore, 10) : undefined;
      loadLibrary(
        undefined,
        urlSearchQuery || undefined,
        urlFilter,
        around,
        sort,
        aroundBefore,
        urlOwnership
      ).then(() => {
        if (!urlSelectedBook) {
          if (aroundBefore && aroundBefore > 0) scrollItemToTop(around);
          else scrollContainerToTop();
        }
        // Clear the around params after using it
        updateUrlParams({ around: null, around_before: null });
      });
    }
  });

  // Track IDs we've already handled to avoid re-triggering on list updates
  let handledUrlSelectedBook = $state<string | null>(null);

  // Sync URL selection to state when it changes
  $effect(() => {
    // Skip if no URL selection, or we've already handled this ID
    if (!urlSelectedBook || urlSelectedBook === handledUrlSelectedBook) return;

    const bookInStore = books.find((b) => b.id === urlSelectedBook);
    if (bookInStore) {
      handledUrlSelectedBook = urlSelectedBook;
      setSelection(new Set([urlSelectedBook]));
      setShowDetails(true);
      // Scroll to the selected book card
      setTimeout(() => {
        const element = document.querySelector(`[data-book-id="${urlSelectedBook}"]`);
        element?.scrollIntoView({ block: 'center', behavior: 'smooth' });
      }, 50);
    } else if (books.length > 0 && !urlAround) {
      // Book not in store and no around param - try to fetch by ID for details panel only
      // Don't use ensureBookInStore as it adds to end of list incorrectly
      // The around parameter should be used for proper positioning
      handledUrlSelectedBook = urlSelectedBook;
      api
        .getBook(urlSelectedBook)
        .then((book) => {
          // Just select it for the details panel, but navigate with around for proper list positioning
          ensureBookInStore(book);
          setSelection(new Set([urlSelectedBook]));
          setShowDetails(true);
        })
        .catch((e) => {
          console.error('Failed to fetch book by ID:', e);
        });
    }
  });

  // Reset handled ID when URL selection is cleared
  $effect(() => {
    if (urlSelectedBook === null) {
      handledUrlSelectedBook = null;
    }
  });

  // Load data when navigation stack changes
  $effect(() => {
    const entity = nav.current;
    if (!entity) return;
    if (entity.data) return; // Already have data

    // Load data based on type
    if (entity.type === 'book') {
      loadNavBookData(entity.id);
    } else if (entity.type === 'series') {
      loadNavSeriesData(entity.id);
    } else if (entity.type === 'author') {
      loadNavAuthorData(entity.id);
    }
  });

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
      const [series, seriesBooks] = await Promise.all([
        api.getSeriesById(seriesId),
        api.getSeriesBooks(seriesId, true)
      ]);
      updateCurrentEntityData(series);
      navSeriesBooks = seriesBooks;
    } catch (e) {
      console.error('Failed to load series:', e);
    } finally {
      setLoadingEntity(false);
    }
  }

  async function loadNavAuthorData(authorId: string) {
    setLoadingEntity(true);
    try {
      const [author, authorBooks] = await Promise.all([
        api.getAuthor(authorId),
        api.getAuthorBooks(authorId, true)
      ]);
      updateCurrentEntityData(author);
      navAuthorBooks = authorBooks;
    } catch (e) {
      console.error('Failed to load author:', e);
    } finally {
      setLoadingEntity(false);
    }
  }

  // Shelf options for picker
  const shelfOptions = (): SearchSelectItem[] =>
    shelves.map((shelf) => ({ value: shelf.id, label: shelf.name }));

  const isNewShelf = () => shelfTargetId && !shelves.find((s) => s.id === shelfTargetId);

  // Handlers
  const handleToggleBook = (bookId: string, event?: MouseEvent | KeyboardEvent) => {
    if (!multiSelect && window.matchMedia('(max-width: 768px)').matches) {
      goto(`/library/books/${bookId}`);
      return;
    }
    toggleBook(bookId, event);
    // Only update URL for single selection - multiselect doesn't use URL state
    if (!multiSelect) {
      updateUrlParams({ book: bookId });
    }
  };

  const handleLoadMore = () => {
    loadMoreBooks();
  };

  const handleLoadPrevious = () => {
    loadPreviousBooks();
  };

  const handleRefresh = async () => {
    const sort = sortForFilter(urlFilter);
    await loadLibrary(undefined, searchQuery, urlFilter, undefined, sort, undefined, urlOwnership);
  };

  const handleToggleMultiSelect = () => {
    const newState = !multiSelect;
    setMultiSelect(newState);
    if (!newState) {
      setSelection(new Set());
      updateUrlParams({ book: null });
    }
  };

  // Track the last filter to detect changes
  let lastFilter = $state<string>('recent');

  // Reload library when filter changes (sort order and backend filter change)
  // Skip if there's an around param - the around effect will handle the load
  $effect(() => {
    if (urlFilter !== lastFilter) {
      lastFilter = urlFilter;
      // If there's an around param, let the around effect handle the reload
      if (urlAround) {
        return;
      }
      const sort = sortForFilter(urlFilter);
      // Reload from the beginning with the new filter and sort order. A new list starts
      // at the top: the scroll container keeps its offset across the swap.
      loadLibrary(
        undefined,
        searchQuery || undefined,
        urlFilter,
        undefined,
        sort,
        undefined,
        urlOwnership
      ).then(scrollContainerToTop);
    }
  });

  // Reload library when ownership changes
  $effect(() => {
    if (urlOwnership !== lastOwnership) {
      lastOwnership = urlOwnership;
      const sort = sortForFilter(urlFilter);
      loadLibrary(
        undefined,
        searchQuery || undefined,
        urlFilter,
        undefined,
        sort,
        undefined,
        urlOwnership
      ).then(scrollContainerToTop);
    }
  });

  const handleFilterChange = (filter: 'all' | 'recent' | 'pending' | 'synced') => {
    updateUrlParams({ filter: filter === 'recent' ? null : filter });
  };

  const handleOwnershipChange = (ownership: 'all' | 'owned' | 'wanted' | 'orphaned') => {
    // Always set the ownership param explicitly (don't null it for 'all')
    updateUrlParams({ ownership });
  };

  // Jump to letter navigation - calls backend to find first book starting with letter
  // Always uses 'all' filter with alphabetical sort since letter navigation only makes sense alphabetically
  const handleJumpToLetter = async (letter: string | null) => {
    if (!letter) {
      // "All" button - reload without around parameter
      const sort = sortForFilter(urlFilter);
      await loadLibrary(
        undefined,
        searchQuery || undefined,
        urlFilter,
        undefined,
        sort,
        undefined,
        urlOwnership
      );
      scrollContainerToTop();
      return;
    }

    loading = true;
    try {
      // Always search in 'all' filter - letter navigation requires alphabetical sort
      const result = await api.getFirstBookByLetter(letter, 'all', true, formatParam());
      if (result) {
        // Navigate with around parameter, force 'all' filter, and around_before=0 to show
        // the target letter at the top (not centered)
        goto(`/library?around=${result.id}&filter=all&around_before=0`);
      } else {
        showMessage(`No books starting with "${letter}"`);
      }
    } catch {
      showMessage(`Failed to find books for "${letter}"`);
    } finally {
      loading = false;
    }
  };

  const handleNavigate = (params: {
    filter?: 'all' | 'recent' | 'pending' | 'synced';
    ownership?: 'all' | 'owned' | 'wanted' | 'orphaned';
  }) => {
    const urlParams: Record<string, string | null> = {};

    if (params.filter !== undefined) {
      urlParams.filter = params.filter === 'recent' ? null : params.filter;
    }
    if (params.ownership !== undefined) {
      // Always set ownership explicitly (don't null it for 'all')
      urlParams.ownership = params.ownership;
    }

    updateUrlParams(urlParams);
  };

  const handleCloseDetails = () => {
    setShowDetails(false);
    updateUrlParams({ book: null });
  };

  const handleForceResync = async () => {
    if (selected.size === 0) return;
    loading = true;
    try {
      await api.touchBooks(Array.from(selected));
      showMessage('Books will be re-synced');
      const sort = sortForFilter(urlFilter);
      await loadLibrary(undefined, undefined, urlFilter, undefined, sort, undefined, urlOwnership);
    } catch {
      showMessage('Force resync failed');
    } finally {
      loading = false;
    }
  };

  const handleEdit = () => {
    editMode = !editMode;
  };

  const handleInlineEditSave = async (data: BookFormData, coverFile?: File) => {
    const book = selectedBook;
    if (!book) return;

    loading = true;
    try {
      await api.updateBook(book.id, {
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

      // Upload cover if provided
      if (coverFile) {
        await api.uploadBookCover(book.id, coverFile);
      }

      showMessage('Book updated');
      editMode = false;
      const sort = sortForFilter(urlFilter);
      await loadLibrary(undefined, undefined, urlFilter, undefined, sort, undefined, urlOwnership);
    } catch {
      showMessage('Update failed');
    } finally {
      loading = false;
    }
  };

  const handleEditCancel = () => {
    editMode = false;
    // Clear search results when exiting edit mode
    editSearchResults = [];
    editProviders = [];
  };

  // Want/skip handlers for non-owned books
  const handleWantBook = async () => {
    const book = selectedBook;
    if (!book) return;

    await api.wantBook(book.id);
    showMessage(`Added "${book.title}" to wanted list`);
    // Refresh to update book status in store
    const updatedBook = await api.getBook(book.id);
    ensureBookInStore(updatedBook);
  };

  const handleSkipBook = async () => {
    const book = selectedBook;
    if (!book) return;

    await api.skipBook(book.id);
    showMessage(`Skipped "${book.title}"`);
    // Refresh to update book status in store
    const updatedBook = await api.getBook(book.id);
    ensureBookInStore(updatedBook);
  };

  const handleSaveEdits = async (data: EditData, coverFile?: File) => {
    const book = selectedBook;
    if (!book) return;

    loading = true;
    try {
      // Upload cover if a file was provided
      if (coverFile) {
        const formData = new FormData();
        formData.append('cover', coverFile);
        await fetch(`${apiBase}/api/v1/library/books/${book.id}/cover`, {
          method: 'POST',
          credentials: 'include',
          body: formData
        });
      }

      // Build the update payload
      const payload = {
        title: data.title,
        // Convert string[] to AuthorInput[] (old modal format to new API format)
        authors: data.authors.map((name) => ({ name })),
        series_name: data.series_name || undefined,
        series_id: data.series_id || undefined,
        series_index: data.series_index ?? undefined,
        language: data.language || undefined,
        publisher: data.publisher || undefined,
        pubdate: data.pubdate || undefined,
        description: data.description || undefined,
        identifiers: Object.keys(data.identifiers).length > 0 ? data.identifiers : undefined,
        series_memberships: data.series_memberships
      };

      await api.updateBook(book.id, payload);
      showMessage('Book updated');
      showEditModal = false;
      const sort = sortForFilter(urlFilter);
      await loadLibrary(undefined, undefined, urlFilter, undefined, sort, undefined, urlOwnership);
    } catch {
      showMessage('Update failed');
    } finally {
      loading = false;
    }
  };

  const handleDelete = async () => {
    const book = selectedBook;
    if (!book) return;

    deleting = true;
    try {
      await api.deleteBook(book.id);
      showMessage('Book deleted');
      showDeleteConfirm = false;
      handleCloseDetails();
      const sort = sortForFilter(urlFilter);
      await loadLibrary(undefined, undefined, urlFilter, undefined, sort, undefined, urlOwnership);
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete book');
    } finally {
      deleting = false;
    }
  };

  const handleMetadataResultsSelected = (
    results: MetadataResult[] | AuthorSearchResult[] | SeriesSearchResult[]
  ) => {
    // Close the modal and store results for the form
    showLinkMetadataModal = false;

    // For books, we expect MetadataResult[]
    const bookResults = results as MetadataResult[];

    // Get provider info from results
    const providerMap = new Map<string, ProviderInfo>();
    for (const result of bookResults) {
      if (!providerMap.has(result.provider)) {
        providerMap.set(result.provider, {
          id: result.provider,
          name: getProviderName(result.provider),
          code: getProviderCode(result.provider),
          color: getProviderColor(result.provider),
          priority: providerMap.size
        });
      }
    }

    editSearchResults = bookResults;
    editProviders = Array.from(providerMap.values());

    // Extract identifiers from results
    const identifiers: Record<string, string> = {};
    for (const result of bookResults) {
      if (result.external_id && result.provider) {
        identifiers[result.provider] = result.external_id;
      }
      if (result.identifiers) {
        Object.assign(identifiers, result.identifiers);
      }
    }

    // Also update the book in the store with new identifiers
    if (selectedBook && Object.keys(identifiers).length > 0) {
      const updatedBook = {
        ...selectedBook,
        identifiers: { ...selectedBook.identifiers, ...identifiers }
      };
      ensureBookInStore(updatedBook);
    }

    showMessage(`Loaded metadata from ${bookResults.length} source(s)`);
  };

  const handleLinkHardcover = async (hardcoverId: string) => {
    const book = selectedBook;
    if (!book) return;

    const bookId = book.id;
    loading = true;
    try {
      // Merge the new hardcover ID with existing identifiers
      const identifiers = { ...book.identifiers, hardcover: hardcoverId };
      await api.updateBook(bookId, { identifiers });
      showMessage('Hardcover linked successfully');
      showLinkHardcoverModal = false;

      // Fetch the updated book and update it in-place (don't reload entire library)
      const updatedBook = await api.getBook(bookId);
      ensureBookInStore(updatedBook);
      // Selection should already be set, but ensure it's still there
      setSelection(new Set([bookId]));
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to link Hardcover');
    } finally {
      loading = false;
    }
  };

  const handleBulkDelete = async () => {
    if (selected.size === 0) return;

    const confirmed = confirm(`Delete ${selected.size} book(s)? This cannot be undone.`);
    if (!confirmed) return;

    loading = true;
    try {
      for (const bookId of selected) {
        await api.deleteBook(bookId);
      }
      showMessage(`${selected.size} book(s) deleted`);
      setSelection(new Set());
      setShowDetails(false);
      updateUrlParams({ book: null });
      const sort = sortForFilter(urlFilter);
      await loadLibrary(
        undefined,
        searchQuery || undefined,
        urlFilter,
        undefined,
        sort,
        undefined,
        urlOwnership
      );
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete books');
    } finally {
      loading = false;
    }
  };

  const handleOpenShelfPicker = () => {
    showShelfPicker = true;
    shelfTargetId = '';
  };

  const handleAddToShelf = async () => {
    if (!shelfTargetId || selected.size === 0) return;

    loading = true;
    try {
      // Check if this is an existing shelf or a new shelf name
      const existingShelf = shelves.find((s) => s.id === shelfTargetId);
      let targetId = shelfTargetId;

      if (!existingShelf) {
        // Create new shelf
        const result = await api.createShelf(shelfTargetId);
        showMessage('Shelf created');
        await loadShelves();
        targetId = result.id;
      }

      await api.addToShelf(targetId, Array.from(selected));
      showMessage('Added to shelf');
      showShelfPicker = false;
    } catch {
      showMessage('Add to shelf failed');
    } finally {
      loading = false;
    }
  };

  const handleSendBooksToDevice = async (deviceIds: string[]) => {
    if (deviceIds.length === 0 || selected.size === 0) return;
    loading = true;
    try {
      for (const deviceId of deviceIds) {
        await api.addDeviceBooks(deviceId, Array.from(selected));
      }
      showMessage('Books sent to device(s)');
      showDevicePicker = false;
    } catch {
      showMessage('Send to device failed');
    } finally {
      loading = false;
    }
  };

  // Navigation handlers - push to stack for sidebar navigation
  // Push the current book first so back navigation returns to it
  const handleAuthorClick = (_authorName: string, authorId: string) => {
    // Turn off book details sidebar - we're switching to nav sidebar mode
    setShowDetails(false);
    if (selectedBook) {
      // Push book first so we can go back to it
      pushEntity('book', selectedBook.id, selectedBook);
    }
    pushEntity('author', authorId);
  };

  const handleSeriesClick = (seriesId: string) => {
    // Turn off book details sidebar - we're switching to nav sidebar mode
    setShowDetails(false);
    if (selectedBook) {
      // Push book first so we can go back to it
      pushEntity('book', selectedBook.id, selectedBook);
    }
    pushEntity('series', seriesId);
  };

  // Handle back navigation in author/series sidebar
  const handleNavBack = () => {
    popEntity();
    navSeriesBooks = [];
    navAuthorBooks = [];
  };

  // Handle close navigation sidebar
  const handleNavClose = () => {
    clearStack();
    navSeriesBooks = [];
    navAuthorBooks = [];
    // Also clear selection to prevent book details sidebar from reopening
    setSelection(new Set());
    updateUrlParams({ book: null });
  };

  // Navigate from nav sidebar to full page view
  const handleNavExpand = () => {
    const entity = nav.current;
    if (!entity) return;
    const wasEditing = editMode;
    editMode = false;

    if (entity.type === 'book') {
      goto(`/library/books/${entity.id}`, wasEditing ? { state: { editing: true } } : undefined);
    } else if (entity.type === 'series') {
      goto(`/series/${entity.id}`);
    } else if (entity.type === 'author') {
      goto(`/authors/${entity.id}`);
    }
    clearStack();
  };

  // Handle book click from series/author sidebar - push to stack for inline viewing
  const handleNavBookClick = (book: Book) => {
    if (book.is_in_library !== false) {
      pushEntity('book', book.id, book);
    }
  };

  const handleGenreClick = (genre: string) => {
    // Filter library by genre - use search for now
    setSearchQuery(genre);
    updateUrlParams({ q: genre });
    const sort = sortForFilter(urlFilter);
    loadLibrary(undefined, genre, urlFilter, undefined, sort, undefined, urlOwnership).then(
      scrollContainerToTop
    );
  };

  const handleSearch = (query: string) => {
    setSearchQuery(query);
    updateUrlParams({ q: query || null });
    const sort = sortForFilter(urlFilter);
    loadLibrary(
      undefined,
      query || undefined,
      urlFilter,
      undefined,
      sort,
      undefined,
      urlOwnership
    ).then(scrollContainerToTop);
  };
</script>

<svelte:head>
  <title>Library — Kobbler</title>
</svelte:head>

<div class="library-layout page-container">
  <div class="library-main">
    {#if error}
      <div class="error-wrapper">
        <ErrorBanner
          message={error}
          onRetry={() =>
            loadLibrary(
              undefined,
              urlSearchQuery || undefined,
              urlFilter,
              undefined,
              sortForFilter(urlFilter),
              undefined,
              urlOwnership
            )}
          onDismiss={() => (error = '')}
        />
      </div>
    {/if}

    <LibraryPage
      {books}
      {booksNextCursor}
      {booksPrevCursor}
      {booksTotal}
      {selected}
      {loading}
      loadState={loadState.library}
      {multiSelect}
      {showRefresh}
      filter={urlFilter}
      ownership={urlOwnership}
      {searchQuery}
      onToggleBook={handleToggleBook}
      onLoadMore={handleLoadMore}
      onLoadPrevious={handleLoadPrevious}
      onRefresh={handleRefresh}
      onToggleMultiSelect={handleToggleMultiSelect}
      onFilterChange={handleFilterChange}
      onOwnershipChange={handleOwnershipChange}
      onSearch={handleSearch}
      onJumpToLetter={handleJumpToLetter}
      onNavigate={handleNavigate}
      {availableLetters}
      onAdd={() => (showAddBookModal = true)}
      {hasOrphanedBooks}
    />
  </div>
</div>

<!-- Unified Sidebar - one sidebar that switches content based on mode -->
<ExpandableSidebar
  open={sidebarOpen}
  expanded={editMode}
  title={sidebarTitle}
  showEdit={showEditButton}
  showRefresh={showRefreshButton}
  {editMode}
  onClose={() => {
    if (editMode) {
      editMode = false;
    } else if (showNavSidebar) {
      handleNavClose();
    } else {
      handleCloseDetails();
    }
  }}
  onExpand={() => {
    if (showNavSidebar) {
      handleNavExpand();
    } else if (selectedBook) {
      const wasEditing = editMode;
      editMode = false;
      goto(
        `/library/books/${selectedBook.id}`,
        wasEditing ? { state: { editing: true } } : undefined
      );
    }
  }}
  onToggleEdit={showNavSidebar && currentNavEntity?.type !== 'book' ? undefined : handleEdit}
  onRefresh={() => {
    if (showNavSidebar) {
      if (currentNavEntity?.type === 'book') {
        loadNavBookData(currentNavEntity.id);
      } else if (currentNavEntity?.type === 'series') {
        loadNavSeriesData(currentNavEntity.id);
      } else if (currentNavEntity?.type === 'author') {
        loadNavAuthorData(currentNavEntity.id);
      }
    } else {
      handleRefresh();
    }
  }}
  onSearch={() => (showLinkMetadataModal = true)}
>
  {#snippet headerLeft()}
    {#if showNavSidebar && nav.canGoBack}
      <button class="icon-btn small" type="button" title="Back" onclick={handleNavBack}>
        <span class="material-icons-outlined">arrow_back</span>
      </button>
    {/if}
  {/snippet}

  {#snippet footer()}
    {#if editMode && selectedBook}
      <div class="sidebar-footer-actions">
        <button
          class="btn-danger-outline"
          type="button"
          onclick={() => (showDeleteConfirm = true)}
          disabled={deleting || loading}
        >
          {deleting ? 'Deleting...' : 'Delete Book'}
        </button>
        <div style="flex: 1;"></div>

        <button class="btn-sec" type="button" onclick={handleEditCancel} disabled={loading}
          >Cancel</button
        >

        <button
          class="btn-primary"
          type="button"
          onclick={() => formRequestSave?.()}
          disabled={!formCanSave}
        >
          {loading ? 'Saving...' : 'Save Changes'}
        </button>
      </div>
    {/if}
  {/snippet}

  <!-- Book Details (non-navigation mode) -->
  {#if showBookDetailsSidebar && selectedBook}
    {#if editMode}
      {#key selectedBook?.id}
        <BookForm
          book={selectedBook}
          searchResults={editSearchResults}
          providers={editProviders}
          onSave={handleInlineEditSave}
          onCancel={handleEditCancel}
          onDelete={() => (showDeleteConfirm = true)}
          saving={loading}
          {deleting}
          hideFooter
          bind:requestSave={formRequestSave}
          bind:canSave={formCanSave}
        />
      {/key}
    {:else}
      <BookDetailsPanel
        book={selectedBook}
        context="library"
        {devices}
        {loading}
        embedded
        onClose={handleCloseDetails}
        onForceResync={handleForceResync}
        onSendToDevice={() => (showDevicePicker = true)}
        onAddToShelf={handleOpenShelfPicker}
        onSearchReleases={stats.stats.kobload_configured
          ? () => (showReleaseSearchModal = true)
          : undefined}
        onLinkSource={() => (showLinkMetadataModal = true)}
        onAuthorClick={handleAuthorClick}
        onSeriesClick={handleSeriesClick}
        onGenreClick={handleGenreClick}
        onRefresh={handleRefresh}
        onWant={selectedBook?.is_in_library ? undefined : handleWantBook}
        onSkip={selectedBook?.is_in_library ? undefined : handleSkipBook}
      />
    {/if}
    <!-- Navigation mode content -->
  {:else if showNavSidebar}
    {#if nav.loadingEntity}
      <div class="loading-state">Loading...</div>
    {:else if currentNavEntity?.type === 'book' && currentNavBook}
      <BookDetailsPanel
        book={currentNavBook}
        context="library"
        {devices}
        {loading}
        embedded
        onClose={handleNavClose}
        onForceResync={() =>
          api.touchBooks([currentNavBook.id]).then(() => loadNavBookData(currentNavBook.id))}
        onSendToDevice={() => (showDevicePicker = true)}
        onAddToShelf={handleOpenShelfPicker}
        onSearchReleases={stats.stats.kobload_configured
          ? () => (showReleaseSearchModal = true)
          : undefined}
        onLinkSource={() => (showLinkMetadataModal = true)}
        onAuthorClick={handleAuthorClick}
        onSeriesClick={handleSeriesClick}
        onGenreClick={handleGenreClick}
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
    {:else if currentNavEntity?.type === 'series' && currentNavSeries}
      <SeriesSidebarContent
        series={currentNavSeries}
        books={navSeriesBooks}
        loadState={{ status: nav.loadingEntity ? 'loading' : 'success', error: '' }}
        onAuthorClick={handleAuthorClick}
        onBookClick={handleNavBookClick}
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
    {:else if currentNavEntity?.type === 'author' && currentNavAuthor}
      <AuthorSidebarContent
        author={currentNavAuthor}
        books={navAuthorBooks}
        loadState={{ status: nav.loadingEntity ? 'loading' : 'success', error: '' }}
        loading={nav.loadingEntity}
        onSeriesClick={handleSeriesClick}
        onBookClick={handleNavBookClick}
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

<!-- Multi-select action bar - show when in multiSelect mode with any selection, or 2+ selected via shift/ctrl -->
<MultiSelectBar
  count={selected.size}
  visible={(multiSelect && selected.size > 0) || selected.size > 1}
  onSync={handleForceResync}
  onSendToDevice={() => (showDevicePicker = true)}
  onAddToShelf={handleOpenShelfPicker}
  onDelete={handleBulkDelete}
  onClear={() => {
    setSelection(new Set());
    setShowDetails(false);
    updateUrlParams({ book: null });
  }}
/>

<!-- Edit Modal -->
{#if showEditModal && selectedBook}
  <BookEditModal
    book={selectedBook}
    mode="edit"
    {seriesList}
    {loading}
    onSave={handleSaveEdits}
    onClose={() => (showEditModal = false)}
    onShowMessage={showMessage}
  />
{/if}

<!-- Device Picker Modal -->
{#if showDevicePicker}
  <DevicePickerModal
    {devices}
    bookCount={selected.size}
    {loading}
    onClose={() => (showDevicePicker = false)}
    onSend={handleSendBooksToDevice}
  />
{/if}

<!-- Delete Confirm Modal -->
{#if showDeleteConfirm && selectedBook}
  <DeleteConfirmModal
    title="Delete Book"
    message={`Are you sure you want to delete "${selectedBook.title}"? This will remove the book and all associated data (shelves, reading states, etc.). This action cannot be undone.`}
    confirmLabel={deleting ? 'Deleting...' : 'Delete Book'}
    loading={deleting}
    onCancel={() => (showDeleteConfirm = false)}
    onConfirm={handleDelete}
  />
{/if}

<!-- Link Metadata Modal (full metadata search) -->
{#if showLinkMetadataModal && selectedBook}
  <AddEntityModal
    mode="book"
    linkMode={true}
    existingId={selectedBook.id}
    initialSearchValues={{
      title: selectedBook.title,
      author: selectedBook.authors?.[0]?.name
    }}
    onComplete={() => (showLinkMetadataModal = false)}
    onResultsSelected={handleMetadataResultsSelected}
    onCancel={() => (showLinkMetadataModal = false)}
  />
{/if}

<!-- Quick Link Provider Modal -->
{#if showLinkHardcoverModal && selectedBook}
  <QuickLinkProviderModal
    bookTitle={selectedBook.title}
    bookAuthor={selectedBook.authors?.[0]?.name}
    provider="hardcover"
    providerName="Hardcover"
    onLink={handleLinkHardcover}
    onClose={() => (showLinkHardcoverModal = false)}
  />
{/if}

<!-- Release Search Modal -->
{#if showReleaseSearchModal && selectedBook}
  <ReleaseSearchModal
    entityTitle={selectedBook.title}
    searchFn={(query, scope) => searchBookReleases(selectedBook.id, query, scope)}
    scopeLabel="book"
    smartSearchFn={() => searchBookReleases(selectedBook.id)}
    queueFn={(release, source) =>
      queueBookRelease(selectedBook.id, release as Record<string, unknown>, source)}
    wantFn={async () => {
      await api.wantBook(selectedBook.id);
      showMessage(`Searching for best match for "${selectedBook.title}"`);
      handleRefresh();
    }}
    initialQuery={`${selectedBook.title} ${selectedBook.authors?.[0]?.name ?? ''}`.trim()}
    onClose={() => (showReleaseSearchModal = false)}
  />
{/if}

<!-- Add Book Modal -->
{#if showAddBookModal}
  <AddEntityModal
    mode="book"
    onComplete={(id) => {
      showAddBookModal = false;
      // Refresh the library to show the new book
      handleRefresh();
      // Optionally navigate to the new book
      goto(`/library/books/${id}`);
    }}
    onCancel={() => (showAddBookModal = false)}
  />
{/if}

<!-- Shelf Picker Modal -->
{#if showShelfPicker}
  <button
    class="modal-backdrop"
    type="button"
    aria-label="Close shelf picker"
    onclick={() => (showShelfPicker = false)}
  ></button>
  <div class="modal shelf-modal">
    <div class="modal-header">
      <h3>Add to Shelf</h3>
      <button
        class="icon-btn"
        type="button"
        aria-label="Close"
        onclick={() => (showShelfPicker = false)}
      >
        <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor">
          <path d="M6.4 5.6 5 7l5 5-5 5 1.4 1.4 5-5 5 5L17.8 17l-5-5 5-5-1.4-1.4-5 5-5-5z"></path>
        </svg>
      </button>
    </div>
    <div class="modal-content">
      <div class="shelf-select-row">
        <SearchSelect
          bind:value={shelfTargetId}
          items={shelfOptions()}
          placeholder="Select or create shelf..."
          allowCustom
        />
        <button
          class="btn"
          class:alt={!isNewShelf()}
          type="button"
          onclick={handleAddToShelf}
          disabled={loading || !shelfTargetId}
        >
          {isNewShelf() ? 'Create & Add' : 'Add'}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .error-wrapper {
    padding: 12px 24px 0;
  }

  .library-layout {
    min-height: 0;
  }

  .sidebar-footer-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .sidebar-footer-actions :global(button) {
    padding: 8px 16px;
    font-size: 12px;
  }

  /* Modal styles */
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    z-index: 100;
    border: none;
    cursor: pointer;
  }

  .modal {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    z-index: 101;
    min-width: 300px;
    max-width: 90vw;
  }

  .shelf-modal {
    width: min(90vw, 400px);
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--ring);
  }

  .modal-header h3 {
    margin: 0;
    font-size: 16px;
  }

  .modal-content {
    padding: 20px;
  }

  .shelf-select-row {
    display: flex;
    gap: 10px;
    align-items: center;
  }

  .shelf-select-row :global(.search-select) {
    flex: 1;
  }

  .icon-btn {
    flex-shrink: 0;
    border-radius: 8px;
    padding: 6px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--muted);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transition: all 0.15s ease;
  }

  .icon-btn:hover {
    background: var(--ring);
    color: var(--ink);
  }

  .btn {
    padding: 10px 16px;
    border-radius: 10px;
    border: none;
    background: var(--accent);
    color: #1d130c;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .btn:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
  }

  .btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn.alt {
    background: var(--card);
    border: 1px solid var(--ring);
    color: var(--ink);
  }

  .btn.alt:hover:not(:disabled) {
    background: var(--ring);
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
