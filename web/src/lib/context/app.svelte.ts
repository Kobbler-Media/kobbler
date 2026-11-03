/**
 * App context - shared functionality across all route pages
 * Provides message display, clipboard helpers, and common state
 */

import { showToast, setShowDetails, getUIState } from '#lib/stores/ui.svelte.js';
import {
  getLibraryState,
  setSelection,
  setBooks,
  setSeries,
  setAuthors,
  setShelves,
  setShelfItems,
  setSeriesItems,
  setAuthorItems,
  patchBook,
  startLoad,
  finishLoad,
  failLoad,
  resetLoad
} from '#lib/stores/library.svelte.js';
import {
  getDevicesState,
  setDevices,
  setActivityEvents,
  setUsers,
  setIngestTokens,
  setProxyLogs
} from '#lib/stores/devices.svelte.js';
import { getAuthState, setAuthed } from '#lib/stores/auth.svelte.js';
import { ensureStream, closeStream } from '#lib/services/eventstream.svelte.js';
import * as api from '#lib/api/client.js';
import type { Book, PaginatedResponse } from '#lib/types.js';
import type { ProxyLogsQuery } from '#lib/api/client.js';
import { getFormatFilter } from '#lib/stores/format-filter.svelte.js';

const apiBase = import.meta.env.VITE_API_BASE_URL || '';

// Request ID for cancelling stale requests
let libraryRequestId = 0;

/**
 * Show a toast message
 */
export function showMessage(message: string) {
  showToast(message);
}

/**
 * Copy text to clipboard with feedback
 */
export async function copyText(value: string, label: string) {
  try {
    await navigator.clipboard.writeText(value);
    showMessage(`${label} copied`);
  } catch {
    showMessage('Copy failed');
  }
}

/**
 * Toggle book selection
 */
export function toggleBook(bookId: string, event?: MouseEvent | KeyboardEvent) {
  const library = getLibraryState();
  const ui = getUIState();
  const modifier = event && (event.metaKey || event.ctrlKey || event.shiftKey);
  const useMulti = modifier || ui.multiSelect;

  if (!useMulti) {
    // If clicking the already-selected item, deselect it
    if (library.selected.size === 1 && library.selected.has(bookId)) {
      // eslint-disable-next-line svelte/prefer-svelte-reactivity
      setSelection(new Set());
      setShowDetails(false);
      return;
    }
    // eslint-disable-next-line svelte/prefer-svelte-reactivity
    setSelection(new Set([bookId]));
    setShowDetails(true);
    return;
  }

  // eslint-disable-next-line svelte/prefer-svelte-reactivity
  const newSelection = new Set(library.selected);
  if (newSelection.has(bookId)) {
    newSelection.delete(bookId);
  } else {
    newSelection.add(bookId);
  }
  setSelection(newSelection);
  setShowDetails(newSelection.size > 0);
}

// Track the current sort order and filter for pagination consistency
let currentBookSort: 'title' | 'created_at' = 'title';
let currentBookFilter: string | undefined;
let currentOwnership: 'all' | 'owned' | 'wanted' | 'orphaned' = 'all';
// Track search state for offset-based pagination with MeiliSearch
let currentBookSearch: string | undefined;
let currentSearchOffset = 0;
// Track if there are more search results (from backend's next_cursor)
let searchHasMore = false;

/**
 * Load library books
 * @param around - Optional ID to center the view around (for jump-to-item navigation)
 * @param sort - Sort order: 'title' (alphabetical) or 'created_at' (recent first)
 * @param ownership - Ownership filter: 'all' (all books), 'owned' (in library), 'wanted' (want_status=wanted), 'orphaned' (no media root)
 * @param limit - Page size override; used to re-request a window at its current size
 */
