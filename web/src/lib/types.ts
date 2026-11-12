// Shared TypeScript types for Kobbler web UI

export type AuthorRef = {
  id: string | null;
  name: string;
};

/** Ingest-specific metadata sidecar (file provenance, pending entities, conversion state). */
export type IngestInfo = {
  file_path: string;
  file_name: string;
  file_size: number;
  file_hash: string | null;
  source_format: string;
  has_cover: boolean;
  conversion_error: string | null;
  update_target_book_id: string | null;
  pending_authors: Array<{ name: string; hardcover_id?: string }> | null;
  pending_series: { name: string; hardcover_id?: string } | null;
  pending_series_index: number | null;
  source_metadata: Record<string, unknown> | null;
};

export type Book = {
  id: string;
  title: string;
  authors: AuthorRef[];
  description: string | null;
  publisher: string | null;
  pubdate: string | null;
  series_id: string | null;
  series_name: string | null;
  series_index: number | null;
  language: string | null;
  formats: string[];
  synced: boolean;
  pending_sync: boolean;
  cover_url: string | null;
  extracted_cover_url?: string | null;
  created_at: string;
  updated_at: string;
  identifiers: Record<string, string>;
  // Extended metadata fields
  genres: string[] | null;
  rating: number | null;
  rating_count: number | null;
  page_count: number | null;
  // Tracking/discovery fields (optional - present for author/series book lists)
  is_in_library?: boolean;
  want_status?: string;
  // Ingest queue status (null for library books, status string for items still in queue)
  ingest_status?: string | null;
  // Ingest-specific data (only present for items in the ingest queue)
  ingest_info?: IngestInfo | null;
  // Audiobook fields
  narrator?: string | null;
  duration_seconds?: number | null;
  is_abridged?: boolean;
  has_audiobook?: boolean;
  want_format?: 'ebook' | 'audiobook' | 'both';
  media_root_id?: string | null;
  series_memberships?: SeriesMembership[];
  shelves?: Array<{ id: string; name: string }>;
};

export type SeriesMembership = {
  series_id: string;
  series_name: string;
  series_index: number | null;
  is_primary: boolean;
};

export type UserSummary = {
  id: string;
  email: string;
  role: string;
  display_name?: string | null;
  // IANA zone name; null means quiet hours fall back to the server's zone.
  timezone?: string | null;
  is_kids: boolean;
  avatar_url?: string | null;
};

export type ChatIdentityStatus = 'disabled' | 'awaiting_first_message' | 'active';

export type ChatIdentity = {
  id: string;
  transport: string;
  address?: string | null;
  external_id?: string | null;
  user_id: string;
  user_email: string;
  user_display_name?: string | null;
  user_role: string;
  max_role: string;
  effective_role: string;
  enabled: boolean;
  status: ChatIdentityStatus;
  created_at: string;
  last_seen_at?: string | null;
};

export type ChatPendingPairing = {
  id: string;
  transport: string;
  external_id: string;
  code: string;
  display_name?: string | null;
  space?: string | null;
  created_at: string;
  expires_at: string;
};

export type ChatTransportConfigField = {
  key: string;
  label: string;
  field_type: string;
  required: boolean;
  description?: string;
  help_url?: string;
};

export type ChatTransportInfo = {
  id: string;
  display_name: string;
  description: string;
  config_fields: ChatTransportConfigField[];
  capabilities: {
    edit_in_place: boolean;
    cards: boolean;
    buttons: boolean;
    files: boolean;
  };
};

export type ChatIdentitiesResponse = {
  identities: ChatIdentity[];
  pending_pairings: ChatPendingPairing[];
  transports: ChatTransportInfo[];
};

export type NotificationTrigger = 'on_grab' | 'on_import' | 'on_failed' | 'on_media_corrupt';

export type MuteDuration =
  | 'an_hour'
  | 'a_couple_of_hours'
  | 'the_rest_of_today'
  | 'until_tomorrow'
  | 'a_week'
  | 'indefinitely';

export type ChatNotificationPrefs = {
  enabled: boolean;
  triggers: NotificationTrigger[];
  muted_until: string | null;
  muted: boolean;
  /** Null in both when there are no quiet hours. `quiet_from` later than
   *  `quiet_to` is a window that crosses midnight, which is the ordinary case. */
  quiet_from: string | null;
  quiet_to: string | null;
  timezone: string | null;
  timezone_label: string;
  summary: string;
};

/** The `set_notification_prefs` argument surface verbatim, so the settings page
 *  and the chat bot cannot drift. */
export type ChatNotificationPrefsUpdate = {
  action:
    | 'mute'
    | 'resume'
    | 'enable_trigger'
    | 'disable_trigger'
    | 'set_quiet_hours'
    | 'clear_quiet_hours'
    | 'status';
  trigger?: NotificationTrigger | 'everything';
  duration?: MuteDuration;
  quiet_from?: string;
  quiet_to?: string;
};

export type ChatFactKind = 'prefers' | 'likes' | 'avoids';

export type ChatProfileFact = {
  id: string;
  kind: ChatFactKind;
  value: string;
  source_id?: string | null;
  created_at: string;
};

export type ChatSessionSummary = {
  id: string;
  transport: string;
  summary: string;
  created_at: string;
  closed_at?: string | null;
};

export type ChatMemoryResponse = {
  facts: ChatProfileFact[];
  summaries: ChatSessionSummary[];
  max_facts: number;
  kinds: ChatFactKind[];
};

// Admin view: counts and timestamps. `content` is present only on the
// admin's own row — conversation content belongs to the person who had it.
export type ChatMemoryUser = {
  user_id: string;
  email: string;
  display_name?: string | null;
  session_count: number;
  message_count: number;
  summary_count: number;
  fact_count: number;
  last_seen?: string | null;
  content?: ChatMemoryResponse;
};

export type ChatForgetResult = {
  sessions: number;
  messages: number;
  facts: number;
  turn_logs: number;
};

/**
 * Which kinds of media a library-wide maintenance pass should walk.
 *
 * Mirrors `MediaScope` in `api/src/types.rs`. Omitting it means every kind the
 * pass handles.
 */
export type MediaScope = 'movie' | 'episode' | 'audiobook';

export type DeviceType = 'kobo' | 'web' | 'kindle' | 'generic';

export type DeviceSummary = {
  id: string;
  name: string | null;
  user_id: string;
  token: string;
  sync_url: string;
  kobo_api_url: string | null;
  last_sync_at: string | null;
  device_type: DeviceType;
  auto_sync_new_books: boolean;
};

// Device activity types for sidebar display
export type MiniBook = {
  id: string;
  title: string;
  cover_url: string | null;
};

export type MiniBookWithProgress = MiniBook & {
  progress_percent: number;
};

export type MiniBookWithTimestamp = MiniBook & {
  timestamp: string;
};

export type DeviceActivity = {
  last_sync_at: string | null;
  pending_sync: MiniBook[];
  recently_read: MiniBookWithProgress[];
  recently_synced: MiniBookWithTimestamp[];
  // Kindle-specific
  pending_delivery: MiniBook[];
  recently_delivered: MiniBookWithTimestamp[];
  last_delivery_at: string | null;
};

export type ActivityEvent = {
  id: string;
  event_kind: string;
  entity_type: string | null;
  entity_id: string | null;
  title: string;
  subtitle: string | null;
  image_url: string | null;
  url: string | null;
  created_at: string;
};

