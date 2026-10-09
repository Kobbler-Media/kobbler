/**
 * Shared formatting utilities for Kobbler web
 */

/**
 * Format a date string to a localized string
 * @param dateStr - ISO date string or null
 * @param fallback - Fallback text when dateStr is null (default: 'Never')
 * @returns Formatted date string
 */
export function formatDate(dateStr: string | null, fallback = 'Never'): string {
  if (!dateStr) return fallback;
  return new Date(dateStr).toLocaleString();
}

/**
 * Format a date as a relative time (e.g., "2 hours ago")
 * @param dateStr - ISO date string
 * @returns Relative time string
 */
export function formatDistanceToNow(dateStr: string): string {
  const date = new Date(dateStr);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffSeconds = Math.floor(diffMs / 1000);
  const diffMinutes = Math.floor(diffSeconds / 60);
  const diffHours = Math.floor(diffMinutes / 60);
  const diffDays = Math.floor(diffHours / 24);

  if (diffSeconds < 60) {
    return 'just now';
  } else if (diffMinutes < 60) {
    return `${diffMinutes} minute${diffMinutes === 1 ? '' : 's'} ago`;
  } else if (diffHours < 24) {
    return `${diffHours} hour${diffHours === 1 ? '' : 's'} ago`;
  } else if (diffDays < 30) {
    return `${diffDays} day${diffDays === 1 ? '' : 's'} ago`;
  } else {
    return date.toLocaleDateString();
  }
}

/**
 * Format bytes to a human-readable size string
 * @param bytes - Number of bytes
 * @returns Formatted size string (e.g., "1.5 MB")
 */