export async function loadLibrary(
  cursor?: string,
  search?: string,
  filter?: string,
  around?: string,
  sort?: 'title' | 'created_at',
  aroundBefore?: number,
  ownership?: 'all' | 'owned' | 'wanted' | 'orphaned',
  limit?: number
): Promise<void> {
  const requestId = ++libraryRequestId;
  startLoad('library');

  // Update tracked sort order and filter when provided (on initial/fresh loads)
  if (sort) {
    currentBookSort = sort;
  }
  if (filter !== undefined) {
    currentBookFilter = filter;
  }
  if (ownership !== undefined) {
    currentOwnership = ownership;
  }

  // Track search state for offset-based pagination
  // Reset offset when search query changes or when starting fresh (no cursor)
  if (search !== currentBookSearch || !cursor) {
    currentBookSearch = search;
    currentSearchOffset = 0;
  }

  try {
    // eslint-disable-next-line svelte/prefer-svelte-reactivity
    const params = new URLSearchParams();

    // Apply ownership filter
    if (currentOwnership === 'owned') {
      params.set('in_library', 'true');
    } else if (currentOwnership === 'wanted') {
      params.set('want_status', 'wanted');
    } else if (currentOwnership === 'orphaned') {
      params.set('in_library', 'true');
      params.set('orphaned', 'true');
    } else if (currentOwnership === 'all') {
      // 'all' means owned + explicitly wanted, excluding pipeline states
      params.set('want_status', 'library_visible');
    }

    // When searching, use offset-based pagination (MeiliSearch)
    // When not searching, use cursor-based pagination (database)
    if (search?.trim()) {
      params.set('search', search.trim());
      // Only pass cursor for non-search queries, use search_offset for search
      if (currentSearchOffset > 0) {
        params.set('search_offset', String(currentSearchOffset));
      }
    } else if (cursor) {
      params.set('cursor', cursor);
    }
    // Handle filters:
    // - 'all' and 'recent' are handled by sort order, not filter
    // - 'pending' and 'synced' use the filter parameter (entitlement status)
    if (filter && filter !== 'all' && filter !== 'recent') {
      params.set('filter', filter);
    }
    if (around) params.set('around', around);
    if (aroundBefore !== undefined) params.set('around_before', String(aroundBefore));
    if (limit !== undefined) params.set('limit', String(limit));
    const fmt = getFormatFilter();
    if (fmt !== 'all') params.set('format', fmt);
    // Always pass the sort parameter for proper pagination
    params.set('sort', currentBookSort);

    const queryString = params.toString();
    const url = `${apiBase}/api/v1/library/books${queryString ? `?${queryString}` : ''}`;

    const resp = await fetch(url, { credentials: 'include' });

    if (requestId !== libraryRequestId) return;

    if (!resp.ok) {
      if (resp.status === 401) {
        closeStream();
        setAuthed(false);
      }
      if (around && resp.status === 404) {
        // The anchor book can vanish between render and request (it was just deleted).
        // Retry without it rather than blanking the list.
        return await loadLibrary(
          cursor,
          search,
          filter,
          undefined,
          sort,
          undefined,
          ownership,
          limit
        );
      }
      failLoad('library', 'Failed to load library.');
      return;
    }

    const data: PaginatedResponse<Book> = await resp.json();
    if (requestId !== libraryRequestId) return;

    // Track if there are more search results (for search pagination)
    if (currentBookSearch?.trim()) {
      searchHasMore = !!data.next_cursor;
    }

    // Determine if we should replace or append:
    // - Replace when no cursor AND search offset is 0 (fresh load)
    // - Append when cursor is set (cursor pagination) OR search offset > 0 (search pagination)
    const isReplace = !cursor && currentSearchOffset === 0;
    setBooks(
      data.items,
      data.next_cursor,
      data.total_count,
      isReplace ? 'replace' : 'append',
      data.prev_cursor
    );
    setAuthed(true);
    ensureStream(true);
    finishLoad('library');

    // Only prune selection on replace (not append/prepend which extend the list)
    if (isReplace) {
      const library = getLibraryState();
      const bookIds = library.books.map((book) => book.id);
      const validSelection = Array.from(library.selected).filter((id) => bookIds.includes(id));
      // eslint-disable-next-line svelte/prefer-svelte-reactivity
      setSelection(new Set(validSelection));
    }
  } catch {
    if (requestId === libraryRequestId) {
      failLoad('library', 'Failed to load library. Check API connectivity.');
    }
  }
}

/**
 * Reload the visible library window without moving the reader.
 *
 * A plain reload asks for one default-size page, so an infinitely-scrolled list
 * collapses back to its first page and the browser clamps scrollTop into unrelated
 * books. This re-requests the window at its current size, centred on `anchorId`, so
 * the DOM the reader is looking at comes back unchanged. Callers pin the exact offset
 * with captureScrollAnchor()/restoreScrollAnchor().
 *
 * Falls back to a plain reload while searching: search results are relevance-ordered
 * and offset-paginated, which the `around` window cannot express.
 */
