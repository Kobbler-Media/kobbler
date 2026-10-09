import { goto } from '$app/navigation';
import { get } from 'svelte/store';
import { page } from '$app/stores';

/**
 * Update multiple URL search params at once.
 * Pass null or empty string to remove a param.
 * Uses replaceState to avoid polluting browser history on every change.
 */
export function updateUrlParams(params: Record<string, string | number | null | undefined>) {
  const currentPage = get(page);
  const url = new URL(currentPage.url.href);
  for (const [key, value] of Object.entries(params)) {
    if (value === null || value === undefined || value === '') {
      url.searchParams.delete(key);
    } else {
      url.searchParams.set(key, String(value));
    }
  }
  // Navigate to the current path with updated search params
  const target = url.pathname + url.search;
  // eslint-disable-next-line svelte/no-navigation-without-resolve -- Already on the current page, just updating params
  goto(target, { replaceState: true, keepFocus: true, noScroll: true });
}

/**
 * Get a numeric URL search param.
 * Returns the parsed number or the default value.
 */
export function getNumParam(
  searchParams: URLSearchParams,
  key: string,
  defaultValue: number
): number {
  const val = searchParams.get(key);
  if (val === null) return defaultValue;
  const parsed = parseInt(val, 10);
  return Number.isNaN(parsed) ? defaultValue : parsed;
}
