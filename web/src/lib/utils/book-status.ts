import type { Book } from '#lib/types.js';
import { getWantStatusOverride } from '#lib/stores/wantStatus.svelte.js';

export type StatusBadgeContext = 'card' | 'detail' | 'list';

export interface StatusBadge {
  label: string;
  class: string;
}

/**
 * Get the status badge for a book on library cards and detail panels.
 * Rules:
 * - pending_sync → "PENDING"
 * - synced → no badge (synced is the expected done state)
 * - active want statuses (searching/downloading/ingesting/watching) → respective badge
 * - search_failed → "FAILED"
 * - wanted (explicitly) → "WANTED"
 * - skipped → "SKIPPED"
 * - otherwise → no badge
 */
export function getBookStatusBadge(book: Book): StatusBadge | null {
  if (book.pending_sync) return { label: 'PENDING', class: 'pending' };
  // synced = no badge anywhere

  const ws = getWantStatusOverride(book.id) ?? book.want_status;
  switch (ws) {
    case 'searching':
      return { label: 'SEARCHING', class: 'searching' };
    case 'downloading':
      return { label: 'DOWNLOADING', class: 'downloading' };
    case 'ingesting':
      return { label: 'INGESTING', class: 'downloading' };
    case 'watching':
      return { label: 'WATCHING', class: 'watching' };
    case 'search_failed':
      return { label: 'FAILED', class: 'failed' };
    case 'wanted':
      return { label: 'WANTED', class: 'wanted' };
    case 'skipped':
      return { label: 'SKIPPED', class: 'skipped' };
    default:
      return null;
  }
}

/**
 * Get the status badge for a book in a series/author book list.
 * Shows ownership status alongside want status for non-owned books.
 */
/** Whether a non-owned book should show the "Want" button. */
export function shouldShowWantButton(statusClass: string): boolean {
  return statusClass === 'missing' || statusClass === 'failed' || statusClass === 'skipped';
}

/** Whether a non-owned book should show the "Skip" button. */
export function shouldShowSkipButton(statusClass: string): boolean {
  return (
    statusClass === 'missing' ||
    statusClass === 'failed' ||
    statusClass === 'wanted' ||
    statusClass === 'watching' ||
    statusClass === 'searching' ||
    statusClass === 'downloading' ||
    statusClass === 'ingesting'
  );
}

export function getBookListStatusBadge(book: Book): { class: string; text: string } {
  if (book.is_in_library !== false) {
    if (book.synced) return { class: 'synced', text: 'Synced' };
    return { class: 'owned', text: 'Owned' };
  }
  const ws = getWantStatusOverride(book.id) ?? book.want_status;
  switch (ws) {
    case 'searching':
      return { class: 'searching', text: 'Searching…' };
    case 'downloading':
      return { class: 'downloading', text: 'Downloading…' };
    case 'ingesting':
      return { class: 'downloading', text: 'Ingesting…' };
    case 'watching':
      return { class: 'watching', text: 'Watching' };
    case 'search_failed':
      return { class: 'failed', text: 'Failed' };
    case 'wanted':
      return { class: 'wanted', text: 'Wanted' };
    case 'skipped':
      return { class: 'skipped', text: 'Skipped' };
    default:
      return { class: 'missing', text: 'Missing' };
  }
}