export type SeriesSummary = {
  id: string;
  name: string;
  book_count: number;
  wanted_count: number;
  authors: AuthorRef[];
  updated_at: string;
  cover_urls: string[];
  // Metadata fields
  identifiers?: Record<string, string>;
  image_url?: string | null;
  description?: string | null;
  /** Auto-want mode: 'inherit' (default) | 'none' | 'new_only' | 'all' | 'fill_gaps' */
  auto_want_mode?: string;
  total_book_count?: number | null;
  /** Whether series is monitored for auto-want */
  is_monitored?: boolean;
  /** When monitoring started */
  monitoring_started_at?: string | null;
  /** Metadata sources for source attribution during editing */
  sources?: Array<{
    provider: string;
    name: string;
    description?: string | null;
    image_url?: string | null;
  }>;
};

export type ShelfSummary = {
  id: string;
  name: string;
  user_id: string;
  book_count: number;
  last_modified: string;
  cover_urls: string[];
};

export type Author = {
  id: string;
  name: string;
  book_count: number;
  series_count: number;
  /** Total books in DB for this author (regardless of ownership) */
  total_book_count: number;
  /** Total series in DB for this author (regardless of ownership) */
  total_series_count: number;
  wanted_count: number;
  cover_urls: string[];
  /** External identifiers (e.g., {"hardcover": "123"}) */
  identifiers?: Record<string, string>;
  /** Author image URL */
  image_url?: string | null;
  /** Author bio */
  bio?: string | null;
  /** Whether author is monitored for auto-want */
  is_monitored?: boolean;
  /** Auto-want mode: 'none' | 'new_only' | 'all' */
  auto_want_mode?: string;
  /** Total known books from provider */
  known_book_count?: number | null;
  /** Top series names for disambiguation (e.g., "Stormlight Archive, Mistborn") */
  top_series_names?: string | null;
  /** Metadata sources for source attribution during editing */
  sources?: Array<{
    provider: string;
    name: string;
    bio?: string | null;
    image_url?: string | null;
  }>;
};

export type AuthorSummary = Author;

export type GenreSummary = {
  id: string;
  name: string;
  book_count: number;
};

export type IngestTokenSummary = {
  id: string;
  token: string;
  name: string | null;
  created_at: string;
  revoked: boolean;
};

export type SessionSummary = {
  id: string;
  user_id: string;
  email: string;
  display_name: string | null;
  device_name: string | null;
  client_kind: string | null;
  created_at: string;
  expires_at: string;
  last_used_at: string | null;
  created_by_email: string | null;
  is_current: boolean;
};

// A playback failure a client reported. `detail` is verbatim what the player's
// error dialog showed, so a row is exactly what the person was looking at.
export type PlaybackReport = {
  id: string;
  client_kind: string;
  app_version: string | null;
  device_label: string | null;
  media_type: string | null;
  media_id: string | null;
  stream_url: string | null;
  session_id: string | null;
  detail: string;
  created_at: string;
};

export type WantedItem = {
  id: string;
  kind: string;
  provider: string;
  provider_id: string | null;
  title: string;
  author: string | null;
  series: string | null;
  series_index: number | null;
  isbn10: string | null;
  isbn13: string | null;
  cover_url: string | null;
  want_status: string;
  attempt_status: string | null;
  last_attempt_at: string | null;
  created_at: string;
  updated_at: string;
};

export type WantedStatusCounts = {
  pending: number;
  wanted: number;
  searching: number;
  downloading: number;
  ingesting: number;
  watching: number;
  search_failed: number;
  queued: number;
  kobload_error: number;
  needs_review: number;
  completed: number;
  failed: number;
};

export type ListWantedResponse = {
  items: WantedItem[];
  status_counts: WantedStatusCounts;
};

export type WantedAttempt = {
  id: string;
  wanted_id: string;
  source: string;
  external_id: string | null;
  release_payload: Record<string, unknown> | null;
  download_client_id: number | null;
  download_url: string | null;
  status: string;
  message: string | null;
  created_at: string;
};

export type WantedDetail = {
  item: WantedItem;
  attempts: WantedAttempt[];
};

export type WantedStatusResponse = {
  pattern: WantedStatusPattern | null;
  matches: WantedMatch[];
  not_tracked?: boolean;
};

export type WantedStatusPattern = {
  id: string;
  status: string;
  source_id: string;
  enabled: boolean;
};

export type WantedMatch = {
  id: string;
  release_title: string;
  match_score: number;
  quality_score?: number;
  action: string;
  held: boolean;
  hold_until?: string;
  download_id?: string;
  rejected: boolean;
  created_at: string;
};

export type KobloadCategory = {
  id: number;
  name?: string;
  subCategories?: KobloadCategory[];
};

export type TorrentStats = {
  seeders?: number;
  peers?: number;
  leechers?: number;
};

export type UsenetStats = {
  grabs?: number;
};

export type KobloadRelease = {
  guid?: string;
  indexer_id?: number;
  title?: string;
  size?: number;
  indexer?: string;
  protocol?: string;
  publish_date?: string;
  download_url?: string;
  categories?: KobloadCategory[];
  /** Torrent-specific stats (only present when protocol="torrent") */
  torrent?: TorrentStats;
  /** Usenet-specific stats (only present when protocol="usenet") */
  usenet?: UsenetStats;
  [key: string]: unknown;
};

export type UnavailableBook = {
  id: string;
  title: string;
  authors: string[];
  unavailable_files: number;
  total_files: number;
  unavailable_since: string | null;
  cover_url: string | null;
};

export type UnavailableBooksResponse = {
  items: UnavailableBook[];
  total_count: number;
};

export type IngestQueueStatusCounts = {
  extracting: number;
  pending: number;
  converting: number;
  approved: number;
  rejected: number;
  error: number;
};

/** A group of ingest queue items with the same base filename (different formats) */
export type IngestQueueGroup = {
  /** Unique key for this group (directory|basename) */
  group_key: string;
  /** All items in this group (Book-shaped responses with ingest_info) */
  items: Book[];
  /** Available formats in this group (e.g., ["EPUB", "MOBI"]) */
  formats: string[];
  /** ID of the primary (preferred) item */
  primary_item_id: string;
  /** Computed display status for the group */
  display_status: string;
};

export type IngestQueueListResponse = {
  groups: IngestQueueGroup[];
  total_groups: number;
  status_counts: IngestQueueStatusCounts;
};

export type MetadataProvider = {
  id: string;
  provider_type: string;
  enabled: boolean;
  priority: number;
  api_key: string | null;
  rate_limit_per_minute: number;
  created_at: string;
  updated_at: string;
};

export type UserNotification = {
  id: string;
  title: string;
  message: string;
  entity_type: string | null;
  entity_id: string | null;
  entity_url: string | null;
  read: boolean;
  created_at: string;
};

export type JobDisplay = {
  title: string;
  subtitle?: string;
  image_url?: string;
};

export type Job = {
  id: string;
  job_type: string;
  status: 'pending' | 'running' | 'completed' | 'failed';
  payload: Record<string, unknown>;
  result: Record<string, unknown> | null;
  error: string | null;
  progress: number | null;
  created_at: string;
  started_at: string | null;
  completed_at: string | null;
  description: string | null;
  entity_type: string | null;
  entity_id: string | null;
  display: JobDisplay | null;
  scheduled: boolean;
};

export type JobEvent = {
  job_id: string;
  job_type: string;
  status: string;
  progress: number | null;
  error: string | null;
  result: Record<string, unknown> | null;
  description: string | null;
  entity_type: string | null;
  entity_id: string | null;
  display: JobDisplay | null;
  created_at: string;
  scheduled: boolean;
};

// Pagination types
export type PaginatedResponse<T> = {
  items: T[];
  next_cursor: string | null;
  prev_cursor?: string | null;
  total_count?: number;
};

// UI state types
export type PageKey =
  | 'library'
  | 'series'
  | 'authors'
  | 'shelves'
  | 'monitored'
  | 'wanted'
  | 'queue'
  | 'devices'
  | 'activity'
  | 'proxy-logs'
  | 'settings'
  | 'ingest-queue';
