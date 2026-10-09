// Library data store using Svelte 5 runes

import { SvelteSet } from 'svelte/reactivity';
import type {
  Book,
  SeriesSummary,
  AuthorSummary,
  ShelfSummary,
  LoadKey,
  LoadState,
  LoadStatus
} from '$lib/types';

// Books
let books = $state.raw<Book[]>([]);
let booksNextCursor = $state<string | null>(null);
let booksPrevCursor = $state<string | null>(null);
let booksTotal = $state<number>(0);

// Series
let seriesList = $state.raw<SeriesSummary[]>([]);
let seriesNextCursor = $state<string | null>(null);
let seriesPrevCursor = $state<string | null>(null);
let seriesTotal = $state<number>(0);

// Authors
let authors = $state.raw<AuthorSummary[]>([]);
let authorsNextCursor = $state<string | null>(null);
let authorsPrevCursor = $state<string | null>(null);
let authorsTotal = $state<number>(0);

// Shelves
let shelves = $state.raw<ShelfSummary[]>([]);
let shelvesNextCursor = $state<string | null>(null);
let shelvesTotal = $state<number>(0);
let shelfItems = $state.raw<Book[]>([]);

// Series items (for sidebar)
let seriesItems = $state.raw<Book[]>([]);

// Author items (for sidebar)
let authorItems = $state.raw<Book[]>([]);

// Selection (SvelteSet is reactive for mutations)
const selected = new SvelteSet<string>();

// Load states
let loadState = $state<Record<LoadKey, LoadState>>({
  library: { status: 'idle', error: '' },
  series: { status: 'idle', error: '' },
  seriesItems: { status: 'idle', error: '' },
  authors: { status: 'idle', error: '' },
  authorItems: { status: 'idle', error: '' },
  activity: { status: 'idle', error: '' },
  devices: { status: 'idle', error: '' },
  users: { status: 'idle', error: '' },
  ingestTokens: { status: 'idle', error: '' },
  shelves: { status: 'idle', error: '' },
  shelfItems: { status: 'idle', error: '' },
  proxyLogs: { status: 'idle', error: '' },
  ingestQueue: { status: 'idle', error: '' }
});

// Getters
export function getLibraryState() {
  return {
    get books() {
      return books;
    },
    get booksNextCursor() {
      return booksNextCursor;
    },
    get booksPrevCursor() {
      return booksPrevCursor;
    },
    get booksTotal() {
      return booksTotal;
    },
    get seriesList() {
      return seriesList;
    },
    get seriesNextCursor() {
      return seriesNextCursor;
    },
    get seriesPrevCursor() {
      return seriesPrevCursor;
    },
    get seriesTotal() {
      return seriesTotal;
    },
    get authors() {
      return authors;
    },
    get authorsNextCursor() {
      return authorsNextCursor;
    },
    get authorsPrevCursor() {
      return authorsPrevCursor;
    },
    get authorsTotal() {
      return authorsTotal;
    },
    get shelves() {
      return shelves;
    },
    get shelvesNextCursor() {
      return shelvesNextCursor;
    },
    get shelvesTotal() {
      return shelvesTotal;
    },
    get shelfItems() {
      return shelfItems;
    },
    get seriesItems() {
      return seriesItems;
    },
    get authorItems() {
      return authorItems;
    },
    get selected() {
      return selected;
    },
    get loadState() {
      return loadState;
    }
  };
}

// Books
export function setBooks(
  newBooks: Book[],
  nextCursor: string | null,
  total?: number,
  mode: 'replace' | 'append' | 'prepend' = 'replace',
  prevCursor?: string | null
) {
  if (mode === 'append') {
    // Deduplicate: only add books not already in the list
    const existingIds: Record<string, boolean> = {};
    for (const b of books) existingIds[b.id] = true;
    const uniqueNew = newBooks.filter((b) => !existingIds[b.id]);
    books = [...books, ...uniqueNew];
  } else if (mode === 'prepend') {
    // Deduplicate: only add books not already in the list
    const existingIds: Record<string, boolean> = {};
    for (const b of books) existingIds[b.id] = true;
    const uniqueNew = newBooks.filter((b) => !existingIds[b.id]);
    books = [...uniqueNew, ...books];
  } else {
    // Replace mode - reset prev cursor to avoid stale state
    books = newBooks;
    booksPrevCursor = prevCursor ?? null;
  }
  booksNextCursor = nextCursor;
  // Update prev_cursor on prepend (not replace - handled above, not append - forward pagination)
  if (mode === 'prepend' && prevCursor !== undefined) {
    booksPrevCursor = prevCursor;
  }
  // Only update total on first page (when total is provided)
  if (total !== undefined) {
    booksTotal = total;
  }
}

// Series
export function setSeries(
  newSeries: SeriesSummary[],
  nextCursor: string | null,
  total?: number,
  mode: 'replace' | 'append' | 'prepend' = 'replace',
  prevCursor?: string | null
) {
  if (mode === 'append') {
    const existingIds: Record<string, boolean> = {};
    for (const s of seriesList) existingIds[s.id] = true;
    const uniqueNew = newSeries.filter((s) => !existingIds[s.id]);
    seriesList = [...seriesList, ...uniqueNew];
  } else if (mode === 'prepend') {
    const existingIds: Record<string, boolean> = {};
    for (const s of seriesList) existingIds[s.id] = true;
    const uniqueNew = newSeries.filter((s) => !existingIds[s.id]);
    seriesList = [...uniqueNew, ...seriesList];
  } else {
    // Replace mode - reset prev cursor to avoid stale state
    seriesList = newSeries;
    seriesPrevCursor = prevCursor ?? null;
  }
  seriesNextCursor = nextCursor;
  if (mode === 'prepend' && prevCursor !== undefined) {
    seriesPrevCursor = prevCursor;
  }
  if (total !== undefined) {
    seriesTotal = total;
  }
}

