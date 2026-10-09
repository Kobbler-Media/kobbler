export interface WantStatusDisplay {
  icon: string;
  cssClass: string;
}

const ACTIVELY_WANTED_STATUSES = new Set(['wanted', 'searching', 'downloading', 'watching']);

// True while an entity is wanted or being acquired. The acquisition pipeline advances
// want_status wanted -> searching -> downloading -> watching within ~1s of wanting, so
// gating a "Remove from wanted" toggle on only 'wanted' makes it flip back to "Want".
export function isActivelyWanted(wantStatus: string | null | undefined): boolean {
  return wantStatus != null && ACTIVELY_WANTED_STATUSES.has(wantStatus);
}

export function getWantStatusDisplay(
  isInLibrary: boolean,
  wantStatus: string
): WantStatusDisplay | null {
  if (isInLibrary) return { icon: 'check_circle', cssClass: 'in-library' };
  switch (wantStatus) {
    case 'wanted':
      return { icon: 'bookmark', cssClass: 'wanted' };
    case 'searching':
      return { icon: 'search', cssClass: 'searching' };
    case 'downloading':
      return { icon: 'download', cssClass: 'downloading' };
    case 'search_failed':
      return { icon: 'error_outline', cssClass: 'search-failed' };
    case 'ingesting':
      return { icon: 'sync', cssClass: 'ingesting' };
    case 'failed':
      return { icon: 'error', cssClass: 'failed' };
    case 'skipped':
      return { icon: 'block', cssClass: 'skipped' };
    case 'watching':
      return { icon: 'visibility', cssClass: 'watching' };
    default:
      return null;
  }
}

/**
 * Whether a bulk unwant would do anything: at least one episode is outstanding —
 * wanted or mid-acquisition, and not yet in the library. Gates the "Unwant All"
 * controls, which are noise on a show with nothing queued.
 */
export function hasOutstandingWants(
  episodes: { is_in_library: boolean; want_status: string }[]
): boolean {
  return episodes.some((e) => !e.is_in_library && e.want_status !== 'none');
}

const OUTSTANDING_BOOK_STATUSES = new Set([
  'wanted',
  'searching',
  'downloading',
  'watching',
  'search_failed'
]);

/**
 * The book equivalent of [`hasOutstandingWants`]. Books have one status episodes
 * do not: 'pending' means discovered but never asked for, so it does not count
 * as outstanding and a bulk unwant leaves it alone.
 */
export function hasOutstandingBookWants(
  books: { is_in_library?: boolean; want_status?: string }[]
): boolean {
  return books.some(
    (b) =>
      b.is_in_library !== true &&
      b.want_status != null &&
      OUTSTANDING_BOOK_STATUSES.has(b.want_status)
  );
}

/**
 * Toast text for a bulk unwant. Mentions the auto-want mode when the API had to
 * switch one off, since that is a setting change the user did not ask for by name.
 * `scope` is what was unwanted from ("show", "season", "league", "author"…) and
 * `item` what was in it; books report no auto-want change, so it is optional.
 */
export function unwantMessage(
  result: { unwanted_count: number; auto_want_disabled?: boolean },
  scope: string,
  item = 'episode'
): string {
  const n = result.unwanted_count;
  const base =
    n === 0
      ? `No outstanding wants in this ${scope}`
      : `Unwanted ${n} ${item}${n === 1 ? '' : 's'}`;
  return result.auto_want_disabled ? `${base} — auto-want turned off so they stay unwanted` : base;
}