export async function reloadLibraryInPlace(opts: {
  /** Id of the card to keep in view; falls back to the first loaded book. */
  anchorId?: string;
  search?: string;
  filter?: string;
  sort?: 'title' | 'created_at';
  ownership?: 'all' | 'owned' | 'wanted' | 'orphaned';
}): Promise<void> {
  const books = getLibraryState().books;
  if (books.length === 0 || opts.search?.trim()) {
    await loadLibrary(
      undefined,
      opts.search,
      opts.filter,
      undefined,
      opts.sort,
      undefined,
      opts.ownership
    );
    return;
  }

  const anchorIndex = opts.anchorId ? books.findIndex((b) => b.id === opts.anchorId) : -1;
  const index = anchorIndex >= 0 ? anchorIndex : 0;
  const limit = Math.min(Math.max(books.length, 50), 200);
  await loadLibrary(
    undefined,
    opts.search,
    opts.filter,
    books[index].id,
    opts.sort,
    Math.min(index, Math.floor(limit / 2)),
    opts.ownership,
    limit
  );
}

/**
 * Refresh a single book in-place via SSE update.
 * Fetches the book by ID and patches it in the store without replacing the list.
 */
export async function refreshBook(bookId: string): Promise<void> {
  try {
    const book = await api.getBook(bookId);
    patchBook(book);
  } catch {
    // Book may have been deleted or is not visible — ignore
  }
}

/**
 * Load more books (pagination forward)
 * Uses offset-based pagination when searching (MeiliSearch)
 * Uses cursor-based pagination otherwise (database)
 */
export function loadMoreBooks() {
  const library = getLibraryState();
  if (library.loadState.library.status === 'loading') return;

  // When searching, use offset-based pagination
  if (currentBookSearch?.trim()) {
    // Check if backend indicated more search results
    if (searchHasMore) {
      currentSearchOffset += library.books.length - currentSearchOffset; // Increment by items loaded
      loadLibrary(
        undefined, // no cursor for search pagination
        currentBookSearch,
        currentBookFilter,
        undefined,
        currentBookSort,
        undefined,
        currentOwnership
      );
    }
  } else if (library.booksNextCursor) {
    // Not searching - use cursor pagination
    loadLibrary(
      library.booksNextCursor,
      undefined,
      currentBookFilter,
      undefined,
      currentBookSort,
      undefined,
      currentOwnership
    );
  }
}

/**
 * Load previous books (pagination backward)
 */
export async function loadPreviousBooks(): Promise<void> {
  const library = getLibraryState();
  if (!library.booksPrevCursor || library.loadState.library.status === 'loading') {
    return;
  }

  startLoad('library');
  try {
    // eslint-disable-next-line svelte/prefer-svelte-reactivity
    const params = new URLSearchParams();

    // Apply ownership filter
    if (currentOwnership === 'owned') {
      params.set('in_library', 'true');
    } else if (currentOwnership === 'wanted') {
      params.set('want_status', 'wanted');
    } else if (currentOwnership === 'orphaned') {
      params.set('in_library', 'true');
      params.set('orphaned', 'true');
    }

    params.set('cursor', library.booksPrevCursor);
    params.set('direction', 'backward');
    params.set('sort', currentBookSort);
    // Handle filters consistently with loadLibrary
    if (currentBookFilter && currentBookFilter !== 'all' && currentBookFilter !== 'recent') {
      params.set('filter', currentBookFilter);
    }
    const prevFmt = getFormatFilter();
    if (prevFmt !== 'all') params.set('format', prevFmt);
    const url = `${apiBase}/api/v1/library/books?${params.toString()}`;
    const resp = await fetch(url, { credentials: 'include' });

    if (!resp.ok) {
      failLoad('library', 'Failed to load previous books.');
      return;
    }

    const data: PaginatedResponse<Book> = await resp.json();
    // Prepend the new items and update prev_cursor
    setBooks(data.items, library.booksNextCursor, undefined, 'prepend', data.prev_cursor);
    finishLoad('library');
  } catch {
    failLoad('library', 'Failed to load previous books.');
  }
}

// Track current series search query and offset
let currentSeriesSearch = '';
let currentSeriesSearchOffset = 0;
let seriesSearchHasMore = false;
// Track current authors search query and offset
let currentAuthorsSearch = '';
let currentAuthorsSearchOffset = 0;
let authorsSearchHasMore = false;

/**
 * Load series list
 * @param around - Optional ID to center the view around (for jump-to-item navigation)
 * @param searchOffset - Offset for search pagination (used internally)
 */
