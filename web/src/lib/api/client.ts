// Centralized API client with httpOnly cookie authentication
// All requests use credentials: 'include' to send session cookies

import type {
  AudiobookChapter,
  Book,
  BulkEpisodeAction,
  BulkEpisodeResult,
  CalendarItem,
  ChatFactKind,
  ChatForgetResult,
  ChatIdentitiesResponse,
  ChatNotificationPrefs,
  ChatNotificationPrefsUpdate,
  ChatMemoryResponse,
  ChatMemoryUser,
  ChatProfileFact,
  UserSummary,
  DeviceSummary,
  DeviceActivity,
  ActivityEvent,
  SeriesSummary,
  ShelfSummary,
  AuthorSummary,
  GenreSummary,
  IngestTokenSummary,
  PlaybackReport,
  SessionSummary,
  PaginatedResponse,
  PersonDetail,
  PersonFilmographyCredit,
  ProviderLinkIssue,
  Job,
  WantedDetail,
  ListWantedResponse,
  AuthorSearchResult,
  SeriesSearchResult,
  KobloadDownloadListItem,
  VideoMetadataProvider,
  SubtitleProvider,
  SubtitleSearchResult,
  TracksResponse,
  MovieSearchResult,
  TvSearchResult,
  Movie,
  MovieFile,
  MovieWatchState,
  TvShow,
  TvSeason,
  TvEpisode,
  TvEpisodeFile,
  TvWatchState,
  VideoSyncProvider,
  TraktDeviceCode,
  TraktPollResult,
  MediaRoot,
  MediaRootFile,
  PlaybackHandoffResponse,
  PlaybackSessionResponse,
  PlaybackSubtitleTrack,
  DashboardResponse,
  WatchNextItem,
  RecentItem as DashboardRecentItem,
  MigrationPreviewResponse,
  MigrationImportResponse,
  MigrationService,
  IptvProvider,
  IptvChannel,
  IptvSource,
  IptvEpgEntry,
  IptvGroup,
  IptvRecording,
  IptvSeriesRule,
  IptvUnmatchedSource,
  IptvProgrammeWatch,
  IptvWatchMatch
} from '#lib/types.js';
import type { ClientCapabilities } from '#lib/utils/media.js';
import { playbackClient } from '#lib/utils/playback-client.js';

const apiBase = import.meta.env.VITE_API_BASE_URL || '';

/**
 * Build a query string from a params object.
 * Undefined/null values are filtered out.
 * Numbers are converted to strings.
 * Returns empty string if no valid params, otherwise '?key=value&...'
 */
function buildQueryString(
  params: Record<string, string | number | boolean | undefined | null>
): string {
  const searchParams = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== null && value !== '') {
      searchParams.set(key, String(value));
    }
  }
  const qs = searchParams.toString();
  return qs ? `?${qs}` : '';
}

/**
 * Inflight request deduplication. Concurrent calls with the same cache key
 * share a single in-flight promise. The entry is cleared once the promise
 * settles, so subsequent calls start fresh.
 */
const inflight = new Map<string, Promise<unknown>>();

function dedup<T>(key: string, fn: () => Promise<T>): Promise<T> {
  const existing = inflight.get(key);
  if (existing) return existing as Promise<T>;
  const p = fn().finally(() => inflight.delete(key));
  inflight.set(key, p);
  return p;
}

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
    public reason?: string,
    public detail?: string
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

function optionalString(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() ? value.trim() : undefined;
}

type FetchOptions = Parameters<typeof fetch>[1];

async function request<T>(path: string, options: FetchOptions = {}): Promise<T> {
  const headers: Record<string, string> = {};
  if (options.body) {
    headers['Content-Type'] = 'application/json';
  }
  const response = await fetch(`${apiBase}${path}`, {
    ...options,
    credentials: 'include',
    headers: {
      ...headers,
      ...options.headers
    }
  });

  if (!response.ok) {
    let message = `Request failed: ${response.status}`;
    let reason: string | undefined;
    let detail: string | undefined;
    const text = await response.text().catch(() => '');
    try {
      const error = JSON.parse(text);
      message = optionalString(error?.message) ?? optionalString(error?.error) ?? message;
      reason = optionalString(error?.reason);
      detail = optionalString(error?.detail);
    } catch {
      message = optionalString(text) ?? optionalString(response.statusText) ?? message;
    }
    throw new ApiError(response.status, message, reason, detail);
  }

  // Handle empty responses (204 No Content, 201 Created with no body, etc.)
  const contentLength = response.headers.get('content-length');
  if (response.status === 204 || contentLength === '0') {
    return undefined as T;
  }

  const text = await response.text();
  if (!text) {
    return undefined as T;
  }

  return JSON.parse(text);
}

// Auth
export async function login(email: string, password: string): Promise<{ token: string }> {
  return request('/api/v1/admin/login', {
    method: 'POST',
    body: JSON.stringify({ email, password })
  });
}

// Library Stats
import type { LibraryStats, TranscriptBacklog } from '#lib/types.js';
import type { MediaScope, SearchSuggestion } from '#lib/types.js';

export async function getLibraryStats(format?: string): Promise<LibraryStats> {
  const qs = format && format !== 'all' ? `?format=${format}` : '';
  return request(`/api/v1/library/stats${qs}`);
}

// Library
export async function getBooks(options?: {
  cursor?: string;
  limit?: number;
  search?: string;
  in_library?: boolean;
  want_status?: string;
  around?: string;
  /** Offset for search pagination (0-indexed). When searching, use this instead of cursor. */
  search_offset?: number;
  /** Filter to books with no media root (orphaned) */
  orphaned?: boolean;
}): Promise<PaginatedResponse<Book>> {
  return request(`/api/v1/library/books${buildQueryString(options ?? {})}`);
}

export async function getBook(bookId: string): Promise<Book> {
  return request(`/api/v1/library/books/${bookId}`);
}

/** Move a book's files to a media root directory. */
export async function moveBookToMediaRoot(
  bookId: string,
  mediaRootId: string
): Promise<{ moved_files: number; skipped: number; errors: string[] }> {
  return request(`/api/v1/admin/books/${bookId}/move-to-media-root`, {
    method: 'POST',
    body: JSON.stringify({ media_root_id: mediaRootId })
  });
}

/** Author input with optional external ID for linking */
export interface AuthorInput {
  name: string;
  hardcover_id?: string;
}

export async function updateBook(
  bookId: string,
  data: {
    title?: string;
    authors?: AuthorInput[];
    series_id?: string;
    series_name?: string | null;
    series_hardcover_id?: string | null;
    series_index?: number | null;
    language?: string | null;
    publisher?: string | null;
    pubdate?: string | null;
    description?: string | null;
    genres?: string[];
    identifiers?: Record<string, string>;
    series_memberships?: { series_id: string; series_index: number | null; is_primary: boolean }[];
  }
): Promise<void> {
  return request(`/api/v1/library/books/${bookId}`, {
    method: 'PATCH',
    body: JSON.stringify(data)
  });
}

export async function setBookPrimarySeries(bookId: string, seriesId: string): Promise<void> {
  return request(`/api/v1/library/books/${bookId}/series-primary`, {
    method: 'PUT',
    body: JSON.stringify({ series_id: seriesId })
  });
}

export interface BookCreatePayload {
  title: string;
  authors?: AuthorInput[];
  description?: string | null;
  language?: string | null;
  publisher?: string | null;
  pubdate?: string | null;
  /** Series ID (preferred over series_name when selecting from dropdown) */
  series_id?: string;
  series_name?: string | null;
  series_hardcover_id?: string | null;
  series_index?: number | null;
  identifiers?: Record<string, string>;
  genres?: string[];
  page_count?: number | null;
  rating?: number | null;
  rating_count?: number | null;
  cover_url?: string | null;
  /** If true, creates as a wanted book (is_in_library=false, want_status='wanted') */
  want?: boolean;
}

/**
 * Create a new book (wanted or library book without files)
 */
