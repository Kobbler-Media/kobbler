/**
 * Ephemeral preview data for discover detail pages.
 * Set by discover cards before navigation, consumed by the detail page
 * to show content instantly while the full detail loads from the API.
 */

export type DiscoverPreview = {
  media_type: string;
  provider: string;
  provider_id: string;
  title: string;
  year?: number | null;
  description?: string | null;
  poster_url?: string | null;
  backdrop_url?: string | null;
  genres?: string[];
  rating?: number | null;
  rating_count?: number | null;
  identifiers?: Record<string, string>;
  library_id?: string | null;
  want_status?: string | null;
  is_in_library?: boolean | null;
};

let current: DiscoverPreview | null = null;

/** Store preview data before navigating to a detail page. */
export function setDiscoverPreview(preview: DiscoverPreview) {
  current = preview;
}

/** Consume preview data (returns it once, then clears). */
export function consumeDiscoverPreview(
  mediaType: string,
  provider: string,
  providerId: string
): DiscoverPreview | null {
  if (
    current &&
    current.media_type === mediaType &&
    current.provider === provider &&
    current.provider_id === providerId
  ) {
    const result = current;
    current = null;
    return result;
  }
  current = null;
  return null;
}