export type LoadStatus = 'idle' | 'loading' | 'error' | 'success';
export type LoadKey =
  | 'library'
  | 'series'
  | 'seriesItems'
  | 'authors'
  | 'authorItems'
  | 'activity'
  | 'devices'
  | 'users'
  | 'ingestTokens'
  | 'shelves'
  | 'shelfItems'
  | 'proxyLogs'
  | 'ingestQueue';
export type LoadState = { status: LoadStatus; error: string };

// Callback types for component props
// Tracked Author types
export type TrackedAuthor = {
  id: string;
  provider: string;
  provider_id: string;
  name: string;
  image_url: string | null;
  is_implicit: boolean;
  enabled: boolean;
  monitoring_enabled: boolean;
  auto_want_mode: string;
  known_book_count: number;
  last_checked_at: string | null;
  tracking_started_at: string | null;
  created_at: string;
};

export type AuthorSearchResult = {
  provider: string;
  external_id: string;
  name: string;
  image_url: string | null;
  book_count: number | null;
  /** Series names this author has written (for identification in search results) */
  series_names: string[];
  confidence: number;
};

export type SeriesSearchResultBook = {
  title: string;
  authors: string[];
  series_index: number | null;
};

export type SeriesSearchResult = {
  provider: string;
  name: string;
  authors: string[];
  description: string | null;
  book_count: number | null;
  image_url: string | null;
  books: SeriesSearchResultBook[];
  external_id: string | null;
  slug: string | null;
  confidence: number;
};

// Structured progress information from download clients
export type DownloadClientProgress = {
  /** Estimated completion time as UTC timestamp (ISO 8601) */
  eta_time?: string | null;
  /** Current download rate in bytes per second */
  download_rate?: number | null;
  /** Total download size in bytes */
  total_size?: number | null;
};

// Kobload SSE event types
export type KobloadDownloadListItem = {
  id: string;
  name: string;
  protocol: string;
  status: string;
  progress: number | null;
  last_poll_at: string | null;
  last_poll_error: string | null;
  created_at: string;
  updated_at: string;
  download_client_id: string | null;
  external_id: string | null;
  file_path: string | null;
  error: string | null;
  callback_sent_at: string | null;
  import_confirmed_at: string | null;
  files_json: unknown | null;
  release_payload: unknown | null;
  release_interpretation_json: unknown | null;
  metadata: unknown;
  callback_attempts: number;
  callback_last_attempt_at: string | null;
  callback_error: string | null;
  files_scan_error: string | null;
  /** Structured progress information from download client */
  client_progress: DownloadClientProgress | null;
  download_client_name: string | null;
  api_key_id: string | null;
  api_key_name: string | null;
};

export type KobloadDownloadSnapshotEvent = {
  kind: 'kobload_download_snapshot';
  downloads: KobloadDownloadListItem[];
};

export type KobloadDownloadCreatedEvent = {
  kind: 'kobload_download_created';
  download: KobloadDownloadListItem;
};

export type KobloadDownloadUpdatedEvent = {
  kind: 'kobload_download_updated';
  download: KobloadDownloadListItem;
};

export type KobloadDownloadEvent =
  KobloadDownloadSnapshotEvent | KobloadDownloadCreatedEvent | KobloadDownloadUpdatedEvent;

// Media info for enriched downloads
export type DownloadMediaInfo = {
  id: string;
  title: string;
  cover_url: string | null;
  author: string | null;
  author_id: string | null;
  series: string | null;
  series_id: string | null;
  series_index: number | null;
  subtitle: string | null;
};

// Enriched download with media info
export type EnrichedDownload = KobloadDownloadListItem & {
  media: DownloadMediaInfo | null;
};

// Response from kobbler downloads endpoint
export type EnrichedDownloadsResponse = {
  items: EnrichedDownload[];
  next_cursor: string | null;
};

// Library stats for nav sidebar counts
export type LibraryStats = {
  books_count: number;
  audiobooks_count: number;
  series_count: number;
  authors_count: number;
  shelves_count: number;
  downloads_active: number;
  ingest_pending: number;
  movies_count: number;
  tv_shows_count: number;
  people_count: number;
  sports_count: number;
  kobload_configured: boolean;
};

// =============================================================================
// Book Files with Provenance
// =============================================================================

export type BookFileSourceType = 'kobload' | 'calibre_plugin' | 'manual_upload' | 'unknown';

export type BookFileInfo = {
  id: string;
  format: string;
  size_bytes: number | null;
  filename: string;
  file_path: string;
  source_type: BookFileSourceType;
  source_metadata: {
    download_id?: string;
    release_title?: string;
    indexer?: string;
    info_hash?: string;
    size?: number;
    protocol?: string;
    download_url?: string;
  };
  added_at: string;
  available: boolean;
};

export type AudioFileInfo = {
  id: string;
  file_index: number;
  format: string;
  filename: string;
  file_size: number;
  duration_seconds: number | null;
  available: boolean;
  intro_end_seconds?: number | null;
  outro_start_seconds?: number | null;
};

export type AudioSummary = {
  count: number;
  total_size: number;
  total_duration_seconds: number | null;
  format: string;
};

export type BookFilesResponse = {
  files: BookFileInfo[];
  audio_files?: AudioFileInfo[];
  audio_summary?: AudioSummary;
};

// =============================================================================
// Library Search and Download
// =============================================================================

export type PreviousDownloadInfo = {
  downloaded_at: string;
  book_id: string;
  book_title: string;
  file_id: string;
};

export type RejectedReleaseInfo = {
  reason: string;
  release_title?: string;
};

export type IndexerRelease = {
  guid?: string;
  title: string;
  size?: number;
  indexer?: string;
  seeders?: number;
  leechers?: number;
  grabs?: number;
  protocol?: string;
  download_url?: string;
  info_hash?: string;
  publish_date?: string;
  previously_downloaded?: PreviousDownloadInfo;
  rejected_release?: RejectedReleaseInfo;
  [key: string]: unknown;
};

export type LibrarySearchResponse = {
  results: IndexerRelease[];
};

export type DownloadToLibraryRequest = {
  release: IndexerRelease;
  book_id?: string;
  create_book?: {
    title: string;
    author?: string;
    series?: string;
    series_index?: number;
  };
};

export type DownloadToLibraryResponse = {
  book_id: string;
  attempt_id: string;
  download_id?: string;
};

// =============================================================================
// Reading Data Types
// =============================================================================

export type ReadingStatus = 'want_to_read' | 'reading' | 'finished' | null;

export type ReadingData = {
  /** User's personal reading status */
  status: ReadingStatus;
  /** Progress percent from device sync (0-100) */
  progress_percent: number | null;
  /** Total spent reading minutes across all devices */
  spent_reading_minutes: number | null;
  /** Book page count for calculating page from percent */
  page_count: number | null;
  /** When the user started reading */
  started_at: string | null;
  /** When the user finished reading */
  finished_at: string | null;
  /** User's rating (1-5) */
  rating: number | null;
  /** Source of rating: 'manual', 'hardcover', 'kobo' */
  rating_source: string | null;
  /** Community rating from book metadata */
  community_rating: number | null;
  /** Community rating count */
  community_rating_count: number | null;
};

export type UpdateReadingRequest = {
  status?: string;
  progress_percent?: number;
  page?: number;
  rating?: number;
};

export type LogReadingTimeRequest = {
  minutes: number;
};

// =============================================================================
// Video / Movies / TV Types
// =============================================================================

export type VideoConfigField = {
  key: string;
  label: string;
  field_type: string;
  required: boolean;
  description?: string;
  help_url?: string;
  default?: string;
  current_value?: unknown;
};

export type VideoMetadataProvider = {
  id: string;
  provider_type: string;
  enabled: boolean;
  priority: number;
  display_name: string;
  description: string;
  website: string;
  capabilities: string[];
  config_fields: VideoConfigField[];
};

