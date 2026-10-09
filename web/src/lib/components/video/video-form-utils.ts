/**
 * Shared helpers for video form SourceField integration.
 * Mirrors BookForm's source value logic for movie/TV search results.
 */
import type { MovieSearchResult, TvSearchResult, VideoIdentifiers } from '$lib/types';
import { getProviderCode, getProviderName } from '$lib/utils/format';

export type VideoSourceValue = {
  source: string; // Provider code (e.g., 'TMDB')
  sourceName: string; // Provider name (e.g., 'TMDB')
  value: string;
};

type VideoResult = MovieSearchResult | TvSearchResult;

/**
 * Build per-field source values from search results with provider badges.
 * Transform converts the raw field value to a display string.
 */
export function buildVideoSourceValues(
  field: string,
  results: VideoResult[],
  transform?: (val: unknown) => string
): VideoSourceValue[] {
  if (!results?.length) return [];
  const values: VideoSourceValue[] = [];
  for (const result of results) {
    const rawVal = (result as Record<string, unknown>)[field];
    if (rawVal !== null && rawVal !== undefined && rawVal !== '') {
      const strVal = transform ? transform(rawVal) : String(rawVal);
      if (strVal) {
        values.push({
          source: getProviderCode(result.provider),
          sourceName: getProviderName(result.provider),
          value: strVal
        });
      }
    }
  }
  return values;
}

/**
 * Get highest-priority value for a field from search results.
 * Results should already be in priority order.
 */
export function getVideoInitialByPriority(field: string, results: VideoResult[]): string {
  if (!results?.length) return '';
  for (const result of results) {
    const val = (result as Record<string, unknown>)[field];
    if (val !== null && val !== undefined && val !== '') {
      return String(val);
    }
  }
  return '';
}

/**
 * Get the provider code of the first result that has a non-empty value for the field.
 */
export function getVideoInitialSourceForField(
  field: string,
  results: VideoResult[]
): string | undefined {
  if (!results?.length) return undefined;
  for (const result of results) {
    const val = (result as Record<string, unknown>)[field];
    if (val !== null && val !== undefined && val !== '') {
      return getProviderCode(result.provider);
    }
  }
  return undefined;
}

/**
 * Merge identifiers from search results with existing entity identifiers.
 * Existing identifiers take precedence (won't be overwritten).
 */
export function buildVideoIdentifiers(
  results: VideoResult[],
  existingIdentifiers?: Record<string, unknown>,
  /** When true, search result identifiers override existing ones for the same key. */
  resultsWin?: boolean
): Array<{ key: string; value: string }> {
  const ids: Array<{ key: string; value: string }> = [];
  const seen = new Set<string>();

  // Collect search result identifiers
  const fromResults = new Map<string, string>();
  if (results?.length) {
    for (const result of results) {
      if (result.identifiers) {
        const identifiers = result.identifiers as VideoIdentifiers;
        const entries: [string, string | null | undefined][] = [
          ['tmdb_id', identifiers.tmdb_id],
          ['imdb_id', identifiers.imdb_id],
          ['tvmaze_id', identifiers.tvmaze_id],
          ['tvdb_id', identifiers.tvdb_id]
        ];
        for (const [key, value] of entries) {
          if (value && !fromResults.has(key)) {
            fromResults.set(key, value);
          }
        }
      }
      if (result.external_id && result.provider) {
        const key = `${result.provider}_id`;
        if (!fromResults.has(key)) {
          fromResults.set(key, result.external_id);
        }
      }
    }
  }

  // Add existing identifiers, but let search results override when resultsWin is set
  if (existingIdentifiers) {
    for (const [key, value] of Object.entries(existingIdentifiers)) {
      if (value != null && String(value)) {
        const effectiveValue =
          resultsWin && fromResults.has(key) ? fromResults.get(key)! : String(value);
        ids.push({ key, value: effectiveValue });
        seen.add(key);
      }
    }
  }

  // Add remaining search result identifiers not already covered by existing
  for (const [key, value] of fromResults) {
    if (!seen.has(key)) {
      ids.push({ key, value });
      seen.add(key);
    }
  }

  return ids;
}

/**
 * Build cover options from search results for CoverSelector.
 */
export function buildVideoCoverOptions(
  results: VideoResult[],
  field: 'poster_url' | 'backdrop_url' | 'logo_url' = 'poster_url'
): Array<{ url: string; provider: string }> {
  if (!results?.length) return [];
  const options: Array<{ url: string; provider: string }> = [];
  for (const result of results) {
    const url = (result as Record<string, unknown>)[field];
    if (typeof url === 'string' && url) {
      options.push({ url, provider: getProviderName(result.provider) });
    }
  }
  return options;
}