// Authors
export function setAuthors(
  newAuthors: AuthorSummary[],
  nextCursor: string | null,
  total?: number,
  mode: 'replace' | 'append' | 'prepend' = 'replace',
  prevCursor?: string | null
) {
  if (mode === 'append') {
    const existingIds: Record<string, boolean> = {};
    for (const a of authors) existingIds[a.id] = true;
    const uniqueNew = newAuthors.filter((a) => !existingIds[a.id]);
    authors = [...authors, ...uniqueNew];
  } else if (mode === 'prepend') {
    const existingIds: Record<string, boolean> = {};
    for (const a of authors) existingIds[a.id] = true;
    const uniqueNew = newAuthors.filter((a) => !existingIds[a.id]);
    authors = [...uniqueNew, ...authors];
  } else {
    // Replace mode - reset prev cursor to avoid stale state
    authors = newAuthors;
    authorsPrevCursor = prevCursor ?? null;
  }
  authorsNextCursor = nextCursor;
  if (mode === 'prepend' && prevCursor !== undefined) {
    authorsPrevCursor = prevCursor;
  }
  if (total !== undefined) {
    authorsTotal = total;
  }
}

// Shelves
export function setShelves(
  newShelves: ShelfSummary[],
  nextCursor: string | null,
  total?: number,
  mode: 'replace' | 'append' | 'prepend' = 'replace'
) {
  if (mode === 'append') {
    const existingIds: Record<string, boolean> = {};
    for (const s of shelves) existingIds[s.id] = true;
    const uniqueNew = newShelves.filter((s) => !existingIds[s.id]);
    shelves = [...shelves, ...uniqueNew];
  } else if (mode === 'prepend') {
    const existingIds: Record<string, boolean> = {};
    for (const s of shelves) existingIds[s.id] = true;
    const uniqueNew = newShelves.filter((s) => !existingIds[s.id]);
    shelves = [...uniqueNew, ...shelves];
  } else {
    shelves = newShelves;
  }
  shelvesNextCursor = nextCursor;
  if (total !== undefined) {
    shelvesTotal = total;
  }
}

export function setShelfItems(items: Book[]) {
  shelfItems = items;
}

export function setSeriesItems(items: Book[]) {
  seriesItems = items;
}

export function setAuthorItems(items: Book[]) {
  authorItems = items;
}

// Update a single book in-place (for SSE-triggered refreshes without full reload)
export function patchBook(updated: Book) {
  const idx = books.findIndex((b) => b.id === updated.id);
  if (idx >= 0) {
    books = books.map((b, i) => (i === idx ? updated : b));
  }
}

// Update only the total count (for SSE stats refresh without full reload)
export function updateBooksTotal(total: number) {
  booksTotal = total;
}

// Ensure a book is in the books array (for navigation from sidebars)
export function ensureBookInStore(book: Book) {
  if (!books.some((b) => b.id === book.id)) {
    books = [book, ...books];
  }
}

// Ensure an author is in the authors array (for navigation from search)
export function ensureAuthorInStore(author: AuthorSummary) {
  if (!authors.some((a) => a.id === author.id)) {
    authors = [author, ...authors];
  }
}

// Ensure a series is in the series array (for navigation from search)
export function ensureSeriesInStore(series: SeriesSummary) {
  if (!seriesList.some((s) => s.id === series.id)) {
    seriesList = [series, ...seriesList];
  }
}

// Selection (mutate instead of reassign for reactivity)
export function toggleSelected(bookId: string, multi = false) {
  if (!multi) {
    selected.clear();
    selected.add(bookId);
    return;
  }
  if (selected.has(bookId)) {
    selected.delete(bookId);
  } else {
    selected.add(bookId);
  }
}

export function clearSelection() {
  selected.clear();
}

export function setSelection(bookIds: Set<string>) {
  selected.clear();
  for (const id of bookIds) {
    selected.add(id);
  }
}

// Load state helpers
export function startLoad(key: LoadKey) {
  loadState = { ...loadState, [key]: { status: 'loading' as LoadStatus, error: '' } };
}

export function finishLoad(key: LoadKey) {
  loadState = { ...loadState, [key]: { status: 'success' as LoadStatus, error: '' } };
}

export function failLoad(key: LoadKey, error: string) {
  loadState = { ...loadState, [key]: { status: 'error' as LoadStatus, error } };
}

export function resetLoad(key: LoadKey) {
  loadState = { ...loadState, [key]: { status: 'idle' as LoadStatus, error: '' } };
}

// Clear all data
export function clearLibrary() {
  books = [];
  booksNextCursor = null;
  booksPrevCursor = null;
  booksTotal = 0;
  seriesList = [];
  seriesNextCursor = null;
  seriesPrevCursor = null;
  seriesTotal = 0;
  seriesItems = [];
  authors = [];
  authorsNextCursor = null;
  authorsPrevCursor = null;
  authorsTotal = 0;
  authorItems = [];
  shelves = [];
  shelvesNextCursor = null;
  shelvesTotal = 0;
  shelfItems = [];
  selected.clear();
}