export type SubtitleSearchResult = {
  provider: string;
  id: string;
  language: string;
  format: string;
  is_forced: boolean;
  is_hearing_impaired: boolean;
  rating: number | null;
  download_count: number | null;
};

export type SubtitleTrackInfo = {
  id: string;
  movie_file_id: string | null;
  episode_file_id: string | null;
  track_index: number;
  codec: string;
  language: string | null;
  language_name: string | null;
  title: string | null;
  is_forced: boolean;
  is_default: boolean;
  is_hearing_impaired: boolean;
};

export type ExternalSubtitleInfo = {
  id: string;
  movie_file_id: string | null;
  episode_file_id: string | null;
  file_path: string;
  file_size: number | null;
  format: string;
  language: string | null;
  language_name: string | null;
  title: string | null;
  is_forced: boolean;
  is_hearing_impaired: boolean;
  source: string;
  available: boolean;
};

export type FileTracksInfo = {
  file_id: string;
  file_path: string;
  subtitle_tracks: SubtitleTrackInfo[];
  external_subtitles: ExternalSubtitleInfo[];
};

export type TracksResponse = {
  files: FileTracksInfo[];
};

export type SubtitleProvider = {
  id: string;
  provider_type: string;
  enabled: boolean;
  priority: number;
  display_name: string;
  description: string;
  website: string;
  capabilities: string[];
  config_fields: VideoConfigField[];
};

export type VideoIdentifiers = {
  tmdb_id?: string | null;
  imdb_id?: string | null;
  tvmaze_id?: string | null;
  tvdb_id?: string | null;
};

export type SearchSuggestion = {
  title: string;
  media_type: 'movie' | 'tv' | 'book' | 'author' | 'livetv' | 'iptv';
  source: 'library' | 'tmdb' | 'hardcover' | 'guide';
};

export type MovieSearchResult = {
  provider: string;
  external_id: string;
  title: string;
  year: number | null;
  poster_url: string | null;
  backdrop_url: string | null;
  description: string | null;
  rating: number | null;
  rating_count: number | null;
  genres: string[];
  identifiers: VideoIdentifiers;
  library_id?: string | null;
  want_status?: string | null;
  is_in_library?: boolean | null;
};

export type TvSearchResult = {
  provider: string;
  external_id: string;
  title: string;
  year: number | null;
  poster_url: string | null;
  backdrop_url: string | null;
  description: string | null;
  rating: number | null;
  rating_count: number | null;
  network: string | null;
  status: string | null;
  genres: string[];
  identifiers: VideoIdentifiers;
  library_id?: string | null;
  want_status?: string | null;
  is_in_library?: boolean | null;
};

// Video metadata detail response types (from provider detail endpoints)

export type VideoMediaImage = {
  url: string;
  image_type: string;
  width: number | null;
  height: number | null;
};

export type VideoDetailCastMember = {
  name: string;
  character: string | null;
  profile_url: string | null;
  order: number | null;
  external_id: string | null;
};

export type VideoDetailCrewMember = {
  name: string;
  department: string;
  job: string;
  profile_url: string | null;
  external_id: string | null;
};

export type ReleaseDates = {
  region: string;
  premiere: string | null;
  theatrical_limited: string | null;
  theatrical: string | null;
  digital: string | null;
  physical: string | null;
  tv: string | null;
};

export type MovieDetailResult = {
  provider: string;
  provider_id: string;
  title: string;
  original_title: string | null;
  year: number | null;
  runtime: number | null;
  description: string | null;
  tagline: string | null;
  poster_url: string | null;
  backdrop_url: string | null;
  logo_url: string | null;
  identifiers: VideoIdentifiers;
  genres: string[];
  rating: number | null;
  rating_count: number | null;
  content_rating: string | null;
  release_date: string | null;
  release_dates?: ReleaseDates | null;
  cast: VideoDetailCastMember[];
  crew: VideoDetailCrewMember[];
  images: VideoMediaImage[];
};

export type SeasonSummary = {
  season_number: number;
  episode_count: number | null;
  air_date: string | null;
  poster_url: string | null;
  title: string | null;
};

export type TvShowDetailResult = {
  provider: string;
  provider_id: string;
  title: string;
  year: number | null;
  status: string | null;
  description: string | null;
  poster_url: string | null;
  backdrop_url: string | null;
  logo_url: string | null;
  identifiers: VideoIdentifiers;
  genres: string[];
  rating: number | null;
  rating_count: number | null;
  network: string | null;
  content_rating: string | null;
  first_aired: string | null;
  runtime: number | null;
  seasons: SeasonSummary[];
  cast: VideoDetailCastMember[];
  crew: VideoDetailCrewMember[];
  images: VideoMediaImage[];
};

export type EpisodeDetailResult = {
  episode_number: number;
  title: string | null;
  air_date: string | null;
  /** Full broadcast instant when the provider publishes one; absent means the day is known but the time is not. */
  air_time?: string | null;
  runtime: number | null;
  still_url: string | null;
  description: string | null;
};

export type SeasonDetailResult = {
  provider: string;
  show_id: string;
  season_number: number;
  title: string | null;
  air_date: string | null;
  description: string | null;
  poster_url: string | null;
  episodes: EpisodeDetailResult[];
};

export type PersonDetail = {
  id: string;
  name: string;
  sort_name?: string;
  image_url?: string;
  bio?: string;
  birth_date?: string;
  death_date?: string;
  identifiers: Record<string, string>;
  created_at: string;
  updated_at: string;
  is_monitored: boolean;
  auto_want_mode: string;
  known_for_department?: string;
  place_of_birth?: string;
};

export type PersonFilmographyCredit = {
  provider_id: string;
  media_type: string;
  title: string;
  year?: number;
  poster_url?: string;
  character?: string;
  department?: string;
  job?: string;
  credit_type: string;
  in_library: boolean;
  library_id?: string;
  want_status?: string;
  dismissed: boolean;
};

export type CastMember = {
  id: string;
  person_id: string;
  person_name: string;
  person_image_url: string | null;
  character_name: string | null;
  cast_order: number | null;
};

export type CrewMember = {
  person_id: string;
  person_name: string;
  department: string;
  job: string;
};

export type MovieFile = {
  id: string;
  movie_id: string;
  file_path: string;
  file_size: number | null;
  format: string;
  quality: string | null;
  languages: string | null;
  subtitles: string | null;
  source_type: string | null;
  source_metadata: Record<string, unknown> | null;
  available: boolean;
  created_at: string;
};

export type Movie = {
  id: string;
  title: string;
  sort_title: string | null;
  original_title: string | null;
  year: number | null;
  description: string | null;
  poster_url: string | null;
  backdrop_url: string | null;
  logo_url: string | null;
  identifiers: Record<string, unknown>;
  genres: string[];
  rating: number | null;
  rating_count: number | null;
  content_rating: string | null;
  // null defers to content_rating; only an admin sets it, and metadata never does.
  kids_override?: boolean | null;
  kids_allowed?: boolean;
  runtime: number | null;
  is_in_library: boolean;
  want_status: string;
  watched?: boolean | null;
  watch_position_seconds?: number | null;
  in_watchlist?: boolean;
  quality_profile_id: string | null;
  preferred_languages: string[] | null;
  media_root_id: string | null;
  created_at: string;
  updated_at: string;
  streamed_at: string | null;
  release_date?: string | null;
  release_dates?: ReleaseDates | null;
  // Detail fields (present in get_movie response)
  cast?: CastMember[];
  crew?: CrewMember[];
};

export type MovieWatchState = {
  id: string;
  movie_id: string;
  user_id: string;
  watched: boolean;
  watched_at: string | null;
  position_seconds: number | null;
  rating: number | null;
  created_at: string;
  updated_at: string;
};

