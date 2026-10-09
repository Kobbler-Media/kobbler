/** Map entity_type + entity_id to a navigable route path. Returns null for unknown types. */
export function entityRoute(
  entityType: string | null | undefined,
  entityId: string | null | undefined
): string | null {
  if (!entityType || !entityId) return null;
  switch (entityType) {
    case 'book':
      return `/library/books/${entityId}`;
    case 'author':
      return `/authors/${entityId}`;
    case 'series':
      return `/series/${entityId}`;
    case 'movie':
      return `/movies/${entityId}`;
    case 'tv_show':
      return `/tv/${entityId}`;
    case 'tv_season':
      return `/tv/seasons/${entityId}`;
    case 'tv_episode':
      return `/tv/episodes/${entityId}`;
    case 'person':
      return `/people/${entityId}`;
    default:
      return null;
  }
}

/** Human-readable label for an entity type. */
export function entityTypeLabel(entityType: string): string {
  switch (entityType) {
    case 'book':
      return 'Book';
    case 'author':
      return 'Author';
    case 'series':
      return 'Series';
    case 'movie':
      return 'Movie';
    case 'tv_show':
      return 'TV Show';
    case 'tv_season':
      return 'Season';
    case 'tv_episode':
      return 'Episode';
    case 'person':
      return 'Person';
    case 'media_root':
      return 'Media Root';
    default:
      return entityType
        .split('_')
        .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
        .join(' ');
  }
}

const DISCOVER_ENTITY_TYPES: Record<string, string> = {
  movie: 'movie',
  tv: 'tv_show',
  book: 'book'
};

/** Route to the library detail page for a discover item we already own, or null if we don't. */
export function discoverLibraryRoute(
  mediaType: string | null | undefined,
  libraryId: string | null | undefined
): string | null {
  if (!libraryId) return null;
  return entityRoute(DISCOVER_ENTITY_TYPES[mediaType ?? ''], libraryId);
}
