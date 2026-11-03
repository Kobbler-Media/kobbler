import type { MovieSearchResult, TvSearchResult } from '#lib/types.js';

export type OverrideableVideoField = 'title' | 'description' | 'poster_url' | 'rating' | 'year';

export type OverrideableTvField = OverrideableVideoField | 'network' | 'status';

export interface VideoFieldOption {
  provider: string;
  value: unknown;
  index: number;
}

export function getFieldOptions<T extends MovieSearchResult | TvSearchResult>(
  field: string,
  selectedIndices: number[],
  results: T[]
): VideoFieldOption[] {
  return selectedIndices
    .map((i) => ({
      provider: results[i].provider,
      value: (results[i] as Record<string, unknown>)[field],
      index: i
    }))
    .filter((opt) => {
      if (opt.value === null || opt.value === undefined) return false;
      if (typeof opt.value === 'string' && opt.value === '') return false;
      if (Array.isArray(opt.value) && opt.value.length === 0) return false;
      return true;
    });
}
