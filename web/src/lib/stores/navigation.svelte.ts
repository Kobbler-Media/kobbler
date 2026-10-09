/**
 * Navigation stack store for sidebar and full-page view navigation.
 *
 * This enables stacked navigation within the library:
 * - Book → Author → Series → Book (etc.)
 * - Movie → Person → back
 * - TvShow → Season → Episode → Person → back
 * - Back button pops the stack
 * - Separate stacks for sidebar and full-page modes
 */

import type {
  Book,
  SeriesSummary,
  AuthorSummary,
  Movie,
  TvShow,
  TvSeason,
  TvEpisode,
  PersonDetail,
  IptvChannel
} from '$lib/types';

export type EntityType =
  | 'book'
  | 'series'
  | 'author'
  | 'movie'
  | 'tv_show'
  | 'season'
  | 'episode'
  | 'person'
  | 'iptv_channel';

export type NavigationData =
  | Book
  | SeriesSummary
  | AuthorSummary
  | Movie
  | TvShow
  | TvSeason
  | TvEpisode
  | PersonDetail
  | IptvChannel;

export interface NavigationItem {
  type: EntityType;
  id: string;
  // Cached data for immediate display (may need refresh)
  data?: NavigationData;
}

// Navigation stack state
let stack = $state<NavigationItem[]>([]);
let isFullPage = $state(false);

// Track loading state for current entity
let loadingEntity = $state(false);

/**
 * Push an entity onto the navigation stack.
 * Opens the sidebar if not already open.
 */
export function pushEntity(type: EntityType, id: string, data?: NavigationData) {
  // Don't push if already viewing this exact entity
  const current = stack[stack.length - 1];
  if (current?.type === type && current?.id === id) {
    return;
  }

  stack = [...stack, { type, id, data }];
}

/**
 * Pop the top entity from the stack.
 * If stack becomes empty, closes the sidebar/full-page view.
 */
export function popEntity(): NavigationItem | undefined {
  if (stack.length === 0) return undefined;

  const popped = stack[stack.length - 1];
  stack = stack.slice(0, -1);
  return popped;
}

/**
 * Clear the entire navigation stack.
 * Closes the sidebar/full-page view.
 */
export function clearStack() {
  stack = [];
  isFullPage = false;
}

/**
 * Replace the current entity's data (after loading/refresh).
 */
export function updateCurrentEntityData(data: NavigationData) {
  if (stack.length === 0) return;

  const current = stack[stack.length - 1];
  stack = [...stack.slice(0, -1), { ...current, data }];
}

/**
 * Get reactive state accessors.
 */
export function getNavigationState() {
  return {
    get stack() {
      return stack;
    },
    get isFullPage() {
      return isFullPage;
    },
    get isOpen() {
      return stack.length > 0;
    },
    get canGoBack() {
      return stack.length > 1;
    },
    get current() {
      return stack[stack.length - 1];
    },
    get loadingEntity() {
      return loadingEntity;
    }
  };
}

export function setLoadingEntity(value: boolean) {
  loadingEntity = value;
}