export type TvEpisodeFile = {
  id: string;
  episode_ids: string[];
  file_path: string;
  file_size: number | null;
  format: string;
  quality: string | null;
  languages: string | null;
  subtitles: string | null;
  source_type: string | null;
  source_metadata: Record<string, unknown> | null;
  available: boolean;
  created_at: string;
};

export type TvEpisode = {
  id: string;
  season_id: string;
  episode_identifier: string;
  episode_sort_order: number;
  title: string | null;
  description: string | null;
  still_url: string | null;
  air_date: string | null;
  /** Absent when only the broadcast day is known — never render a time from air_date alone. */
  air_time?: string | null;
  runtime: number | null;
  is_in_library: boolean;
  want_status: string;
  watched?: boolean | null;
  watch_position_seconds?: number | null;
  created_at: string;
  updated_at: string;
};

/**
 * Actions the bulk-episode endpoint accepts. `delete_files` strips the media but
 * keeps the episodes so they can be re-acquired; `delete_episodes` removes the
 * rows too, for episodes that should never have existed.
 */
export type BulkEpisodeAction =
  'delete_files' | 'delete_episodes' | 'want' | 'unwant' | 'mark_watched' | 'mark_unwatched';

export type BulkEpisodeResult = {
  episodes_affected: number;
  files_deleted: number;
  auto_want_disabled: boolean;
};

export type TvSeason = {
  id: string;
  show_id: string;
  season_identifier: string;
  season_sort_order: number;
  title: string | null;
  description: string | null;
  poster_url: string | null;
  air_date: string | null;
  episode_count: number | null;
  is_monitored: boolean;
  is_special: boolean;
  auto_want_mode: string;
  actual_episode_count?: number;
  /** Sports season-list endpoint only. */
  wanted_event_count?: number;
  in_library_event_count?: number;
  watched_event_count?: number;
  created_at: string;
  episodes: TvEpisode[];
};

export type TvShow = {
  id: string;
  title: string;
  sort_title: string | null;
  original_title: string | null;
  year: number | null;
  status: string | null;
  description: string | null;
  poster_url: string | null;
  backdrop_url: string | null;
  logo_url: string | null;
  identifiers: Record<string, unknown>;
  genres: string[];
  rating: number | null;
  rating_count: number | null;
  content_rating: string | null;
  kids_override?: boolean | null;
  kids_allowed?: boolean;
  network: string | null;
  is_monitored: boolean;
  auto_want_mode: string;
  quality_profile_id: string | null;
  preferred_languages: string[] | null;
  media_root_id: string | null;
  first_aired: string | null;
  last_aired: string | null;
  next_airing: string | null;
  runtime: number | null;
  total_episode_count: number | null;
  aired_episode_count?: number;
  in_library_count?: number;
  created_at: string;
  updated_at: string;
  streamed_at: string | null;
  sport_metadata?: Record<string, unknown> | null;
  /** Sports league detail only, totalled across every season. */
  event_count?: number;
  in_library_event_count?: number;
  wanted_event_count?: number;
  watched_event_count?: number;
  // Detail fields (present in get_show response)
  seasons?: TvSeason[];
  continue_episode?: ContinueEpisode | null;
  cast?: CastMember[];
  crew?: CrewMember[];
  in_watchlist?: boolean;
};

export type ContinueEpisode = {
  season_idx: number;
  episode_idx: number;
  episode_id: string;
  position_seconds: number;
};

export type TvWatchState = {
  id: string;
  episode_id: string;
  user_id: string;
  watched: boolean;
  watched_at: string | null;
  position_seconds: number | null;
  rating: number | null;
  created_at: string;
  updated_at: string;
};

export type TvShowWatchState = {
  watched_episodes: number;
  total_episodes: number;
  rating: number | null;
  last_watched_at: string | null;
};

// =============================================================================
// Video Sync Providers
// =============================================================================

export type VideoSyncProvider = {
  provider_type: string;
  enabled: boolean;
  config: Record<string, unknown>;
  display_name: string;
  description: string;
  website: string;
  uses_oauth: boolean;
};

export type TraktDeviceCode = {
  device_code: string;
  user_code: string;
  verification_url: string;
  expires_in: number;
  interval: number;
};

export type TraktPollResult = {
  status: 'authorized' | 'pending' | 'expired' | 'denied';
};

// =============================================================================
// Dashboard Types
// =============================================================================

export type RecentItem = {
  item_type: 'book' | 'movie' | 'episode' | 'event';
  id: string;
  title: string;
  subtitle: string;
  image_url: string | null;
  backdrop_url: string | null;
  link: string;
  added_at: string;
};

export type RecentBook = {
  id: string;
  title: string;
  authors: string;
  cover_url: string | null;
  added_at: string;
};

export type RecentMovie = {
  id: string;
  title: string;
  year: number | null;
  poster_url: string | null;
  backdrop_url: string | null;
  added_at: string;
};

export type RecentTvEntry = {
  show_id: string;
  show_title: string;
  show_poster_url: string | null;
  show_backdrop_url: string | null;
  season_identifier: string;
  episode_id: string;
  episode_identifier: string;
  episode_title: string | null;
  added_at: string;
};

export type WatchNextItem = {
  media_type: 'movie' | 'episode' | 'event';
  media_id: string;
  title: string;
  subtitle: string | null;
  image_url: string | null;
  backdrop_url: string | null;
  link: string;
  progress_percent: number | null;
  position_seconds: number | null;
  duration_seconds: number | null;
  updated_at: string;
};

export type ReadNextItem = {
  media_type: 'book' | 'audiobook';
  media_id: string;
  title: string;
  subtitle: string | null;
  image_url: string | null;
  link: string;
  progress_percent: number | null;
  position_seconds: number | null;
  duration_seconds: number | null;
  updated_at: string;
};

export type CalendarItem = {
  id: string;
  media_type: 'tv' | 'movie' | 'book' | 'sport';
  date: string;
  title: string;
  subtitle: string | null;
  image_url: string | null;
  link: string;
};

export type DashboardResponse = {
  read_next: ReadNextItem[];
  recent_books: RecentBook[];
  recent_ebooks: RecentBook[];
  recent_audiobooks: RecentBook[];
  recent_movies: RecentMovie[];
  recent_tv: RecentTvEntry[];
  recent_sports: RecentTvEntry[];
};

// =============================================================================
// Handler Types
// =============================================================================

// =============================================================================
// Migration Types (Sonarr/Radarr import)
// =============================================================================

export type MigrationService = 'sonarr' | 'radarr';

export type MigrationPreviewItem = {
  title: string;
  year: number | null;
  tmdb_id: string | null;
  tvdb_id: string | null;
  imdb_id: string | null;
  monitored: boolean;
  has_files: boolean;
  source_status: string | null;
  import_status: string;
  kobbler_id: string | null;
  source_id: number;
  episode_stats: MigrationEpisodeStats | null;
};

export type MigrationEpisodeStats = {
  total: number;
  with_file: number;
  monitored: number;
};

export type MigrationPreviewSummary = {
  total: number;
  new: number;
  existing: number;
  no_tmdb_id: number;
};

export type MigrationRootFolderMapping = {
  path: string;
  suggested_media_root_id: string | null;
  suggested_media_root_name: string | null;
};

export type MigrationPreviewResponse = {
  service: string;
  summary: MigrationPreviewSummary;
  items: MigrationPreviewItem[];
  root_folders: MigrationRootFolderMapping[];
};

export type MigrationSkippedItem = {
  title: string;
  reason: string;
};

export type MigrationImportResponse = {
  imported: number;
  skipped: number;
  skipped_reasons: MigrationSkippedItem[];
  scans_triggered: string[];
};