export function formatSize(bytes: number | null | undefined, fallback = ''): string {
  if (!bytes) return fallback;
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`;
}

/**
 * Get the CSS class for a status value
 * @param status - Status string
 * @returns CSS class name for the status
 */
export function getStatusColor(status: string): string {
  switch (status) {
    case 'extracting':
      return 'extracting';
    case 'pending':
      return 'pending';
    case 'converting':
      return 'converting';
    case 'approved':
      return 'approved';
    case 'rejected':
      return 'rejected';
    case 'error':
      return 'error';
    default:
      return '';
  }
}

/**
 * Format a series label with optional authors
 * @param name - Series name
 * @param authors - Array of author names
 * @returns Formatted series label
 */
export function formatSeriesLabel(name: string, authors: string[]): string {
  return authors.length > 0 ? `${name} (${authors.join(', ')})` : name;
}

/**
 * Format a count with the correct singular/plural form
 * @param count - The number to display
 * @param singular - Singular form of the word (e.g., "book")
 * @param plural - Optional plural form (defaults to singular + "s")
 * @returns Formatted string (e.g., "1 book" or "5 books")
 */
export function pluralize(count: number, singular: string, plural?: string): string {
  const word = count === 1 ? singular : (plural ?? `${singular}s`);
  return `${count} ${word}`;
}

/**
 * Format seconds into a duration string like "1:23:45" or "12:34"
 * @param totalSeconds - Total number of seconds
 * @returns Formatted duration string
 */
export function formatDuration(totalSeconds: number): string {
  const h = Math.floor(totalSeconds / 3600);
  const m = Math.floor((totalSeconds % 3600) / 60);
  const s = Math.floor(totalSeconds % 60);
  if (h > 0) {
    return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
  }
  return `${m}:${String(s).padStart(2, '0')}`;
}

/**
 * Format season/episode identifiers as a scene code (e.g., "S01E02").
 * Non-numeric identifiers (specials, date-based events) are passed through.
 */
export function formatEpisodeCode(seasonIdentifier: string, episodeIdentifier: string): string {
  const s = parseInt(seasonIdentifier, 10);
  const e = parseInt(episodeIdentifier, 10);
  if (Number.isNaN(s) || Number.isNaN(e)) {
    return `${seasonIdentifier} ${episodeIdentifier}`.trim();
  }
  return `S${String(s).padStart(2, '0')}E${String(e).padStart(2, '0')}`;
}

/**
 * Label an episode the way release names spell it (e.g., "The Show S01E02"),
 * suitable for both display and as a manual release-search query.
 */
export function formatEpisodeSearchTitle(
  showTitle: string | null | undefined,
  seasonIdentifier: string,
  episodeIdentifier: string
): string {
  const code = formatEpisodeCode(seasonIdentifier, episodeIdentifier);
  return showTitle ? `${showTitle} ${code}` : code;
}

// === Provider Formatting ===

/**
 * Provider display configuration
 */
export interface ProviderConfig {
  /** Short code (2-3 chars) for compact display */
  code: string;
  /** Full display name */
  name: string;
  /** CSS color variable or hex color */
  color: string;
}

/**
 * Metadata provider configurations
 * Keys support multiple variants (e.g., 'hardcover', 'hardcover_id')
 */
const PROVIDER_CONFIGS: Record<string, ProviderConfig> = {
  hardcover: { code: 'HC', name: 'Hardcover', color: 'var(--monitored)' },
  hardcover_id: { code: 'HC', name: 'Hardcover', color: 'var(--monitored)' },
  googlebooks: { code: 'GB', name: 'Google Books', color: 'var(--muted)' },
  google_books: { code: 'GB', name: 'Google Books', color: 'var(--muted)' },
  openlibrary: { code: 'OL', name: 'Open Library', color: 'var(--success)' },
  open_library: { code: 'OL', name: 'Open Library', color: 'var(--success)' },
  goodreads: { code: 'GR', name: 'Goodreads', color: 'var(--warning)' },
  isbn: { code: 'ISBN', name: 'ISBN', color: 'var(--ink)' },
  isbn13: { code: 'ISBN', name: 'ISBN-13', color: 'var(--ink)' },
  isbn10: { code: 'ISBN', name: 'ISBN-10', color: 'var(--ink)' },
  amazon: { code: 'AZ', name: 'Amazon', color: 'var(--warning)' },
  asin: { code: 'ASIN', name: 'Amazon ASIN', color: 'var(--warning)' },
  google: { code: 'GB', name: 'Google Books', color: 'var(--muted)' },
  librarything: { code: 'LT', name: 'LibraryThing', color: 'var(--muted)' },
  // Video metadata providers — colors chosen for easy visual distinction
  tmdb: { code: 'TMDB', name: 'TMDB', color: '#01b4e4' },
  tvmaze: { code: 'TVMz', name: 'TVMaze', color: '#3c948b' },
  omdb: { code: 'OMDB', name: 'OMDb', color: '#e6b91e' },
  imdb: { code: 'IMDB', name: 'IMDb', color: '#f5c518' },
  tvdb: { code: 'TVDB', name: 'TheTVDB', color: '#9146ff' }
};

/**
 * Normalize a provider key to a standard form
 */
function normalizeProviderKey(provider: string): string {
  return provider.toLowerCase().replace(/[-_\s]/g, '');
}

/**
 * Get provider configuration by name
 * @param provider - Provider name (case-insensitive, handles variants)
 * @returns ProviderConfig or undefined if not found
 */
export function getProviderConfig(provider: string): ProviderConfig | undefined {
  const normalized = normalizeProviderKey(provider);
  // Try exact match first
  if (PROVIDER_CONFIGS[provider]) return PROVIDER_CONFIGS[provider];
  // Try normalized match
  for (const [key, config] of Object.entries(PROVIDER_CONFIGS)) {
    if (normalizeProviderKey(key) === normalized) return config;
  }
  return undefined;
}

/**
 * Get short code for a provider (2-3 chars)
 * @param provider - Provider name
 * @returns Short code (e.g., 'HC' for Hardcover)
 */
export function getProviderCode(provider: string): string {
  const config = getProviderConfig(provider);
  if (config) return config.code;
  // Fallback: first 2 chars uppercased
  return provider.substring(0, 2).toUpperCase();
}

/**
 * Get display name for a provider
 * @param provider - Provider name
 * @returns Human-readable name (e.g., 'Hardcover')
 */
export function getProviderName(provider: string): string {
  const config = getProviderConfig(provider);
  if (config) return config.name;
  // Fallback: capitalize first letter
  return provider.charAt(0).toUpperCase() + provider.slice(1);
}

/**
 * Get color for a provider (supports both names like "hardcover" and codes like "HC")
 * @param provider - Provider name or code
 * @returns CSS color value
 */
export function getProviderColor(provider: string): string {
  const config = getProviderConfig(provider);
  if (config) return config.color;
  // Fallback: try matching by code (e.g., "HC" → hardcover)
  const upper = provider.toUpperCase();
  for (const cfg of Object.values(PROVIDER_CONFIGS)) {
    if (cfg.code === upper) return cfg.color;
  }
  return 'var(--muted)';
}

const SHOW_STATUS_LABELS: Record<string, string> = {
  continuing: 'Continuing',
  ended: 'Ended',
  canceled: 'Cancelled',
  upcoming: 'Upcoming'
};

/** Pretty-print a normalized TV show status. */
export function formatShowStatus(status: string): string {
  return SHOW_STATUS_LABELS[status] ?? status;
}
