/**
 * Downloads store for tracking kobload downloads via SSE events.
 */

import type { KobloadDownloadEvent } from '#lib/types.js';
import type { EnrichedDownload } from '#lib/api/client.js';
import * as api from '#lib/api/client.js';

// State
let downloads = $state.raw<EnrichedDownload[]>([]);
let loading = $state(false);
let error = $state<string | null>(null);
let nextCursor = $state<string | null>(null);
// Track if we've loaded enriched data from API (to avoid SSE overwriting it)
let hasEnrichedData = $state(false);

/**
 * Get the downloads state.
 */
export function getDownloadsState() {
  return {
    get downloads() {
      return downloads;
    },
    get loading() {
      return loading;
    },
    get error() {
      return error;
    },
    get nextCursor() {
      return nextCursor;
    }
  };
}

/**
 * Fetch downloads from the API (initial load).
 * This returns enriched data with media info.
 */
export async function fetchDownloads(params?: api.DownloadsQueryParams): Promise<void> {
  loading = true;
  error = null;
  try {
    const response = await api.getDownloads(params);
    downloads = response.items || [];
    nextCursor = response.next_cursor;
    hasEnrichedData = true;
  } catch (e) {
    error = e instanceof Error ? e.message : 'Failed to fetch downloads';
    console.error('Failed to fetch downloads:', e);
  } finally {
    loading = false;
  }
}

/**
 * Load more downloads (pagination).
 */
export async function loadMoreDownloads(): Promise<void> {
  if (!nextCursor || loading) return;

  loading = true;
  try {
    const response = await api.getDownloads({ cursor: nextCursor });
    downloads = [...downloads, ...response.items];
    nextCursor = response.next_cursor;
  } catch (e) {
    error = e instanceof Error ? e.message : 'Failed to load more downloads';
    console.error('Failed to load more downloads:', e);
  } finally {
    loading = false;
  }
}

// Debounce buffer — plain (non-reactive) to avoid triggering updates per event
let downloadEventBuffer: KobloadDownloadEvent[] = [];
let downloadFlushTimer: ReturnType<typeof setTimeout> | null = null;

function applyDownloadEvent(
  current: EnrichedDownload[],
  event: KobloadDownloadEvent
): EnrichedDownload[] {
  if (event.kind === 'kobload_download_snapshot') {
    if (!hasEnrichedData) {
      return event.downloads.map((d) => ({ ...d, media: null }));
    }
    const mediaLookup: Record<string, EnrichedDownload['media']> = {};
    for (const d of current) {
      mediaLookup[d.id] = d.media;
    }
    return event.downloads.map((d) => ({
      ...d,
      media: mediaLookup[d.id] ?? null
    }));
  } else if (event.kind === 'kobload_download_created') {
    const enriched: EnrichedDownload = { ...event.download, media: null };
    return [enriched, ...current];
  } else if (event.kind === 'kobload_download_updated') {
    const idx = current.findIndex((d) => d.id === event.download.id);
    if (idx !== -1) {
      const existingMedia = current[idx].media;
      const updated: EnrichedDownload = { ...event.download, media: existingMedia };
      return [...current.slice(0, idx), updated, ...current.slice(idx + 1)];
    }
    const enriched: EnrichedDownload = { ...event.download, media: null };
    return [enriched, ...current];
  }
  return current;
}

function flushDownloadEvents() {
  downloadFlushTimer = null;
  if (downloadEventBuffer.length === 0) return;

  const pending = downloadEventBuffer;
  downloadEventBuffer = [];

  let current = downloads;
  for (const event of pending) {
    current = applyDownloadEvent(current, event);
  }
  // Single reactive write for the entire batch
  downloads = current;
}

/**
 * Handle a kobload download event from the SSE stream.
 * Debounced (100ms trailing edge) to coalesce bursts into a single reactive write.
 */
export function handleKobloadDownloadEvent(event: KobloadDownloadEvent) {
  downloadEventBuffer.push(event);
  if (!downloadFlushTimer) {
    downloadFlushTimer = setTimeout(flushDownloadEvents, 100);
  }
}

/**
 * Get active downloads (downloading, queued, etc.)
 */
export function getActiveDownloads(): EnrichedDownload[] {
  return downloads.filter(
    (d) =>
      d.status === 'pending' ||
      d.status === 'downloading' ||
      d.status === 'queued' ||
      d.status === 'processing'
  );
}

/**
 * Get completed downloads (success or failure)
 */
export function getCompletedDownloads(): EnrichedDownload[] {
  return downloads.filter(
    (d) =>
      d.status === 'completed' ||
      d.status === 'imported' ||
      d.status === 'failed' ||
      d.status === 'error'
  );
}

/**
 * Clear the downloads store.
 */
export function clearDownloads() {
  downloads = [];
  nextCursor = null;
}

/**
 * Cancel a download
 */
export async function cancelDownload(downloadId: string): Promise<void> {
  await api.cancelDownload(downloadId);
  // Remove from local state or mark as cancelled
  const idx = downloads.findIndex((d) => d.id === downloadId);
  if (idx !== -1) {
    downloads = downloads.filter((d) => d.id !== downloadId);
  }
}

/**
 * Retry a download (triggers new search)
 */
export async function retryDownload(downloadId: string): Promise<{ book_id: string }> {
  const result = await api.retryDownload(downloadId);
  // Update local state to show searching
  const idx = downloads.findIndex((d) => d.id === downloadId);
  if (idx !== -1) {
    const updated = { ...downloads[idx], status: 'searching' };
    downloads = [...downloads.slice(0, idx), updated, ...downloads.slice(idx + 1)];
  }
  return { book_id: result.book_id };
}

/**
 * Retry the ingest callback for a completed download
 * (useful when the initial callback failed)
 */
export async function retryCallback(downloadId: string): Promise<void> {
  await api.retryCallback(downloadId);
  // Update local state to show processing
  const idx = downloads.findIndex((d) => d.id === downloadId);
  if (idx !== -1) {
    const updated = { ...downloads[idx], callback_error: null, status: 'processing' };
    downloads = [...downloads.slice(0, idx), updated, ...downloads.slice(idx + 1)];
  }
}