export type LoginHandler = (email: string, password: string) => Promise<void>;
export type NavigateHandler = (page: PageKey) => void;
export type BookSelectHandler = (bookId: string, event?: MouseEvent | KeyboardEvent) => void;
export type SaveEditsHandler = (bookId: string, payload: Record<string, unknown>) => Promise<void>;
export type ShelfActionHandler = (shelfId: string) => void;
export type CreateShelfHandler = (name: string) => Promise<string | null>;
export type SearchInputHandler = (value: string) => void;
export type SendToDeviceHandler = (deviceIds: string[], bookIds: string[]) => Promise<void>;

/** Author with optional Hardcover ID */
export interface AuthorInput {
  name: string;
  hardcover_id?: string;
}

export interface BookFormData {
  title: string;
  sort_title?: string;
  /** Authors with optional Hardcover IDs */
  authors: AuthorInput[];
  /** Series ID (when selected from dropdown) */
  series_id?: string;
  /** Series name (when typed/created, or for display) */
  series_name?: string | null;
  /** Hardcover series ID for enrichment */
  series_hardcover_id?: string | null;
  series_index?: number | null;
  publisher?: string | null;
  pubdate?: string | null;
  description?: string | null;
  language?: string | null;
  page_count?: number | null;
  identifiers?: Record<string, string>;
  genres?: string[];
  rating?: number | null;
  cover_url?: string | null;
  want?: boolean;
  narrator?: string | null;
  is_abridged?: boolean;
  want_format?: 'ebook' | 'audiobook' | 'both';
  /** Bulk-replace all series memberships */
  series_memberships?: SeriesMembership[];
}

export interface AuthorFormData {
  name: string;
  identifiers?: Record<string, string>;
  image_url?: string | null;
  bio?: string | null;
  is_monitored?: boolean;
  auto_want_mode?: string;
  coverFile?: File;
}

export interface SeriesFormData {
  name: string;
  identifiers?: Record<string, string>;
  image_url?: string | null;
  description?: string | null;
  total_book_count?: number | null;
  is_monitored?: boolean;
  auto_want_mode?: string;
  coverFile?: File;
}

/** A Hardcover link reconcile could not settle on its own. */
export interface ProviderLinkIssue {
  entity_type: 'author' | 'series' | 'book';
  entity_id: string;
  name: string | null;
  provider: string;
  external_id: string;
  /** deleted: gone upstream but holds user data, so unlinked. merge_conflict: merged into an entity that also holds user data. */
  kind: 'deleted' | 'merge_conflict';
  canonical_external_id: string | null;
  conflicting_entity_id: string | null;
  detected_at: string;
}

// =============================================================================
// Media Roots
// =============================================================================

export type MediaRootStats = {
  total_files: number;
  added: number;
  review: number;
  ambiguous: number;
  unmatched: number;
  ignored: number;
  pending: number;
  identified: number;
};

export type NasHealthSnapshot = {
  status: 'healthy' | 'degraded' | 'unreachable' | 'disabled';
  reconnects_last_hour: number;
  note: string | null;
};

export type VisionHealthSnapshot = {
  status: 'healthy' | 'degraded' | 'failing' | 'idle' | 'disabled';
  attempted: number;
  ok: number;
  failed: number;
  last_error: string | null;
  note: string | null;
};

export type MediaRoot = {
  id: string;
  path: string;
  name: string;
  media_type: 'tv' | 'movie' | 'book' | 'audiobook' | 'sport';
  enabled: boolean;
  auto_add: boolean;
  scan_status: 'idle' | 'scanning' | 'error';
  last_scan_at: string | null;
  scan_error: string | null;
  error_count: number;
  naming_template: string | null;
  write_sidecars: boolean;
  write_images: boolean;
  default_entitlement_scope: string;
  is_default: boolean;
  max_concurrent_io: number;
  scan_interval_seconds: number | null;
  last_scan_completed_at: string | null;
  created_at: string;
  updated_at: string;
  stats?: MediaRootStats;
  deferred_ingest_count: number;
};

export type MediaRootFile = {
  id: string;
  media_root_id: string;
  file_path: string;
  file_name: string;
  file_size: number | null;
  match_status:
    'pending' | 'identified' | 'added' | 'review' | 'ambiguous' | 'unmatched' | 'ignored';
  entity_type: string | null;
  entity_id: string | null;
  interpretation: Record<string, unknown> | null;
  match_candidates: MatchCandidate[] | null;
  sidecar_metadata: Record<string, unknown> | null;
  available: boolean;
  created_at: string;
};

export type MatchCandidate = {
  provider: string;
  provider_id: string;
  title: string;
  year: number | null;
  poster_url?: string | null;
  cover_url?: string | null;
  authors?: string[];
  isbn?: string | null;
  confidence?: number;
  resolved?: boolean;
};

/** Lightweight file response (no match_candidates / sidecar_metadata). */
export type MediaRootFileLite = {
  id: string;
  media_root_id: string;
  file_path: string;
  file_name: string;
  file_size: number | null;
  match_status: MediaRootFile['match_status'];
  entity_type: string | null;
  entity_id: string | null;
  interpretation: Record<string, unknown> | null;
  available: boolean;
  created_at: string;
};

export type EntitySubgroupSummary = {
  season: number;
  label: string;
  file_count: number;
  review_count: number;
};

export type EntityGroupSummary = {
  group_key: string;
  label: string;
  year: number | null;
  file_count: number;
  review_count: number;
  unmatched_count: number;
  ambiguous_count: number;
  added_count: number;
  poster_url: string | null;
  subgroups: EntitySubgroupSummary[];
  group_status: 'review' | 'ambiguous' | 'added' | 'mixed';
  authors: string[] | null;
  series_name: string | null;
  series_index: number | null;
  provider: string | null;
  provider_id: string | null;
  confidence: number | null;
  cover_url: string | null;
};

export type EntityGroupsResponse = {
  groups: EntityGroupSummary[];
  ungrouped_count: number;
};

export type StatusCountsResponse = {
  counts: Record<string, number>;
};

export type FileListLiteResponse = {
  items: MediaRootFileLite[];
  total_count: number;
};

export type DirectoryEntry = {
  name: string;
  path: string;
  file_count: number;
  review_count: number;
  ambiguous_count: number;
  unmatched_count: number;
  added_count: number;
};

// =============================================================================
// Video Playback Types
// =============================================================================

export type PlaybackMethod = 'direct_play' | 'remux' | 'audio_transcode' | 'video_transcode';

export type PlaybackSubtitleTrack = {
  track_id: string;
  stream_index: number;
  codec: string;
  language: string | null;
  language_name: string | null;
  title: string | null;
  url: string | null;
  format: 'vtt' | 'ass' | 'burn_in';
  is_default: boolean;
  is_forced: boolean;
  is_hearing_impaired: boolean;
  requires_transcode: boolean;
};

export type PlaybackAudioTrack = {
  stream_index: number;
  codec: string;
  channels: number;
  language: string | null;
  language_name: string | null;
  title: string | null;
  is_default: boolean;
};

export type PlaybackSessionResponse = {
  session_id: string;
  method: PlaybackMethod;
  playlist_url: string | null;
  direct_url: string | null;
  duration_seconds: number;
  subtitles: PlaybackSubtitleTrack[];
  audio_tracks: PlaybackAudioTrack[];
  thumbnails_url?: string;
  lufs?: number;
  chapters?: VideoChapter[];
  skip_segments?: SkipSegment[];
  start_time?: number;
  media_id?: string;
  media_type?: string;
  preferred_audio_stream_index?: number;
  /** Where the picture sits inside the frame when bars are baked into the encode, as fractions of it. */
  picture?: { x: number; y: number; width: number; height: number };
};

export type VideoChapter = {
  index: number;
  title: string | null;
  start_seconds: number;
  end_seconds: number;
};