export async function createBook(data: BookCreatePayload): Promise<{ id: string }> {
  return request('/api/v1/library/books', {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

/**
 * Upload a cover file for a book via multipart form
 */
export async function uploadBookCover(bookId: string, file: File): Promise<void> {
  const formData = new FormData();
  formData.append('cover', file);

  const response = await fetch(`${apiBase}/api/v1/library/books/${bookId}/cover`, {
    method: 'PUT',
    credentials: 'include',
    body: formData
  });

  if (!response.ok) {
    const text = await response.text().catch(() => 'Upload failed');
    throw new ApiError(response.status, text);
  }
}

/**
 * Copy the extracted (from ebook file) cover to the primary cover location.
 */
export async function useExtractedCover(bookId: string): Promise<void> {
  return request(`/api/v1/library/books/${bookId}/cover/use-extracted`, {
    method: 'POST'
  });
}

/**
 * Set a book's cover by having the backend download it from a URL.
 * Avoids CORS issues when the browser can't fetch external CDN images.
 */
export async function setCoverFromUrl(bookId: string, url: string): Promise<void> {
  return request(`/api/v1/library/books/${bookId}/cover-from-url`, {
    method: 'POST',
    body: JSON.stringify({ url })
  });
}

export async function syncBooks(bookIds: string[]): Promise<void> {
  return request('/api/v1/library/sync', {
    method: 'POST',
    body: JSON.stringify({ book_ids: bookIds })
  });
}

export async function touchBooks(bookIds: string[]): Promise<void> {
  return request('/api/v1/library/books/touch', {
    method: 'POST',
    body: JSON.stringify({ book_ids: bookIds })
  });
}

// Reading data (status, progress, rating)
import type { ReadingData, UpdateReadingRequest, LogReadingTimeRequest } from '#lib/types.js';

/**
 * Get reading data for a book (current user's status, progress, rating)
 */
export async function getReadingData(bookId: string): Promise<ReadingData> {
  return request(`/api/v1/admin/books/${bookId}/reading`);
}

/**
 * Update reading data for a book (status, progress, rating)
 */
export async function updateReadingData(bookId: string, data: UpdateReadingRequest): Promise<void> {
  return request(`/api/v1/admin/books/${bookId}/reading`, {
    method: 'PUT',
    body: JSON.stringify(data)
  });
}

/**
 * Log reading time for a book
 */
export async function logReadingTime(bookId: string, data: LogReadingTimeRequest): Promise<void> {
  return request(`/api/v1/admin/books/${bookId}/reading/log-time`, {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

/**
 * Find the first book starting with a given letter (for alphabet bar jump navigation).
 * Returns the book ID and title (for cursor creation) or null if no books found.
 */
export async function getFirstBookByLetter(
  letter: string,
  filter?: string,
  inLibrary?: boolean,
  format?: string
): Promise<{ id: string; title: string } | null> {
  try {
    return await request(
      `/api/v1/library/books/first-by-letter${buildQueryString({
        letter,
        filter,
        in_library: inLibrary,
        format
      })}`
    );
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) {
      return null;
    }
    throw e;
  }
}

/**
 * Get available letters that have at least one book (for alphabet bar).
 * Returns letters sorted A-Z then # (for non-alphabetic).
 */
export async function getAvailableLetters(inLibrary?: boolean, format?: string): Promise<string[]> {
  const resp: { letters: string[] } = await request(
    `/api/v1/library/books/available-letters${buildQueryString({ in_library: inLibrary, format })}`
  );
  return resp.letters;
}

// Series
export async function getSeries(
  options: GetSeriesOptions = {}
): Promise<PaginatedResponse<SeriesSummary>> {
  return request(
    `/api/v1/library/series${buildQueryString(options as Record<string, string | number | boolean | undefined | null>)}`
  );
}

export async function getSeriesById(seriesId: string): Promise<SeriesSummary> {
  return request(`/api/v1/library/series/${seriesId}`);
}

export async function getSeriesBooks(seriesId: string, includeNotOwned = false): Promise<Book[]> {
  return request(
    `/api/v1/library/series/${seriesId}/books${buildQueryString({ include_not_owned: includeNotOwned || undefined })}`
  );
}

export async function renameSeries(seriesId: string, name: string): Promise<void> {
  return request(`/api/v1/library/series/${seriesId}`, {
    method: 'PATCH',
    body: JSON.stringify({ name })
  });
}

export async function mergeSeries(sourceId: string, targetId: string): Promise<void> {
  return request('/api/v1/library/series/merge', {
    method: 'POST',
    body: JSON.stringify({ source_id: sourceId, target_id: targetId })
  });
}

export async function deleteSeries(seriesId: string): Promise<void> {
  return request(`/api/v1/library/series/${seriesId}`, {
    method: 'DELETE'
  });
}

export interface SeriesUpdatePayload {
  name?: string;
  identifiers?: Record<string, string>;
  image_url?: string | null;
  description?: string | null;
  total_book_count?: number | null;
  is_monitored?: boolean;
  auto_want_mode?: string;
}

export async function updateSeries(seriesId: string, payload: SeriesUpdatePayload): Promise<void> {
  return request(`/api/v1/library/series/${seriesId}`, {
    method: 'PUT',
    body: JSON.stringify(payload)
  });
}

export async function reorderSeriesBook(
  seriesId: string,
  bookId: string,
  seriesIndex: number
): Promise<void> {
  return request(`/api/v1/library/series/${seriesId}/reorder`, {
    method: 'POST',
    body: JSON.stringify({
      book_id: bookId,
      series_index: seriesIndex
    })
  });
}

// Authors
export interface GetAuthorsOptions {
  cursor?: string;
  limit?: number;
  search?: string;
  is_monitored?: boolean;
  include_no_books?: boolean;
  around?: string;
  direction?: 'forward' | 'backward';
  /** Offset for search pagination (0-indexed). When searching, use this instead of cursor. */
  search_offset?: number;
  /** Filter by media format: "ebook" or "audiobook" */
  format?: string;
}

export async function getAuthors(
  options: GetAuthorsOptions = {}
): Promise<PaginatedResponse<AuthorSummary>> {
  return request(
    `/api/v1/library/authors${buildQueryString(options as Record<string, string | number | boolean | undefined | null>)}`
  );
}

export async function getAuthor(authorId: string): Promise<AuthorSummary> {
  return request(`/api/v1/library/authors/${authorId}`);
}

export async function renameAuthor(authorId: string, name: string): Promise<void> {
  return request(`/api/v1/library/authors/${authorId}`, {
    method: 'PATCH',
    body: JSON.stringify({ name })
  });
}

export async function mergeAuthors(sourceId: string, targetId: string): Promise<void> {
  return request('/api/v1/library/authors/merge', {
    method: 'POST',
    body: JSON.stringify({ source_id: sourceId, target_id: targetId })
  });
}

export async function deleteAuthor(authorId: string): Promise<void> {
  return request(`/api/v1/library/authors/${authorId}`, {
    method: 'DELETE'
  });
}

export interface AuthorUpdatePayload {
  name?: string;
  identifiers?: Record<string, string>;
  image_url?: string | null;
  bio?: string | null;
  is_monitored?: boolean;
  auto_want_mode?: string;
}

export async function updateAuthor(authorId: string, payload: AuthorUpdatePayload): Promise<void> {
  return request(`/api/v1/library/authors/${authorId}`, {
    method: 'PUT',
    body: JSON.stringify(payload)
  });
}

export async function getAuthorBooks(authorId: string, includeNotOwned = false): Promise<Book[]> {
  return request(
    `/api/v1/library/authors/${authorId}/books${buildQueryString({ include_not_owned: includeNotOwned || undefined })}`
  );
}

export async function updateAuthorMonitoring(
  authorId: string,
  payload: { is_monitored?: boolean; auto_want_mode?: string }
): Promise<void> {
  return request(`/api/v1/library/authors/${authorId}/monitoring`, {
    method: 'PUT',
    body: JSON.stringify(payload)
  });
}

export async function refreshAuthorMetadata(authorId: string): Promise<{ message: string }> {
  return request(`/api/v1/library/authors/${authorId}/refresh`, {
    method: 'POST'
  });
}

export async function getGenres(): Promise<GenreSummary[]> {
  return request('/api/v1/library/genres');
}

export async function wantBook(bookId: string): Promise<void> {
  return request(`/api/v1/library/books/${bookId}/want`, {
    method: 'POST'
  });
}

export async function skipBook(bookId: string): Promise<void> {
  return request(`/api/v1/library/books/${bookId}/skip`, {
    method: 'POST'
  });
}

export interface AuthorMetadataSearchResult {
  provider: string;
  external_id: string;
  slug: string | null;
  name: string;
  image_url: string | null;
  bio: string | null;
  book_count: number | null;
  confidence: number;
}

export async function searchAuthorMetadata(
  name: string,
  provider?: string
): Promise<{ results: AuthorMetadataSearchResult[] }> {
  return request(`/api/v1/admin/metadata/search-authors${buildQueryString({ name, provider })}`);
}

export async function applyAuthorMetadata(
  authorId: string,
  payload: {
    provider: string;
    provider_id: string;
    provider_slug?: string | null;
    image_url?: string | null;
    bio?: string | null;
  }
): Promise<void> {
  return request(`/api/v1/library/authors/${authorId}/apply-metadata`, {
    method: 'POST',
    body: JSON.stringify(payload)
  });
}

export async function applySeriesMetadata(
  seriesId: string,
  payload: {
    provider: string;
    provider_id: string;
    provider_slug?: string | null;
    image_url?: string | null;
    description?: string | null;
    total_book_count?: number | null;
  }
): Promise<void> {
  return request(`/api/v1/library/series/${seriesId}/apply-metadata`, {
    method: 'POST',
    body: JSON.stringify(payload)
  });
}

/**
 * Update auto_want_mode for a series.
 * - 'inherit' (default) = use author's mode
 * - 'none' | 'new_only' | 'all' | 'fill_gaps' = explicit override
 */
export async function updateSeriesAutoWant(
  seriesId: string,
  autoWantMode: 'inherit' | 'none' | 'new_only' | 'all' | 'fill_gaps'
): Promise<void> {
  return request(`/api/v1/library/series/${seriesId}/monitoring`, {
    method: 'PUT',
    body: JSON.stringify({ auto_want_mode: autoWantMode })
  });
}

/**
 * Refresh series metadata from Hardcover (triggers series_enrich job).
 * Requires series to have a hardcover ID linked via apply-metadata.
 */
export async function refreshSeriesMetadata(seriesId: string): Promise<{ message: string }> {
  return request(`/api/v1/library/series/${seriesId}/refresh`, {
    method: 'POST'
  });
}

/** Hardcover links that need a person: deleted upstream while holding user data, or merge conflicts. */
export async function getProviderLinkIssues(): Promise<ProviderLinkIssue[]> {
  return request('/api/v1/library/provider-link-issues');
}

// Shelves
export async function getShelves(
  cursor?: string,
  limit?: number
): Promise<PaginatedResponse<ShelfSummary>> {
  return request(`/api/v1/library/shelves${buildQueryString({ cursor, limit })}`);
}

export async function createShelf(name: string): Promise<{ id: string }> {
  return request('/api/v1/library/shelves', {
    method: 'POST',
    body: JSON.stringify({ name })
  });
}

export async function renameShelf(shelfId: string, name: string): Promise<void> {
  return request(`/api/v1/library/shelves/${shelfId}/rename`, {
    method: 'POST',
    body: JSON.stringify({ name })
  });
}

export async function deleteShelf(shelfId: string): Promise<void> {
  return request(`/api/v1/library/shelves/${shelfId}`, {
    method: 'DELETE'
  });
}

export async function getShelfItems(shelfId: string): Promise<Book[]> {
  return request(`/api/v1/library/shelves/${shelfId}/items`);
}

export async function addToShelf(shelfId: string, bookIds: string[]): Promise<void> {
  return request(`/api/v1/library/shelves/${shelfId}/items`, {
    method: 'POST',
    body: JSON.stringify({ book_ids: bookIds })
  });
}

export async function removeFromShelf(shelfId: string, bookIds: string[]): Promise<void> {
  return request(`/api/v1/library/shelves/${shelfId}/items`, {
    method: 'DELETE',
    body: JSON.stringify({ book_ids: bookIds })
  });
}

// Activity
export async function getActivity(
  cursor?: string,
  limit?: number
): Promise<PaginatedResponse<ActivityEvent>> {
  return request(`/api/v1/library/activity${buildQueryString({ cursor, limit })}`);
}

// Devices
export async function getDevices(): Promise<DeviceSummary[]> {
  return request('/api/v1/library/devices');
}

export async function createDevice(
  userId: string,
  name?: string,
  deviceType?: string
): Promise<{
  device_id: string;
  device_token: string;
  sync_url: string;
}> {
  return request('/api/v1/admin/devices', {
    method: 'POST',
    body: JSON.stringify({
      user_id: userId,
      name: name || undefined,
      device_type: deviceType || undefined
    })
  });
}

export async function rotateDeviceToken(deviceId: string): Promise<{
  device_id: string;
  device_token: string;
  sync_url: string;
}> {
  return request(`/api/v1/admin/devices/${deviceId}/token`, {
    method: 'POST'
  });
}

export async function resyncDevice(deviceId: string): Promise<void> {
  return request(`/api/v1/admin/devices/${deviceId}/resync`, {
    method: 'POST'
  });
}

export async function updateDevice(
  deviceId: string,
  updates: { name?: string; kobo_api_url?: string; auto_sync_new_books?: boolean }
): Promise<void> {
  return request(`/api/v1/admin/devices/${deviceId}`, {
    method: 'PATCH',
    body: JSON.stringify(updates)
  });
}

export async function updateDeviceSyncProvider(
  deviceId: string,
  providerType: string,
  enabled: boolean
): Promise<void> {
  return request(`/api/v1/admin/devices/${deviceId}/sync-providers/${providerType}`, {
    method: 'PUT',
    body: JSON.stringify({ enabled })
  });
}

export async function deleteDevice(deviceId: string): Promise<void> {
  return request(`/api/v1/admin/devices/${deviceId}`, {
    method: 'DELETE'
  });
}

// Active device types
export interface ActiveDeviceTypes {
  kobo: boolean;
  kindle: boolean;
  web: boolean;
  generic: boolean;
}

export async function getActiveDeviceTypes(): Promise<ActiveDeviceTypes> {
  return request('/api/v1/admin/device-types');
}

// Device Activity (for sidebar display)
export async function getDeviceActivity(deviceId: string): Promise<DeviceActivity> {
  return request(`/api/v1/admin/devices/${deviceId}/activity`);
}

// Device Books (per-device entitlements)
export interface DeviceBookSummary {
  book_id: string;
  title: string;
  author_sort: string | null;
}

export async function getDeviceBooks(deviceId: string): Promise<DeviceBookSummary[]> {
  return request(`/api/v1/admin/devices/${deviceId}/books`);
}

export async function addDeviceBooks(
  deviceId: string,
  bookIds: string[]
): Promise<{ added: number }> {
  return request(`/api/v1/admin/devices/${deviceId}/books`, {
    method: 'POST',
    body: JSON.stringify({ book_ids: bookIds })
  });
}

// Book Devices (per-book entitlements - inverse of Device Books)
export interface BookDeviceStatus {
  device_id: string;
  device_name: string | null;
  device_type: 'kobo' | 'kindle' | 'generic';
  entitled: boolean;
  is_removed: boolean;
}

export interface BookDevicesResponse {
  devices: BookDeviceStatus[];
  all_users_entitled: boolean;
}

export async function getBookDevices(bookId: string): Promise<BookDevicesResponse> {
  return request(`/api/v1/admin/books/${bookId}/devices`);
}

export async function addBookDevice(bookId: string, deviceId: string): Promise<void> {
  return request(`/api/v1/admin/books/${bookId}/devices/${deviceId}`, {
    method: 'POST'
  });
}

export async function removeBookDevice(bookId: string, deviceId: string): Promise<void> {
  return request(`/api/v1/admin/books/${bookId}/devices/${deviceId}`, {
    method: 'DELETE'
  });
}

export async function addBookAllDevices(bookId: string): Promise<void> {
  return request(`/api/v1/admin/books/${bookId}/devices/all`, {
    method: 'POST'
  });
}

// Chat Identities
export async function getChatIdentities(): Promise<ChatIdentitiesResponse> {
  return request('/api/v1/admin/chat/identities');
}

export async function createChatIdentity(data: {
  transport: string;
  user_id: string;
  address?: string;
  external_id?: string;
  max_role?: string;
  enabled?: boolean;
}): Promise<string> {
  return request('/api/v1/admin/chat/identities', {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

export async function updateChatIdentity(
  identityId: string,
  data: {
    address?: string | null;
    external_id?: string | null;
    user_id?: string;
    max_role?: string;
    enabled?: boolean;
  }
): Promise<void> {
  return request(`/api/v1/admin/chat/identities/${identityId}`, {
    method: 'PATCH',
    body: JSON.stringify(data)
  });
}

export async function deleteChatIdentity(identityId: string): Promise<void> {
  return request(`/api/v1/admin/chat/identities/${identityId}`, {
    method: 'DELETE'
  });
}

export async function approveChatPairing(
  pairingId: string,
  data: { identity_id?: string; user_id?: string; max_role?: string }
): Promise<void> {
  return request(`/api/v1/admin/chat/pairings/${pairingId}/approve`, {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

export async function rejectChatPairing(pairingId: string): Promise<void> {
  return request(`/api/v1/admin/chat/pairings/${pairingId}`, {
    method: 'DELETE'
  });
}

// Chat memory — Tier 3 profile facts and session summaries. Every one of these
// is scoped to the caller server-side; there is no "read someone else's" call.
export async function getChatMemory(): Promise<ChatMemoryResponse> {
  return request('/api/v1/admin/chat/memory');
}

export async function listChatMemoryUsers(): Promise<ChatMemoryUser[]> {
  return request('/api/v1/admin/chat/memory/users');
}

export async function createChatFact(data: {
  kind: ChatFactKind;
  value: string;
}): Promise<ChatProfileFact> {
  return request('/api/v1/admin/chat/memory/facts', {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

export async function updateChatFact(
  factId: string,
  data: { kind: ChatFactKind; value: string }
): Promise<ChatProfileFact> {
  return request(`/api/v1/admin/chat/memory/facts/${factId}`, {
    method: 'PATCH',
    body: JSON.stringify(data)
  });
}

export async function deleteChatFact(factId: string): Promise<void> {
  return request(`/api/v1/admin/chat/memory/facts/${factId}`, {
    method: 'DELETE'
  });
}

export async function forgetChatMemory(): Promise<ChatForgetResult> {
  return request('/api/v1/admin/chat/memory', { method: 'DELETE' });
}

// Users
export async function getUsers(): Promise<UserSummary[]> {
  return request('/api/v1/admin/users');
}

export async function createUser(
  email: string,
  password: string,
  role: string = 'viewer',
  display_name?: string,
  is_kids: boolean = false
): Promise<{ user_id: string }> {
  return request('/api/v1/admin/users', {
    method: 'POST',
    body: JSON.stringify({ email, password, role, display_name, is_kids })
  });
}

export async function updateUser(
  userId: string,
  data: {
    email?: string;
    password?: string;
    role?: string;
    display_name?: string;
    timezone?: string;
    is_kids?: boolean;
  }
): Promise<{ user_id: string; email: string; role: string }> {
  return request(`/api/v1/admin/users/${userId}`, {
    method: 'PATCH',
    body: JSON.stringify(data)
  });
}

// Empty string clears it, putting the user back on the server's clock.
export async function updateMyTimezone(timezone: string): Promise<void> {
  return request('/api/v1/admin/me/timezone', {
    method: 'PUT',
    body: JSON.stringify({ timezone })
  });
}

export async function getMyNotificationPrefs(): Promise<ChatNotificationPrefs> {
  return request('/api/v1/admin/me/notification-prefs');
}

// One PUT per change, and the response is the whole new state: the server owns
// what a mute or a trigger flip implies, exactly as it does for the chat bot.
export async function updateMyNotificationPrefs(
  update: ChatNotificationPrefsUpdate
): Promise<ChatNotificationPrefs> {
  return request('/api/v1/admin/me/notification-prefs', {
    method: 'PUT',
    body: JSON.stringify(update)
  });
}

export async function deleteUser(userId: string): Promise<void> {
  return request(`/api/v1/admin/users/${userId}`, {
    method: 'DELETE'
  });
}

// Linked sessions (auth tokens)
export async function getSessions(all = false): Promise<SessionSummary[]> {
  return request(`/api/v1/admin/sessions${all ? '?all=true' : ''}`);
}

export async function getPlaybackReports(limit = 100): Promise<{ items: PlaybackReport[] }> {
  return request(`/api/v1/admin/diagnostics/playback?limit=${limit}`);
}

export async function refreshSession(
  deviceName?: string,
  clientKind?: string
): Promise<{ expires_at: string }> {
  return request('/api/v1/admin/sessions/refresh', {
    method: 'POST',
    body: JSON.stringify({ device_name: deviceName, client_kind: clientKind })
  });
}

export async function revokeSession(sessionId: string): Promise<void> {
  return request(`/api/v1/admin/sessions/${sessionId}`, { method: 'DELETE' });
}

export async function mintDeviceToken(
  userId: string,
  deviceName?: string
): Promise<{ user_id: string; token: string; expires_at: string }> {
  return request(`/api/v1/admin/users/${userId}/device-token`, {
    method: 'POST',
    body: JSON.stringify({ device_name: deviceName })
  });
}

// Ingest Tokens
export async function getIngestTokens(): Promise<IngestTokenSummary[]> {
  return request('/api/v1/admin/ingest-tokens');
}

export async function createIngestToken(name?: string): Promise<{ token: string }> {
  return request('/api/v1/admin/ingest-tokens', {
    method: 'POST',
    body: name ? JSON.stringify({ name }) : undefined
  });
}

export async function createPlaybackToken(
  ttlHours?: number
): Promise<{ token: string; expires_at: string }> {
  return request('/api/v1/admin/playback/tokens', {
    method: 'POST',
    body: ttlHours ? JSON.stringify({ ttl_hours: ttlHours }) : undefined
  });
}

export async function revokePlaybackToken(token: string): Promise<void> {
  return request(`/api/v1/admin/playback/tokens/${token}`, {
    method: 'DELETE'
  });
}

export async function revokeIngestToken(tokenId: string): Promise<void> {
  return request(`/api/v1/admin/ingest-tokens/${tokenId}/revoke`, {
    method: 'POST'
  });
}

export async function renameIngestToken(id: string, name: string): Promise<void> {
  return request(`/api/v1/admin/ingest-tokens/${id}`, {
    method: 'PATCH',
    body: JSON.stringify({ name })
  });
}

// Book management
export async function removeBookEntitlements(bookId: string): Promise<void> {
  return request(`/api/v1/admin/books/${bookId}/remove`, {
    method: 'POST'
  });
}

export async function restoreBookEntitlements(bookId: string): Promise<void> {
  return request(`/api/v1/admin/books/${bookId}/restore`, {
    method: 'POST'
  });
}

export async function deleteBook(bookId: string): Promise<void> {
  return request(`/api/v1/library/books/${bookId}`, {
    method: 'DELETE'
  });
}

// List admin devices (different from library devices endpoint)
export async function getAdminDevices(): Promise<DeviceSummary[]> {
  return request('/api/v1/admin/devices');
}

// Proxy Logs
export interface EntityRef {
  uuid: string;
  entity_type: string;
  name: string;
  source: string;
}

export interface ResolvedEntity {
  entity_type: string;
  name: string;
}

export interface ProxyLogEntry {
  id: string;
  device_id: string | null;
  request_method: string;
  request_path: string;
  request_query: string | null;
  request_headers: Record<string, string> | null;
  request_body: string | null;
  request_body_size: number;
  upstream_status: number | null;
  upstream_headers: Record<string, string> | null;
  upstream_body: string | null;
  upstream_body_size: number;
  response_status: number | null;
  response_headers: Record<string, string> | null;
  response_body: string | null;
  response_body_size: number;
  decision: string;
  upstream_url: string | null;
  upstream_duration_ms: number | null;
  total_duration_ms: number | null;
  severity: string;
  notes: string | null;
  created_at: string;
  has_request_blob: boolean;
  has_response_blob: boolean;
  entity_refs: EntityRef[];
  has_alert: boolean;
  alert_message: string | null;
}

export interface ProxyLogsResponse {
  logs: ProxyLogEntry[];
  total: number;
}

export interface ProxyLogsQuery {
  limit?: number;
  offset?: number;
  device_id?: string;
  severity?: string;
  path?: string;
  method?: string;
  has_alert?: boolean;
}

// Calendar
export async function getCalendar(from: string, to: string): Promise<CalendarItem[]> {
  const res: { items: CalendarItem[] } = await request(
    `/api/v1/admin/calendar${buildQueryString({ from, to })}`
  );
  return res.items;
}

// Dashboard
export async function getDashboard(): Promise<DashboardResponse> {
  return request('/api/v1/admin/dashboard');
}

export async function getProxyLogs(query: ProxyLogsQuery = {}): Promise<ProxyLogsResponse> {
  return request(
    `/api/v1/admin/proxy-logs${buildQueryString({
      limit: query.limit,
      offset: query.offset,
      device_id: query.device_id,
      severity: query.severity,
      path: query.path,
      method: query.method,
      has_alert: query.has_alert
    })}`
  );
}

export async function resolveUuids(
  ids: string[]
): Promise<{ entities: Record<string, ResolvedEntity | null> }> {
  return request(`/api/v1/admin/uuid-resolve${buildQueryString({ ids: ids.join(',') })}`);
}

export async function clearProxyLogs(): Promise<{ deleted: number }> {
  return request('/api/v1/admin/proxy-logs', { method: 'DELETE' });
}

export async function clearMetadataCache(): Promise<{ deleted: number }> {
  return request('/api/v1/admin/metadata-cache', { method: 'DELETE' });
}

export async function triggerLibraryCleanup(): Promise<{ job_id: string }> {
  return request('/api/v1/admin/library-cleanup', { method: 'POST' });
}

export async function triggerMetadataWriteAll(): Promise<{ message: string; count: number }> {
  return request('/api/v1/admin/metadata-write-all', { method: 'POST' });
}

// Get URL for downloading proxy logs as a ZIP
export function getProxyLogsExportUrl(includeBlobs: boolean = true): string {
  return `${apiBase}/api/v1/admin/proxy-logs/export?include_blobs=${includeBlobs}`;
}

// System Config
export interface SystemConfig {
  public_url: string | null;
  public_proxy_url: string;
  kobo_storeapi_base_url: string;
  kobo_mode: 'merge' | 'passthrough' | 'local';
  proxy_logging_enabled: boolean;
  proxy_log_retention_days: number;
  blob_capture_enabled: boolean;
  rewrite_kobo_urls: boolean;
  kobo_sync_batch_size: number;
  default_entitlement_mode: string;
  activity_limit: number;
  default_page_size: number;
  max_page_size: number;
  watcher_enabled: boolean;
  watcher_scan_interval_seconds: number;
  media_root_scan_interval_seconds: number;
  metadata_providers_enabled: boolean;
  book_title_suggestions_enabled: boolean;
  metadata_cache_days: number;
  wanted_default_provider: string | null;
  wanted_min_score: number | null;
  kobload_base_url: string | null;
  kobload_api_key: string | null;
  kobload_ingest_url: string | null;
  kobload_communication_mode: string | null;
  meili_url: string | null;
  meili_master_key: string | null;
  semantic_search_enabled: boolean;
  semantic_search_ratio: number;
  koblem_service_url: string | null;
  koblem_api_key: string | null;
  /** Server-side opt-in (KOBLEM_UI_ENABLED env var), not a stored setting — read-only here. */
  koblem_ui_enabled: boolean;
  chat_enabled: boolean;
  chat_llm_provider: string | null;
  chat_llm_base_url: string | null;
  chat_llm_api_key: string | null;
  chat_llm_model: string | null;
  chat_llm_thinking: string | null;
  ebook_convert_path: string | null;
  ebook_meta_path: string | null;
  sonarr_url: string | null;
  sonarr_api_key: string | null;
  radarr_url: string | null;
  radarr_api_key: string | null;
  video_storage_root: string | null;
  transcode_cache_dir: string | null;
  cache_dir: string | null;
  transcript_mode: string;
  stt_service_url: string | null;
  job_concurrency: Record<string, number> | null;
  tts_enabled: boolean;
  tts_service_url: string | null;
  tts_service_type: string;
  tts_default_voice: string | null;
  tts_default_temperature: number | null;
  enrichment_enabled: boolean;
  hw_encoder: string | null;
  gpu_preempt_tts: boolean;
  /// When true, kobbler routes vision/TTS/STT through koblem's priority GPU lock.
  /// When false, kobbler's local GpuLock handles gating (standalone mode).
  route_gpu_via_koblem: boolean;
  skip_detect_mode: string;
  /// Image embedding server (EmbeddingGemma-2), or koblem; also serves text when
  /// `text_embedding_service_url` is unset.
  vision_service_url: string | null;
  text_embedding_service_url: string | null;
  iptv_language_filter: string[];
  iptv_auto_create_channels: boolean;
  iptv_health_check_enabled: boolean;
  debrid_cache_dir: string | null;
  debrid_cache_ttl_secs: number;
  debrid_cache_max_bytes: number;
  preferred_audio_language: string;
  preferred_subtitle_language: string;
  mkv_cue_trim_enabled: boolean;
  discover_region: string;
  discover_streaming_services: Array<{ id: number; name: string }>;
  cast_receiver_app_id: string | null;
  livetv_guide_source: string;
  livetv_buffer_policy: string;
  livetv_buffer_cap_hours: number;
  livetv_clash_policy: string;
  livetv_ad_posture: string;
  livetv_ad_disable_genres: string[];
  livetv_ad_train_from_corrections: boolean;
  livetv_recording_path: string | null;
  livetv_deinterlace_web: boolean;
  livetv_transcode_height: number;
}

export interface SystemConfigUpdate {
  public_url?: string;
  public_proxy_url?: string;
  kobo_storeapi_base_url?: string;
  kobo_mode?: string;
  proxy_logging_enabled?: boolean;
  proxy_log_retention_days?: number;
  blob_capture_enabled?: boolean;
  rewrite_kobo_urls?: boolean;
  kobo_sync_batch_size?: number;
  default_entitlement_mode?: string;
  activity_limit?: number;
  default_page_size?: number;
  max_page_size?: number;
  watcher_enabled?: boolean;
  watcher_scan_interval_seconds?: number;
  media_root_scan_interval_seconds?: number;
  metadata_providers_enabled?: boolean;
  book_title_suggestions_enabled?: boolean;
  metadata_cache_days?: number;
  wanted_default_provider?: string | null;
  wanted_min_score?: number | null;
  kobload_base_url?: string | null;
  kobload_api_key?: string | null;
  kobload_ingest_url?: string | null;
  kobload_communication_mode?: string | null;
  meili_url?: string | null;
  meili_master_key?: string | null;
  semantic_search_enabled?: boolean;
  semantic_search_ratio?: number;
  koblem_service_url?: string | null;
  koblem_api_key?: string | null;
  chat_enabled?: boolean;
  chat_llm_provider?: string | null;
  chat_llm_base_url?: string | null;
  chat_llm_api_key?: string | null;
  chat_llm_model?: string | null;
  chat_llm_thinking?: string | null;
  ebook_convert_path?: string | null;
  ebook_meta_path?: string | null;
  sonarr_url?: string | null;
  sonarr_api_key?: string | null;
  radarr_url?: string | null;
  radarr_api_key?: string | null;
  video_storage_root?: string | null;
  transcode_cache_dir?: string | null;
  cache_dir?: string | null;
  transcript_mode?: string;
  stt_service_url?: string | null;
  job_concurrency?: Record<string, number> | null;
  tts_enabled?: boolean;
  tts_service_url?: string | null;
  tts_service_type?: string;
  tts_default_voice?: string | null;
  tts_default_temperature?: number | null;
  enrichment_enabled?: boolean;
  hw_encoder?: string | null;
  gpu_preempt_tts?: boolean;
  route_gpu_via_koblem?: boolean;
  skip_detect_mode?: string;
  vision_service_url?: string | null;
  text_embedding_service_url?: string | null;
  iptv_language_filter?: string[];
  iptv_auto_create_channels?: boolean;
  iptv_health_check_enabled?: boolean;
  debrid_cache_dir?: string | null;
  debrid_cache_ttl_secs?: number;
  debrid_cache_max_bytes?: number;
  preferred_audio_language?: string;
  preferred_subtitle_language?: string;
  mkv_cue_trim_enabled?: boolean;
  discover_region?: string;
  discover_streaming_services?: Array<{ id: number; name: string }>;
  cast_receiver_app_id?: string | null;
  livetv_guide_source?: string;
  livetv_buffer_policy?: string;
  livetv_buffer_cap_hours?: number;
  livetv_clash_policy?: string;
  livetv_ad_posture?: string;
  livetv_ad_disable_genres?: string[];
  livetv_ad_train_from_corrections?: boolean;
  livetv_recording_path?: string | null;
  livetv_deinterlace_web?: boolean;
  livetv_transcode_height?: number;
}

export async function getSystemConfig(): Promise<SystemConfig> {
  return request('/api/v1/admin/config');
}

export async function updateSystemConfig(updates: SystemConfigUpdate): Promise<SystemConfig> {
  return request('/api/v1/admin/config', {
    method: 'PATCH',
    body: JSON.stringify(updates)
  });
}

// Job concurrency

export interface JobConcurrencyItem {
  job_type: string;
  name: string;
  description: string;
  concurrency: number;
  is_default: boolean;
  default: number;
  resource_notes: string;
  resource_group: string;
  pending: number;
  running: number;
}

export interface JobConcurrencyResponse {
  jobs: JobConcurrencyItem[];
  restart_required: boolean;
}

export async function getJobConcurrency(): Promise<JobConcurrencyResponse> {
  return request('/api/v1/admin/jobs/concurrency');
}

export async function updateJobConcurrency(job_type: string, concurrency: number): Promise<void> {
  await request('/api/v1/admin/jobs/concurrency', {
    method: 'PUT',
    body: JSON.stringify({ job_type, concurrency })
  });
}

// Chat

export interface ChatStatus {
  enabled: boolean;
  configured: boolean;
  provider: string | null;
  model: string | null;
}

export async function getChatStatus(): Promise<ChatStatus> {
  return request('/api/v1/admin/chat/status');
}

// Wanted + Kobload

export interface ListWantedQuery {
  search?: string;
  status?: string;
}

export async function listWanted(query: ListWantedQuery = {}): Promise<ListWantedResponse> {
  return request(
    `/api/v1/admin/wanted${buildQueryString({
      search: query.search?.trim(),
      status: query.status?.trim()
    })}`
  );
}

export async function createWanted(payload: {
  kind: string;
  provider: string;
  provider_id?: string | null;
  title: string;
  author?: string | null;
  series?: string | null;
  series_index?: number | null;
  isbn10?: string | null;
  isbn13?: string | null;
  cover_url?: string | null;
}): Promise<{ id: string }> {
  return request('/api/v1/admin/wanted', {
    method: 'POST',
    body: JSON.stringify(payload)
  });
}

export async function getWanted(id: string): Promise<WantedDetail> {
  return request(`/api/v1/admin/wanted/${id}`);
}

/**
 * Search releases for a book via Kobload.
 * Without query: uses metadata/identifiers (smart search).
 * With query: free text search override.
 */
export async function searchBookReleases(
  bookId: string,
  query?: string,
  scope?: string
): Promise<LibrarySearchResponse> {
  const params = new URLSearchParams();
  if (query && query.trim()) {
    params.set('q', query.trim());
  }
  if (scope) {
    params.set('scope', scope);
  }
  const suffix = params.toString();
  return request(`/api/v1/admin/books/${bookId}/releases${suffix ? `?${suffix}` : ''}`);
}

/**
 * Queue a specific release for download for a book.
 */
export async function queueBookRelease(
  bookId: string,
  release: Record<string, unknown>,
  source: string
): Promise<void> {
  await request(`/api/v1/admin/books/${bookId}/queue`, {
    method: 'POST',
    body: JSON.stringify({ source, release })
  });
}

export async function deleteWanted(id: string): Promise<void> {
  await request(`/api/v1/admin/wanted/${id}`, {
    method: 'DELETE'
  });
}

// Wanted status and actions
export async function getWantedStatus(
  id: string
): Promise<import('#lib/types.js').WantedStatusResponse> {
  return request(`/api/v1/admin/wanted/${id}/status`);
}

export async function searchWantedNow(id: string): Promise<void> {
  await request(`/api/v1/admin/wanted/${id}/search-now`, {
    method: 'POST'
  });
}

export async function grabHeldMatch(id: string): Promise<void> {
  await request(`/api/v1/admin/wanted/${id}/grab-held`, {
    method: 'POST'
  });
}

export async function rejectWantedDownload(id: string, reason?: string): Promise<void> {
  await request(`/api/v1/admin/wanted/${id}/reject-download`, {
    method: 'POST',
    body: JSON.stringify({ reason })
  });
}

// Get URL for downloading a proxy log blob
export function getProxyLogBlobUrl(logId: string, blobType: 'request' | 'response'): string {
  return `${apiBase}/api/v1/admin/proxy-logs/${logId}/blob?blob_type=${blobType}`;
}

// Metadata Providers
export type ConfigFieldType = 'string' | 'secret' | 'integer' | 'boolean' | 'url';

export type Capability =
  | 'isbn_lookup'
  | 'title_search'
  | 'author_search'
  | 'cover_fetch'
  | 'series_info'
  | 'subject_info'
  | 'descriptions'
  | 'page_count'
  | 'publisher_info'
  | 'publish_date'
  | 'language_info';

export interface ConfigField {
  key: string;
  label: string;
  field_type: ConfigFieldType;
  required: boolean;
  description: string | null;
  help_url: string | null;
  default: string | null;
  current_value: unknown | null;
}

export interface MetadataProvider {
  id: string;
  provider_type: string;
  enabled: boolean;
  priority: number;
  // From static ProviderInfo
  display_name: string;
  description: string;
  website: string;
  capabilities: Capability[];
  // Dynamic config
  config_fields: ConfigField[];
}

export interface MetadataProviderUpdate {
  enabled?: boolean;
  priority?: number;
  config?: Record<string, unknown>;
}

export async function getMetadataProviders(): Promise<MetadataProvider[]> {
  return request('/api/v1/admin/metadata/providers');
}

export async function updateMetadataProvider(
  providerId: string,
  updates: MetadataProviderUpdate
): Promise<void> {
  return request(`/api/v1/admin/metadata/providers/${providerId}`, {
    method: 'PATCH',
    body: JSON.stringify(updates)
  });
}

// Jobs
export interface JobsResponse {
  jobs: Job[];
  total: number;
}

export interface JobsQuery {
  limit?: number;
  offset?: number;
  status?: string;
  job_type?: string;
  exclude_routine?: boolean;
}

export async function getJobs(query: JobsQuery = {}): Promise<JobsResponse> {
  return request(
    `/api/v1/admin/jobs${buildQueryString({
      limit: query.limit,
      offset: query.offset,
      status: query.status,
      job_type: query.job_type,
      exclude_routine: query.exclude_routine
    })}`
  );
}

export async function getJob(jobId: string): Promise<Job> {
  return request(`/api/v1/admin/jobs/${jobId}`);
}

export async function getRunningJobsCount(excludeRoutine = true): Promise<{ count: number }> {
  return request(
    `/api/v1/admin/jobs/running${buildQueryString({ exclude_routine: excludeRoutine })}`
  );
}

export async function deleteJob(jobId: string): Promise<{ deleted: boolean; message: string }> {
  return request(`/api/v1/admin/jobs/${jobId}`, { method: 'DELETE' });
}

export async function cancelJob(jobId: string): Promise<{ cancelled: boolean; message: string }> {
  return request(`/api/v1/admin/jobs/${jobId}/cancel`, { method: 'POST' });
}

export async function deleteCompletedJobs(): Promise<{ deleted: number; message: string }> {
  return request(`/api/v1/admin/jobs/completed`, { method: 'DELETE' });
}

// Backup & Restore
export function getBackupUrl(includeProxyBlobs: boolean = false): string {
  const params = includeProxyBlobs ? '?include_proxy_blobs=true' : '';
  return `${apiBase}/api/v1/admin/backup${params}`;
}

export interface RestoreStats {
  tables_restored: number;
  books_restored: number;
  files_restored: number;
}

export interface RestoreResult {
  success: boolean;
  message: string;
  stats: RestoreStats;
}

// Ingest Queue
export async function getIngestQueueItem(itemId: string): Promise<Book> {
  return request(`/api/v1/admin/ingest-queue/${itemId}`);
}

export async function approveIngestItem(itemId: string, deviceIds?: string[]): Promise<void> {
  return request(`/api/v1/admin/ingest-queue/${itemId}/approve`, {
    method: 'POST',
    body: JSON.stringify({ device_ids: deviceIds })
  });
}

export async function rejectIngestItem(itemId: string): Promise<void> {
  return request(`/api/v1/admin/ingest-queue/${itemId}/reject`, {
    method: 'POST'
  });
}

export async function restoreIngestQueueItem(itemId: string): Promise<void> {
  return request(`/api/v1/admin/ingest-queue/${itemId}/restore`, {
    method: 'POST'
  });
}

export async function uploadIngestCover(itemId: string, file: File): Promise<void> {
  const formData = new FormData();
  formData.append('cover', file);
  const response = await fetch(`${apiBase}/api/v1/admin/ingest-queue/${itemId}/cover`, {
    method: 'PUT',
    credentials: 'include',
    body: formData
  });
  if (!response.ok) {
    throw new ApiError(response.status, 'Cover upload failed');
  }
}

export async function restoreBackup(file: File): Promise<RestoreResult> {
  const response = await fetch(`${apiBase}/api/v1/admin/restore`, {
    method: 'POST',
    credentials: 'include',
    headers: {
      'Content-Type': 'application/gzip'
    },
    body: file
  });

  if (!response.ok) {
    let message = `Restore failed: ${response.status}`;
    try {
      const text = await response.text();
      if (text.trim()) {
        message = text.trim();
      }
    } catch {
      // Use default message
    }
    throw new ApiError(response.status, message);
  }

  return response.json();
}

// Orphan Cleanup
export async function triggerOrphanCleanup(): Promise<{ job_id: string }> {
  return request('/api/v1/admin/orphan-cleanup', { method: 'POST' });
}

// Sync Provider Settings
export interface SyncProviderSettings {
  provider_type: string;
  enabled: boolean;
  has_api_key: boolean;
}

export interface UpdateSyncProviderSettings {
  enabled?: boolean;
  api_key?: string;
}

export async function getSyncProviders(): Promise<SyncProviderSettings[]> {
  return request('/api/v1/admin/me/sync-providers');
}

export async function updateSyncProvider(
  providerType: string,
  settings: UpdateSyncProviderSettings
): Promise<void> {
  return request(`/api/v1/admin/me/sync-providers/${providerType}`, {
    method: 'PUT',
    body: JSON.stringify(settings)
  });
}

// Book Linking
export interface BookProviderLink {
  provider: string;
  external_id: string;
}

export interface BookLinksResponse {
  book_id: string;
  links: BookProviderLink[];
}

export interface SyncBookResult {
  external_id: string;
  title: string;
  authors: string[];
  year: string | null;
  cover_url: string | null;
  confidence: number;
}

export interface SearchResultsResponse {
  results: SyncBookResult[];
}

export async function getBookLinks(bookId: string): Promise<BookLinksResponse> {
  return request(`/api/v1/admin/sync/books/${bookId}/links`);
}

export async function searchProviderForBook(
  bookId: string,
  provider: string,
  query: string
): Promise<SearchResultsResponse> {
  return request(`/api/v1/admin/sync/books/${bookId}/links/${provider}/search`, {
    method: 'POST',
    body: JSON.stringify({ query })
  });
}

export async function linkBookToProvider(
  bookId: string,
  provider: string,
  externalId: string
): Promise<void> {
  return request(`/api/v1/admin/sync/books/${bookId}/links/${provider}`, {
    method: 'POST',
    body: JSON.stringify({ external_id: externalId })
  });
}

export async function unlinkBookFromProvider(bookId: string, provider: string): Promise<void> {
  return request(`/api/v1/admin/sync/books/${bookId}/links/${provider}`, {
    method: 'DELETE'
  });
}

export async function rateBookOnProvider(
  bookId: string,
  provider: string,
  rating: number
): Promise<void> {
  return request(`/api/v1/admin/sync/books/${bookId}/rate/${provider}`, {
    method: 'POST',
    body: JSON.stringify({ rating })
  });
}

// Kobload quality profiles (proxied from Kobload)
export type KobloadQualityProfile = {
  id: string;
  name: string;
  media_kind: string;
  is_default: boolean;
  min_resolution: string | null;
  max_resolution: string | null;
  preferred_resolution: string | null;
};

export async function getKobloadQualityProfiles(): Promise<KobloadQualityProfile[]> {
  return request('/api/v1/admin/kobload/quality-profiles');
}

// Kobload connection testing
export type TestConnectionResult = {
  success: boolean;
  message: string;
};

export async function testKobloadConnection(config?: {
  kobload_base_url?: string;
  kobload_api_key?: string;
}): Promise<TestConnectionResult> {
  return request('/api/v1/admin/test-kobload', {
    method: 'POST',
    body: config ? JSON.stringify(config) : undefined
  });
}

// Dev Snapshot (Dev Mode Only)
export interface DevSnapshotStatus {
  exists: boolean;
  size: number | null;
  created_at: string | null;
  dev_mode: boolean;
}

export interface DevSnapshotSaveResult {
  success: boolean;
  message: string;
  books_count: number;
  files_count: number;
}

export async function getDevSnapshotStatus(): Promise<DevSnapshotStatus> {
  return request('/api/v1/admin/dev/snapshot');
}

export async function saveDevSnapshot(): Promise<DevSnapshotSaveResult> {
  return request('/api/v1/admin/dev/snapshot/save', {
    method: 'POST'
  });
}

export async function deleteDevSnapshot(): Promise<{ success: boolean; message: string }> {
  return request('/api/v1/admin/dev/snapshot', {
    method: 'DELETE'
  });
}

// Author Monitoring (uses library endpoints - authors with is_monitored=true)

export interface ListMonitoredAuthorsResponse {
  authors: AuthorSummary[];
}

export interface SearchAuthorsResponse {
  results: AuthorSearchResult[];
}

export interface CreateMonitoredAuthorPayload {
  provider: string;
  provider_id: string;
  name: string;
  image_url?: string | null;
  bio?: string | null;
  auto_want_mode?: string;
  is_monitored?: boolean;
}

export interface AuthorCreatePayload {
  name: string;
  identifiers?: Record<string, string>;
  image_url?: string | null;
  bio?: string | null;
  is_monitored?: boolean;
  auto_want_mode?: string;
}

export interface MonitoredAuthorWithBooks {
  author: AuthorSummary;
  books: Book[];
}

/**
 * List monitored authors (authors with is_monitored=true)
 */
export async function listMonitoredAuthors(): Promise<ListMonitoredAuthorsResponse> {
  const data = await getAuthors({ is_monitored: true, include_no_books: true });
  return { authors: data.items };
}

/**
 * Get a monitored author with their books (including non-library books)
 */
export async function getMonitoredAuthor(id: string): Promise<MonitoredAuthorWithBooks> {
  const [authorsData, books] = await Promise.all([
    getAuthors({ is_monitored: true, include_no_books: true }),
    getAuthorBooks(id, true) // include_not_owned=true
  ]);
  const author = authorsData.items.find((a) => a.id === id);
  if (!author) {
    throw new Error('Author not found');
  }
  return { author, books };
}

/**
 * Create a new author (low-level)
 */
export async function createAuthor(payload: AuthorCreatePayload): Promise<{ id: string }> {
  return request('/api/v1/library/authors', {
    method: 'POST',
    body: JSON.stringify(payload)
  });
}

/**
 * Create and monitor a new author from metadata provider
 */
export async function createMonitoredAuthor(
  payload: CreateMonitoredAuthorPayload
): Promise<{ id: string }> {
  // Create author with all fields in one request
  return request('/api/v1/library/authors', {
    method: 'POST',
    body: JSON.stringify({
      name: payload.name,
      identifiers: payload.provider_id ? { [payload.provider]: payload.provider_id } : undefined,
      image_url: payload.image_url,
      bio: payload.bio,
      is_monitored: payload.is_monitored ?? true,
      auto_want_mode: payload.auto_want_mode || 'none'
    })
  });
}

/**
 * Update monitoring settings for an author
 */
export async function updateMonitoredAuthor(
  id: string,
  payload: { auto_want_mode?: string; is_monitored?: boolean }
): Promise<void> {
  return updateAuthorMonitoring(id, payload);
}

/**
 * Stop monitoring an author (sets is_monitored=false)
 */
export async function deleteMonitoredAuthor(id: string): Promise<void> {
  return updateAuthorMonitoring(id, { is_monitored: false });
}

/**
 * Refresh metadata for a monitored author
 */
export async function refreshMonitoredAuthor(id: string): Promise<{
  message: string;
}> {
  return refreshAuthorMetadata(id);
}

/**
 * Search for authors via metadata provider
 */
export async function searchAuthors(query: string): Promise<SearchAuthorsResponse> {
  return request(`/api/v1/admin/metadata/search-authors${buildQueryString({ name: query })}`);
}

export interface SearchSeriesResponse {
  results: SeriesSearchResult[];
}

/**
 * Search for series via metadata provider
 */
export async function searchSeries(query: string): Promise<SearchSeriesResponse> {
  return request(`/api/v1/admin/metadata/search-series${buildQueryString({ name: query })}`);
}

// Metadata Merging

/** Author with optional provider ID for enrichment */
export interface AuthorWithId {
  name: string;
  provider_id?: string;
}

export interface MetadataResult {
  provider: string;
  title: string;
  /** Simple author names */
  authors: string[];
  /** Authors with provider IDs (e.g., Hardcover author IDs) */
  authors_with_ids?: AuthorWithId[];
  isbn: string | null;
  description: string | null;
  publisher: string | null;
  publish_date: string | null;
  language: string | null;
  page_count: number | null;
  cover_url: string | null;
  series: string | null;
  /** Hardcover series ID for enrichment */
  series_provider_id: string | null;
  series_index: number | null;
  subjects: string[];
  external_id: string | null;
  confidence: number;
  rating: number | null;
  rating_count: number | null;
  identifiers: Record<string, string> | null;
}

export interface MergeMetadataRequest {
  results: MetadataResult[];
}

export interface MergeMetadataResponse {
  merged: MetadataResult;
}

/**
 * Merge multiple metadata results by priority
 * First result = highest priority
 */
export async function mergeMetadata(results: MetadataResult[]): Promise<MergeMetadataResponse> {
  return request('/api/v1/admin/metadata/merge', {
    method: 'POST',
    body: JSON.stringify({ results })
  });
}

/**
 * Want a book (add to wanted list)
 */
export async function wantMonitoredBook(bookId: string): Promise<void> {
  return wantBook(bookId);
}

/**
 * Skip a book
 */
export async function skipMonitoredBook(bookId: string): Promise<void> {
  return skipBook(bookId);
}

/**
 * Want all pending books for an author (bulk operation)
 */
export async function wantAllPendingForAuthor(authorId: string): Promise<{ wanted_count: number }> {
  return request(`/api/v1/library/authors/${authorId}/want-pending`, {
    method: 'POST'
  });
}

/**
 * Drop every outstanding want on an author's books (bulk operation)
 */
export async function unwantAllForAuthor(authorId: string): Promise<{ unwanted_count: number }> {
  return request(`/api/v1/library/authors/${authorId}/want-pending`, {
    method: 'DELETE'
  });
}

/**
 * Want all pending books for a series (bulk operation)
 */
export async function wantAllPendingForSeries(seriesId: string): Promise<{ wanted_count: number }> {
  return request(`/api/v1/library/series/${seriesId}/want-pending`, {
    method: 'POST'
  });
}

/**
 * Drop every outstanding want on a series' books (bulk operation)
 */
export async function unwantAllForSeries(seriesId: string): Promise<{ unwanted_count: number }> {
  return request(`/api/v1/library/series/${seriesId}/want-pending`, {
    method: 'DELETE'
  });
}

// Series Monitoring (uses library endpoints - series with is_monitored=true)

export interface GetSeriesOptions {
  cursor?: string;
  limit?: number;
  search?: string;
  is_monitored?: boolean;
  include_no_books?: boolean;
  around?: string;
  direction?: 'forward' | 'backward';
  /** Offset for search pagination (0-indexed). When searching, use this instead of cursor. */
  search_offset?: number;
  /** Filter by media format: "ebook" or "audiobook" */
  format?: string;
}

export interface ListMonitoredSeriesResponse {
  series: SeriesSummary[];
}

/**
 * List series with optional filters
 */
export async function listSeries(
  options: GetSeriesOptions = {}
): Promise<PaginatedResponse<SeriesSummary>> {
  return request(
    `/api/v1/library/series${buildQueryString(options as Record<string, string | number | boolean | undefined | null>)}`
  );
}

/**
 * List monitored series (series with is_monitored=true)
 */
export async function listMonitoredSeries(): Promise<ListMonitoredSeriesResponse> {
  const data = await listSeries({ is_monitored: true, include_no_books: true });
  return { series: data.items };
}

/**
 * Update monitoring settings for a series
 */
export async function updateSeriesMonitoring(
  seriesId: string,
  payload: { is_monitored?: boolean; auto_want_mode?: string }
): Promise<void> {
  return request(`/api/v1/library/series/${seriesId}/monitoring`, {
    method: 'PUT',
    body: JSON.stringify(payload)
  });
}

export interface CreateMonitoredSeriesPayload {
  provider: string;
  provider_id: string;
  name: string;
  image_url?: string | null;
  description?: string | null;
  total_book_count?: number | null;
  auto_want_mode?: string;
  is_monitored?: boolean;
}

export interface SeriesCreatePayload {
  name: string;
  identifiers?: Record<string, string>;
  image_url?: string | null;
  description?: string | null;
  total_book_count?: number | null;
  is_monitored?: boolean;
  auto_want_mode?: string;
}

/**
 * Create a new series (low-level)
 */
export async function createSeries(payload: SeriesCreatePayload): Promise<{ id: string }> {
  return request('/api/v1/library/series', {
    method: 'POST',
    body: JSON.stringify(payload)
  });
}

/**
 * Create and monitor a new series from metadata provider
 */
export async function createMonitoredSeries(
  payload: CreateMonitoredSeriesPayload
): Promise<{ id: string }> {
  // Create series with all fields in one request
  return request('/api/v1/library/series', {
    method: 'POST',
    body: JSON.stringify({
      name: payload.name,
      identifiers: payload.provider_id ? { [payload.provider]: payload.provider_id } : undefined,
      image_url: payload.image_url,
      description: payload.description,
      total_book_count: payload.total_book_count,
      is_monitored: payload.is_monitored ?? true,
      auto_want_mode: payload.auto_want_mode || 'none'
    })
  });
}

// Unified Search (Command Palette) - Federated Search

export interface SearchBookDocument {
  id: string;
  title: string;
  authors: string[];
  series_name: string | null;
  series_index: number | null;
  cover_url: string | null;
  is_in_library: boolean;
  want_status: string;
  ingest_status: string | null;
  has_audiobook: boolean;
  has_ebook: boolean;
  narrator: string | null;
}

export interface SearchAuthorDocument {
  id: string;
  name: string;
  image_url: string | null;
  book_count: number;
  is_monitored: boolean;
}

export interface SearchSeriesDocument {
  id: string;
  name: string;
  book_count: number;
  is_monitored: boolean;
}

export interface SearchGenreDocument {
  id: string;
  name: string;
  book_count: number;
}

export interface SearchPageDocument {
  id: string;
  label: string;
  path: string;
  aliases: string[];
  icon: string;
}

export interface SearchCommandDocument {
  id: string;
  label: string;
  action: string;
  aliases: string[];
  icon: string;
}

export interface SearchMovieDocument {
  id: string;
  title: string;
  year: number | null;
  directors: string[];
  genres: string[];
  poster_url: string | null;
  is_in_library: boolean;
  want_status: string;
  first_letter: string;
}

export interface SearchTvShowDocument {
  id: string;
  title: string;
  year: number | null;
  genres: string[];
  poster_url: string | null;
  is_monitored: boolean;
  status: string | null;
  network: string | null;
  first_letter: string;
}

export interface SearchTvSeasonDocument {
  id: string;
  show_id: string;
  show_title: string;
  display_title: string;
  season_title: string | null;
  season_identifier: string;
  season_code: string;
  poster_url: string | null;
  episode_count: number;
  is_monitored: boolean;
  is_special: boolean;
}

export interface SearchTvEpisodeDocument {
  id: string;
  show_id: string;
  season_id: string;
  show_title: string;
  display_title: string;
  episode_title: string | null;
  episode_code: string;
  still_url: string | null;
  is_in_library: boolean;
  want_status: string;
}

export interface SearchPersonDocument {
  id: string;
  name: string;
  sort_name: string | null;
  image_url: string | null;
  bio: string | null;
  movie_count: number;
  tv_show_count: number;
}

export interface SearchIptvChannelDocument {
  id: string;
  name: string;
  category: string | null;
  icon_url: string | null;
  group_name: string | null;
  source_count: number;
}

export interface SearchIptvEpgDocument {
  id: string;
  channel_id: string;
  channel_name: string;
  title: string;
  description: string | null;
  categories: string[];
  start_time: number;
  stop_time: number;
  start_time_iso: string;
  stop_time_iso: string;
  channel_icon_url: string | null;
  is_live: boolean;
}

export interface SearchLivetvChannelDocument {
  id: string;
  name: string;
  lcn: number | null;
  network: string | null;
  region: string | null;
  type_label: string;
  logo_url: string | null;
  hd: boolean;
  enabled: boolean;
  has_guide: boolean;
}

export interface SearchLivetvGuideDocument {
  id: string;
  channel_id: string;
  channel_name: string;
  title: string;
  episode_title: string | null;
  synopsis: string | null;
  genres: string[];
  start_time: number;
  start_time_iso: string;
  end_time_iso: string;
  image_url: string | null;
  is_now: boolean;
}

/** A single search hit with type discrimination */
export interface SearchSettingDocument {
  id: string;
  field_name: string;
  label: string;
  description: string;
  page_path: string;
  page_label: string;
  aliases: string[];
}

export type SearchHit =
  | ({ type: 'book' } & SearchBookDocument)
  | ({ type: 'author' } & SearchAuthorDocument)
  | ({ type: 'series' } & SearchSeriesDocument)
  | ({ type: 'genre' } & SearchGenreDocument)
  | ({ type: 'page' } & SearchPageDocument)
  | ({ type: 'command' } & SearchCommandDocument)
  | ({ type: 'setting' } & SearchSettingDocument)
  | ({ type: 'movie' } & SearchMovieDocument)
  | ({ type: 'tv_show' } & SearchTvShowDocument)
  | ({ type: 'tv_season' } & SearchTvSeasonDocument)
  | ({ type: 'tv_episode' } & SearchTvEpisodeDocument)
  | ({ type: 'person' } & SearchPersonDocument)
  | ({ type: 'transcript' } & TranscriptSearchResult)
  | ({ type: 'iptv_channel' } & SearchIptvChannelDocument)
  | ({ type: 'iptv_epg' } & SearchIptvEpgDocument)
  | ({ type: 'livetv_channel' } & SearchLivetvChannelDocument)
  | ({ type: 'livetv_guide' } & SearchLivetvGuideDocument);

export interface SearchMediaRootFileDocument {
  id: string;
  file_name: string;
  file_path: string;
  media_root_id: string;
  media_root_name: string;
  match_status: string;
  entity_type: string | null;
  show_name: string | null;
  movie_title: string | null;
  book_title: string | null;
  series_name: string | null;
}

/**
 * Unified search response with federated results.
 * `top_hits` contains the best hit of each type, ordered by relevance score.
 * `remaining` contains all other results in relevance order.
 */
export interface UnifiedSearchResponse {
  top_hits: SearchHit[];
  remaining: SearchHit[];
}

/** Legacy types for backward compatibility */
export type UnifiedSearchBook = SearchBookDocument;
export type UnifiedSearchAuthor = SearchAuthorDocument;
export type UnifiedSearchSeries = SearchSeriesDocument;

/**
 * Unified search across all entity types using MeiliSearch federated search.
 * Returns results ranked by relevance, with top_hits containing the best
 * hit of each type and remaining containing everything else.
 */
export async function unifiedSearch(query: string, limit = 5): Promise<UnifiedSearchResponse> {
  return request(`/api/v1/admin/search${buildQueryString({ q: query, limit })}`);
}

/**
 * Trigger a full reindex of all search indexes
 */
export async function triggerReindex(): Promise<{ message: string; job_id: string }> {
  return request('/api/v1/admin/search/reindex', { method: 'POST' });
}

/**
 * Trigger an enrichment backfill (thumbnails + LUFS for files missing them)
 *
 * `mediaType` restricts the walk to one kind. This pass decodes every file it
 * touches, and the library holds ~16x as many episode files as movie files, so
 * a change that only affects one kind should not re-analyse the rest.
 */
export async function triggerEnrichmentBackfill(
  opts: { allVideo?: boolean; mediaType?: MediaScope } = {}
): Promise<{ job_id: string }> {
  return request('/api/v1/admin/playback/enrich-backfill', {
    method: 'POST',
    body: JSON.stringify({
      all_video: opts.allVideo ?? false,
      media_type: opts.mediaType ?? null
    })
  });
}

/**
 * Run the media integrity check now.
 *
 * `stampOnly` compares each file against the size and mtime its cache was built
 * from — free, and what the daily schedule already does. The full pass also
 * samples file contents for the empty blocks an unfinished download leaves,
 * which reads ~768 KB per file across the library.
 *
 * `remediate` deletes what the content sample flags, so it needs a human call.
 * `remediateMissing` only drops rows whose file is already gone from disk,
 * which is what frees the episode to be wanted again; the daily schedule
 * already does that much.
 */
export async function runIntegrityCheck(
  opts: { stampOnly?: boolean; remediate?: boolean; remediateMissing?: boolean } = {}
): Promise<{ job_id: string }> {
  return request('/api/v1/admin/playback/integrity-check', {
    method: 'POST',
    body: JSON.stringify({
      stamp_only: opts.stampOnly ?? false,
      remediate: opts.remediate ?? false,
      remediate_missing: opts.remediateMissing ?? false
    })
  });
}

/**
 * Queue skip detection across the library.
 *
 * One job per season (detection is season-scoped) plus one per movie. By
 * default only content whose cache is newer than its last detection is queued;
 * `force` re-detects everything, which is what you want after changing the
 * detection logic itself.
 *
 * `mediaType` narrows the run to `'movie'` or `'episode'`. Detection runs a
 * credit-text classify per file on both paths, so a forced run over
 * everything is a full-library GPU pass — scope it when only one path changed.
 */
export async function rerunSkipDetect(
  force = false,
  mediaType?: MediaScope
): Promise<{
  queued: number;
  failed: number;
  seasons: number;
  movies: number;
  forced: boolean;
  media_type: MediaScope | null;
}> {
  return request('/api/v1/admin/playback/skip-detect-rerun', {
    method: 'POST',
    body: JSON.stringify({ force, media_type: mediaType ?? null })
  });
}

/**
 * Trigger a reindex of transcript indexes only (audiobook transcriptions and ebook text)
 */
export async function reindexTranscripts(): Promise<{ message: string; job_id: string }> {
  return request('/api/v1/admin/search/reindex', {
    method: 'POST',
    body: JSON.stringify({ entity_types: ['transcripts'] })
  });
}

// Recents (Command Palette Frecency)

export interface RecentItem {
  entity_type: string;
  entity_id: string;
  label: string | null;
  access_count: number;
  last_accessed_at: string;
}

export async function getRecents(): Promise<RecentItem[]> {
  return request('/api/v1/admin/recents');
}

export async function recordRecentAccess(entityType: string, entityId: string): Promise<void> {
  return request('/api/v1/admin/recents', {
    method: 'POST',
    body: JSON.stringify({ entity_type: entityType, entity_id: entityId })
  });
}

export async function clearRecents(): Promise<void> {
  return request('/api/v1/admin/recents', { method: 'DELETE' });
}

// Library Search and Download

import type {
  BookFilesResponse,
  LibrarySearchResponse,
  DownloadToLibraryRequest,
  DownloadToLibraryResponse
} from '../types';

export interface LibrarySearchParams {
  q: string;
  book_id?: string;
  /** "any" drops the book category filter; omit to scope the search to books. */
  scope?: string;
}

/**
 * Search for releases via Kobload (independent of wanted system)
 */
export async function searchLibraryReleases(
  params: LibrarySearchParams
): Promise<LibrarySearchResponse> {
  const searchParams = new URLSearchParams();
  searchParams.set('q', params.q);
  if (params.book_id) {
    searchParams.set('book_id', params.book_id);
  }
  if (params.scope) {
    searchParams.set('scope', params.scope);
  }
  return request(`/api/v1/library/search/releases?${searchParams.toString()}`);
}

/**
 * Download a release to an existing book or create a new one
 */
export async function downloadToLibrary(
  payload: DownloadToLibraryRequest
): Promise<DownloadToLibraryResponse> {
  return request('/api/v1/library/search/download', {
    method: 'POST',
    body: JSON.stringify(payload)
  });
}

/**
 * Get all files for a book with provenance information
 */
export async function getBookFiles(bookId: string): Promise<BookFilesResponse> {
  return dedup(`bookFiles:${bookId}`, () => request(`/api/v1/library/books/${bookId}/files`));
}

/**
 * Delete a specific book file
 */
export async function deleteBookFile(bookId: string, fileId: string): Promise<void> {
  await request(`/api/v1/library/books/${bookId}/files/${fileId}`, {
    method: 'DELETE'
  });
}

export async function moveBookFile(
  bookId: string,
  fileId: string,
  targetBookId: string
): Promise<void> {
  await request(`/api/v1/library/books/${bookId}/files/${fileId}/move`, {
    method: 'POST',
    body: JSON.stringify({ target_book_id: targetBookId })
  });
}

/**
 * Reject the active download for an entity and re-open for new matches.
 * POST /api/v1/admin/library/{media_type}/{entity_id}/reject-download
 */
export async function rejectDownload(
  mediaType: 'book' | 'movie' | 'tv_episode',
  entityId: string,
  reason?: string
): Promise<void> {
  return request(`/api/v1/admin/library/${mediaType}/${entityId}/reject-download`, {
    method: 'POST',
    body: JSON.stringify({ reason: reason ?? null })
  });
}

/**
 * Replace a bad file: delete it, record the rejection, and re-want.
 * POST /api/v1/admin/library/{media_type}/{entity_id}/replace
 */
export async function replaceFile(
  mediaType: 'book' | 'movie' | 'tv_episode',
  entityId: string,
  fileId: string,
  reason?: string
): Promise<void> {
  return request(`/api/v1/admin/library/${mediaType}/${entityId}/replace`, {
    method: 'POST',
    body: JSON.stringify({ file_id: fileId, reason: reason ?? null })
  });
}

/**
 * Delete a specific movie file
 * DELETE /api/v1/admin/movies/:id/files/:file_id
 */
export async function deleteMovieFile(movieId: string, fileId: string): Promise<void> {
  await request(`/api/v1/admin/movies/${movieId}/files/${fileId}`, {
    method: 'DELETE'
  });
}

export async function moveMovieFile(
  movieId: string,
  fileId: string,
  targetMovieId: string
): Promise<void> {
  await request(`/api/v1/admin/movies/${movieId}/files/${fileId}/move`, {
    method: 'POST',
    body: JSON.stringify({ target_movie_id: targetMovieId })
  });
}

/**
 * Delete a specific TV episode file
 * DELETE /api/v1/admin/tv/episodes/:id/files/:file_id
 */
export async function deleteEpisodeFile(episodeId: string, fileId: string): Promise<void> {
  await request(`/api/v1/admin/tv/episodes/${episodeId}/files/${fileId}`, {
    method: 'DELETE'
  });
}

export async function moveEpisodeFile(
  episodeId: string,
  fileId: string,
  targetEpisodeId: string
): Promise<void> {
  await request(`/api/v1/admin/tv/episodes/${episodeId}/files/${fileId}/move`, {
    method: 'POST',
    body: JSON.stringify({ target_episode_id: targetEpisodeId })
  });
}

export async function getNextEpisode(
  episodeId: string
): Promise<import('#lib/types.js').NextEpisodeInfo | null> {
  try {
    return await request(`/api/v1/admin/tv/episodes/${episodeId}/next`);
  } catch {
    return null;
  }
}

// Search Status
export interface SearchBackendStatus {
  backend_type: string;
  connected: boolean;
  url: string;
  book_count: number | null;
  author_count: number | null;
  series_count: number | null;
  genre_count: number | null;
  embedded_document_count: number | null;
  error: string | null;
}

export interface TestConnectionRequest {
  url: string;
  master_key: string;
}

export interface TestConnectionResponse {
  success: boolean;
  message: string;
  version: string | null;
}

/**
 * Get search backend status
 */
export async function getSearchStatus(): Promise<SearchBackendStatus> {
  return request('/api/v1/admin/search/status');
}

/**
 * Test a MeiliSearch connection without affecting the running backend
 */
export async function testSearchConnection(
  params: TestConnectionRequest
): Promise<TestConnectionResponse> {
  return request('/api/v1/admin/search/test', {
    method: 'POST',
    body: JSON.stringify(params)
  });
}

// Downloads (Kobload proxy)
export interface DownloadsQueryParams {
  limit?: number;
  cursor?: string;
  status?: string;
}

export interface DownloadMediaInfo {
  id: string;
  title: string;
  cover_url: string | null;
  author: string | null;
  author_id: string | null;
  series: string | null;
  series_id: string | null;
  series_index: number | null;
  subtitle: string | null;
}

// Enriched download: all kobload fields flattened, plus optional media info
export type EnrichedDownload = KobloadDownloadListItem & {
  media: DownloadMediaInfo | null;
};

export interface EnrichedDownloadsResponse {
  items: EnrichedDownload[];
  next_cursor: string | null;
}

/**
 * Get current downloads from Kobload with media enrichment
 */
export async function getDownloads(
  params: DownloadsQueryParams = {}
): Promise<EnrichedDownloadsResponse> {
  const qs = new URLSearchParams();
  if (params.limit) qs.set('limit', String(params.limit));
  if (params.cursor) qs.set('cursor', params.cursor);
  if (params.status) qs.set('status', params.status);
  const query = qs.toString();
  return request(`/api/v1/admin/downloads${query ? `?${query}` : ''}`);
}

/**
 * Cancel a download
 */
export async function cancelDownload(downloadId: string): Promise<void> {
  return request(`/api/v1/admin/downloads/${downloadId}/cancel`, {
    method: 'POST'
  });
}

/**
 * Retry downloading a book (triggers new search)
 */
export async function retryDownload(
  downloadId: string
): Promise<{ message: string; book_id: string }> {
  return request(`/api/v1/admin/downloads/${downloadId}/retry`, {
    method: 'POST'
  });
}

/**
 * Retry the ingest callback for a completed download
 */
export async function retryCallback(downloadId: string): Promise<{ message: string }> {
  return request(`/api/v1/admin/downloads/${downloadId}/retry-callback`, {
    method: 'POST'
  });
}

/**
 * Get the URL for downloading a book file for the web reader.
 */
export function getBookFileUrl(bookId: string, format?: string): string {
  const params = format ? `?format=${encodeURIComponent(format)}` : '';
  return `${apiBase}/api/v1/admin/books/${bookId}/file${params}`;
}

/**
 * Get saved reader progress for a book.
 */
export async function getReaderProgress(
  bookId: string
): Promise<{ progress_percent: number | null; cfi: string | null }> {
  return request(`/api/v1/admin/books/${bookId}/reader-progress`);
}

/**
 * Save reader progress for a book.
 */
export async function saveReaderProgress(
  bookId: string,
  data: {
    progress_percent: number;
    cfi?: string;
    minutes_since_last_save?: number;
    closing?: boolean;
  }
): Promise<void> {
  return request(`/api/v1/admin/books/${bookId}/reader-progress`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data)
  });
}

// Kindle

export interface KindleTarget {
  id: string;
  device_serial: string;
  device_name: string | null;
  enabled: boolean;
}

export interface KindleAccount {
  connected: boolean;
  account_id: string | null;
  targets: KindleTarget[];
}

export interface KindleDelivery {
  id: string;
  target_id: string;
  device_name: string | null;
  delivered_at: string;
}

export async function getKindleOAuthUrl(): Promise<{ url: string }> {
  return request('/api/v1/admin/kindle/oauth-url');
}

export async function completeKindleOAuth(
  redirectUrl: string,
  deviceId?: string
): Promise<{ account_id: string }> {
  return request('/api/v1/admin/kindle/oauth-complete', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ redirect_url: redirectUrl, device_id: deviceId })
  });
}

export async function getKindleAccount(): Promise<KindleAccount> {
  return request('/api/v1/admin/kindle/account');
}

export async function disconnectKindle(): Promise<void> {
  return request('/api/v1/admin/kindle/account', { method: 'DELETE' });
}

export async function refreshKindleDevices(
  deviceId?: string
): Promise<{ targets: KindleTarget[] }> {
  return request('/api/v1/admin/kindle/refresh-devices', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ device_id: deviceId })
  });
}

export async function updateKindleTarget(targetId: string, enabled: boolean): Promise<void> {
  return request(`/api/v1/admin/kindle/targets/${targetId}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ enabled })
  });
}

export async function sendToKindle(
  bookId: string,
  targetId: string,
  force = false
): Promise<{ job_id: string }> {
  return request(`/api/v1/admin/books/${bookId}/kindle`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ target_id: targetId, force })
  });
}

export async function getBookKindleDeliveries(
  bookId: string
): Promise<{ deliveries: KindleDelivery[] }> {
  return request(`/api/v1/admin/books/${bookId}/kindle/deliveries`);
}

// Video Metadata Providers

export async function getVideoMetadataProviders(): Promise<VideoMetadataProvider[]> {
  return request('/api/v1/admin/video-metadata/providers');
}

export async function updateVideoMetadataProvider(
  id: string,
  data: {
    enabled?: boolean;
    priority?: number;
    config?: Record<string, unknown>;
  }
): Promise<void> {
  return request(`/api/v1/admin/video-metadata/providers/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(data)
  });
}

export async function getSubtitleProviders(): Promise<SubtitleProvider[]> {
  return request('/api/v1/admin/subtitle-providers');
}

export async function updateSubtitleProvider(
  id: string,
  data: {
    enabled?: boolean;
    priority?: number;
    config?: Record<string, unknown>;
  }
): Promise<void> {
  return request(`/api/v1/admin/subtitle-providers/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(data)
  });
}

export async function searchMovieSubtitles(
  movieId: string,
  data: { languages: string[]; forced_only?: boolean }
): Promise<SubtitleSearchResult[]> {
  return request(`/api/v1/admin/movies/${movieId}/subtitles/search`, {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

export async function searchEpisodeSubtitles(
  episodeId: string,
  data: { languages: string[]; forced_only?: boolean }
): Promise<SubtitleSearchResult[]> {
  return request(`/api/v1/admin/tv/episodes/${episodeId}/subtitles/search`, {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

export async function downloadSubtitle(data: {
  provider: string;
  subtitle_id: string;
  movie_file_id?: string;
  episode_file_id?: string;
  language?: string;
  is_forced?: boolean;
  is_hearing_impaired?: boolean;
}): Promise<{ id: string; file_path: string }> {
  return request('/api/v1/admin/subtitles/download', {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

export async function getMovieTracks(movieId: string): Promise<TracksResponse> {
  return dedup(`movieTracks:${movieId}`, () => request(`/api/v1/admin/movies/${movieId}/tracks`));
}

export async function getEpisodeTracks(episodeId: string): Promise<TracksResponse> {
  return dedup(`episodeTracks:${episodeId}`, () =>
    request(`/api/v1/admin/tv/episodes/${episodeId}/tracks`)
  );
}

export async function getSearchSuggestions(
  query: string,
  media: string,
  limit = 8
): Promise<SearchSuggestion[]> {
  const resp = await request<{ suggestions: SearchSuggestion[] }>(
    `/api/v1/admin/search/suggestions${buildQueryString({ q: query, media, limit })}`
  );
  return resp.suggestions;
}

export async function searchMovies(
  query: string,
  options?: { year?: number; provider?: string }
): Promise<MovieSearchResult[]> {
  const params: Record<string, string | number | undefined> = { q: query, ...options };
  const resp = await request<{ results: MovieSearchResult[] }>(
    `/api/v1/admin/video-metadata/search/movies${buildQueryString(params)}`
  );
  return resp.results;
}

export async function searchTvShows(
  query: string,
  options?: { provider?: string }
): Promise<TvSearchResult[]> {
  const params: Record<string, string | undefined> = { q: query, ...options };
  const resp = await request<{ results: TvSearchResult[] }>(
    `/api/v1/admin/video-metadata/search/tv${buildQueryString(params)}`
  );
  return resp.results;
}

export async function searchBooks(
  query: string,
  options?: { provider?: string }
): Promise<MetadataResult[]> {
  const params: Record<string, string | undefined> = { title: query, ...options };
  const resp = await request<{ results: MetadataResult[] }>(
    `/api/v1/admin/metadata/search${buildQueryString(params)}`
  );
  return resp.results;
}

/**
 * Merge multiple video metadata search results via backend priority merge.
 * Results should be in priority order (first = highest priority).
 */
export async function mergeVideoMetadata(
  type: 'movie' | 'tv',
  results: MovieSearchResult[] | TvSearchResult[]
): Promise<MovieSearchResult | TvSearchResult> {
  return request('/api/v1/admin/video-metadata/merge', {
    method: 'POST',
    body: JSON.stringify({ type, results })
  });
}

export async function getVideoMovieDetails(
  providerId: string,
  provider?: string
): Promise<import('#lib/types.js').MovieDetailResult> {
  const params: Record<string, string | undefined> = { provider };
  return request(`/api/v1/admin/video-metadata/movies/${providerId}${buildQueryString(params)}`);
}

export async function getVideoTvDetails(
  providerId: string,
  provider?: string
): Promise<import('#lib/types.js').TvShowDetailResult> {
  const params: Record<string, string | undefined> = { provider };
  return request(`/api/v1/admin/video-metadata/tv/${providerId}${buildQueryString(params)}`);
}

export async function getVideoSeasonDetails(
  providerId: string,
  seasonNumber: number,
  provider?: string
): Promise<import('#lib/types.js').SeasonDetailResult> {
  const params: Record<string, string | undefined> = { provider };
  return request(
    `/api/v1/admin/video-metadata/tv/${providerId}/season/${seasonNumber}${buildQueryString(params)}`
  );
}

// Movies

export async function getMovies(options?: {
  search?: string;
  is_in_library?: boolean;
  want_status?: string;
  cursor?: string;
  direction?: string;
  around?: string;
  around_before?: number;
  sort?: string;
  limit?: number;
}): Promise<PaginatedResponse<Movie>> {
  return request(`/api/v1/admin/movies${buildQueryString(options ?? {})}`);
}

export async function getMovie(id: string): Promise<Movie> {
  return request(`/api/v1/admin/movies/${id}`);
}

export async function createMovie(data: {
  title: string;
  year?: number | null;
  description?: string | null;
  poster_url?: string | null;
  backdrop_url?: string | null;
  identifiers?: Record<string, unknown>;
  genres?: string[];
  rating?: number | null;
  rating_count?: number | null;
  content_rating?: string | null;
  runtime?: number | null;
  media_root_id?: string | null;
}): Promise<{ id: string }> {
  return request('/api/v1/admin/movies', {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

export async function updateMovie(id: string, data: Record<string, unknown>): Promise<void> {
  return request(`/api/v1/admin/movies/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(data)
  });
}

export async function deleteMovie(id: string): Promise<void> {
  return request(`/api/v1/admin/movies/${id}`, { method: 'DELETE' });
}

export async function wantMovie(id: string): Promise<void> {
  return request(`/api/v1/admin/movies/${id}/want`, { method: 'POST' });
}

export async function unwantMovie(id: string): Promise<void> {
  return request(`/api/v1/admin/movies/${id}/want`, { method: 'DELETE' });
}

export async function searchMovieReleases(
  movieId: string,
  query?: string,
  scope?: string
): Promise<LibrarySearchResponse> {
  const search = new URLSearchParams();
  if (query) search.set('q', query);
  if (scope) search.set('scope', scope);
  const params = search.toString() ? `?${search.toString()}` : '';
  return request(`/api/v1/admin/movies/${movieId}/releases${params}`);
}

export async function queueMovieRelease(
  movieId: string,
  release: Record<string, unknown>,
  source: string
): Promise<void> {
  return request(`/api/v1/admin/movies/${movieId}/queue`, {
    method: 'POST',
    body: JSON.stringify({ release, source })
  });
}

export async function getMovieFiles(id: string): Promise<MovieFile[]> {
  return dedup(`movieFiles:${id}`, () => request(`/api/v1/admin/movies/${id}/files`));
}

export async function refreshMovieMetadata(id: string): Promise<void> {
  return request(`/api/v1/admin/movies/${id}/refresh-metadata`, { method: 'POST' });
}

export async function getMovieWatchState(id: string): Promise<MovieWatchState | null> {
  return dedup(`movieWatchState:${id}`, () => request(`/api/v1/admin/movies/${id}/watch-state`));
}

export async function updateMovieWatchState(
  id: string,
  data: {
    watched?: boolean;
    watched_at?: string | null;
    position_seconds?: number | null;
    rating?: number | null;
  }
): Promise<{ id: string }> {
  return request(`/api/v1/admin/movies/${id}/watch-state`, {
    method: 'PUT',
    body: JSON.stringify(data)
  });
}

export async function updateMoviePosition(
  id: string,
  positionSeconds: number
): Promise<{ id: string; watched: boolean | null }> {
  return request(`/api/v1/admin/movies/${id}/watch-state/position`, {
    method: 'PATCH',
    body: JSON.stringify({ position_seconds: positionSeconds, ...playbackClient() })
  });
}

export function moviePosterUrl(id: string): string {
  return `${apiBase}/api/v1/admin/movies/${id}/poster`;
}

export async function uploadMoviePoster(id: string, file: File): Promise<void> {
  const response = await fetch(`${apiBase}/api/v1/admin/movies/${id}/poster`, {
    method: 'POST',
    credentials: 'include',
    body: file
  });
  if (!response.ok) {
    const text = await response.text().catch(() => 'Upload failed');
    throw new ApiError(response.status, text);
  }
}

export async function uploadMovieBackdrop(id: string, file: File): Promise<void> {
  const response = await fetch(`${apiBase}/api/v1/admin/movies/${id}/backdrop`, {
    method: 'POST',
    credentials: 'include',
    body: file
  });
  if (!response.ok) {
    const text = await response.text().catch(() => 'Upload failed');
    throw new ApiError(response.status, text);
  }
}

export async function uploadMovieLogo(id: string, file: File): Promise<void> {
  const response = await fetch(`${apiBase}/api/v1/admin/movies/${id}/logo`, {
    method: 'POST',
    credentials: 'include',
    body: file
  });
  if (!response.ok) {
    const text = await response.text().catch(() => 'Upload failed');
    throw new ApiError(response.status, text);
  }
}

export async function getMovieFirstByLetter(
  letter: string
): Promise<{ id: string; title: string } | null> {
  try {
    return await request(`/api/v1/admin/movies/first-by-letter${buildQueryString({ letter })}`);
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) return null;
    throw e;
  }
}

export async function getMovieAvailableLetters(): Promise<string[]> {
  const resp = await request<{ letters: string[] }>('/api/v1/admin/movies/available-letters');
  return resp.letters;
}

// People

export async function getPerson(id: string): Promise<PersonDetail> {
  return request(`/api/v1/admin/people/${id}`);
}

// Person filmography
export async function getPersonFilmography(
  personId: string
): Promise<{ credits: PersonFilmographyCredit[] }> {
  return request(`/api/v1/admin/people/${personId}/filmography`);
}

export async function wantPersonCredit(
  personId: string,
  providerId: string,
  mediaType: string,
  title?: string,
  year?: number
): Promise<{ id: string; media_type: string }> {
  return request(`/api/v1/admin/people/${personId}/filmography/want`, {
    method: 'POST',
    body: JSON.stringify({ provider_id: providerId, media_type: mediaType, title, year })
  });
}

export async function dismissPersonCredit(
  personId: string,
  providerId: string,
  mediaType: string
): Promise<void> {
  await request(`/api/v1/admin/people/${personId}/filmography/dismiss`, {
    method: 'POST',
    body: JSON.stringify({ provider_id: providerId, media_type: mediaType })
  });
}

export async function undismissPersonCredit(
  personId: string,
  providerId: string,
  mediaType: string
): Promise<void> {
  await request(`/api/v1/admin/people/${personId}/filmography/dismiss`, {
    method: 'DELETE',
    body: JSON.stringify({ provider_id: providerId, media_type: mediaType })
  });
}

export async function updatePersonMonitoring(
  personId: string,
  data: { is_monitored?: boolean; auto_want_mode?: string }
): Promise<void> {
  await request(`/api/v1/admin/people/${personId}/monitoring`, {
    method: 'PUT',
    body: JSON.stringify(data)
  });
}

export async function refreshPersonMetadata(personId: string): Promise<void> {
  await request(`/api/v1/admin/people/${personId}/refresh`, {
    method: 'POST'
  });
}

// TV Shows

export async function getTvShows(options?: {
  search?: string;
  is_monitored?: boolean;
  status?: string;
  cursor?: string;
  direction?: string;
  around?: string;
  around_before?: number;
  sort?: string;
  limit?: number;
}): Promise<PaginatedResponse<TvShow>> {
  return request(`/api/v1/admin/tv/shows${buildQueryString(options ?? {})}`);
}

export async function getTvShow(id: string): Promise<TvShow> {
  return request(`/api/v1/admin/tv/shows/${id}`);
}

export async function createTvShow(data: {
  title: string;
  year?: number | null;
  description?: string | null;
  poster_url?: string | null;
  backdrop_url?: string | null;
  identifiers?: Record<string, unknown>;
  genres?: string[];
  network?: string | null;
  media_root_id?: string | null;
}): Promise<{ id: string }> {
  return request('/api/v1/admin/tv/shows', {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

export async function updateTvShow(id: string, data: Record<string, unknown>): Promise<void> {
  return request(`/api/v1/admin/tv/shows/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(data)
  });
}

export async function deleteTvShow(id: string): Promise<void> {
  return request(`/api/v1/admin/tv/shows/${id}`, { method: 'DELETE' });
}

export async function setTvShowMonitored(
  id: string,
  monitored: boolean,
  autoWantMode: string
): Promise<void> {
  return request(`/api/v1/admin/tv/shows/${id}/monitor`, {
    method: 'POST',
    body: JSON.stringify({ is_monitored: monitored, auto_want_mode: autoWantMode })
  });
}

export type WatchlistMediaType = 'show' | 'movie';

export interface WatchlistEntry {
  media_type: WatchlistMediaType;
  media_id: string;
  title: string;
  subtitle: string | null;
  image_url: string | null;
  link: string;
  added_at: string;
}

export async function listWatchlist(): Promise<WatchlistEntry[]> {
  return request('/api/v1/admin/watchlist');
}

export async function addToWatchlist(
  mediaType: WatchlistMediaType,
  mediaId: string
): Promise<void> {
  return request('/api/v1/admin/watchlist', {
    method: 'POST',
    body: JSON.stringify({ media_type: mediaType, media_id: mediaId })
  });
}

export async function removeFromWatchlist(
  mediaType: WatchlistMediaType,
  mediaId: string
): Promise<void> {
  return request(`/api/v1/admin/watchlist/${mediaType}/${mediaId}`, { method: 'DELETE' });
}

export async function refreshTvShowMetadata(id: string): Promise<void> {
  return request(`/api/v1/admin/tv/shows/${id}/refresh-metadata`, { method: 'POST' });
}

export async function bulkUpdateTvShows(
  showIds: string[],
  updates: { auto_want_mode?: string; quality_profile_id?: string | null }
): Promise<void> {
  return request('/api/v1/admin/tv/shows/bulk', {
    method: 'POST',
    body: JSON.stringify({
      show_ids: showIds,
      ...updates
    })
  });
}

export async function wantAllMissing(showId: string): Promise<void> {
  return request(`/api/v1/admin/tv/shows/${showId}/want-all-missing`, { method: 'POST' });
}

export interface UnwantResult {
  unwanted_count: number;
  /** True when a refilling auto-want mode had to be turned off to make it stick. */
  auto_want_disabled: boolean;
}

export async function unwantAll(showId: string): Promise<UnwantResult> {
  return request(`/api/v1/admin/tv/shows/${showId}/want-all-missing`, { method: 'DELETE' });
}

export async function runMediaAnalyzeShow(
  showId: string,
  force = false
): Promise<{ enqueued: number; run_ids: string[] }> {
  const q = force ? '?force=true' : '';
  return request(`/api/v1/admin/tv/shows/${showId}/media-analyze${q}`, { method: 'POST' });
}

export async function runMediaAnalyzeSeason(
  seasonId: string,
  force = false
): Promise<{ enqueued: number; run_ids: string[] }> {
  const q = force ? '?force=true' : '';
  return request(`/api/v1/admin/tv/seasons/${seasonId}/media-analyze${q}`, { method: 'POST' });
}

export async function runMediaAnalyzeMovie(
  movieId: string,
  force = false
): Promise<{ enqueued: number; run_ids: string[] }> {
  const q = force ? '?force=true' : '';
  return request(`/api/v1/admin/movies/${movieId}/media-analyze${q}`, { method: 'POST' });
}

export async function getSeason(id: string): Promise<TvSeason> {
  return request(`/api/v1/admin/tv/seasons/${id}`);
}

export async function wantSeason(id: string): Promise<void> {
  return request(`/api/v1/admin/tv/seasons/${id}/want`, { method: 'POST' });
}

export async function unwantSeason(id: string): Promise<UnwantResult> {
  return request(`/api/v1/admin/tv/seasons/${id}/want`, { method: 'DELETE' });
}

export async function setSeasonAutoWant(seasonId: string, mode: string): Promise<void> {
  return request(`/api/v1/admin/tv/seasons/${seasonId}/auto-want`, {
    method: 'POST',
    body: JSON.stringify({ auto_want_mode: mode })
  });
}

export async function getEpisode(id: string): Promise<TvEpisode> {
  return request(`/api/v1/admin/tv/episodes/${id}`);
}

export interface EpisodeCastMember {
  person_id: string;
  name: string;
  character: string | null;
  image_url: string | null;
  cast_order: number | null;
}

export interface EpisodeCrewMember {
  person_id: string;
  name: string;
  department: string | null;
  job: string | null;
  image_url: string | null;
}

export interface EpisodeCastResponse {
  episode_id: string;
  show_id: string;
  show_title: string;
  season_identifier: string;
  episode_identifier: string;
  episode_title: string | null;
  /** 'unavailable' means the guest list is unknown, not that there were no guests. */
  guest_source: 'cache' | 'provider' | 'unavailable';
  series_regulars: EpisodeCastMember[];
  guest_stars: EpisodeCastMember[];
  crew: EpisodeCrewMember[];
}

export async function getEpisodeCast(episodeId: string): Promise<EpisodeCastResponse> {
  return dedup(`episodeCast:${episodeId}`, () =>
    request(`/api/v1/admin/tv/episodes/${episodeId}/cast`)
  );
}

export async function wantEpisode(id: string): Promise<void> {
  return request(`/api/v1/admin/tv/episodes/${id}/want`, { method: 'POST' });
}

export async function unwantEpisode(id: string): Promise<void> {
  return request(`/api/v1/admin/tv/episodes/${id}/want`, { method: 'DELETE' });
}

export async function searchEpisodeReleases(
  episodeId: string,
  query?: string,
  scope?: string
): Promise<LibrarySearchResponse> {
  const search = new URLSearchParams();
  if (query) search.set('q', query);
  if (scope) search.set('scope', scope);
  const params = search.toString() ? `?${search.toString()}` : '';
  return request(`/api/v1/admin/tv/episodes/${episodeId}/releases${params}`);
}

export async function queueEpisodeRelease(
  episodeId: string,
  release: Record<string, unknown>,
  source: string
): Promise<void> {
  return request(`/api/v1/admin/tv/episodes/${episodeId}/queue`, {
    method: 'POST',
    body: JSON.stringify({ release, source })
  });
}

export async function updateEpisode(id: string, data: Record<string, unknown>): Promise<void> {
  return request(`/api/v1/admin/tv/episodes/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(data)
  });
}

export async function getEpisodeFiles(episodeId: string): Promise<TvEpisodeFile[]> {
  return dedup(`episodeFiles:${episodeId}`, () =>
    request(`/api/v1/admin/tv/episodes/${episodeId}/files`)
  );
}

export async function getEpisodeWatchState(episodeId: string): Promise<TvWatchState | null> {
  return dedup(`episodeWatchState:${episodeId}`, () =>
    request(`/api/v1/admin/tv/episodes/${episodeId}/watch-state`)
  );
}

export async function updateEpisodeWatchState(
  episodeId: string,
  data: {
    watched?: boolean;
    watched_at?: string | null;
    position_seconds?: number | null;
    rating?: number | null;
  }
): Promise<{ id: string }> {
  return request(`/api/v1/admin/tv/episodes/${episodeId}/watch-state`, {
    method: 'PUT',
    body: JSON.stringify(data)
  });
}

export async function bulkUpdateShowWatchState(
  showId: string,
  watched: boolean
): Promise<{ updated_count: number }> {
  return request(`/api/v1/admin/tv/shows/${showId}/watch-state`, {
    method: 'PUT',
    body: JSON.stringify({ watched })
  });
}

export async function bulkUpdateSeasonWatchState(
  seasonId: string,
  watched: boolean
): Promise<{ updated_count: number }> {
  return request(`/api/v1/admin/tv/seasons/${seasonId}/watch-state`, {
    method: 'PUT',
    body: JSON.stringify({ watched })
  });
}

export async function updateEpisodePosition(
  episodeId: string,
  positionSeconds: number
): Promise<{ id: string; watched: boolean | null }> {
  return request(`/api/v1/admin/tv/episodes/${episodeId}/watch-state/position`, {
    method: 'PATCH',
    body: JSON.stringify({ position_seconds: positionSeconds, ...playbackClient() })
  });
}

// TV Show Ratings
export async function getTvShowRating(id: string): Promise<{ rating: number | null }> {
  return dedup(`tvShowRating:${id}`, () => request(`/api/v1/admin/tv/shows/${id}/rating`));
}

export async function updateTvShowRating(id: string, rating: number): Promise<{ id: string }> {
  return request(`/api/v1/admin/tv/shows/${id}/rating`, {
    method: 'PUT',
    body: JSON.stringify({ rating })
  });
}

export async function deleteTvShowRating(id: string): Promise<void> {
  return request(`/api/v1/admin/tv/shows/${id}/rating`, {
    method: 'DELETE'
  });
}

export function tvShowPosterUrl(id: string): string {
  return `${apiBase}/api/v1/admin/tv/shows/${id}/poster`;
}

export async function uploadTvShowPoster(id: string, file: File): Promise<void> {
  const response = await fetch(`${apiBase}/api/v1/admin/tv/shows/${id}/poster`, {
    method: 'POST',
    credentials: 'include',
    body: file
  });
  if (!response.ok) {
    const text = await response.text().catch(() => 'Upload failed');
    throw new ApiError(response.status, text);
  }
}

export async function uploadTvShowBackdrop(id: string, file: File): Promise<void> {
  const response = await fetch(`${apiBase}/api/v1/admin/tv/shows/${id}/backdrop`, {
    method: 'POST',
    credentials: 'include',
    body: file
  });
  if (!response.ok) {
    const text = await response.text().catch(() => 'Upload failed');
    throw new ApiError(response.status, text);
  }
}

export async function uploadTvShowLogo(id: string, file: File): Promise<void> {
  const response = await fetch(`${apiBase}/api/v1/admin/tv/shows/${id}/logo`, {
    method: 'POST',
    credentials: 'include',
    body: file
  });
  if (!response.ok) {
    const text = await response.text().catch(() => 'Upload failed');
    throw new ApiError(response.status, text);
  }
}

export async function getTvShowFirstByLetter(
  letter: string
): Promise<{ id: string; title: string } | null> {
  try {
    return await request(`/api/v1/admin/tv/shows/first-by-letter${buildQueryString({ letter })}`);
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) return null;
    throw e;
  }
}

export async function getTvShowAvailableLetters(): Promise<string[]> {
  const resp = await request<{ letters: string[] }>('/api/v1/admin/tv/shows/available-letters');
  return resp.letters;
}

// Season CRUD

export async function createSeason(
  showId: string,
  data: {
    season_identifier: string;
    season_sort_order: number;
    title?: string;
    description?: string;
    air_date?: string;
    episode_count?: number;
    is_special?: boolean;
  }
): Promise<{ id: string }> {
  return request(`/api/v1/admin/tv/shows/${showId}/seasons`, {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

export async function updateSeason(id: string, data: Record<string, unknown>): Promise<void> {
  return request(`/api/v1/admin/tv/seasons/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(data)
  });
}

export async function deleteSeason(id: string): Promise<void> {
  return request(`/api/v1/admin/tv/seasons/${id}`, { method: 'DELETE' });
}

export async function uploadSeasonPoster(id: string, file: File): Promise<void> {
  const bytes = await file.arrayBuffer();
  await fetch(`${apiBase}/api/v1/admin/tv/seasons/${id}/poster`, {
    method: 'POST',
    credentials: 'include',
    headers: { 'Content-Type': 'application/octet-stream' },
    body: bytes
  });
}

export function seasonPosterUrl(id: string): string {
  return `${apiBase}/api/v1/admin/tv/seasons/${id}/poster`;
}

// Episode Create/Delete

export async function createEpisode(
  seasonId: string,
  data: {
    episode_identifier: string;
    episode_sort_order: number;
    title?: string;
    description?: string;
    air_date?: string;
    runtime?: number;
  }
): Promise<{ id: string }> {
  return request(`/api/v1/admin/tv/seasons/${seasonId}/episodes`, {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

export async function deleteEpisode(id: string): Promise<void> {
  return request(`/api/v1/admin/tv/episodes/${id}`, { method: 'DELETE' });
}

/**
 * Apply one action to many episodes at once. Episode IDs may span seasons and
 * shows. IDs that no longer exist are skipped rather than failing the batch.
 */
export async function bulkEpisodeAction(
  episodeIds: string[],
  action: BulkEpisodeAction
): Promise<BulkEpisodeResult> {
  return request('/api/v1/admin/tv/episodes/bulk', {
    method: 'POST',
    body: JSON.stringify({ episode_ids: episodeIds, action })
  });
}

// People CRUD

export async function createPerson(data: {
  name: string;
  sort_name?: string;
  identifiers?: Record<string, unknown>;
  image_url?: string;
  bio?: string;
  birth_date?: string;
  death_date?: string;
}): Promise<{ id: string }> {
  return request('/api/v1/admin/people', {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

export async function updatePerson(id: string, data: Record<string, unknown>): Promise<void> {
  return request(`/api/v1/admin/people/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(data)
  });
}

export async function deletePerson(id: string): Promise<void> {
  return request(`/api/v1/admin/people/${id}`, { method: 'DELETE' });
}

export async function uploadPersonImage(id: string, file: File): Promise<void> {
  const bytes = await file.arrayBuffer();
  await fetch(`${apiBase}/api/v1/admin/people/${id}/image`, {
    method: 'POST',
    credentials: 'include',
    headers: { 'Content-Type': 'application/octet-stream' },
    body: bytes
  });
}

export function personImageUrl(id: string): string {
  return `${apiBase}/api/v1/admin/people/${id}/image`;
}

export interface PersonListItem {
  id: string;
  name: string;
  sort_name: string | null;
  image_url: string | null;
}

export interface ListPeopleOptions {
  q?: string;
  limit?: number;
  cursor?: string;
  direction?: 'forward' | 'backward';
  search_offset?: number;
}

export async function listPeople(
  opts: ListPeopleOptions = {}
): Promise<PaginatedResponse<PersonListItem>> {
  const params: Record<string, string | number> = {};
  if (opts.q) params.q = opts.q;
  if (opts.limit) params.limit = opts.limit;
  if (opts.cursor) params.cursor = opts.cursor;
  if (opts.direction) params.direction = opts.direction;
  if (opts.search_offset !== undefined) params.search_offset = opts.search_offset;
  return request(`/api/v1/admin/people${buildQueryString(params)}`);
}

// Cast/Crew Management

export interface CastInput {
  person_id: string;
  character_name?: string | null;
  cast_order?: number | null;
}

export interface CrewInput {
  person_id: string;
  department: string;
  job: string;
}

export async function replaceMovieCast(movieId: string, cast: CastInput[]): Promise<void> {
  return request(`/api/v1/admin/movies/${movieId}/cast`, {
    method: 'PUT',
    body: JSON.stringify(cast)
  });
}

export async function replaceMovieCrew(movieId: string, crew: CrewInput[]): Promise<void> {
  return request(`/api/v1/admin/movies/${movieId}/crew`, {
    method: 'PUT',
    body: JSON.stringify(crew)
  });
}

export async function replaceTvShowCast(showId: string, cast: CastInput[]): Promise<void> {
  return request(`/api/v1/admin/tv/shows/${showId}/cast`, {
    method: 'PUT',
    body: JSON.stringify(cast)
  });
}

export async function replaceTvShowCrew(showId: string, crew: CrewInput[]): Promise<void> {
  return request(`/api/v1/admin/tv/shows/${showId}/crew`, {
    method: 'PUT',
    body: JSON.stringify(crew)
  });
}

// Episode Still Image

export async function uploadEpisodeStill(id: string, file: File): Promise<void> {
  const bytes = await file.arrayBuffer();
  await fetch(`${apiBase}/api/v1/admin/tv/episodes/${id}/still`, {
    method: 'POST',
    credentials: 'include',
    headers: { 'Content-Type': 'application/octet-stream' },
    body: bytes
  });
}

export function episodeStillUrl(id: string): string {
  return `${apiBase}/api/v1/admin/tv/episodes/${id}/still`;
}

// Video Sync Providers (Trakt, etc.)

export async function getVideoSyncProviders(): Promise<VideoSyncProvider[]> {
  return request('/api/v1/admin/video-sync/providers');
}

export async function updateVideoSyncProvider(
  providerType: string,
  data: { enabled?: boolean; config?: Record<string, unknown> }
): Promise<void> {
  return request(`/api/v1/admin/video-sync/providers/${providerType}`, {
    method: 'PATCH',
    body: JSON.stringify(data)
  });
}

export async function traktAuthorize(clientId: string): Promise<TraktDeviceCode> {
  return request('/api/v1/admin/video-sync/trakt/authorize', {
    method: 'POST',
    body: JSON.stringify({ client_id: clientId })
  });
}

export async function traktPollDeviceCode(
  deviceCode: string,
  clientId: string,
  clientSecret: string
): Promise<TraktPollResult> {
  return request(`/api/v1/admin/video-sync/trakt/poll/${deviceCode}`, {
    method: 'POST',
    body: JSON.stringify({ client_id: clientId, client_secret: clientSecret })
  });
}

export async function traktDisconnect(): Promise<void> {
  return request('/api/v1/admin/video-sync/trakt/disconnect', { method: 'POST' });
}

export async function traktImportHistory(mediaType?: string): Promise<{ status: string }> {
  return request('/api/v1/admin/video-sync/trakt/import-history', {
    method: 'POST',
    body: JSON.stringify({ media_type: mediaType })
  });
}

// Filesystem Browsing

export type DirectoryEntry = {
  name: string;
  path: string;
};

export type BrowseResponse = {
  current_path: string;
  parent_path: string | null;
  directories: DirectoryEntry[];
};

export async function browseDirectories(path?: string): Promise<BrowseResponse> {
  const params = path ? `?path=${encodeURIComponent(path)}` : '';
  return request(`/api/v1/admin/filesystem/browse${params}`);
}

export type DetectMediaTypeResponse = {
  suggested_type: 'tv' | 'movie' | 'book' | 'audiobook' | null;
  confidence: 'high' | 'low' | null;
  counts: { book: number; video: number; audio: number; subtitle: number };
  files_sampled: number;
};

export async function detectMediaType(path: string): Promise<DetectMediaTypeResponse> {
  return request(`/api/v1/admin/filesystem/detect-media-type?path=${encodeURIComponent(path)}`);
}

// Media Roots

export async function listMediaRoots(): Promise<MediaRoot[]> {
  return request('/api/v1/admin/media-roots');
}

export async function getNasHealth(): Promise<import('#lib/types.js').NasHealthSnapshot> {
  return request('/api/v1/admin/system/nas-health');
}

export async function getVisionHealth(): Promise<import('#lib/types.js').VisionHealthSnapshot> {
  return request('/api/v1/admin/system/vision-health');
}

export async function createMediaRoot(data: {
  path: string;
  name?: string;
  media_type: string;
  auto_add?: boolean;
  naming_template?: string | null;
  write_sidecars?: boolean;
  write_images?: boolean;
  is_default?: boolean;
}): Promise<MediaRoot> {
  return request('/api/v1/admin/media-roots', {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

export async function getMediaRoot(id: string): Promise<MediaRoot> {
  return request(`/api/v1/admin/media-roots/${id}`);
}

export async function updateMediaRoot(
  id: string,
  data: {
    path?: string;
    name?: string;
    enabled?: boolean;
    auto_add?: boolean;
    naming_template?: string | null;
    write_sidecars?: boolean;
    write_images?: boolean;
    is_default?: boolean;
    max_concurrent_io?: number;
    /**
     * Per-root scan interval override. Sentinel: `0` means "clear override and
     * use the global default"; a positive number sets the override; omit the
     * field to leave it untouched.
     */
    scan_interval_seconds?: number;
  }
): Promise<MediaRoot> {
  return request(`/api/v1/admin/media-roots/${id}`, {
    method: 'PUT',
    body: JSON.stringify(data)
  });
}

export async function setDefaultMediaRoot(id: string): Promise<void> {
  return request(`/api/v1/admin/media-roots/${id}/set-default`, { method: 'POST' });
}

export async function deleteMediaRoot(id: string): Promise<void> {
  return request(`/api/v1/admin/media-roots/${id}`, { method: 'DELETE' });
}

export async function triggerMediaRootScan(id: string): Promise<{ job_id: string }> {
  return request(`/api/v1/admin/media-roots/${id}/scan`, { method: 'POST' });
}

export async function triggerSidecarRewrite(): Promise<{ job_id: string; message: string }> {
  return request('/api/v1/admin/media-roots/sidecar-rewrite', { method: 'POST' });
}

export async function triggerWriteSidecars(id: string): Promise<{ job_id: string }> {
  return request(`/api/v1/admin/media-roots/${id}/write-sidecars`, { method: 'POST' });
}

export async function triggerWriteImages(id: string): Promise<{ job_id: string }> {
  return request(`/api/v1/admin/media-roots/${id}/write-images`, { method: 'POST' });
}

export interface FileListResponse {
  items: MediaRootFile[];
  total_count: number;
}

export async function listMediaRootFiles(
  id: string,
  options?: { status?: string; limit?: number; offset?: number }
): Promise<FileListResponse> {
  return request(`/api/v1/admin/media-roots/${id}/files${buildQueryString(options ?? {})}`);
}

export async function getMediaRootFile(fileId: string): Promise<MediaRootFile> {
  return request(`/api/v1/admin/media-roots/files/${fileId}`);
}

export async function listMediaRootFilesLite(
  id: string,
  options?: {
    status?: string;
    group_key?: string;
    season?: number;
    directory?: string;
    limit?: number;
    offset?: number;
  }
): Promise<import('#lib/types.js').FileListLiteResponse> {
  const qs = buildQueryString({ ...options, lite: true });
  // buildQueryString strips empty strings, but directory='' is meaningful (root-level only).
  // Append it manually if directory was explicitly provided.
  if (options?.directory !== undefined) {
    const sep = qs ? '&' : '?';
    const dirParam = `directory=${encodeURIComponent(options.directory)}`;
    if (!qs.includes('directory=')) {
      return request(`/api/v1/admin/media-roots/${id}/files${qs}${sep}${dirParam}`);
    }
  }
  return request(`/api/v1/admin/media-roots/${id}/files${qs}`);
}

export async function listMediaRootDirectories(
  id: string,
  parent?: string
): Promise<import('#lib/types.js').DirectoryEntry[]> {
  const params = parent ? `?parent=${encodeURIComponent(parent)}` : '';
  return request(`/api/v1/admin/media-roots/${id}/directories${params}`);
}

export async function listEntityGroups(
  rootId: string
): Promise<import('#lib/types.js').EntityGroupsResponse> {
  return request(`/api/v1/admin/media-roots/${rootId}/entity-groups`);
}

export async function listStatusCounts(
  rootId: string
): Promise<import('#lib/types.js').StatusCountsResponse> {
  return request(`/api/v1/admin/media-roots/${rootId}/status-counts`);
}

export async function triggerMediaRootReorganize(
  id: string,
  dryRun = false,
  includeUnconfirmed = false
): Promise<{ job_id: string }> {
  return request(`/api/v1/admin/media-roots/${id}/reorganize`, {
    method: 'POST',
    body: JSON.stringify({ dry_run: dryRun, include_unconfirmed: includeUnconfirmed })
  });
}

export async function resolveMediaRootFileMatch(
  fileId: string,
  data: { provider: string; provider_id: string; title: string; year?: number | string | null }
): Promise<{ status: string }> {
  return request(`/api/v1/admin/media-roots/files/${fileId}/match`, {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

export async function addMediaRootFileToLibrary(fileId: string): Promise<{ status: string }> {
  return request(`/api/v1/admin/media-roots/files/${fileId}/add`, { method: 'POST' });
}

export async function ignoreMediaRootFile(fileId: string): Promise<void> {
  return request(`/api/v1/admin/media-roots/files/${fileId}/ignore`, { method: 'POST' });
}

export async function rematchMediaRootFile(fileId: string): Promise<void> {
  return request(`/api/v1/admin/media-roots/files/${fileId}/rematch`, { method: 'POST' });
}

export async function bulkResolveMediaRootFiles(fileIds: string[]): Promise<{ job_id: string }> {
  return request('/api/v1/admin/media-roots/files/bulk-resolve', {
    method: 'POST',
    body: JSON.stringify({ file_ids: fileIds })
  });
}

export async function bulkManualResolveMediaRootFiles(
  fileIds: string[],
  candidate: { provider: string; provider_id: string; title: string; year?: number | string | null }
): Promise<{ job_id: string }> {
  return request('/api/v1/admin/media-roots/files/bulk-manual-resolve', {
    method: 'POST',
    body: JSON.stringify({ file_ids: fileIds, ...candidate })
  });
}

export async function matchMediaRootToLibrary(
  rootId: string
): Promise<{ job_id: string; file_count: number }> {
  return request(`/api/v1/admin/media-roots/${rootId}/match-to-library`, { method: 'POST' });
}

export async function bulkRematchMediaRootFiles(fileIds: string[]): Promise<void> {
  return request('/api/v1/admin/media-roots/files/bulk-rematch', {
    method: 'POST',
    body: JSON.stringify({ file_ids: fileIds })
  });
}

export async function bulkIgnoreMediaRootFiles(fileIds: string[]): Promise<void> {
  return request('/api/v1/admin/media-roots/files/bulk-ignore', {
    method: 'POST',
    body: JSON.stringify({ file_ids: fileIds })
  });
}

export async function directoryAction(
  mediaRootId: string,
  directory: string,
  action: 'ignore' | 'rematch'
): Promise<{ count: number }> {
  return request(`/api/v1/admin/media-roots/${mediaRootId}/directory-action`, {
    method: 'POST',
    body: JSON.stringify({ directory, action })
  });
}

export async function linkMediaRootFileToEntity(
  fileId: string,
  data: { entity_type: string; entity_id: string }
): Promise<{ status: string; entity_type: string; entity_id: string }> {
  return request(`/api/v1/admin/media-roots/files/${fileId}/link`, {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

export async function bulkLinkMediaRootFilesToEntity(
  fileIds: string[],
  entityType: string,
  entityId: string
): Promise<{ linked: number }> {
  return request('/api/v1/admin/media-roots/files/bulk-link', {
    method: 'POST',
    body: JSON.stringify({ file_ids: fileIds, entity_type: entityType, entity_id: entityId })
  });
}

export interface MediaRootFileSearchResult {
  id: string;
  file_name: string;
  file_path: string;
  match_status: string;
  entity_type: string | null;
  show_name: string | null;
  movie_title: string | null;
  book_title: string | null;
  series_name: string | null;
}

export async function reinterpretMediaRootFiles(
  rootId: string,
  status?: string
): Promise<{ count: number }> {
  return request(`/api/v1/admin/media-roots/${rootId}/reinterpret`, {
    method: 'POST',
    body: JSON.stringify({ status })
  });
}

export async function searchMediaRootFiles(
  rootId: string,
  query: string,
  limit?: number,
  offset?: number
): Promise<MediaRootFileSearchResult[]> {
  const params: Record<string, string | number> = { q: query };
  if (limit) params.limit = limit;
  if (offset) params.offset = offset;
  return request(`/api/v1/admin/media-roots/${rootId}/files/search${buildQueryString(params)}`);
}

// Video Playback

export async function createPlaybackSession(params: {
  media_type: 'movie' | 'episode' | 'recording';
  file_id: string;
  capabilities: ClientCapabilities;
  audio_track_index?: number;
  subtitle_track_index?: number;
  start_time?: number;
}): Promise<PlaybackSessionResponse> {
  return request('/api/v1/admin/playback/sessions', {
    method: 'POST',
    body: JSON.stringify(params)
  });
}

/**
 * The newest position any of this user's devices saved for the show (or movie)
 * `mediaId` belongs to, plus the scope id `watch_progress` events carry for it.
 */
export async function getPlaybackHandoff(
  mediaType: 'movie' | 'episode',
  mediaId: string
): Promise<PlaybackHandoffResponse> {
  return request(
    `/api/v1/admin/playback/handoff?media_type=${mediaType}&media_id=${encodeURIComponent(mediaId)}`
  );
}

export async function destroyPlaybackSession(sessionId: string): Promise<void> {
  return dedup(`destroySession:${sessionId}`, () =>
    request(`/api/v1/admin/playback/sessions/${sessionId}`, {
      method: 'DELETE'
    })
  );
}

export async function heartbeatSession(sessionId: string): Promise<boolean> {
  try {
    const res = await fetch(`/api/v1/admin/playback/sessions/${sessionId}/heartbeat`, {
      credentials: 'include'
    });
    return res.ok;
  } catch {
    return false;
  }
}

export interface SessionFeed {
  requests_open: number;
  bytes_served: number;
  idle_ms?: number;
  source?: 'local' | 'proxy';
  downloaded_bytes?: number;
  total_bytes?: number;
  download_bps?: number;
}

export interface PlaybackSessionStatus {
  session_id: string;
  state: 'ready' | 'preparing' | 'failed';
  method: string;
  is_live: boolean;
  error?: string;
  message?: string;
  feed?: SessionFeed;
  source_readable?: boolean;
  preferred_audio_stream_index?: number;
}

export type SessionStatusResult =
  { kind: 'status'; status: PlaybackSessionStatus } | { kind: 'missing' } | { kind: 'unknown' };

export async function getPlaybackSessionStatus(sessionId: string): Promise<SessionStatusResult> {
  try {
    const status = await request<PlaybackSessionStatus>(
      `/api/v1/admin/playback/sessions/${sessionId}/status`
    );
    return { kind: 'status', status };
  } catch (e) {
    if (e instanceof ApiError && (e.status === 404 || e.status === 410)) return { kind: 'missing' };
    return { kind: 'unknown' };
  }
}

export async function seekPlayback(
  sessionId: string,
  timeSeconds: number
): Promise<{
  seek_generation: number;
  from_cache: boolean;
  start_time: number;
  sub_segment_offset: number;
}> {
  return request(`/api/v1/admin/playback/sessions/${sessionId}/seek`, {
    method: 'POST',
    body: JSON.stringify({ time_seconds: timeSeconds })
  });
}

/**
 * Tier-3 corruption recovery: ask the server to flip the session into transcode
 * mode and restart from the keyframe at-or-before `atSeconds` so the player can
 * reload its manifest and resume on a clean re-encoded stream.
 *
 * Resolves to `{ seek_generation }` on success — the caller appends `?g=N` to
 * the playlist URL and reloads it. Throws on 4xx/5xx; in particular 409 means
 * the session is already transcoding (don't loop).
 */
export async function reportPlaybackDecodeError(
  sessionId: string,
  atSeconds: number
): Promise<{
  seek_generation: number;
  start_time: number;
  force_video_transcode: boolean;
}> {
  return request(`/api/v1/admin/playback/sessions/${sessionId}/decode-error`, {
    method: 'POST',
    body: JSON.stringify({ at_seconds: atSeconds })
  });
}

export async function switchAudioTrack(
  sessionId: string,
  audioStreamIndex: number | null,
  resumeTime?: number,
  burnInSubtitleIndex?: number
): Promise<PlaybackSessionResponse> {
  return request(`/api/v1/admin/playback/sessions/${sessionId}/switch-audio`, {
    method: 'POST',
    body: JSON.stringify({
      audio_stream_index: audioStreamIndex,
      resume_time: resumeTime,
      burn_in_subtitle_index: burnInSubtitleIndex
    })
  });
}

export async function downloadSubtitleToSession(
  sessionId: string,
  params: {
    provider: string;
    subtitle_id: string;
    language?: string;
    is_forced?: boolean;
    is_hearing_impaired?: boolean;
  }
): Promise<PlaybackSubtitleTrack> {
  return request(`/api/v1/admin/playback/sessions/${sessionId}/subtitles`, {
    method: 'POST',
    body: JSON.stringify(params)
  });
}

// Sports Leagues (reuses TV data model: TvShow, TvSeason, TvEpisode)

export async function getSportsLeagues(options?: {
  search?: string;
  is_monitored?: boolean;
  status?: string;
  cursor?: string;
  direction?: string;
  around?: string;
  around_before?: number;
  sort?: string;
  limit?: number;
}): Promise<PaginatedResponse<TvShow>> {
  return request(`/api/v1/admin/sports/leagues${buildQueryString(options ?? {})}`);
}

export async function getSportsLeague(id: string): Promise<TvShow> {
  return request(`/api/v1/admin/sports/leagues/${id}`);
}

export async function createSportsLeague(data: {
  title: string;
  year?: number | null;
  description?: string | null;
  poster_url?: string | null;
  backdrop_url?: string | null;
  identifiers?: Record<string, unknown>;
  genres?: string[];
  network?: string | null;
  media_root_id?: string | null;
}): Promise<{ id: string }> {
  return request('/api/v1/admin/sports/leagues', {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

export async function updateSportsLeague(id: string, data: Record<string, unknown>): Promise<void> {
  return request(`/api/v1/admin/sports/leagues/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(data)
  });
}

export async function deleteSportsLeague(id: string): Promise<void> {
  return request(`/api/v1/admin/sports/leagues/${id}`, { method: 'DELETE' });
}

export async function setSportsLeagueMonitored(
  id: string,
  monitored: boolean,
  autoWantMode?: string
): Promise<void> {
  return request(`/api/v1/admin/sports/leagues/${id}/monitor`, {
    method: 'POST',
    body: JSON.stringify({ is_monitored: monitored, auto_want_mode: autoWantMode ?? 'none' })
  });
}

export async function refreshSportsMetadata(id: string): Promise<void> {
  return request(`/api/v1/admin/sports/leagues/${id}/refresh`, { method: 'POST' });
}

export async function wantAllMissingSports(id: string): Promise<void> {
  return request(`/api/v1/admin/sports/leagues/${id}/want-all-missing`, { method: 'POST' });
}

export async function unwantAllSports(id: string): Promise<UnwantResult> {
  return request(`/api/v1/admin/sports/leagues/${id}/want-all-missing`, { method: 'DELETE' });
}

export function sportsLeaguePosterUrl(id: string): string {
  return `${apiBase}/api/v1/admin/sports/leagues/${id}/poster`;
}

export async function uploadSportsLeaguePoster(id: string, file: File): Promise<void> {
  const response = await fetch(`${apiBase}/api/v1/admin/sports/leagues/${id}/poster`, {
    method: 'POST',
    credentials: 'include',
    body: file
  });
  if (!response.ok) {
    const text = await response.text().catch(() => 'Upload failed');
    throw new ApiError(response.status, text);
  }
}

export async function uploadSportsLeagueBackdrop(id: string, file: File): Promise<void> {
  const response = await fetch(`${apiBase}/api/v1/admin/sports/leagues/${id}/backdrop`, {
    method: 'POST',
    credentials: 'include',
    body: file
  });
  if (!response.ok) {
    const text = await response.text().catch(() => 'Upload failed');
    throw new ApiError(response.status, text);
  }
}

export async function uploadSportsLeagueLogo(id: string, file: File): Promise<void> {
  const response = await fetch(`${apiBase}/api/v1/admin/sports/leagues/${id}/logo`, {
    method: 'POST',
    credentials: 'include',
    body: file
  });
  if (!response.ok) {
    const text = await response.text().catch(() => 'Upload failed');
    throw new ApiError(response.status, text);
  }
}

export async function getSportsLeagueSeasons(
  leagueId: string,
  options?: {
    limit?: number;
    offset?: number;
    sort?: string;
  }
): Promise<{ items: TvSeason[]; total_count: number }> {
  const qs = buildQueryString({
    limit: options?.limit,
    offset: options?.offset,
    sort: options?.sort
  });
  return request(`/api/v1/admin/sports/leagues/${leagueId}/seasons${qs}`);
}

export async function getSportsSeason(id: string): Promise<TvSeason> {
  return request(`/api/v1/admin/sports/seasons/${id}`);
}

export async function createSportsSeason(
  leagueId: string,
  data: {
    season_identifier: string;
    season_sort_order: number;
    title?: string;
    description?: string;
    air_date?: string;
    episode_count?: number;
    is_special?: boolean;
  }
): Promise<{ id: string }> {
  return request(`/api/v1/admin/sports/leagues/${leagueId}/seasons`, {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

export async function updateSportsSeason(id: string, data: Record<string, unknown>): Promise<void> {
  return request(`/api/v1/admin/sports/seasons/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(data)
  });
}

export async function deleteSportsSeason(id: string): Promise<void> {
  return request(`/api/v1/admin/sports/seasons/${id}`, { method: 'DELETE' });
}

export async function wantSportsSeason(id: string): Promise<void> {
  return request(`/api/v1/admin/sports/seasons/${id}/want`, { method: 'POST' });
}

export async function unwantSportsSeason(id: string): Promise<UnwantResult> {
  return request(`/api/v1/admin/sports/seasons/${id}/want`, { method: 'DELETE' });
}

export async function setSportsSeasonAutoWant(id: string, mode: string): Promise<void> {
  return request(`/api/v1/admin/sports/seasons/${id}/auto-want`, {
    method: 'POST',
    body: JSON.stringify({ auto_want_mode: mode })
  });
}

export function sportsSeasonPosterUrl(id: string): string {
  return `${apiBase}/api/v1/admin/sports/seasons/${id}/poster`;
}

export async function uploadSportsSeasonPoster(id: string, file: File): Promise<void> {
  const bytes = await file.arrayBuffer();
  await fetch(`${apiBase}/api/v1/admin/sports/seasons/${id}/poster`, {
    method: 'POST',
    credentials: 'include',
    headers: { 'Content-Type': 'application/octet-stream' },
    body: bytes
  });
}

export async function getSportsEvent(id: string): Promise<TvEpisode> {
  return request(`/api/v1/admin/sports/events/${id}`);
}

export async function createSportsEvent(
  seasonId: string,
  data: {
    episode_identifier: string;
    episode_sort_order: number;
    title?: string;
    description?: string;
    air_date?: string;
    runtime?: number;
  }
): Promise<{ id: string }> {
  return request(`/api/v1/admin/sports/seasons/${seasonId}/events`, {
    method: 'POST',
    body: JSON.stringify(data)
  });
}

export async function updateSportsEvent(id: string, data: Record<string, unknown>): Promise<void> {
  return request(`/api/v1/admin/sports/events/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(data)
  });
}

export async function deleteSportsEvent(id: string): Promise<void> {
  return request(`/api/v1/admin/sports/events/${id}`, { method: 'DELETE' });
}

export function sportsEventStillUrl(id: string): string {
  return `${apiBase}/api/v1/admin/sports/events/${id}/still`;
}

export async function uploadSportsEventStill(id: string, file: File): Promise<void> {
  const bytes = await file.arrayBuffer();
  await fetch(`${apiBase}/api/v1/admin/sports/events/${id}/still`, {
    method: 'POST',
    credentials: 'include',
    headers: { 'Content-Type': 'application/octet-stream' },
    body: bytes
  });
}

export async function wantSportsEvent(id: string): Promise<void> {
  return request(`/api/v1/admin/sports/events/${id}/want`, { method: 'POST' });
}

export async function unwantSportsEvent(id: string): Promise<void> {
  return request(`/api/v1/admin/sports/events/${id}/want`, { method: 'DELETE' });
}

export async function searchSportsEventReleases(
  id: string,
  query?: string,
  scope?: string
): Promise<LibrarySearchResponse> {
  const search = new URLSearchParams();
  if (query) search.set('q', query);
  if (scope) search.set('scope', scope);
  const params = search.toString() ? `?${search.toString()}` : '';
  return request(`/api/v1/admin/sports/events/${id}/releases${params}`);
}

export async function queueSportsEventRelease(
  id: string,
  source: string,
  release: Record<string, unknown>
): Promise<void> {
  return request(`/api/v1/admin/sports/events/${id}/queue`, {
    method: 'POST',
    body: JSON.stringify({ release, source })
  });
}

export async function getSportsEventFiles(id: string): Promise<TvEpisodeFile[]> {
  return request(`/api/v1/admin/sports/events/${id}/files`);
}

export async function moveSportsEventFile(
  eventId: string,
  fileId: string,
  targetEventId: string
): Promise<void> {
  await request(`/api/v1/admin/sports/events/${eventId}/files/${fileId}/move`, {
    method: 'POST',
    body: JSON.stringify({ target_event_id: targetEventId })
  });
}

export async function searchSportsLeagues(query: string): Promise<TvSearchResult[]> {
  const params: Record<string, string | undefined> = { q: query };
  const resp = await request<{ results: TvSearchResult[] }>(
    `/api/v1/admin/sports/search${buildQueryString(params)}`
  );
  return resp.results;
}

export async function getSportsLeagueFirstByLetter(
  letter: string
): Promise<{ id: string; title: string } | null> {
  try {
    return await request(
      `/api/v1/admin/sports/leagues/first-by-letter${buildQueryString({ letter })}`
    );
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) return null;
    throw e;
  }
}

export async function getSportsLeagueAvailableLetters(): Promise<string[]> {
  const resp = await request<{ letters: string[] }>(
    '/api/v1/admin/sports/leagues/available-letters'
  );
  return resp.letters;
}

// Notifications

export interface NotificationTrigger {
  id: string;
  display_name: string;
  description: string;
}

export interface NotificationConfigField {
  key: string;
  label: string;
  field_type: 'string' | 'secret' | 'url' | 'number';
  required: boolean;
  description?: string;
  help_url?: string;
  default?: string;
}

export interface NotificationAgentType {
  id: string;
  display_name: string;
  description: string;
  supported_triggers: string[];
  config_fields: NotificationConfigField[];
}

export interface NotificationAgent {
  id: string;
  agent_type: string;
  name: string;
  enabled: boolean;
  config: Record<string, unknown>;
  triggers: string[];
  created_at: string;
  updated_at: string;
  display_name: string;
  description: string;
}

export interface CreateNotificationAgentRequest {
  agent_type: string;
  name: string;
  enabled?: boolean;
  config: Record<string, unknown>;
  triggers: string[];
}

export interface UpdateNotificationAgentRequest {
  name?: string;
  enabled?: boolean;
  config?: Record<string, unknown>;
  triggers?: string[];
}

export async function listNotificationTypes(): Promise<NotificationAgentType[]> {
  return request('/api/v1/admin/notifications/types');
}

export async function listNotificationAgents(): Promise<NotificationAgent[]> {
  return request('/api/v1/admin/notifications/agents');
}

export async function createNotificationAgent(
  input: CreateNotificationAgentRequest
): Promise<NotificationAgent> {
  return request('/api/v1/admin/notifications/agents', {
    method: 'POST',
    body: JSON.stringify(input)
  });
}

export async function updateNotificationAgent(
  id: string,
  input: UpdateNotificationAgentRequest
): Promise<NotificationAgent> {
  return request(`/api/v1/admin/notifications/agents/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(input)
  });
}

export async function deleteNotificationAgent(id: string): Promise<void> {
  return request(`/api/v1/admin/notifications/agents/${id}`, {
    method: 'DELETE'
  });
}

export async function testNotificationAgent(id: string): Promise<void> {
  return request(`/api/v1/admin/notifications/agents/${id}/test`, {
    method: 'POST'
  });
}

export async function getVapidKey(): Promise<string> {
  const data = await request<{ public_key: string }>(
    '/api/v1/admin/notifications/webpush/vapid-key'
  );
  return data.public_key;
}

export async function subscribeWebPush(subscription: {
  endpoint: string;
  p256dh: string;
  auth: string;
  user_agent?: string;
}): Promise<void> {
  return request('/api/v1/admin/notifications/webpush/subscribe', {
    method: 'POST',
    body: JSON.stringify(subscription)
  });
}

export async function unsubscribeWebPush(endpoint: string): Promise<void> {
  return request('/api/v1/admin/notifications/webpush/subscribe', {
    method: 'DELETE',
    body: JSON.stringify({ endpoint })
  });
}

// Notification Inbox (in-app)

import type { UserNotification } from '#lib/types.js';

export interface NotificationInboxResponse {
  notifications: UserNotification[];
  total: number;
}

export async function getNotificationInbox(
  params: {
    limit?: number;
    offset?: number;
    unread_only?: boolean;
  } = {}
): Promise<NotificationInboxResponse> {
  const query = new URLSearchParams();
  if (params.limit != null) query.set('limit', String(params.limit));
  if (params.offset != null) query.set('offset', String(params.offset));
  if (params.unread_only) query.set('unread_only', 'true');
  const qs = query.toString();
  return request(`/api/v1/admin/notifications/inbox${qs ? `?${qs}` : ''}`);
}

export async function getUnreadNotificationCount(): Promise<number> {
  const data = await request<{ count: number }>('/api/v1/admin/notifications/inbox/unread-count');
  return data.count;
}

export async function markNotificationRead(id: string): Promise<void> {
  return request(`/api/v1/admin/notifications/inbox/${id}/read`, {
    method: 'PATCH'
  });
}

export async function markAllNotificationsRead(): Promise<void> {
  return request('/api/v1/admin/notifications/inbox/mark-all-read', {
    method: 'POST'
  });
}

// Migration (Sonarr/Radarr import)

export async function testMigrationConnection(
  service: MigrationService,
  url: string,
  apiKey: string
): Promise<{ version: string }> {
  return request('/api/v1/admin/migration/test', {
    method: 'POST',
    body: JSON.stringify({ service, url, api_key: apiKey })
  });
}

export async function fetchMigrationPreview(
  service: MigrationService
): Promise<MigrationPreviewResponse> {
  return request('/api/v1/admin/migration/preview', {
    method: 'POST',
    body: JSON.stringify({ service })
  });
}

export async function startMigrationImport(params: {
  service: MigrationService;
  tmdb_ids: string[];
  root_mapping: Record<string, string>;
}): Promise<MigrationImportResponse> {
  return request('/api/v1/admin/migration/import', {
    method: 'POST',
    body: JSON.stringify(params)
  });
}

// Audiobook API

// Browsers seek a VBR mp3 by estimate and then report the requested time; the server
// serves mp3s in mp4 for this flag and leaves every other format as it is.
export function getAudioStreamUrl(bookId: string, fileId: string): string {
  return `/api/v1/admin/books/${bookId}/audio/${fileId}?container=mp4`;
}

/** Get chapters for an audiobook file. */
export async function getAudiobookChapters(
  bookId: string,
  fileId: string
): Promise<AudiobookChapter[]> {
  return request(`/api/v1/admin/books/${bookId}/audio/${fileId}/chapters`);
}

/** Get saved audiobook playback position. */
export async function getAudioPosition(
  bookId: string
): Promise<{ file_id: string; position_seconds: number; progress_percent: number } | null> {
  try {
    return await request(`/api/v1/admin/books/${bookId}/audio-position`);
  } catch {
    return null;
  }
}

/** Save audiobook playback position. */
export async function saveAudioPosition(
  bookId: string,
  data: { file_id: string; position_seconds: number; progress_percent: number }
): Promise<void> {
  await request(`/api/v1/admin/books/${bookId}/audio-position`, {
    method: 'PUT',
    body: JSON.stringify(data)
  });
}

// Audiobook Chapter Management

import type { UnifiedChapter } from '#lib/types.js';

/** Get unified chapter timeline for a book (all files, with global timestamps). */
export async function getBookChapters(bookId: string): Promise<UnifiedChapter[]> {
  return await request(`/api/v1/admin/books/${bookId}/chapters`);
}

/** Bulk replace all chapters for a single audio file. */
export async function replaceFileChapters(
  bookId: string,
  fileId: string,
  chapters: Array<{
    chapter_index: number;
    title: string | null;
    start_seconds: number;
    end_seconds: number;
  }>
): Promise<void> {
  await request(`/api/v1/admin/books/${bookId}/audio/${fileId}/chapters`, {
    method: 'PUT',
    body: JSON.stringify(chapters)
  });
}

/** Update a single chapter. */
export async function updateChapter(
  bookId: string,
  chapterId: string,
  data: { title: string | null; start_seconds: number; end_seconds: number }
): Promise<void> {
  await request(`/api/v1/admin/books/${bookId}/chapters/${chapterId}`, {
    method: 'PATCH',
    body: JSON.stringify(data)
  });
}

/** Delete a single chapter. */
export async function deleteChapter(bookId: string, chapterId: string): Promise<void> {
  await request(`/api/v1/admin/books/${bookId}/chapters/${chapterId}`, {
    method: 'DELETE'
  });
}

/** Auto-generate one chapter per file for multi-file audiobooks. */
export async function generateChapters(
  bookId: string,
  force = false
): Promise<{ generated: number }> {
  const query = force ? '?force=true' : '';
  return await request(`/api/v1/admin/books/${bookId}/generate-chapters${query}`, {
    method: 'POST'
  });
}

/** Enqueue audiobook probe job (ffprobe + chapter extraction). */
export async function probeAudioFiles(bookId: string): Promise<void> {
  await request(`/api/v1/admin/books/${bookId}/probe-audio`, {
    method: 'POST'
  });
}

/** Fetch chapters from Audnexus by ASIN. */
export async function fetchAudnexusChapters(
  bookId: string,
  asin: string
): Promise<UnifiedChapter[]> {
  return await request(`/api/v1/admin/books/${bookId}/fetch-chapters`, {
    method: 'POST',
    body: JSON.stringify({ asin })
  });
}

// Transcription & Text Search

export interface TranscriptSegment {
  id: string;
  file_id: string;
  segment_index: number;
  start_seconds: number;
  end_seconds: number;
  text: string;
  chapter_title?: string;
}

export interface TranscriptSearchResult {
  id: string;
  book_id: string;
  source_type: 'audio' | 'ebook';
  file_id: string;
  segment_index: number;
  start_seconds?: number;
  cfi?: string;
  text: string;
  book_title: string;
  chapter_title?: string;
}

export interface SyncPositionResult {
  cfi?: string;
  file_id?: string;
  seconds?: number;
  text_snippet: string;
  confidence: number;
}

/** Enqueue audiobook transcription job. */
export async function transcribeAudiobook(bookId: string): Promise<void> {
  await request(`/api/v1/admin/books/${bookId}/transcribe`, {
    method: 'POST'
  });
}

/** Enqueue ebook text extraction job. */
export async function extractEbookText(bookId: string): Promise<void> {
  await request(`/api/v1/admin/books/${bookId}/extract-text`, {
    method: 'POST'
  });
}

export async function getTranscriptBacklog(): Promise<TranscriptBacklog> {
  return await request('/api/v1/admin/books/transcript-backlog');
}

/** Re-transcribe every missing or outdated transcript, a day of audio per batch. */
export async function startTranscriptRefresh(): Promise<{ job_id: string }> {
  return await request('/api/v1/admin/books/bulk-transcribe', {
    method: 'POST'
  });
}

/** Bulk chapter detection for all books with transcripts. */
export async function bulkChapterDetect(): Promise<{ queued: number }> {
  return await request('/api/v1/admin/books/bulk-chapter-detect', {
    method: 'POST'
  });
}

/** Bulk chapter re-detection for books with detected chapters. */
export async function bulkRedetectChapters(): Promise<{ queued: number }> {
  return await request('/api/v1/admin/books/bulk-redetect-chapters', {
    method: 'POST'
  });
}

/** Detect chapters for a single book (silence analysis). */
export async function detectChapters(bookId: string): Promise<void> {
  return await request(`/api/v1/admin/books/${bookId}/detect-chapters`, {
    method: 'POST'
  });
}

/** Detect text-cue chapters from transcript for a single book. */
export async function detectTextChapters(bookId: string): Promise<void> {
  return await request(`/api/v1/admin/books/${bookId}/detect-text-chapters`, {
    method: 'POST'
  });
}

/** Bulk ebook text extraction for all books with ebook files but no segments. */
export async function bulkEbookExtract(): Promise<{ queued: number }> {
  return await request('/api/v1/admin/books/bulk-ebook-extract', {
    method: 'POST'
  });
}

/**
 * Check every converted ebook for damage an older conversion left behind, and
 * re-make the broken ones. Pass `dryRun` to report without rewriting anything.
 * Runs as a job; the result lists what it found.
 */
export async function repairEbooks(dryRun = false): Promise<{ job_id: string; dry_run: boolean }> {
  return await request(`/api/v1/admin/books/repair-ebooks?dry_run=${dryRun}`, {
    method: 'POST'
  });
}

export async function repairAudiobookChapters(
  dryRun = false
): Promise<{ job_id: string; dry_run: boolean }> {
  return await request(`/api/v1/admin/books/repair-audiobook-chapters?dry_run=${dryRun}`, {
    method: 'POST'
  });
}

/**
 * Run the KEPUB catch-up now rather than waiting for its schedule. `outstanding`
 * is how many synced books were missing a KEPUB when asked. The Kobo prefers
 * KEPUB, so those books re-download — and full-page images come back scaled to
 * the screen rather than sitting at their own pixel size.
 */
export async function kepubifySynced(): Promise<{ job_id: string; outstanding: number }> {
  return await request('/api/v1/admin/books/kepubify-synced', {
    method: 'POST'
  });
}

/** Check if a book has transcript segments. */
export async function hasTranscript(bookId: string): Promise<{
  has_audio_transcript: boolean;
  has_ebook_segments: boolean;
  transcript_mode: string;
  full_cast_available: boolean;
}> {
  return await request(`/api/v1/admin/books/${bookId}/transcript/status`);
}

/** Get all transcript segments for a book. */
export async function getTranscript(bookId: string): Promise<TranscriptSegment[]> {
  return await request(`/api/v1/admin/books/${bookId}/transcript`);
}

/** Search transcript segments for a book (hybrid keyword + semantic). */
export async function searchTranscript(
  bookId: string,
  query: string
): Promise<TranscriptSearchResult[]> {
  const q = encodeURIComponent(query);
  return await request(`/api/v1/admin/books/${bookId}/transcript/search?q=${q}`);
}

/** Search transcripts across all books. */
export async function searchAllTranscripts(query: string): Promise<TranscriptSearchResult[]> {
  const q = encodeURIComponent(query);
  return await request(`/api/v1/admin/search/transcripts?q=${q}`);
}

// TTS

export interface TtsVoice {
  id: string;
  name: string;
}

export interface TtsStatus {
  enabled: boolean;
  service_url: string | null;
  voices: TtsVoice[];
  default_voice: string | null;
  /** Backend label — drives engine-specific UI sections in the voice page. */
  engine: 'chatterbox' | 'breeze' | 'kokoro' | 'openai' | 'auto';
  /** Whether the TTS service supports emotion control and voice cloning (chatterbox + breeze). */
  supports_emotion: boolean;
  /** Whether the engine supports word-level forced alignment (read-along
   *  highlight). Breeze only — it is the one engine with an aligner. */
  supports_alignment: boolean;
}

// Read-Along

export interface ReadAlongSyncPoint {
  audio_file_id: string;
  audio_seconds: number;
  ebook_file_id: string;
  ebook_cfi: string;
  /** Where the point falls in the app's reader pack. */
  position: { chapter: number; block: number; offset: number } | null;
  confidence: number;
}

export interface ReadAlongTranscriptSegment {
  id: string;
  file_id: string;
  segment_index: number;
  start_seconds: number;
  end_seconds: number;
  text: string;
}

export interface ReadAlongData {
  sync_points: ReadAlongSyncPoint[];
  transcript_segments: ReadAlongTranscriptSegment[];
  audio_file_ids: string[];
  has_ebook_segments: boolean;
}

/** Get TTS engine status and available voices. */
export async function getTtsStatus(): Promise<TtsStatus> {
  return await request('/api/v1/admin/tts/status');
}

/** Synthesize speech from text. Returns raw Response for streaming. */
export async function synthesizeTts(
  text: string,
  voice?: string,
  params?: { temperature?: number; speed?: number; bookId?: string },
  signal?: AbortSignal
): Promise<Response> {
  // `speed` renders server-side, so the audio and the word timings both arrive
  // in that timebase — the player must not re-apply it. A rate the engine
  // refuses comes back as 1.0 on `paragraph_start.speed` / `sentence.speed`.
  const body: Record<string, string | number> = { text };
  if (voice !== undefined && voice !== '') body.voice = voice;
  if (params?.temperature !== undefined) body.temperature = params.temperature;
  // Always explicit, including 1: kobbler falls back to the active preset's
  // stored speed when the field is absent, so omitting it would let a 1.5x
  // preset override a reader who deliberately chose 1x. Dropping it at natural
  // rate is the ENGINE hop's job, not this one.
  if (params?.speed !== undefined) body.speed = params.speed;
  // The book's full cast, once any of its voices is approved, reads the page.
  if (params?.bookId) body.book_id = params.bookId;

  const response = await fetch(`${apiBase}/api/v1/admin/tts/synthesize`, {
    method: 'POST',
    credentials: 'include',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
    signal
  });

  if (!response.ok) {
    const msg = await response.text().catch(() => 'TTS synthesis failed');
    throw new ApiError(response.status, msg);
  }

  return response;
}

/** Get read-along data (sync points + transcript) for a book. */
export async function getReadAlongData(bookId: string): Promise<ReadAlongData> {
  return await request(`/api/v1/admin/books/${bookId}/read-along-data`);
}

/** Get transcript segments for a book (read-along format). */
export async function getTranscriptSegments(bookId: string): Promise<ReadAlongTranscriptSegment[]> {
  return await request(`/api/v1/admin/books/${bookId}/transcript/segments`);
}

// Audiobook Bookmarks

export interface AudiobookBookmark {
  id: string;
  audiobook_file_id: string | null;
  position_seconds: number;
  note: string | null;
  created_at: string;
}

/** List all bookmarks for a book. */
export async function listBookmarks(bookId: string): Promise<AudiobookBookmark[]> {
  return request(`/api/v1/admin/books/${bookId}/bookmarks`);
}

/** Create a bookmark at a position in a book. */
export async function createBookmark(
  bookId: string,
  data: {
    file_id?: string;
    position_seconds: number;
    note?: string;
  }
): Promise<{ id: string }> {
  return request(`/api/v1/admin/books/${bookId}/bookmarks`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data)
  });
}

/** Delete a bookmark. */
export async function deleteBookmark(bookId: string, bookmarkId: string): Promise<void> {
  await request(`/api/v1/admin/books/${bookId}/bookmarks/${bookmarkId}`, {
    method: 'DELETE'
  });
}

// Voice Presets

export interface UserVoicePreset {
  id: string;
  name: string;
  voice: string | null;
  clip_filename: string | null;
  /** Playback rate. Stored here but never sent to the engine, which ignores
   *  it — the player applies it and divides the word timings to match. */
  speed: number;
  temperature: number | null;
  /** Transcript of the reference clip — raises clone fidelity. NULL on
   *  chatterbox presets. */
  ref_text: string | null;
  /** Voice-design description. Only meaningful without a cloned voice. */
  instructions: string | null;
  /** Built-in narrator token "S0".."S9". */
  speaker: string | null;
  seed: number | null;
  top_p: number | null;
  top_k: number | null;
  repetition_penalty: number | null;
  /** 12.5 Hz FRAMES, not audio tokens. */
  max_new_frames: number | null;
  created_at: string;
}

/** List all voice presets for the current user. */
export async function listVoicePresets(): Promise<UserVoicePreset[]> {
  return await request('/api/v1/admin/tts/presets');
}

/**
 * Field shape shared by create + update. The engine-native fields are all
 * optional; absent = use server default. On update, `null` clears to NULL.
 */
export interface VoicePresetWritable {
  name?: string;
  voice?: string | null;
  speed?: number;
  temperature?: number | null;
  ref_text?: string | null;
  instructions?: string | null;
  speaker?: string | null;
  seed?: number | null;
  top_p?: number | null;
  top_k?: number | null;
  repetition_penalty?: number | null;
  max_new_frames?: number | null;
}

/** Create a new voice preset. */
export async function createVoicePreset(
  data: VoicePresetWritable & { name: string }
): Promise<UserVoicePreset> {
  return await request('/api/v1/admin/tts/presets', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data)
  });
}

/** Update an existing voice preset. */
export async function updateVoicePreset(
  presetId: string,
  data: VoicePresetWritable
): Promise<UserVoicePreset> {
  return await request(`/api/v1/admin/tts/presets/${presetId}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data)
  });
}

/**
 * Run the saved clip through STT and return the transcript. Used by the voice
 * page to autofill `ref_text` after a clip lands. No persistence — caller
 * decides whether to save via `updateVoicePreset({ ref_text })`.
 */
export async function transcribePresetClip(presetId: string): Promise<{ text: string }> {
  return await request(`/api/v1/admin/tts/presets/${presetId}/transcribe-clip`, {
    method: 'POST'
  });
}

/** Sampler params for the seed-audition flow. All optional. */
export interface PreviewSampleParams {
  voice?: string;
  /** Render rate. Rendered server-side, so do NOT also set `playbackRate`. */
  speed?: number;
  temperature?: number;
  top_p?: number;
  top_k?: number;
  repetition_penalty?: number;
  seed?: number;
  instructions?: string;
  speaker?: string;
  max_new_frames?: number;
  /** "wav" | "mp3" — defaults to mp3. */
  response_format?: string;
}

export interface PreviewSample {
  blob: Blob;
  /// Rate the sample was actually rendered at. An engine that can't do the
  /// requested rate renders at 1 instead of failing, so this is the only
  /// honest answer — don't assume the request took.
  speedApplied: number;
}

/**
 * Synthesise a one-shot sample. Used pre-save by the voice page to audition
 * different seeds. Returns the audio blob; nothing is persisted server-side.
 */
export async function previewTtsSample(
  text: string,
  params: PreviewSampleParams = {},
  signal?: AbortSignal
): Promise<PreviewSample> {
  const body: Record<string, unknown> = { text, ...params };
  const response = await fetch(`${apiBase}/api/v1/admin/tts/preview-sample`, {
    method: 'POST',
    credentials: 'include',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
    signal
  });
  if (!response.ok) {
    const msg = await response.text().catch(() => 'preview-sample failed');
    throw new ApiError(response.status, msg);
  }
  const speedHeader = Number(response.headers.get('x-speed-applied'));
  return {
    blob: await response.blob(),
    speedApplied: Number.isFinite(speedHeader) && speedHeader > 0 ? speedHeader : 1
  };
}

/** Delete a voice preset. */
export async function deleteVoicePreset(presetId: string): Promise<void> {
  await request(`/api/v1/admin/tts/presets/${presetId}`, { method: 'DELETE' });
}

/** Upload a voice clip for a preset. */
export async function uploadVoiceClip(presetId: string, file: File): Promise<void> {
  const formData = new FormData();
  formData.append('clip', file);
  await fetch(`${apiBase}/api/v1/admin/tts/presets/${presetId}/clip`, {
    method: 'POST',
    credentials: 'include',
    body: formData
  }).then(async (r) => {
    if (!r.ok) throw new ApiError(r.status, await r.text().catch(() => 'Upload failed'));
  });
}

/** Delete a voice clip from a preset. */
export async function deleteVoiceClip(presetId: string): Promise<void> {
  await request(`/api/v1/admin/tts/presets/${presetId}/clip`, { method: 'DELETE' });
}

/** Get URL for a voice clip audio preview. */
export function getVoiceClipUrl(presetId: string): string {
  return `${apiBase}/api/v1/admin/tts/presets/${presetId}/clip`;
}

/** Get the current user's active voice preset. */
export async function getActivePreset(): Promise<UserVoicePreset | null> {
  return await request('/api/v1/admin/tts/presets/active');
}

/** Set the active voice preset for the current user. */
export async function setActivePreset(presetId: string | null): Promise<void> {
  await request('/api/v1/admin/tts/presets/active', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ preset_id: presetId })
  });
}

export interface AudiobookSummary {
  id: string;
  title: string;
  author?: string | null;
  narrator?: string | null;
  cover_url?: string | null;
}

export interface AudiobookChapterRef {
  index: number;
  title: string;
  file_path: string;
  duration_seconds?: number | null;
}

export interface AudiobookDetail {
  id: string;
  title: string;
  author?: string | null;
  narrator?: string | null;
  chapters: AudiobookChapterRef[];
}

export interface AudiobookClipSuggestion {
  audiobook_id: string;
  chapter_index: number;
  offset_seconds: number;
  duration_seconds: number;
  chapter_total_seconds: number;
}

export async function searchAudiobooks(q: string): Promise<AudiobookSummary[]> {
  const qs = new URLSearchParams({ q });
  return await request(`/api/v1/library/audiobooks?${qs}`);
}

export async function getAudiobook(id: string): Promise<AudiobookDetail> {
  return await request(`/api/v1/library/audiobooks/${encodeURIComponent(id)}`);
}

export async function suggestAudiobookClip(args: {
  audiobookId: string;
  chapterIndex?: number;
  durationSeconds?: number;
}): Promise<AudiobookClipSuggestion> {
  const qs = new URLSearchParams({ audiobook_id: args.audiobookId });
  if (args.chapterIndex != null) qs.set('chapter_index', String(args.chapterIndex));
  if (args.durationSeconds != null) qs.set('duration_seconds', String(args.durationSeconds));
  return await request(`/api/v1/admin/tts/audiobook-clip/suggest?${qs}`);
}

/** URL for streaming a candidate audiobook clip — bind to <audio src=...>. */
export function audiobookClipPreviewUrl(args: {
  audiobookId: string;
  chapterIndex: number;
  offsetSeconds: number;
  durationSeconds: number;
}): string {
  const qs = new URLSearchParams({
    audiobook_id: args.audiobookId,
    chapter_index: String(args.chapterIndex),
    offset_seconds: String(args.offsetSeconds),
    duration_seconds: String(args.durationSeconds)
  });
  return `${apiBase}/api/v1/admin/tts/audiobook-clip?${qs}`;
}

export async function createPresetClipFromAudiobook(
  presetId: string,
  args: {
    audiobookId: string;
    chapterIndex: number;
    offsetSeconds: number;
    durationSeconds: number;
  }
): Promise<UserVoicePreset> {
  return await request(`/api/v1/admin/tts/presets/${presetId}/clip-from-audiobook`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      audiobook_id: args.audiobookId,
      chapter_index: args.chapterIndex,
      offset_seconds: args.offsetSeconds,
      duration_seconds: args.durationSeconds
    })
  });
}

/** Get cross-format sync position. */
export async function getSyncPosition(
  bookId: string,
  params: { from: 'audio'; seconds: number; fileId?: string } | { from: 'ebook'; cfi: string }
): Promise<SyncPositionResult | null> {
  const searchParams = new URLSearchParams({ from: params.from });
  if (params.from === 'audio') {
    searchParams.set('seconds', String(params.seconds));
    if (params.fileId) searchParams.set('file_id', params.fileId);
  } else {
    searchParams.set('cfi', params.cfi);
  }
  try {
    return await request(`/api/v1/admin/books/${bookId}/sync-position?${searchParams}`);
  } catch {
    return null;
  }
}

// Discover (trending/popular content)

export interface DiscoverItem {
  provider: string;
  provider_id: string;
  media_type: 'movie' | 'tv' | 'book';
  title: string;
  year?: number | null;
  description?: string | null;
  poster_url?: string | null;
  backdrop_url?: string | null;
  genres?: string[];
  rating?: number | null;
  rating_count?: number | null;
  authors?: string[];
  identifiers?: Record<string, string>;
  // Library merge fields
  library_id?: string | null;
  want_status?: string | null;
  is_in_library?: boolean | null;
}

export interface DiscoverSection {
  title: string;
  section_key: string;
  sort_order: number;
  items: DiscoverItem[];
}

export interface StremioContinueItem extends DiscoverItem {
  subtitle: string | null;
  season: number | null;
  episode: number | null;
  progress_percent: number | null;
  last_watched: string;
}

export interface ForYouSeed {
  media_type: 'movie' | 'tv';
  tmdb_id: number | null;
  title: string;
}

export interface ForYouItem extends DiscoverItem {
  /** Watched titles that led here, strongest first. */
  because: string[];
  /** The same seeds as `because`, in the same order. */
  because_items: ForYouSeed[];
}

export interface DiscoverResponse {
  watch_next: WatchNextItem[];
  recently_added: DashboardRecentItem[];
  stremio_continue: StremioContinueItem[];
  for_you: ForYouItem[];
  sections: DiscoverSection[];
}

export interface StremioAccount {
  email: string;
  last_synced_at: string | null;
  last_error: string | null;
  item_count: number;
}

export async function getStremioAccount(): Promise<{ account: StremioAccount | null }> {
  return request('/api/v1/admin/stremio/account');
}

export async function linkStremioAccount(
  email: string,
  password: string
): Promise<{ account: StremioAccount | null }> {
  return request('/api/v1/admin/stremio/account', {
    method: 'PUT',
    body: JSON.stringify({ email, password })
  });
}

export async function unlinkStremioAccount(): Promise<void> {
  return request('/api/v1/admin/stremio/account', { method: 'DELETE' });
}

export async function syncStremioAccount(): Promise<void> {
  return request('/api/v1/admin/stremio/account/sync', { method: 'POST' });
}

export interface StremioHistoryItem {
  stremio_id: string;
  media_type: 'movie' | 'tv';
  provider: string;
  provider_id: string;
  title: string;
  year?: number | null;
  poster_url?: string | null;
  progress: string;
  last_watched: string;
  imported: boolean;
  import_error?: string | null;
}

export async function getStremioHistory(): Promise<{
  linked: boolean;
  items: StremioHistoryItem[];
}> {
  return request('/api/v1/admin/stremio/history');
}

export async function importStremioHistory(stremioIds: string[]): Promise<{ selected: number }> {
  return request('/api/v1/admin/stremio/history/imports', {
    method: 'PUT',
    body: JSON.stringify({ stremio_ids: stremioIds })
  });
}

export async function stopStremioImport(stremioId: string): Promise<void> {
  return request(`/api/v1/admin/stremio/history/imports/${encodeURIComponent(stremioId)}`, {
    method: 'DELETE'
  });
}

export interface WatchProviderInfo {
  provider_id: number;
  provider_name: string;
  logo_url?: string | null;
}

export interface DiscoverDetailResponse {
  media_type: string;
  provider: string;
  details: Record<string, unknown>;
  library_id?: string | null;
  want_status?: string | null;
  is_in_library?: boolean | null;
}

export async function getDiscoverLists(): Promise<DiscoverResponse> {
  return request('/api/v1/admin/discover');
}

export interface RecommendationTitle {
  media_type: 'movie' | 'tv';
  tmdb_id: number;
}

export async function dismissForYou(title: RecommendationTitle): Promise<void> {
  return request('/api/v1/admin/discover/for-you/dismiss', {
    method: 'POST',
    body: JSON.stringify(title)
  });
}

export async function undismissForYou(title: RecommendationTitle): Promise<void> {
  return request('/api/v1/admin/discover/for-you/dismiss', {
    method: 'DELETE',
    body: JSON.stringify(title)
  });
}

export type UpNextMediaType = WatchNextItem['media_type'] | 'stremio';

export async function dismissUpNext(mediaType: UpNextMediaType, mediaId: string): Promise<void> {
  return request('/api/v1/admin/up-next/dismiss', {
    method: 'POST',
    body: JSON.stringify({ media_type: mediaType, media_id: mediaId })
  });
}

export type WatchHistorySource = 'local' | 'trakt' | 'stremio';

export interface WatchHistoryItem {
  media_type: 'movie' | 'tv';
  tmdb_id: number | null;
  movie_id?: string | null;
  show_id?: string | null;
  title: string;
  year: number | null;
  poster_url: string | null;
  last_watched_at: string | null;
  sources: WatchHistorySource[];
  excluded: boolean;
}

export async function getWatchHistory(params: {
  source?: WatchHistorySource;
  search?: string;
  limit?: number;
  offset?: number;
}): Promise<{ items: WatchHistoryItem[]; total: number }> {
  const qs = new URLSearchParams();
  if (params.source) qs.set('source', params.source);
  if (params.search) qs.set('search', params.search);
  if (params.limit != null) qs.set('limit', String(params.limit));
  if (params.offset != null) qs.set('offset', String(params.offset));
  return request(`/api/v1/admin/watch-history?${qs}`);
}

export async function setWatchHistoryExcluded(
  items: RecommendationTitle[],
  excluded: boolean
): Promise<void> {
  return request('/api/v1/admin/watch-history/exclude', {
    method: 'POST',
    body: JSON.stringify({ items, excluded })
  });
}

export async function removeWatchHistory(items: RecommendationTitle[]): Promise<void> {
  return request('/api/v1/admin/watch-history/remove', {
    method: 'POST',
    body: JSON.stringify({ items })
  });
}

export async function getDiscoverWatchProviders(
  region?: string
): Promise<{ providers: WatchProviderInfo[]; region: string }> {
  const params = new URLSearchParams();
  if (region) params.set('region', region);
  return request(`/api/v1/admin/discover/watch-providers?${params}`);
}

export async function getDiscoverDetail(
  mediaType: string,
  provider: string,
  providerId: string
): Promise<DiscoverDetailResponse> {
  return request(
    `/api/v1/admin/discover/${encodeURIComponent(mediaType)}/${encodeURIComponent(provider)}/${encodeURIComponent(providerId)}`
  );
}

export interface DiscoverEpisode {
  episode_number: number;
  title: string | null;
  air_date: string | null;
  description: string | null;
  still_url: string | null;
  runtime: number | null;
}

export interface DiscoverSeasonDetail {
  provider: string;
  show_id: string;
  season_number: number;
  title: string | null;
  air_date: string | null;
  description: string | null;
  poster_url: string | null;
  episodes: DiscoverEpisode[];
}

export async function getDiscoverSeason(
  provider: string,
  providerId: string,
  seasonNumber: number
): Promise<DiscoverSeasonDetail> {
  return request(
    `/api/v1/admin/discover/tv/${encodeURIComponent(provider)}/${encodeURIComponent(providerId)}/season/${seasonNumber}`
  );
}

export interface WantDiscoverEpisodeParams {
  provider: string;
  provider_id: string;
  season_number: number;
  episode_number: number;
  title: string;
  year?: number;
  identifiers?: Record<string, string>;
  description?: string;
  poster_url?: string;
  backdrop_url?: string;
  genres?: string[];
  rating?: number;
  rating_count?: number;
  runtime?: number;
  network?: string;
}

export interface WantDiscoverEpisodeResponse {
  show_id: string;
  season_id: string;
  episode_id: string;
}

export async function wantDiscoverEpisode(
  params: WantDiscoverEpisodeParams
): Promise<WantDiscoverEpisodeResponse> {
  return request('/api/v1/admin/discover/want-episode', {
    method: 'POST',
    body: JSON.stringify(params)
  });
}

export type WantDiscoverSeasonParams = Omit<WantDiscoverEpisodeParams, 'episode_number'>;

export interface WantDiscoverSeasonEpisode {
  episode_id: string;
  episode_number: number;
  title: string | null;
}

export interface WantDiscoverSeasonResponse {
  show_id: string;
  season_id: string;
  season_number: number;
  total_episodes: number;
  already_have: number;
  already_wanted: number;
  newly_wanted: number;
  newly_wanted_episodes: WantDiscoverSeasonEpisode[];
  /** False when everything was already present or already wanted, so nothing was re-queued. */
  queued: boolean;
}

export async function wantDiscoverSeason(
  params: WantDiscoverSeasonParams
): Promise<WantDiscoverSeasonResponse> {
  return request('/api/v1/admin/discover/want-season', {
    method: 'POST',
    body: JSON.stringify(params)
  });
}

export async function refreshDiscover(): Promise<void> {
  return request('/api/v1/admin/discover/refresh', { method: 'POST' });
}

// IPTV

export async function getIptvProviders(): Promise<IptvProvider[]> {
  return request('/api/v1/admin/iptv/providers');
}

export async function createIptvProvider(input: {
  name: string;
  url: string;
  username: string;
  password: string;
  preferred_format?: string;
  sync_interval_secs?: number;
}): Promise<IptvProvider> {
  return request('/api/v1/admin/iptv/providers', {
    method: 'POST',
    body: JSON.stringify(input)
  });
}

export async function updateIptvProvider(
  id: string,
  input: {
    name?: string;
    url?: string;
    username?: string;
    password?: string;
    preferred_format?: string;
    sync_interval_secs?: number;
    enabled?: boolean;
  }
): Promise<void> {
  return request(`/api/v1/admin/iptv/providers/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(input)
  });
}

export async function deleteIptvProvider(id: string): Promise<void> {
  return request(`/api/v1/admin/iptv/providers/${id}`, { method: 'DELETE' });
}

export async function syncIptvProvider(id: string): Promise<void> {
  return request(`/api/v1/admin/iptv/providers/${id}/sync`, { method: 'POST' });
}

export async function getIptvChannels(params?: {
  category?: string;
  search?: string;
  language?: string;
  enabled?: boolean;
  limit?: number;
  offset?: number;
}): Promise<{ items: IptvChannel[]; total: number }> {
  return request(`/api/v1/admin/iptv/channels${buildQueryString(params ?? {})}`);
}

export async function getIptvChannel(id: string): Promise<IptvChannel> {
  return request(`/api/v1/admin/iptv/channels/${id}`);
}

export async function getIptvChannelSources(id: string): Promise<IptvSource[]> {
  return request(`/api/v1/admin/iptv/channels/${id}/sources`);
}

export async function toggleIptvChannel(id: string, enabled: boolean): Promise<void> {
  return request(`/api/v1/admin/iptv/channels/${id}`, {
    method: 'PATCH',
    body: JSON.stringify({ enabled })
  });
}

export async function getIptvGroups(): Promise<IptvGroup[]> {
  return request('/api/v1/admin/iptv/groups');
}

export async function getIptvChannelEpg(
  channelId: string,
  params?: {
    from?: string;
    to?: string;
  }
): Promise<IptvEpgEntry[]> {
  const qs: Record<string, string | undefined> = {
    from: params?.from,
    to: params?.to
  };
  return request(`/api/v1/admin/iptv/channels/${channelId}/epg${buildQueryString(qs)}`);
}

export async function getIptvEpgGrid(params?: {
  from?: string;
  to?: string;
  enabled_only?: boolean;
  channel_ids?: string;
}): Promise<{
  grid: Array<{ channel: IptvChannel; programs: IptvEpgEntry[] }>;
  from: string;
  to: string;
}> {
  const qs: Record<string, string | undefined> = {
    from: params?.from,
    to: params?.to,
    enabled_only: params?.enabled_only != null ? String(params.enabled_only) : undefined,
    channel_ids: params?.channel_ids
  };
  return request(`/api/v1/admin/iptv/epg${buildQueryString(qs)}`);
}

export async function getIptvEpgFavourites(params?: { from?: string; to?: string }): Promise<{
  grid: Array<{ channel: IptvChannel; programs: IptvEpgEntry[] }>;
  from: string;
  to: string;
}> {
  const qs: Record<string, string | undefined> = {
    from: params?.from,
    to: params?.to
  };
  return request(`/api/v1/admin/iptv/epg/favourites${buildQueryString(qs)}`);
}

export async function searchIptvEpg(query: string): Promise<IptvEpgEntry[]> {
  return request(`/api/v1/admin/iptv/epg/search${buildQueryString({ query })}`);
}

export async function getIptvEpgNow(params?: { group_id?: string }): Promise<IptvEpgEntry[]> {
  return request(`/api/v1/admin/iptv/epg/now${buildQueryString(params ?? {})}`);
}

export async function getIptvFavourites(): Promise<IptvChannel[]> {
  return request('/api/v1/admin/iptv/favourites');
}

export async function addIptvFavourite(channelId: string): Promise<void> {
  return request('/api/v1/admin/iptv/favourites', {
    method: 'POST',
    body: JSON.stringify({ channel_id: channelId })
  });
}

export async function removeIptvFavourite(channelId: string): Promise<void> {
  return request(`/api/v1/admin/iptv/favourites/${channelId}`, { method: 'DELETE' });
}

export async function getIptvHistory(): Promise<IptvChannel[]> {
  return request('/api/v1/admin/iptv/history');
}

export async function recordIptvWatch(channelId: string, durationSecs?: number): Promise<void> {
  return request('/api/v1/admin/iptv/history', {
    method: 'POST',
    body: JSON.stringify({ channel_id: channelId, duration_secs: durationSecs })
  });
}

export async function createIptvSession(params: {
  channel_id: string;
  source_id?: string;
  capabilities?: ClientCapabilities;
}): Promise<PlaybackSessionResponse> {
  return request('/api/v1/admin/iptv/sessions', {
    method: 'POST',
    body: JSON.stringify(params)
  });
}

export async function getIptvRecordings(status?: string): Promise<IptvRecording[]> {
  return request(`/api/v1/admin/iptv/recordings${buildQueryString({ status })}`);
}

export async function createIptvRecording(input: {
  channel_id: string;
  title: string;
  start_time: string;
  end_time: string;
}): Promise<IptvRecording> {
  return request('/api/v1/admin/iptv/recordings', {
    method: 'POST',
    body: JSON.stringify(input)
  });
}

export async function recordFromEpg(input: {
  epg_id: string;
  pre_padding_secs?: number;
  post_padding_secs?: number;
}): Promise<IptvRecording> {
  return request('/api/v1/admin/iptv/recordings/from-epg', {
    method: 'POST',
    body: JSON.stringify(input)
  });
}

export async function deleteIptvRecording(id: string): Promise<void> {
  return request(`/api/v1/admin/iptv/recordings/${id}`, { method: 'DELETE' });
}

export async function getIptvSeriesRules(): Promise<IptvSeriesRule[]> {
  return request('/api/v1/admin/iptv/series-rules');
}

export async function createIptvSeriesRule(input: {
  title_pattern: string;
  channel_id?: string;
  record_new_only?: boolean;
  keep_count?: number;
}): Promise<IptvSeriesRule> {
  return request('/api/v1/admin/iptv/series-rules', {
    method: 'POST',
    body: JSON.stringify(input)
  });
}

export async function deleteIptvSeriesRule(id: string): Promise<void> {
  return request(`/api/v1/admin/iptv/series-rules/${id}`, { method: 'DELETE' });
}

export async function getIptvUnmatched(
  limit?: number,
  offset?: number
): Promise<{ items: IptvUnmatchedSource[]; total: number }> {
  return request(`/api/v1/admin/iptv/unmatched${buildQueryString({ limit, offset })}`);
}

export async function assignIptvUnmatched(id: string, channelId: string): Promise<void> {
  return request(`/api/v1/admin/iptv/unmatched/${id}/assign`, {
    method: 'POST',
    body: JSON.stringify({ channel_id: channelId })
  });
}

export async function createChannelFromUnmatched(id: string): Promise<IptvChannel> {
  return request(`/api/v1/admin/iptv/unmatched/${id}/create-channel`, { method: 'POST' });
}

export async function dismissIptvUnmatched(id: string): Promise<void> {
  return request(`/api/v1/admin/iptv/unmatched/${id}`, { method: 'DELETE' });
}

export async function getIptvWatches(): Promise<IptvProgrammeWatch[]> {
  return request('/api/v1/admin/iptv/watches');
}

export async function createIptvWatch(input: {
  title_pattern: string;
  match_mode?: string;
}): Promise<{ id: string }> {
  return request('/api/v1/admin/iptv/watches', {
    method: 'POST',
    body: JSON.stringify(input)
  });
}

export async function deleteIptvWatch(id: string): Promise<void> {
  return request(`/api/v1/admin/iptv/watches/${id}`, { method: 'DELETE' });
}

export async function getIptvWatchMatches(): Promise<IptvWatchMatch[]> {
  return request('/api/v1/admin/iptv/watches/matches');
}

export async function getIptvFavouriteGroups(): Promise<string[]> {
  return request('/api/v1/admin/iptv/favourites/groups');
}

export async function addIptvFavouriteGroup(groupId: string): Promise<void> {
  return request(`/api/v1/admin/iptv/favourites/groups/${groupId}`, {
    method: 'POST'
  });
}

export async function removeIptvFavouriteGroup(groupId: string): Promise<void> {
  return request(`/api/v1/admin/iptv/favourites/groups/${groupId}`, {
    method: 'DELETE'
  });
}

export async function mergeIptvChannels(sourceId: string, targetId: string): Promise<void> {
  return request('/api/v1/admin/iptv/channels/merge', {
    method: 'POST',
    body: JSON.stringify({ source_id: sourceId, target_id: targetId })
  });
}

export async function getIptvSettings(): Promise<Record<string, unknown>> {
  return request('/api/v1/admin/iptv/settings');
}

export async function updateIptvSettings(input: Record<string, unknown>): Promise<void> {
  return request('/api/v1/admin/iptv/settings', {
    method: 'PATCH',
    body: JSON.stringify(input)
  });
}

export async function catchupIptv(
  channelId: string,
  epgId: string
): Promise<PlaybackSessionResponse> {
  return request('/api/v1/admin/iptv/catchup', {
    method: 'POST',
    body: JSON.stringify({ channel_id: channelId, epg_id: epgId })
  });
}

// ── Live TV (HDHomeRun tuner) ──

import type {
  LivetvTuner,
  LivetvDiscoveredTuner,
  LivetvChannel,
  LivetvCuratedPatch,
  LivetvFacets,
  LivetvGuideChannel,
  LivetvGuideEntry,
  LivetvGuideNow,
  LivetvBulkAction,
  LivetvLease,
  LivetvSession,
  LivetvBufferState,
  LivetvRecording,
  LivetvRecordingStatus,
  LivetvWatchEventInput,
  LivetvAdBreak,
  LivetvSeriesRule,
  LivetvSeriesRuleInput,
  LivetvSeriesRulePreview,
  LivetvOnNow
} from '#lib/types.js';

export interface LivetvChannelQuery {
  [key: string]: string | number | boolean | undefined;
  search?: string;
  network?: string;
  region?: string;
  kind?: string;
  hd?: boolean;
  enabled?: boolean;
  favourite?: boolean;
  has_guide?: boolean;
  mux?: string;
  with_now?: boolean;
  sort?: string;
  limit?: number;
  offset?: number;
}

export interface LivetvChannelsResponse {
  items: LivetvChannel[];
  total: number;
  facets: LivetvFacets;
}

export interface LivetvGuideQuery {
  [key: string]: string | undefined;
  from?: string;
  to?: string;
  channel_ids?: string;
  search?: string;
  genre?: string;
}

export async function getLivetvTuners(): Promise<{ items: LivetvTuner[] }> {
  return request('/api/v1/admin/livetv/tuners');
}

export async function discoverLivetvTuners(): Promise<{
  candidates?: LivetvDiscoveredTuner[];
}> {
  return request('/api/v1/admin/livetv/tuners', {
    method: 'POST',
    body: JSON.stringify({ discover: true })
  });
}

export async function addLivetvTuner(baseUrl: string): Promise<LivetvTuner> {
  return request('/api/v1/admin/livetv/tuners', {
    method: 'POST',
    body: JSON.stringify({ base_url: baseUrl })
  });
}

export async function deleteLivetvTuner(id: string, force = false): Promise<void> {
  const query = force ? '?force=true' : '';
  return request(`/api/v1/admin/livetv/tuners/${id}${query}`, { method: 'DELETE' });
}

export async function scanLivetvTuner(id: string): Promise<{ job_id: string }> {
  return request(`/api/v1/admin/livetv/tuners/${id}/scan`, { method: 'POST' });
}

export async function syncLivetvGuide(id: string): Promise<{ job_id: string }> {
  return request(`/api/v1/admin/livetv/tuners/${id}/guide-sync`, { method: 'POST' });
}

export async function getLivetvChannels(
  params?: LivetvChannelQuery
): Promise<LivetvChannelsResponse> {
  return request(`/api/v1/admin/livetv/channels${buildQueryString(params ?? {})}`);
}

export async function updateLivetvChannel(
  id: string,
  patch: LivetvCuratedPatch
): Promise<LivetvChannel> {
  return request(`/api/v1/admin/livetv/channels/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(patch)
  });
}

export async function bulkUpdateLivetvChannels(
  ids: string[],
  action: LivetvBulkAction
): Promise<{ updated: number }> {
  return request('/api/v1/admin/livetv/channels/bulk', {
    method: 'POST',
    body: JSON.stringify({ ids, action })
  });
}

export async function getLivetvGuide(
  params?: LivetvGuideQuery
): Promise<{ channels: LivetvGuideChannel[] }> {
  return request(`/api/v1/admin/livetv/guide${buildQueryString(params ?? {})}`);
}

export async function getLivetvGuideNow(params?: {
  channel_ids?: string;
  limit?: number;
}): Promise<{ items: LivetvGuideNow[] }> {
  return request(`/api/v1/admin/livetv/guide/now${buildQueryString(params ?? {})}`);
}

export async function getLivetvGuideEntry(id: string): Promise<LivetvGuideEntry> {
  return request(`/api/v1/admin/livetv/guide/${id}`);
}

export async function getLivetvLeases(): Promise<{
  total: number;
  in_use: number;
  items: LivetvLease[];
}> {
  return request('/api/v1/admin/livetv/leases');
}

export async function createLivetvSession(params: {
  channel_id: string;
  capabilities: ClientCapabilities;
}): Promise<LivetvSession> {
  return request('/api/v1/admin/livetv/sessions', {
    method: 'POST',
    body: JSON.stringify(params)
  });
}

export async function switchLivetvSessionChannel(
  sessionId: string,
  channelId: string
): Promise<LivetvSession> {
  return request(`/api/v1/admin/livetv/sessions/${sessionId}/channel`, {
    method: 'POST',
    body: JSON.stringify({ channel_id: channelId })
  });
}

export async function destroyLivetvSession(sessionId: string): Promise<void> {
  return request(`/api/v1/admin/livetv/sessions/${sessionId}`, { method: 'DELETE' });
}

export async function heartbeatLivetvSession(
  sessionId: string,
  body?: { position_seconds?: number; source?: string }
): Promise<{ ok: boolean }> {
  return request(`/api/v1/admin/livetv/sessions/${sessionId}/heartbeat`, {
    method: 'POST',
    body: JSON.stringify(body ?? {})
  });
}

export async function getLivetvSessionBuffer(sessionId: string): Promise<LivetvBufferState> {
  return request(`/api/v1/admin/livetv/sessions/${sessionId}/buffer`);
}

export async function recordLivetvSession(
  sessionId: string,
  body: { mode: 'programme' | 'open_ended'; guide_id?: string }
): Promise<LivetvRecording> {
  return request(`/api/v1/admin/livetv/sessions/${sessionId}/record`, {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export async function postLivetvWatchEvent(body: LivetvWatchEventInput): Promise<void> {
  return request('/api/v1/admin/livetv/watch-events', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export interface LivetvRecordingQuery {
  [key: string]: string | number | undefined;
  status?: LivetvRecordingStatus;
  from?: string;
  to?: string;
  limit?: number;
  offset?: number;
}

export async function getLivetvRecordings(
  params?: LivetvRecordingQuery
): Promise<{ items: LivetvRecording[]; total: number }> {
  return request(`/api/v1/admin/livetv/recordings${buildQueryString(params ?? {})}`);
}

export async function createLivetvRecording(
  body:
    | { guide_id: string }
    | {
        channel_id: string;
        start_time: string;
        end_time: string;
        title: string;
        padding_before_secs?: number;
        padding_after_secs?: number;
      }
): Promise<LivetvRecording> {
  return request('/api/v1/admin/livetv/recordings', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export async function deleteLivetvRecording(id: string): Promise<void> {
  return request(`/api/v1/admin/livetv/recordings/${id}`, { method: 'DELETE' });
}

export async function stopLivetvRecording(id: string): Promise<LivetvRecording> {
  return request(`/api/v1/admin/livetv/recordings/${id}/stop`, { method: 'POST' });
}

export async function getLivetvRecordingPlayback(id: string): Promise<PlaybackSessionResponse> {
  return request(`/api/v1/admin/livetv/recordings/${id}/playback`);
}

export async function getLivetvAdBreaks(recordingId: string): Promise<{ items: LivetvAdBreak[] }> {
  return request(`/api/v1/admin/livetv/recordings/${recordingId}/ad-breaks`);
}

export async function updateLivetvAdBreak(
  recordingId: string,
  breakId: string,
  body: { start_seconds: number; end_seconds: number }
): Promise<LivetvAdBreak> {
  return request(`/api/v1/admin/livetv/recordings/${recordingId}/ad-breaks/${breakId}`, {
    method: 'PATCH',
    body: JSON.stringify(body)
  });
}

export async function deleteLivetvAdBreak(recordingId: string, breakId: string): Promise<void> {
  return request(`/api/v1/admin/livetv/recordings/${recordingId}/ad-breaks/${breakId}`, {
    method: 'DELETE'
  });
}

export async function getLivetvSeriesRules(): Promise<{ items: LivetvSeriesRule[] }> {
  return request('/api/v1/admin/livetv/series-rules');
}

export async function createLivetvSeriesRule(
  body: LivetvSeriesRuleInput
): Promise<LivetvSeriesRule> {
  return request('/api/v1/admin/livetv/series-rules', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export async function updateLivetvSeriesRule(
  id: string,
  body: Partial<LivetvSeriesRuleInput>
): Promise<LivetvSeriesRule> {
  return request(`/api/v1/admin/livetv/series-rules/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(body)
  });
}

export async function deleteLivetvSeriesRule(id: string): Promise<void> {
  return request(`/api/v1/admin/livetv/series-rules/${id}`, { method: 'DELETE' });
}

export async function previewLivetvSeriesRule(
  body: LivetvSeriesRuleInput
): Promise<LivetvSeriesRulePreview> {
  return request('/api/v1/admin/livetv/series-rules/preview', {
    method: 'POST',
    body: JSON.stringify(body)
  });
}

export async function getLivetvOnNow(): Promise<LivetvOnNow> {
  return request('/api/v1/admin/livetv/on-now');
}

// ── Debrid Streaming ──

export interface StreamOption {
  id: string;
  release_title: string;
  size: number;
  indexer: string;
  info_hash?: string;
  quality?: Record<string, unknown>;
  total_score: number;
  match_score: number;
  quality_score: number;
  debrid_provider: string;
  debrid_provider_id: string;
  cached: boolean;
  source_type: 'torrent' | 'usenet';
  files: Array<{ id: number; path: string; size: number }>;
  publish_date?: string;
  /** Outcome history for this release file; absent when it has none. */
  playback?: 'worked' | 'failed';
  availability?: StreamAvailability;
  group?: StreamGroup;
  audio_languages?: string[];
}

export type StreamAvailability = 'ready' | 'likely' | 'unknown' | 'uncached';

export type StreamGroup = 'played' | 'account' | 'available' | 'uncached' | 'failed';

export interface StreamSearchResponse {
  options: StreamOption[];
  errors: string[];
  /** Info hash of the last successfully used stream for this media item. */
  preferred_info_hash?: string;
  /** File ID within the preferred torrent (for season packs). */
  preferred_file_id?: number;
}

export interface StreamResolveParams {
  debrid_provider_id: string;
  info_hash?: string;
  magnet_url?: string;
  nzb_url?: string;
  file_id?: number;
  media_type?: string;
  media_id?: string;
  release_title?: string;
  debrid_provider?: string;
  quality?: Record<string, unknown>;
  start_time?: number;
  capabilities?: ClientCapabilities;
  estimated_duration?: number;
  auto_create?: {
    title: string;
    year?: number;
    identifiers?: Record<string, string>;
    description?: string;
    poster_url?: string;
    backdrop_url?: string;
    genres?: string[];
    rating?: number;
    rating_count?: number;
    runtime?: number;
    network?: string;
    // TV episode streaming: create full show→season→episode chain
    provider?: string;
    provider_id?: string;
    season_number?: number;
    episode_number?: number;
  };
}

/** The title's release already on disk, playable without a search or the debrid host. */
export async function streamLocal(
  mediaType: string,
  mediaId: string
): Promise<StreamOption | null> {
  try {
    return await request(
      `/api/v1/admin/stream/local${buildQueryString({ media_type: mediaType, media_id: mediaId })}`
    );
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) return null;
    throw e;
  }
}

export async function streamSearch(params: {
  movie_id?: string;
  episode_id?: string;
  title?: string;
  year?: number;
  imdb_id?: string;
  tmdb_id?: number;
  media_type?: string;
  season?: number;
  episode?: number;
}): Promise<StreamSearchResponse> {
  const key = `streamSearch:${JSON.stringify(params)}`;
  return dedup(key, () =>
    request('/api/v1/admin/stream/search', {
      method: 'POST',
      body: JSON.stringify(params)
    })
  );
}

export interface StreamResolveCachingResponse {
  session_id: null;
  status: 'caching';
  debrid_provider_id: string;
  torrent_id: string;
}

export type StreamResolveResponse = PlaybackSessionResponse | StreamResolveCachingResponse;

export function isStreamCaching(resp: StreamResolveResponse): resp is StreamResolveCachingResponse {
  return 'status' in resp && resp.status === 'caching';
}

export async function streamResolve(params: StreamResolveParams): Promise<StreamResolveResponse> {
  return request('/api/v1/admin/stream/resolve', {
    method: 'POST',
    body: JSON.stringify(params)
  });
}

export interface StreamResolveStatusResponse {
  status: 'ready' | 'downloading' | 'error';
  stream_url?: string;
  filename?: string;
  filesize?: number;
  progress?: number;
  message?: string;
}

export async function streamResolveStatus(
  providerId: string,
  torrentId: string
): Promise<StreamResolveStatusResponse> {
  return request(
    `/api/v1/admin/stream/resolve/status?provider_id=${encodeURIComponent(providerId)}&torrent_id=${encodeURIComponent(torrentId)}`
  );
}

export async function deleteStreamSession(sessionId: string): Promise<void> {
  return request(`/api/v1/admin/stream/${sessionId}`, { method: 'DELETE' });
}

export type StreamOutcome = { ok: true } | { ok: false; reason: string };

export async function reportStreamOutcome(
  sessionId: string,
  outcome: StreamOutcome
): Promise<void> {
  return request(`/api/v1/admin/stream/${sessionId}/outcome`, {
    method: 'POST',
    body: JSON.stringify(outcome)
  });
}

export type KeepState = 'queued' | 'downloading' | 'importing' | 'done' | 'failed';

export interface KeepStatus {
  state: KeepState;
  progress?: number;
  error?: string;
}

export async function keepStream(sessionId: string): Promise<{ job_id: string; state: KeepState }> {
  return request(`/api/v1/admin/stream/${sessionId}/keep`, { method: 'POST' });
}

export async function getKeepStatus(sessionId: string): Promise<KeepStatus> {
  return request(`/api/v1/admin/stream/${sessionId}/keep`);
}

export interface StreamPrewarmParams {
  media_type: string;
  media_id?: string;
  auto_create?: StreamResolveParams['auto_create'];
  season?: number;
  episode?: number;
  capabilities?: ClientCapabilities;
  start_time?: number;
}

export interface StreamPrewarmResponse {
  state: 'warming' | 'ready';
  prewarm_id: string;
  option?: StreamOption;
}

export async function streamPrewarm(
  params: StreamPrewarmParams
): Promise<StreamPrewarmResponse | undefined> {
  return request('/api/v1/admin/stream/prewarm', {
    method: 'POST',
    body: JSON.stringify(params)
  });
}

export async function claimStreamPrewarm(prewarmId: string): Promise<StreamResolveResponse> {
  return request(`/api/v1/admin/stream/prewarm/${prewarmId}/claim`, { method: 'POST' });
}

export async function touchStreamPrewarm(prewarmId: string): Promise<void> {
  return request(`/api/v1/admin/stream/prewarm/${prewarmId}/touch`, { method: 'POST' });
}

export async function releaseStreamPrewarm(prewarmId: string, keepalive = false): Promise<void> {
  return request(`/api/v1/admin/stream/prewarm/${prewarmId}`, { method: 'DELETE', keepalive });
}

export interface RecentStreamSession {
  id: string;
  media_type: string;
  media_id: string;
  release_title: string;
  debrid_provider: string;
  filename: string;
  filesize: number;
  pinned: boolean;
  progress_seconds: number | null;
  created_at: string;
  updated_at: string;
  media_title?: string;
  media_image_url?: string;
}

export async function getRecentStreams(): Promise<{ sessions: RecentStreamSession[] }> {
  return request('/api/v1/admin/stream/recent');
}

// Stremio API Keys

export interface StremioApiKey {
  id: string;
  api_key: string;
  name: string;
  created_at: string;
}

export async function getStremioKeys(): Promise<{ keys: StremioApiKey[] }> {
  return request('/api/v1/admin/settings/stremio-keys');
}

export async function createStremioKey(name: string): Promise<StremioApiKey> {
  return request('/api/v1/admin/settings/stremio-keys', {
    method: 'POST',
    body: JSON.stringify({ name })
  });
}

export async function deleteStremioKey(id: string): Promise<void> {
  return request(`/api/v1/admin/settings/stremio-keys/${id}`, { method: 'DELETE' });
}

// Chat turn logs (admin viewer for the chatbot loop)
export interface ChatTurnLogContent {
  user_message: string | null;
  assistant_text: string | null;
  extract_output: unknown | null;
  resolve_query: string | null;
  resolved_ref: string | null;
  resolve_candidates: unknown | null;
  action_args: unknown | null;
  action_result: unknown | null;
  tool_calls: ChatToolCall[];
}

export interface ChatToolCall {
  name: string;
  arguments: unknown | null;
  result: unknown | null;
  error: string | null;
  duration_ms: number;
}

export interface ChatTurnLogEntry {
  id: string;
  session_id: string | null;
  user_id: string | null;
  transport: string;
  succeeded: boolean;
  failed_stage: 'extract' | 'resolve' | 'action' | null;
  turn_error: string | null;
  extract_ms: number | null;
  extract_error: string | null;
  /** Slot names to JSON type, or null where the slot came back empty. Never values. */
  extract_shape: Record<string, string | null> | null;
  resolve_ms: number | null;
  resolve_candidate_count: number | null;
  resolve_error: string | null;
  action_ms: number | null;
  action_name: string | null;
  action_error: string | null;
  llm_provider: string | null;
  llm_model: string | null;
  llm_ms: number | null;
  prompt_tokens: number | null;
  completion_tokens: number | null;
  rounds: number;
  tool_call_count: number;
  tool_names: string[];
  total_ms: number | null;
  created_at: string;
  /** Present only for the turn's own user. Admins who did not have the conversation see metadata only. */
  content?: ChatTurnLogContent;
}

export interface ChatLogsResponse {
  logs: ChatTurnLogEntry[];
  total: number;
}

export interface ChatLogsQuery {
  limit?: number;
  offset?: number;
  transport?: string;
  status?: 'ok' | 'failed';
  stage?: 'extract' | 'resolve' | 'action';
  user_id?: string;
  session_id?: string;
  since?: string;
  until?: string;
  q?: string;
}

export async function getChatLogs(query: ChatLogsQuery = {}): Promise<ChatLogsResponse> {
  return request(
    `/api/v1/admin/chat/logs${buildQueryString({
      limit: query.limit,
      offset: query.offset,
      transport: query.transport,
      status: query.status,
      stage: query.stage,
      user_id: query.user_id,
      session_id: query.session_id,
      since: query.since,
      until: query.until,
      q: query.q
    })}`
  );
}

export async function getChatLog(id: string): Promise<ChatTurnLogEntry> {
  return request(`/api/v1/admin/chat/logs/${id}`);
}

// Full-cast audiobooks
import type {
  BookCast,
  BookCastCharacterLine,
  BookCastLine,
  BookCastTake,
  BookCastText,
  BookCastUnattributedLine,
  BookCastVoice
} from '#lib/types.js';

function castPath(bookId: string, rest = ''): string {
  return `/api/v1/admin/books/${bookId}/cast${rest}`;
}

export async function getBookCast(bookId: string): Promise<BookCast> {
  return request(castPath(bookId));
}

/**
 * No options resumes the analysis; `rerun` runs it again from a pass keeping
 * everything vetted; `fresh` redoes everything from nothing.
 */
export async function startBookCast(
  bookId: string,
  options: { fresh?: boolean; rerun?: 'chapters' | 'bible' } = {}
): Promise<{ job_id: string; job_type: string }> {
  return request(castPath(bookId), { method: 'POST', body: JSON.stringify(options) });
}

export async function startBookCastRender(
  bookId: string
): Promise<{ job_id: string; job_type: string }> {
  return request(castPath(bookId, '/render'), { method: 'POST' });
}

export async function getBookCastChapter(
  bookId: string,
  chapter: number
): Promise<{ lines: BookCastLine[] }> {
  return request(castPath(bookId, `/chapters/${chapter}`));
}

export async function approveAllCastVoices(
  bookId: string
): Promise<{ job_id: string; job_type: string }> {
  return request(castPath(bookId, '/voices/approve-all'), { method: 'POST' });
}

export async function getCastAuditionLines(
  bookId: string,
  characterId: string,
  fromChapter: number
): Promise<{ voice: BookCastVoice; lines: BookCastTake[] }> {
  return request(
    castPath(bookId, `/characters/${characterId}/auditions?from_chapter=${fromChapter}`)
  );
}

export interface CastTakeRequest {
  segment_index: number;
  span_start: number | null;
  seed: number;
  with_cue: boolean;
}

export async function renderCastAudition(
  bookId: string,
  characterId: string,
  fromChapter: number,
  take: CastTakeRequest,
  signal?: AbortSignal
): Promise<Blob> {
  const response = await fetch(
    `${apiBase}${castPath(bookId, `/characters/${characterId}/auditions`)}`,
    {
      method: 'POST',
      credentials: 'include',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ from_chapter: fromChapter, ...take }),
      signal
    }
  );
  if (!response.ok) {
    const msg = await response.text().catch(() => 'audition failed');
    throw new ApiError(response.status, msg);
  }
  return await response.blob();
}

type CastVoices = { voices: BookCastVoice[] };

export async function approveCastVoice(
  bookId: string,
  characterId: string,
  fromChapter: number,
  take: Partial<CastTakeRequest>
): Promise<CastVoices> {
  return request(castPath(bookId, `/characters/${characterId}/voices/${fromChapter}/approval`), {
    method: 'POST',
    body: JSON.stringify(take)
  });
}

export async function unapproveCastVoice(
  bookId: string,
  characterId: string,
  fromChapter: number
): Promise<CastVoices> {
  return request(castPath(bookId, `/characters/${characterId}/voices/${fromChapter}/approval`), {
    method: 'DELETE'
  });
}

/** `preset_id: null` goes back to the designed voice. */
export async function updateCastVoice(
  bookId: string,
  characterId: string,
  fromChapter: number,
  change: { description?: string; preset_id?: string | null }
): Promise<CastVoices> {
  return request(castPath(bookId, `/characters/${characterId}/voices/${fromChapter}`), {
    method: 'PUT',
    body: JSON.stringify(change)
  });
}

export async function addCastVoiceChange(
  bookId: string,
  characterId: string,
  step: { from_chapter: number; change: string; description: string }
): Promise<void> {
  await request(castPath(bookId, `/characters/${characterId}/voices`), {
    method: 'POST',
    body: JSON.stringify(step)
  });
}

export async function removeCastVoiceChange(
  bookId: string,
  characterId: string,
  fromChapter: number
): Promise<void> {
  await request(castPath(bookId, `/characters/${characterId}/voices/${fromChapter}`), {
    method: 'DELETE'
  });
}

export async function getCastCharacterLines(
  bookId: string,
  characterId: string
): Promise<{ lines: BookCastCharacterLine[] }> {
  return request(castPath(bookId, `/characters/${characterId}/lines`));
}

/** Paragraphs `from..=to` of the book (at most 200). */
export async function getCastText(bookId: string, from: number, to: number): Promise<BookCastText> {
  return request(castPath(bookId, `/text${buildQueryString({ from, to })}`));
}

/** Asks the model who says the character's lines (`lines`, or all); poll the job for its suggestions. */
export async function suggestCastSpeakers(
  bookId: string,
  characterId: string,
  lines: { segment_index: number; span_start: number }[] = []
): Promise<{ job_id: string }> {
  return request(castPath(bookId, `/characters/${characterId}/suggest`), {
    method: 'POST',
    body: JSON.stringify({ lines })
  });
}

export async function getUnattributedCastLines(
  bookId: string
): Promise<{ lines: BookCastUnattributedLine[] }> {
  return request(castPath(bookId, '/lines/unattributed'));
}

/** A character's id, `null` for narration, or `'not_speech'` for a quote nobody says. */
export type CastLineSpeaker = string | null | 'not_speech';

export async function reassignCastLine(
  bookId: string,
  segmentIndex: number,
  spanStart: number,
  to: CastLineSpeaker
): Promise<void> {
  const body =
    to === 'not_speech' ? { character_id: null, not_speech: true } : { character_id: to };
  await request(castPath(bookId, `/lines/${segmentIndex}/${spanStart}`), {
    method: 'PUT',
    body: JSON.stringify(body)
  });
}

/** To `character_id`, or else to a new character called `new_name` (the name itself if left out). */
export async function moveCastName(
  bookId: string,
  characterId: string,
  move: { name: string; character_id?: string; new_name?: string }
): Promise<{ id: string }> {
  return request(castPath(bookId, `/characters/${characterId}/names/move`), {
    method: 'POST',
    body: JSON.stringify(move)
  });
}

export async function mergeCastCharacters(
  bookId: string,
  survivorId: string,
  absorbId: string
): Promise<void> {
  await request(castPath(bookId, `/characters/${survivorId}/merge`), {
    method: 'POST',
    body: JSON.stringify({ absorb: absorbId })
  });
}

export async function splitCastCharacter(
  bookId: string,
  characterId: string,
  split: { name: string; aliases: string[]; lines: { segment_index: number; span_start: number }[] }
): Promise<{ id: string }> {
  return request(castPath(bookId, `/characters/${characterId}/split`), {
    method: 'POST',
    body: JSON.stringify(split)
  });
}

export async function attachCastSpeaker(
  bookId: string,
  speaker: string,
  characterId: string
): Promise<{ attached: number }> {
  return request(castPath(bookId, '/speakers/attach'), {
    method: 'POST',
    body: JSON.stringify({ speaker, character_id: characterId })
  });
}

/** `bible` is merged into the character's; a field set to null is removed. */
export async function updateCastCharacter(
  bookId: string,
  characterId: string,
  patch: {
    name?: string;
    aliases?: string[];
    bible?: Record<string, unknown>;
    generic?: boolean;
    pooled?: boolean;
  }
): Promise<void> {
  await request(castPath(bookId, `/characters/${characterId}`), {
    method: 'PATCH',
    body: JSON.stringify(patch)
  });
}