export async function loadSeries(
  cursor?: string,
  search?: string,
  around?: string,
  searchOffset?: number
): Promise<void> {
  const auth = getAuthState();
  if (!auth.isAuthed) {
    resetLoad('series');
    return;
  }

  // If search changed, reset the list and offset
  if (search !== undefined && search !== currentSeriesSearch) {
    currentSeriesSearch = search;
    currentSeriesSearchOffset = 0;
    cursor = undefined; // Reset pagination for new search
  }

  // Use provided offset or current tracked offset
  if (searchOffset !== undefined) {
    currentSeriesSearchOffset = searchOffset;
  }

  startLoad('series');
  try {
    const seriesFmt = getFormatFilter();
    const data = await api.listSeries({
      cursor: currentSeriesSearch?.trim() ? undefined : cursor, // No cursor when searching
      search: currentSeriesSearch || undefined,
      search_offset: currentSeriesSearch?.trim() ? currentSeriesSearchOffset : undefined,
      around,
      format: seriesFmt !== 'all' ? seriesFmt : undefined
    });
    // Track if there are more search results
    if (currentSeriesSearch?.trim()) {
      seriesSearchHasMore = !!data.next_cursor;
    }
    // Replace when starting fresh (no cursor AND offset is 0)
    const isReplace = !cursor && currentSeriesSearchOffset === 0;
    setSeries(
      data.items,
      data.next_cursor,
      data.total_count,
      isReplace ? 'replace' : 'append',
      data.prev_cursor
    );
    finishLoad('series');
  } catch {
    setSeries([], null, undefined, 'replace');
    failLoad('series', 'Failed to load series.');
  }
}

/**
 * Load more series (pagination forward)
 * Uses offset-based pagination when searching (MeiliSearch)
 */
export function loadMoreSeries() {
  const library = getLibraryState();
  if (library.loadState.series.status === 'loading') return;

  // When searching, use offset-based pagination
  if (currentSeriesSearch?.trim()) {
    if (seriesSearchHasMore) {
      const nextOffset = library.seriesList.length;
      loadSeries(undefined, currentSeriesSearch, undefined, nextOffset);
    }
  } else if (library.seriesNextCursor) {
    loadSeries(library.seriesNextCursor, currentSeriesSearch);
  }
}

/**
 * Load previous series (pagination backward)
 */
export async function loadPreviousSeries(): Promise<void> {
  const library = getLibraryState();
  if (!library.seriesPrevCursor || library.loadState.series.status === 'loading') {
    return;
  }

  startLoad('series');
  try {
    const prevSeriesFmt = getFormatFilter();
    const data = await api.listSeries({
      cursor: library.seriesPrevCursor,
      direction: 'backward',
      search: currentSeriesSearch || undefined,
      format: prevSeriesFmt !== 'all' ? prevSeriesFmt : undefined
    });
    // Prepend the new items and update prev_cursor
    setSeries(data.items, library.seriesNextCursor, undefined, 'prepend', data.prev_cursor);
    finishLoad('series');
  } catch {
    failLoad('series', 'Failed to load previous series.');
  }
}

/**
 * Load series books
 */
export async function loadSeriesBooks(seriesId: string): Promise<void> {
  const auth = getAuthState();
  if (!auth.isAuthed || !seriesId) {
    setSeriesItems([]);
    resetLoad('seriesItems');
    return;
  }

  startLoad('seriesItems');
  try {
    const books = await api.getSeriesBooks(seriesId);
    setSeriesItems(books);
    finishLoad('seriesItems');
  } catch {
    setSeriesItems([]);
    failLoad('seriesItems', 'Failed to load series books.');
  }
}

/**
 * Load authors list
 * @param around - Optional ID to center the view around (for jump-to-item navigation)
 * @param search - Optional search query to filter authors
 * @param searchOffset - Offset for search pagination (used internally)
 */
