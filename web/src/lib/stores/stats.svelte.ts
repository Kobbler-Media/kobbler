// Library stats store using Svelte 5 runes
// Provides counts for nav sidebar

import type { LibraryStats } from '$lib/types';
import { getLibraryStats } from '$lib/api/client';
import { getFormatFilter } from '$lib/stores/format-filter.svelte';

const defaultStats: LibraryStats = {
  books_count: 0,
  series_count: 0,
  authors_count: 0,
  shelves_count: 0,
  downloads_active: 0,
  ingest_pending: 0,
  movies_count: 0,
  tv_shows_count: 0,
  people_count: 0,
  sports_count: 0,
  audiobooks_count: 0,
  kobload_configured: false
};

let stats = $state<LibraryStats>(defaultStats);
let loading = $state(false);
let lastFetch = $state(0);
let lastFormat: string | undefined;
let inflightPromise: Promise<void> | null = null;

// Cache duration: 30 seconds
const CACHE_MS = 30_000;

// Debounce timer for SSE-triggered refreshes
let debounceTimer: ReturnType<typeof setTimeout> | null = null;
const DEBOUNCE_MS = 500;

export function getStatsState() {
  return {
    get stats() {
      return stats;
    },
    get loading() {
      return loading;
    }
  };
}

async function doFetch(): Promise<void> {
  loading = true;
  try {
    const fmt = getFormatFilter();
    const formatParam = fmt !== 'all' ? fmt : undefined;
    stats = await getLibraryStats(formatParam);
    lastFetch = Date.now();
    lastFormat = formatParam;
  } catch (e) {
    console.error('Failed to fetch stats:', e);
  } finally {
    loading = false;
  }
}

/**
 * Fetch stats, with trailing debounce when force=true.
 * Rapid-fire SSE events coalesce into a single API call.
 */
export async function fetchStats(force = false): Promise<void> {
  const now = Date.now();
  // Invalidate cache if format changed
  const fmt = getFormatFilter();
  const formatParam = fmt !== 'all' ? fmt : undefined;
  const formatChanged = formatParam !== lastFormat;

  if (!force && !formatChanged && now - lastFetch < CACHE_MS) {
    return;
  }

  // Deduplicate concurrent non-forced fetches (e.g., Sidebar + MobileMenu both mounting)
  if (!force && !formatChanged && inflightPromise) {
    return inflightPromise;
  }

  if (force && !formatChanged) {
    // Debounce forced refreshes — multiple SSE events within DEBOUNCE_MS
    // coalesce into one fetch after the burst settles
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => {
      debounceTimer = null;
      doFetch();
    }, DEBOUNCE_MS);
    return;
  }

  inflightPromise = doFetch().finally(() => {
    inflightPromise = null;
  });
  await inflightPromise;
}

// Refetch immediately when the global format filter changes
if (typeof window !== 'undefined') {
  window.addEventListener('formatfilterchange', () => {
    fetchStats(false);
  });
}

export function updateStats(newStats: Partial<LibraryStats>): void {
  stats = { ...stats, ...newStats };
}

export function clearStats(): void {
  stats = defaultStats;
  lastFetch = 0;
  lastFormat = undefined;
  if (debounceTimer) {
    clearTimeout(debounceTimer);
    debounceTimer = null;
  }
}