export type SkipSegment = {
  segment_type: 'intro' | 'credits' | 'post_credits_scene' | 'recap' | 'ad_break';
  start_seconds: number;
  end_seconds: number;
  confidence?: number;
  source: string;
};

export type NextEpisodeInfo = {
  episode_id: string;
  title: string | null;
  episode_identifier: string;
  season_identifier: string;
  show_title: string;
  still_url: string | null;
  file_id: string | null;
};

/** A position one of this user's devices saved — the `watch_progress` SSE event. */
export type WatchProgress = {
  media_type: 'movie' | 'episode';
  media_id: string;
  /** The show for an episode, the movie itself for a movie. */
  scope_id: string;
  position_seconds: number;
  duration_seconds: number | null;
  watched: boolean;
  client_id: string;
  client_label: string | null;
  saved_at: string;
  episode?: {
    show_title: string;
    title: string | null;
    season_identifier: string;
    episode_identifier: string;
    still_url: string | null;
    file_id: string | null;
  };
  movie_title?: string | null;
};

export type PlaybackHandoffResponse = {
  scope_id: string | null;
  latest: WatchProgress | null;
};

// =============================================================================
// Audiobook Types
// =============================================================================

export type AudiobookChapter = {
  id: string;
  chapter_index: number;
  title: string | null;
  start_seconds: number;
  end_seconds: number;
  file_path?: string;
};

export type UnifiedChapter = {
  id: string;
  chapter_index: number;
  title: string | null;
  start_seconds: number;
  end_seconds: number;
  file_id: string;
  file_path: string;
  global_start_seconds: number;
  global_end_seconds: number;
  source: string;
};

// =============================================================================
// IPTV Types
// =============================================================================

export type IptvProvider = {
  id: string;
  name: string;
  url: string;
  username: string;
  preferred_format: string;
  sync_interval_secs: number;
  enabled: boolean;
  account_info: Record<string, unknown>;
  last_sync_at: string | null;
  last_sync_error: string | null;
  created_at: string;
  updated_at: string;
};

export type IptvChannel = {
  id: string;
  name: string;
  category: string | null;
  language: string | null;
  country: string | null;
  icon_url: string | null;
  enabled: boolean;
  source_count: number;
  current_program: string | null;
  has_epg: boolean;
};

export type IptvSource = {
  id: string;
  channel_id: string;
  provider_id: string;
  provider_name: string | null;
  name: string | null;
  quality: string;
  priority: number;
  health_score: number;
  stream_type: string;
  remote_category_name: string | null;
};

export type IptvEpgEntry = {
  id: string;
  channel_id: string;
  channel_name: string;
  title: string;
  description: string | null;
  categories: string[];
  start_time: string;
  stop_time: string;
};

export type IptvGroup = {
  id: string;
  name: string;
  slug: string;
  icon: string | null;
  display_order: number;
  channel_count: number;
  enabled: boolean;
};

export type IptvRecording = {
  id: string;
  channel_id: string;
  title: string;
  status: string;
  start_time: string;
  end_time: string;
  file_path: string | null;
  file_size_bytes: number | null;
  error_message: string | null;
  duration_seconds: number | null;
};

export type IptvSeriesRule = {
  id: string;
  title_pattern: string;
  channel_id: string | null;
  record_new_only: boolean;
  keep_count: number | null;
  enabled: boolean;
};

export type IptvUnmatchedSource = {
  id: string;
  provider_id: string;
  name: string | null;
  language: string | null;
  suggested_channel_id: string | null;
  suggested_confidence: number | null;
};

export interface IptvProgrammeWatch {
  id: string;
  title_pattern: string;
  match_mode: string;
  enabled: boolean;
  created_at: string;
}

export interface IptvWatchMatch {
  watch_id: string;
  channel_id: string;
  channel_name: string;
  channel_icon_url: string | null;
  epg_title: string;
  start_time: string;
  stop_time: string;
  is_live: boolean;
}

export type LivetvTuner = {
  id: string;
  device_id: string;
  friendly_name: string;
  model_number: string | null;
  firmware_version: string | null;
  base_url: string;
  tuner_count: number;
  last_scan_at: string | null;
  last_guide_sync_at: string | null;
  channel_count: number;
  enabled_channel_count: number;
  leases_in_use: number;
  created_at: string;
  updated_at: string;
};

export type LivetvDiscoveredTuner = {
  device_id: string;
  local_ip: string | null;
  base_url: string;
  already_added: boolean;
};

export type LivetvChannel = {
  id: string;
  tuner_id: string;
  lcn: number | null;
  scan_name: string | null;
  callsign: string | null;
  frequency: number | null;
  program_number: number | null;
  onid: number | null;
  tsid: number | null;
  modulation: string | null;
  video_codec: string | null;
  audio_codec: string | null;
  hd: boolean;
  signal_strength: number | null;
  signal_quality: number | null;
  last_seen_at: string | null;
  enabled: boolean;
  favourite: boolean;
  sort_order: number | null;
  display_name: string | null;
  logo_url: string | null;
  kind: string | null;
  region: string | null;
  network: string | null;
  name: string;
  mux_label: string | null;
  type_label: string;
  has_guide: boolean;
  guide_entry_count: number;
  /** Other members of this channel's stack, best first; filled only with `stacked=true`. */
  alternate_ids: string[];
  now: LivetvGuideNow | null;
  created_at: string;
  updated_at: string;
};

export type LivetvCuratedPatch = {
  enabled?: boolean;
  favourite?: boolean;
  sort_order?: number;
  display_name?: string | null;
  logo_url?: string | null;
  kind?: string | null;
  region?: string | null;
  network?: string | null;
};

export type LivetvFacet = {
  value: string;
  label: string;
  count: number;
};

export type LivetvFacets = {
  network: LivetvFacet[];
  region: LivetvFacet[];
  type: LivetvFacet[];
  guide: LivetvFacet[];
  status: LivetvFacet[];
  mux: LivetvFacet[];
};

export type LivetvGuideEntry = {
  id: string;
  channel_id: string;
  start_time: string;
  end_time: string;
  title: string;
  episode_number: string | null;
  episode_title: string | null;
  synopsis: string | null;
  original_airdate: string | null;
  series_id: string | null;
  genres: string[];
  image_url: string | null;
  poster_url: string | null;
  is_first: boolean;
  episode_key: string | null;
  episode_key_kind: string | null;
  duration_seconds: number;
  is_now: boolean;
  progress_pct: number | null;
  recording: { id: string; status: string } | null;
  repeat_count: number;
};

export type LivetvGuideChannel = {
  channel: LivetvChannel;
  entries: LivetvGuideEntry[];
};

export type LivetvGuideNow = {
  channel: LivetvChannel;
  now: LivetvGuideEntry | null;
  next: LivetvGuideEntry | null;
  minutes_left: number | null;
};

export type LivetvBulkAction = 'enable' | 'hide' | 'favourite' | 'unfavourite';

export type LivetvLease = {
  id: string;
  tuner_id: string;
  channel_id: string;
  channel_name: string;
  tuner_index: number | null;
  holder_kind: string;
  session_id: string | null;
  recording_id: string | null;
  user_id: string | null;
  acquired_at: string;
  expires_at: string | null;
};

export type LivetvSession = {
  session_id: string;
  channel_id: string;
  method: 'direct' | 'transcode';
  playlist_url: string | null;
  direct_url: string | null;
  video_codec: string | null;
  audio_codec: string | null;
  hd: boolean;
  width: number | null;
  height: number | null;
  buffer_start_time: string;
  is_live: boolean;
  duration_seconds: number;
  lease: LivetvLease;
  tuners_in_use: number;
  tuner_count: number;
  now: LivetvGuideEntry | null;
  next: LivetvGuideEntry | null;
};

export type LivetvBufferState = {
  start_time: string;
  end_time: string;
  live_edge_seconds: number;
  duration_seconds: number;
  retained_seconds: number;
  cap_seconds: number;
};