export async function loadAuthors(
  cursor?: string,
  around?: string,
  search?: string,
  searchOffset?: number
): Promise<void> {
  const auth = getAuthState();
  if (!auth.isAuthed) {
    resetLoad('authors');
    return;
  }

  // If search changed, reset the offset
  if (search !== undefined && search !== currentAuthorsSearch) {
    currentAuthorsSearch = search;
    currentAuthorsSearchOffset = 0;
    cursor = undefined;
  }

  // Use provided offset or current tracked offset
  if (searchOffset !== undefined) {
    currentAuthorsSearchOffset = searchOffset;
  }

  startLoad('authors');
  try {
    const authorsFmt = getFormatFilter();
    const data = await api.getAuthors({
      cursor: currentAuthorsSearch?.trim() ? undefined : cursor,
      around,
      search: currentAuthorsSearch || undefined,
      search_offset: currentAuthorsSearch?.trim() ? currentAuthorsSearchOffset : undefined,
      format: authorsFmt !== 'all' ? authorsFmt : undefined
    });
    // Track if there are more search results
    if (currentAuthorsSearch?.trim()) {
      authorsSearchHasMore = !!data.next_cursor;
    }
    // Replace when starting fresh (no cursor AND offset is 0)
    const isReplace = !cursor && currentAuthorsSearchOffset === 0;
    setAuthors(
      data.items,
      data.next_cursor,
      data.total_count,
      isReplace ? 'replace' : 'append',
      data.prev_cursor
    );
    finishLoad('authors');
  } catch {
    setAuthors([], null, undefined, 'replace');
    failLoad('authors', 'Failed to load authors.');
  }
}

/**
 * Load more authors (pagination forward)
 * Uses offset-based pagination when searching (MeiliSearch)
 */
export function loadMoreAuthors() {
  const library = getLibraryState();
  if (library.loadState.authors.status === 'loading') return;

  // When searching, use offset-based pagination
  if (currentAuthorsSearch?.trim()) {
    if (authorsSearchHasMore) {
      const nextOffset = library.authors.length;
      loadAuthors(undefined, undefined, currentAuthorsSearch, nextOffset);
    }
  } else if (library.authorsNextCursor) {
    loadAuthors(library.authorsNextCursor);
  }
}

/**
 * Load previous authors (pagination backward)
 */
export async function loadPreviousAuthors(): Promise<void> {
  const library = getLibraryState();
  if (!library.authorsPrevCursor || library.loadState.authors.status === 'loading') {
    return;
  }

  startLoad('authors');
  try {
    const prevAuthorsFmt = getFormatFilter();
    const data = await api.getAuthors({
      cursor: library.authorsPrevCursor,
      direction: 'backward',
      format: prevAuthorsFmt !== 'all' ? prevAuthorsFmt : undefined
    });
    // Prepend the new items and update prev_cursor
    setAuthors(data.items, library.authorsNextCursor, undefined, 'prepend', data.prev_cursor);
    finishLoad('authors');
  } catch {
    failLoad('authors', 'Failed to load previous authors.');
  }
}

/**
 * Load author books
 */
export async function loadAuthorBooks(authorId: string): Promise<void> {
  const auth = getAuthState();
  if (!auth.isAuthed || !authorId) {
    setAuthorItems([]);
    resetLoad('authorItems');
    return;
  }

  startLoad('authorItems');
  try {
    const resp = await fetch(`${apiBase}/api/v1/library/authors/${authorId}/books`, {
      credentials: 'include'
    });
    if (!resp.ok) {
      setAuthorItems([]);
      failLoad('authorItems', 'Failed to load author books.');
      return;
    }
    setAuthorItems(await resp.json());
    finishLoad('authorItems');
  } catch {
    setAuthorItems([]);
    failLoad('authorItems', 'Failed to load author books.');
  }
}

/**
 * Load shelves list
 */
export async function loadShelves(cursor?: string): Promise<void> {
  const auth = getAuthState();
  if (!auth.isAuthed) {
    resetLoad('shelves');
    return;
  }

  startLoad('shelves');
  try {
    const data = await api.getShelves(cursor);
    setShelves(data.items, data.next_cursor, data.total_count, cursor ? 'append' : 'replace');
    finishLoad('shelves');
  } catch {
    setShelves([], null, undefined, 'replace');
    failLoad('shelves', 'Failed to load shelves.');
  }
}

/**
 * Load more shelves (pagination)
 */
export function loadMoreShelves() {
  const library = getLibraryState();
  if (library.shelvesNextCursor && library.loadState.shelves.status !== 'loading') {
    loadShelves(library.shelvesNextCursor);
  }
}

/**
 * Load shelf items
 */
export async function loadShelfItems(shelfId: string): Promise<void> {
  const auth = getAuthState();
  if (!auth.isAuthed || !shelfId) {
    setShelfItems([]);
    resetLoad('shelfItems');
    return;
  }

  startLoad('shelfItems');
  try {
    const items = await api.getShelfItems(shelfId);
    setShelfItems(items);
    finishLoad('shelfItems');
  } catch {
    setShelfItems([]);
    failLoad('shelfItems', 'Failed to load shelf contents.');
  }
}

