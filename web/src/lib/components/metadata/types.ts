import type { MetadataResult } from '#lib/api/client.js';
import { getProviderName } from '#lib/utils/format.js';

export interface SeriesBook {
  title: string;
  authors: string[];
  series_index: number | null;
  isbn: string | null;
  cover_url: string | null;
  publish_date: string | null;
  external_id: string | null;
}

export interface SeriesResult {
  provider: string;
  name: string;
  authors: string[];
  description: string | null;
  book_count: number | null;
  image_url: string | null;
  books: SeriesBook[];
  external_id: string | null;
  confidence: number;
}

export interface AuthorResult {
  provider: string;
  external_id: string;
  name: string;
  image_url: string | null;
  /** Series names this author has written (for identification in search results) */
  series_names: string[];
  book_count: number | null;
  confidence: number;
}

export type OverrideableField =
  | 'title'
  | 'authors'
  | 'description'
  | 'cover_url'
  | 'isbn'
  | 'publisher'
  | 'publish_date'
  | 'series'
  | 'page_count'
  | 'rating';

export interface FieldOption {
  provider: string;
  value: unknown;
  index: number;
}

// Re-export from shared utility for backward compatibility
export const getProviderLabel = getProviderName;

export function getConfidenceColor(confidence: number): string {
  if (confidence >= 0.8) return 'high';
  if (confidence >= 0.5) return 'medium';
  return 'low';
}

export function getFieldOptions(
  field: OverrideableField,
  selectedForMerge: number[],
  results: MetadataResult[]
): FieldOption[] {
  return selectedForMerge
    .map((i) => ({
      provider: results[i].provider,
      value: results[i][field],
      index: i
    }))
    .filter((opt) => {
      if (opt.value === null || opt.value === undefined) return false;
      if (typeof opt.value === 'string' && opt.value === '') return false;
      if (Array.isArray(opt.value) && opt.value.length === 0) return false;
      return true;
    });
}

export function buildMergedResult(
  selectedForMerge: number[],
  results: MetadataResult[],
  fieldOverrides: Partial<Record<OverrideableField, number>>
): MetadataResult | null {
  if (selectedForMerge.length < 2) {
    return null;
  }

  const getSelectedIndex = (field: OverrideableField): number | null => {
    if (fieldOverrides[field] !== undefined) {
      return fieldOverrides[field]!;
    }
    const options = getFieldOptions(field, selectedForMerge, results);
    return options.length > 0 ? options[0].index : null;
  };

  const getFieldValue = <T>(field: OverrideableField): T | null => {
    const idx = getSelectedIndex(field);
    if (idx === null) return null;
    return results[idx][field] as T;
  };

  // Build merged result
  const merged: MetadataResult = {
    provider: 'merged',
    title: getFieldValue<string>('title') || '',
    authors: getFieldValue<string[]>('authors') || [],
    isbn: getFieldValue<string>('isbn'),
    description: getFieldValue<string>('description'),
    publisher: getFieldValue<string>('publisher'),
    publish_date: getFieldValue<string>('publish_date'),
    language: null,
    page_count: getFieldValue<number>('page_count'),
    cover_url: getFieldValue<string>('cover_url'),
    series: getFieldValue<string>('series'),
    series_provider_id: null,
    series_index: null,
    subjects: [],
    external_id: null,
    confidence: 1.0,
    rating: getFieldValue<number>('rating'),
    rating_count: null,
    identifiers: {}
  };

  // Get series_index from same provider as series
  const seriesIdx = getSelectedIndex('series');
  if (seriesIdx !== null) {
    merged.series_index = results[seriesIdx].series_index;
    merged.series_provider_id = results[seriesIdx].series_provider_id;
  }

  // Get rating_count from same provider as rating
  const ratingIdx = getSelectedIndex('rating');
  if (ratingIdx !== null) {
    merged.rating_count = results[ratingIdx].rating_count;
  }

  // Union all subjects from all sources
  const seenSubjects = new Set<string>();
  for (const idx of selectedForMerge) {
    for (const subject of results[idx].subjects || []) {
      const lower = subject.toLowerCase();
      if (!seenSubjects.has(lower)) {
        seenSubjects.add(lower);
        merged.subjects.push(subject);
      }
    }
  }

  // Aggregate all identifiers
  const identifiers: Record<string, string> = {};
  for (const idx of selectedForMerge) {
    const result = results[idx];
    if (result.external_id) {
      identifiers[result.provider] = result.external_id;
    }
    if (result.isbn) {
      const key = result.isbn.length === 13 ? 'isbn13' : 'isbn10';
      identifiers[key] = result.isbn;
    }
  }
  merged.identifiers = identifiers;

  return merged;
}