export type LivetvRecordingStatus =
  'scheduled' | 'recording' | 'completed' | 'failed' | 'cancelled';

export type LivetvRecording = {
  id: string;
  channel_id: string;
  channel_name: string;
  guide_id: string | null;
  series_rule_id: string | null;
  title: string;
  episode_number: string | null;
  episode_title: string | null;
  episode_key: string | null;
  series_id: string | null;
  synopsis: string | null;
  scheduled_start: string;
  scheduled_end: string;
  actual_start: string | null;
  actual_end: string | null;
  padding_before_secs: number;
  padding_after_secs: number;
  status: LivetvRecordingStatus;
  tuner_index: number | null;
  output_dir: string | null;
  duration_seconds: number | null;
  video_codec: string | null;
  audio_codec: string | null;
  container: string | null;
  size_bytes: number | null;
  episode_id: string | null;
  movie_id: string | null;
  moved_to_guide_id: string | null;
  image_url: string | null;
  error: string | null;
  created_at: string;
  updated_at: string;
  is_in_progress: boolean;
  seconds_on_disk: number | null;
  playable_from_start: boolean;
  ad_breaks: LivetvAdBreak[];
  provenance: { channel_name: string; lcn: number | null; aired: string } | null;
  ad_posture: LivetvAdPosture;
  ad_skip_disabled: boolean;
};

export type LivetvWatchEventInput = {
  channel_id: string;
  guide_id?: string | null;
  started_at: string;
  ended_at: string;
  watched_secs: number;
  programme_secs?: number | null;
  source: string;
};

export type LivetvAdPosture = 'shade' | 'button' | 'auto';

export type LivetvAdBreakMethod = 'fingerprint' | 'black_silence' | 'logo' | 'manual' | string;

export type LivetvAdBreak = {
  id: string;
  recording_id: string;
  start_seconds: number;
  end_seconds: number;
  method: LivetvAdBreakMethod;
  confidence: number | null;
  ads_known: number | null;
  ads_total: number | null;
  corrected_at: string | null;
};

export type LivetvClashAction = 'move' | 'drop' | 'ask';

export type LivetvSeriesRule = {
  id: string;
  series_id: string;
  title: string;
  enabled: boolean;
  channel_ids: string[];
  weekdays: number[];
  window_start: string | null;
  window_end: string | null;
  new_only: boolean;
  min_season: number | null;
  keep_count: number | null;
  keep_until: string | null;
  keep_until_date: string | null;
  padding_before_secs: number;
  padding_after_secs: number;
  on_clash: LivetvClashAction;
  priority: number;
  queued_count: number;
  created_at: string;
  updated_at: string;
};

export type LivetvSeriesRuleInput = {
  series_id: string;
  title: string;
  enabled: boolean;
  channel_ids: string[];
  weekdays: number[];
  window_start: string | null;
  window_end: string | null;
  new_only: boolean;
  min_season: number | null;
  keep_count: number | null;
  keep_until: string | null;
  padding_before_secs: number;
  padding_after_secs: number;
  on_clash: LivetvClashAction;
  priority: number;
};

export type LivetvClusterMember = {
  guide_id: string;
  channel_id: string;
  channel_name: string;
  lcn: number | null;
  hd: boolean;
  start_time: string;
  action: 'record' | 'skip';
  reason: string | null;
};

export type LivetvCluster = {
  label: string;
  start_time: string;
  members: LivetvClusterMember[];
};

export type LivetvSeriesRulePreview = {
  clusters: LivetvCluster[];
  would_record: number;
  suppressed: number;
};

export type LivetvOnNowItem = {
  channel: LivetvChannel;
  guide: LivetvGuideEntry;
  minutes_left: number | null;
  starts_in_minutes: number | null;
  reason: string | null;
  confidence?: number | null;
};

export type LivetvOnNowGroup = {
  key: 'usual' | 'series' | 'timeofday' | 'channel' | 'genre';
  title: string;
  subtitle: string | null;
  items: LivetvOnNowItem[];
};

export type LivetvClashNotice = {
  guide: LivetvGuideEntry;
  reason: string;
  alternative: LivetvGuideEntry | null;
};

export type LivetvOnNow = {
  generated_at: string;
  tuner_notice: LivetvClashNotice | null;
  groups: LivetvOnNowGroup[];
};

export interface TranscriptBacklogTotals {
  books: number;
  files: number;
  hours: number;
}

export interface TranscriptRefreshJob {
  id: string;
  status: string;
  progress: number | null;
  result: { batches?: number; remaining_hours?: number } | null;
}

export interface TranscriptBacklog {
  missing: TranscriptBacklogTotals;
  outdated: TranscriptBacklogTotals;
  refresh_job: TranscriptRefreshJob | null;
}

export type BookCastState =
  'chapters' | 'bible' | 'voices' | 'casting' | 'rendering' | 'done' | 'failed';

export interface BookCastVoice {
  character_id: string;
  from_chapter: number;
  preset_id: string | null;
  design_preset_id: string | null;
  approved_at: string | null;
  preset_name: string | null;
  has_clip: boolean;
  description: string | null;
}

export interface BookCastArcStep {
  from_chapter: number;
  change: string;
  voice: string;
}

export interface BookCastName {
  name: string;
  lines: number;
}

export interface BookCastCharacter {
  id: string;
  name: string;
  aliases: string[];
  names: BookCastName[];
  bible: Record<string, unknown> & { arc?: BookCastArcStep[] };
  line_count: number;
  pooled: boolean;
  generic: boolean;
  narrator: boolean;
  samples: string[];
  voices: BookCastVoice[];
}

export interface BookCastPresetChoice {
  id: string;
  name: string;
  has_clip: boolean;
}

export interface BookCastUnapproved {
  name: string;
  character_id: string | null;
  from_chapter: number | null;
  preset_id: string | null;
  speakers: string[];
}

/** Why a rendered chapter no longer sounds as the cast reads now. */
export interface BookCastStaleChapter {
  chapter_index: number;
  attribution: boolean;
  voices: { character_id: string; name: string | null }[];
}

export interface BookCastRender {
  chapters: { total: number; done: number; stale: number; missing: number };
  stale: BookCastStaleChapter[];
  unapproved: BookCastUnapproved[];
  audiobook: { book_id: string; file_id: string } | null;
  eta_seconds: number | null;
}

export interface BookCastApproving {
  id: string;
  status: string;
  progress: number | null;
}

export interface BookCast {
  state: BookCastState;
  error: string | null;
  updated_at: string | null;
  chapters: { total: number; done: number };
  lines: number;
  unresolved: number;
  characters: BookCastCharacter[];
  unattached: { speaker: string; lines: number }[];
  presets: BookCastPresetChoice[];
  render: BookCastRender;
  eta: { analysis_seconds: number | null };
  approving: BookCastApproving | null;
}

export interface BookCastLineContext {
  before: string | null;
  pre: string;
  quote: string;
  post: string;
  after: string | null;
}

export interface BookCastLine {
  segment_index: number;
  span_start: number;
  span_end: number;
  chapter_index: number;
  speaker: string | null;
  character_id: string | null;
  cue: string | null;
  source: 'gemma' | 'beefy' | 'user';
  not_speech: boolean;
  text: string | null;
  context: BookCastLineContext | null;
  voice: BookCastVoice | null;
}

export interface BookCastUnattributedLine {
  segment_index: number;
  span_start: number;
  span_end: number;
  chapter_index: number;
  chapter_title: string | null;
  speaker: string | null;
  cue: string | null;
  text: string | null;
  context: BookCastLineContext | null;
}

export interface BookCastTake {
  segment_index: number;
  span_start: number | null;
  chapter_index: number;
  text: string;
  cue: string | null;
}