/**
 * Load devices (admin)
 */
export async function loadDevices(): Promise<void> {
  const auth = getAuthState();
  if (!auth.isAuthed) {
    resetLoad('devices');
    return;
  }

  startLoad('devices');
  try {
    const devices = await api.getAdminDevices();
    setDevices(devices);
    finishLoad('devices');
  } catch {
    setDevices([]);
    failLoad('devices', 'Failed to load devices.');
  }
}

/**
 * Load users
 */
export async function loadUsers(): Promise<void> {
  const auth = getAuthState();
  if (!auth.isAuthed) {
    resetLoad('users');
    return;
  }

  startLoad('users');
  try {
    const users = await api.getUsers();
    setUsers(users);
    finishLoad('users');
  } catch {
    setUsers([]);
    failLoad('users', 'Failed to load users.');
  }
}

/**
 * Load activity events
 */
export async function loadActivity(cursor?: string): Promise<void> {
  const auth = getAuthState();
  if (!auth.isAuthed) {
    resetLoad('activity');
    return;
  }

  startLoad('activity');
  try {
    const data = await api.getActivity(cursor);
    setActivityEvents(
      data.items,
      data.next_cursor,
      data.total_count,
      cursor ? 'append' : 'replace'
    );
    finishLoad('activity');
  } catch {
    setActivityEvents([], null, undefined, 'replace');
    failLoad('activity', 'Failed to load activity.');
  }
}

/**
 * Load more activity (pagination)
 */
export function loadMoreActivity() {
  const devices = getDevicesState();
  const library = getLibraryState();
  if (devices.activityNextCursor && library.loadState.activity.status !== 'loading') {
    loadActivity(devices.activityNextCursor);
  }
}

/**
 * Load ingest tokens
 */
export async function loadIngestTokens(): Promise<void> {
  const auth = getAuthState();
  if (!auth.isAuthed) {
    resetLoad('ingestTokens');
    return;
  }

  startLoad('ingestTokens');
  try {
    const tokens = await api.getIngestTokens();
    setIngestTokens(tokens);
    finishLoad('ingestTokens');
  } catch {
    setIngestTokens([]);
    failLoad('ingestTokens', 'Failed to load ingest tokens.');
  }
}

/**
 * Load proxy logs
 */
export async function loadProxyLogs(query: ProxyLogsQuery): Promise<void> {
  const offset = query.offset ?? 0;
  startLoad('proxyLogs');
  try {
    const resp = await api.getProxyLogs(query);
    setProxyLogs(resp.logs, resp.total, offset);
    finishLoad('proxyLogs');
  } catch (e) {
    failLoad('proxyLogs', e instanceof Error ? e.message : 'Unknown error');
  }
}

/**
 * Sync books to devices
 */
export async function syncBooks(bookIds: string[]): Promise<boolean> {
  if (bookIds.length === 0) {
    showMessage('Select at least one book to sync.');
    return false;
  }

  try {
    await api.syncBooks(bookIds);
    showMessage('Sync queued');
    await loadLibrary();
    return true;
  } catch {
    showMessage('Sync failed');
    return false;
  }
}

/**
 * Touch books (force re-sync)
 */
export async function touchBooks(bookIds: string[]): Promise<boolean> {
  if (bookIds.length === 0) {
    return false;
  }

  try {
    await api.touchBooks(bookIds);
    showMessage('Books will be re-synced');
    await loadLibrary();
    return true;
  } catch {
    showMessage('Touch failed');
    return false;
  }
}

/**
 * Lazy-load helpers: only fetch if data has never been loaded (status === 'idle').
 * Subsequent navigations reuse the cached global store data.
 */

export function ensureAuthors(): void {
  const library = getLibraryState();
  if (library.loadState.authors.status === 'idle') {
    loadAuthors();
  }
}

export function ensureShelves(): void {
  const library = getLibraryState();
  if (library.loadState.shelves.status === 'idle') {
    loadShelves();
  }
}

export function ensureUsers(): void {
  const library = getLibraryState();
  if (library.loadState.users.status === 'idle') {
    loadUsers();
  }
}

export function ensureIngestTokens(): void {
  const library = getLibraryState();
  if (library.loadState.ingestTokens.status === 'idle') {
    loadIngestTokens();
  }
}

export function ensureDevices(): void {
  const library = getLibraryState();
  if (library.loadState.devices.status === 'idle') {
    loadDevices();
  }
}
