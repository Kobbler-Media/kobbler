/**
 * Reactive store for live want-status overrides from SSE events.
 * Components check this store first; if an override exists for a book,
 * it takes precedence over the prop-supplied want_status.
 */

import { SvelteMap } from 'svelte/reactivity';

const overrides = new SvelteMap<string, string>();

/** Update (or create) a live status override for a book. */
export function updateWantStatus(bookId: string, status: string) {
  overrides.set(bookId, status);
}

/** Get a live status override, or undefined if none exists. */
export function getWantStatusOverride(bookId: string): string | undefined {
  return overrides.get(bookId);
}

/** Clear all overrides (e.g., on full data reload). */
export function clearWantStatusOverrides() {
  overrides.clear();
}
