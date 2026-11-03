// API-backed frecency tracking for command palette

import { getRecents, recordRecentAccess } from '#lib/api/client.js';
import type { RecentItem } from '#lib/api/client.js';
import type { SearchResult } from './types';
import { pages, commands, sections } from './registry';

/**
 * Fire-and-forget recording of a command palette selection.
 */
export function recordAccess(result: SearchResult): void {
  recordRecentAccess(result.type, result.id).catch(() => {
    // Silently ignore — recents are best-effort
  });
}

/**
 * Resolve a static type (page, command, section) to a SearchResult
 * using the local registry. Returns undefined if not found.
 */
function resolveStaticResult(entityType: string, entityId: string): SearchResult | undefined {
  if (entityType === 'page') {
    const page = pages.find((p) => p.id === entityId);
    if (page) {
      return { type: 'page', id: page.id, label: page.label, path: page.path, icon: page.icon };
    }
  } else if (entityType === 'command') {
    const cmd = commands.find((c) => c.id === entityId);
    if (cmd) {
      return {
        type: 'command',
        id: cmd.id,
        label: cmd.label,
        action: cmd.action,
        icon: cmd.icon
      };
    }
  } else if (entityType === 'section') {
    const sec = sections.find((s) => s.id === entityId);
    if (sec) {
      return { type: 'section', id: sec.id, label: sec.label, path: sec.path };
    }
  }
  return undefined;
}

/**
 * Map an API RecentItem to a SearchResult.
 * DB entities use the live label from the API; static types are resolved locally.
 */
function toSearchResult(item: RecentItem): SearchResult | undefined {
  const entityType = item.entity_type as SearchResult['type'];

  // Static types: resolve display data from local registry
  if (['page', 'command', 'section'].includes(entityType)) {
    return resolveStaticResult(entityType, item.entity_id);
  }

  // DB entities: use live label from API (already stale-filtered)
  if (!item.label) return undefined;

  const base: SearchResult = {
    type: entityType,
    id: item.entity_id,
    label: item.label
  };

  if (entityType === 'book') {
    base.coverUrl = `/api/v1/library/books/${item.entity_id}/cover`;
  }

  return base;
}

/**
 * Fetch recent items from the API and convert to SearchResults.
 */
export async function getRecentResults(): Promise<SearchResult[]> {
  try {
    const items = await getRecents();
    return items.map(toSearchResult).filter((r): r is SearchResult => r !== undefined);
  } catch {
    return [];
  }
}
